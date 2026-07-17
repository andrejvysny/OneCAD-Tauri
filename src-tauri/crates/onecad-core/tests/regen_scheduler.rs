//! R-WP8 policy tests for the [`RegenScheduler`]: debounce, coalesce, latest-wins,
//! preview-over-regen cancellation, the cancel-timeout guard, and clean shutdown.
//!
//! Every test runs under `#[tokio::test(start_paused = true)]` so the tokio clock
//! is virtual and deterministic — debounce/grace windows are crossed with
//! [`tokio::time::advance`], never wall-clock sleeps.
//!
//! The scheduler is exercised through the driver seam only: a scripted
//! [`FakeDriver`] records every [`RegenDirective`] it is handed and lets each test
//! decide when (and whether) a job completes — no [`GeometryEngine`] needed.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;
use tokio::time::advance;

use onecad_core::edit::{CommandOutcome, ProjectionDelta, RegenHint};
use onecad_core::history::DirtyRange;
use onecad_core::ids::SnapshotId;
use onecad_core::regen::{
    ModelSnapshot, Outcome, RegenDirective, RegenRequest, RegenScheduler, RepairSummary,
    SchedulerConfig, SchedulerHandle, SchedulerStatus, StoppedReason, PREVIEW_DEBOUNCE_MS,
};

// ─────────────────────────────────────────────────────────────────────────────
// Scripted FakeDriver
// ─────────────────────────────────────────────────────────────────────────────

/// One recorded job: what was requested, the token the scheduler will fire to
/// cancel it, and the completion channel the test uses to resolve it.
struct StartedJob {
    request: RegenRequest,
    cancel: onecad_core::regen::CancelToken,
    complete: Option<oneshot::Sender<Outcome>>,
}

#[derive(Default)]
struct FakeState {
    started: Vec<StartedJob>,
}

type BoxFut = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// Builds a driver closure over shared `state`. When `respond_to_cancel` is set,
/// a job resolves to [`Outcome::Cancelled`] as soon as its token fires (a
/// cooperative worker); otherwise it ignores cancellation and only ever resolves
/// via the test's completion channel — modelling a wedged worker for the timeout
/// guard.
fn fake_driver(
    state: Arc<Mutex<FakeState>>,
    respond_to_cancel: bool,
) -> impl Fn(RegenDirective) -> BoxFut {
    move |directive: RegenDirective| {
        let (tx, rx) = oneshot::channel::<Outcome>();
        state.lock().unwrap().started.push(StartedJob {
            request: directive.request,
            cancel: directive.cancel.clone(),
            complete: Some(tx),
        });
        let cancel = directive.cancel;
        Box::pin(async move {
            if respond_to_cancel {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => Outcome::Cancelled,
                    out = rx => out.unwrap_or(Outcome::Cancelled),
                }
            } else {
                rx.await.unwrap_or(Outcome::Cancelled)
            }
        })
    }
}

// ── shared-state accessors ───────────────────────────────────────────────────

fn count(state: &Arc<Mutex<FakeState>>) -> usize {
    state.lock().unwrap().started.len()
}

fn nth_request(state: &Arc<Mutex<FakeState>>, i: usize) -> RegenRequest {
    state.lock().unwrap().started[i].request
}

fn nth_cancelled(state: &Arc<Mutex<FakeState>>, i: usize) -> bool {
    state.lock().unwrap().started[i].cancel.is_cancelled()
}

fn complete_nth(state: &Arc<Mutex<FakeState>>, i: usize, outcome: Outcome) {
    let tx = state.lock().unwrap().started[i]
        .complete
        .take()
        .expect("job already completed");
    let _ = tx.send(outcome);
}

fn status(handle: &SchedulerHandle) -> SchedulerStatus {
    handle.subscribe_status().borrow().clone()
}

// ── test harness ─────────────────────────────────────────────────────────────

/// Spawns a scheduler over a fresh fake driver, returning the shared record, the
/// control handle, and the loop's join handle.
fn spawn(
    respond_to_cancel: bool,
    config: SchedulerConfig,
) -> (
    Arc<Mutex<FakeState>>,
    SchedulerHandle,
    tokio::task::JoinHandle<()>,
) {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let driver = fake_driver(state.clone(), respond_to_cancel);
    let (scheduler, handle) = RegenScheduler::with_config(driver, config);
    let jh = tokio::spawn(scheduler.run());
    (state, handle, jh)
}

/// Lets the spawned scheduler drain to a parked state without advancing time.
async fn tick() {
    for _ in 0..12 {
        tokio::task::yield_now().await;
    }
}

/// Advances virtual time by `ms` and lets the scheduler react.
async fn advance_ms(ms: u64) {
    advance(Duration::from_millis(ms)).await;
    tick().await;
}

fn published(generation: u64) -> Arc<ModelSnapshot> {
    Arc::new(ModelSnapshot {
        id: SnapshotId(generation),
        generation,
        step_index: Some(0),
        bodies: vec![],
        stopped_reason: StoppedReason::Completed,
        step_states: vec![],
        signatures: None,
        diagnostics: vec![],
        repair_summary: RepairSummary::default(),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// A single preview does not run until its debounce window elapses, then runs
/// exactly once.
#[tokio::test(start_paused = true)]
async fn preview_debounces_into_one_job() {
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    handle.request(RegenRequest::ToStep(2));
    tick().await;
    assert_eq!(count(&state), 0, "no job before the debounce elapses");
    assert_eq!(status(&handle), SchedulerStatus::Debouncing { step: 2 });

    advance_ms(PREVIEW_DEBOUNCE_MS + 5).await;
    assert_eq!(count(&state), 1, "exactly one job after the debounce");
    assert_eq!(nth_request(&state, 0), RegenRequest::ToStep(2));
}

/// A scrub storm — ten previews spaced 50 ms apart, each inside the 120 ms
/// window — coalesces into a single job carrying the last step.
#[tokio::test(start_paused = true)]
async fn scrub_storm_coalesces_to_one_job() {
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    for step in 0..10usize {
        handle.request(RegenRequest::ToStep(step));
        tick().await;
        if step < 9 {
            advance_ms(50).await; // 50 ms < 120 ms ⇒ every preview resets the timer.
        }
    }
    assert_eq!(count(&state), 0, "nothing runs mid-storm");

    advance_ms(PREVIEW_DEBOUNCE_MS + 5).await;
    assert_eq!(count(&state), 1, "the whole burst collapses to one job");
    assert_eq!(
        nth_request(&state, 0),
        RegenRequest::ToStep(9),
        "last step wins"
    );
}

/// A preview cancels an in-flight `ToEnd` immediately (preview > regen), within a
/// single scheduling tick — long before the preview's own debounce fires.
#[tokio::test(start_paused = true)]
async fn preview_cancels_running_toend() {
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    handle.request(RegenRequest::ToEnd { from: 0 });
    tick().await;
    assert_eq!(count(&state), 1);
    assert_eq!(nth_request(&state, 0), RegenRequest::ToEnd { from: 0 });

    handle.request(RegenRequest::ToStep(3));
    tick().await; // no time advanced.
    assert!(
        nth_cancelled(&state, 0),
        "the running ToEnd is cancelled at once"
    );
    assert_eq!(count(&state), 1, "the preview itself is still debouncing");
    assert_eq!(status(&handle), SchedulerStatus::Debouncing { step: 3 });

    advance_ms(PREVIEW_DEBOUNCE_MS + 5).await;
    assert_eq!(count(&state), 2, "the preview runs after its debounce");
    assert_eq!(nth_request(&state, 1), RegenRequest::ToStep(3));
}

/// A commit `ToEnd` supersedes a still-debouncing preview: it runs immediately
/// and the preview never starts.
#[tokio::test(start_paused = true)]
async fn toend_supersedes_pending_preview() {
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    handle.request(RegenRequest::ToStep(5));
    tick().await;
    assert_eq!(count(&state), 0);
    assert_eq!(status(&handle), SchedulerStatus::Debouncing { step: 5 });

    handle.request(RegenRequest::ToEnd { from: 2 });
    tick().await;
    assert_eq!(
        count(&state),
        1,
        "the commit runs immediately (no debounce)"
    );
    assert_eq!(nth_request(&state, 0), RegenRequest::ToEnd { from: 2 });

    // Past where the preview would have fired — it must never run.
    advance_ms(PREVIEW_DEBOUNCE_MS + 50).await;
    assert_eq!(count(&state), 1, "the superseded preview is dropped");
}

/// Three rapid `ToEnd`s: the first is cancelled, the last runs, the middle one is
/// coalesced away (never started).
#[tokio::test(start_paused = true)]
async fn toend_latest_wins_skips_the_middle() {
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    handle.request(RegenRequest::ToEnd { from: 0 });
    tick().await;
    assert_eq!(count(&state), 1, "the first commit starts");

    handle.request(RegenRequest::ToEnd { from: 1 }); // middle
    handle.request(RegenRequest::ToEnd { from: 2 }); // latest
    tick().await;

    assert!(nth_cancelled(&state, 0), "the first commit is cancelled");
    assert_eq!(count(&state), 2, "only two jobs ever start");
    assert_eq!(
        nth_request(&state, 1),
        RegenRequest::ToEnd { from: 2 },
        "the latest commit runs; the middle is skipped"
    );
}

/// A completed job publishes its snapshot on the mirror channel and settles the
/// status to `Idle` — no auto-`ToEnd` follows a finished job.
#[tokio::test(start_paused = true)]
async fn completion_publishes_and_goes_idle() {
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    handle.request(RegenRequest::ToEnd { from: 0 });
    tick().await;
    assert!(matches!(status(&handle), SchedulerStatus::Running { .. }));
    assert!(handle.subscribe_snapshots().borrow().is_none());

    complete_nth(&state, 0, Outcome::Published(published(1)));
    tick().await;

    assert_eq!(status(&handle), SchedulerStatus::Idle, "settles to Idle");
    let snap = handle.subscribe_snapshots().borrow().clone();
    assert_eq!(snap.expect("snapshot mirrored").generation, 1);
    assert_eq!(count(&state), 1, "nothing else runs on its own");
}

/// A hard engine failure publishes no snapshot and surfaces on the status
/// side-channel as `LastError`.
#[tokio::test(start_paused = true)]
async fn engine_failure_surfaces_as_last_error() {
    use onecad_core::regen::EngineError;
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    handle.request(RegenRequest::ToEnd { from: 0 });
    tick().await;
    complete_nth(
        &state,
        0,
        Outcome::EngineFailed(EngineError::Crashed {
            message: "boom".into(),
        }),
    );
    tick().await;

    assert!(matches!(status(&handle), SchedulerStatus::LastError { .. }));
    assert!(
        handle.subscribe_snapshots().borrow().is_none(),
        "a failed job publishes no snapshot"
    );
}

/// The timeout guard: a wedged driver that ignores cancellation is abandoned once
/// the cancel grace elapses, and the next request proceeds regardless.
#[tokio::test(start_paused = true)]
async fn cancel_timeout_abandons_a_wedged_driver() {
    let config = SchedulerConfig {
        debounce: Duration::from_millis(PREVIEW_DEBOUNCE_MS),
        cancel_grace: Duration::from_secs(2),
    };
    let (state, handle, _jh) = spawn(false, config); // driver ignores cancel.

    handle.request(RegenRequest::ToEnd { from: 0 });
    tick().await;
    assert_eq!(count(&state), 1);

    handle.request(RegenRequest::ToEnd { from: 1 });
    tick().await;
    assert!(nth_cancelled(&state, 0), "the stuck job's token is fired");
    assert_eq!(
        count(&state),
        1,
        "but it hasn't returned, so nothing new starts"
    );

    // Cross the grace window — the scheduler gives up waiting and proceeds.
    advance(Duration::from_secs(3)).await;
    tick().await;
    assert_eq!(count(&state), 2, "the next request runs after the timeout");
    assert_eq!(nth_request(&state, 1), RegenRequest::ToEnd { from: 1 });
}

/// `handle` maps a [`CommandOutcome`]'s [`RegenHint`] to the right request (or
/// none for a metadata-only edit).
#[tokio::test(start_paused = true)]
async fn handle_maps_command_outcome() {
    let (state, handle, _jh) = spawn(true, SchedulerConfig::default());

    // None ⇒ nothing enqueued.
    handle.handle(&CommandOutcome::metadata_only(ProjectionDelta::new()));
    tick().await;
    advance_ms(PREVIEW_DEBOUNCE_MS + 5).await;
    assert_eq!(count(&state), 0);

    // ToEnd ⇒ ToEnd { from = dirty.from }, run immediately.
    handle.handle(&CommandOutcome::dirty_to_end(
        ProjectionDelta::new(),
        DirtyRange::new(3, 7),
    ));
    tick().await;
    assert_eq!(count(&state), 1);
    assert_eq!(nth_request(&state, 0), RegenRequest::ToEnd { from: 3 });

    // PreviewTo(k) ⇒ debounced ToStep(k).
    let preview = CommandOutcome {
        projection_delta: ProjectionDelta::new(),
        dirty: Some(DirtyRange::new(4, 9)),
        regen: RegenHint::PreviewTo(4),
    };
    // A preview cancels the running commit, then debounces.
    handle.handle(&preview);
    tick().await; // process the preview so its debounce anchors at "now".
    advance_ms(PREVIEW_DEBOUNCE_MS + 5).await;
    assert_eq!(count(&state), 2);
    assert_eq!(nth_request(&state, 1), RegenRequest::ToStep(4));
}

/// Shutdown cancels the in-flight job, drops pending work, and lets the loop exit
/// cleanly — and no job starts after shutdown.
#[tokio::test(start_paused = true)]
async fn shutdown_drains_and_stops_new_work() {
    let (state, handle, jh) = spawn(true, SchedulerConfig::default());

    handle.request(RegenRequest::ToEnd { from: 0 });
    tick().await;
    assert_eq!(count(&state), 1);

    handle.shutdown();
    tick().await;

    // The loop exits (drains cleanly).
    let joined = tokio::time::timeout(Duration::from_secs(1), jh).await;
    assert!(joined.is_ok(), "the scheduler task exits on shutdown");
    assert!(nth_cancelled(&state, 0), "the in-flight job was cancelled");

    // Nothing new runs after shutdown.
    handle.request(RegenRequest::ToEnd { from: 1 });
    tick().await;
    advance_ms(PREVIEW_DEBOUNCE_MS + 5).await;
    assert_eq!(count(&state), 1, "no job starts after shutdown");
}
