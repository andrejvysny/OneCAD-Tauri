//! Regen scheduling policy — the debounce / coalesce / cancel funnel.
//!
//! The [`RegenScheduler`] serializes all regen requests into **at most one
//! in-flight job** with **latest-wins** semantics (plan "Rust core specifics";
//! V1/V2 §5.2 debounced edit loop, §14.1 preview-over-regen priority). It owns
//! *policy only* — timing and cancellation — and delegates the actual plan
//! execution to a caller-supplied [`RegenDriver`].
//!
//! ## The ownership seam (why the scheduler is engine-agnostic)
//!
//! The plan lists the scheduler as `RegenScheduler<E: GeometryEngine>`, but a
//! clean split (the "DECIDE cleanly" note) puts the [`GeometryEngine`], the
//! [`RegenExecutor`] and the authoritative `DocumentSession` **behind the driver
//! seam**, not inside the scheduler:
//!
//! * The plan mandates `DocumentSession` is the app layer's **single writer**.
//!   The [`RegenExecutor`] borrows `&mut RegenSession` for its whole prepare →
//!   accept window and reads the [`RevisionGate`] under that same lock. The
//!   scheduler must **not** own that session — it would fight the single-writer
//!   rule.
//! * So the scheduler task **borrows nothing**. At construction it takes a
//!   [`RegenDriver`]: `Fn(`[`RegenDirective`]`) -> impl Future<Output = `[`Outcome`]`>`.
//!   For each job it hands the driver a [`RegenDirective`] (`{job_id, request,
//!   cancel}`) and awaits the [`Outcome`]. The app layer's driver is the single
//!   writer: it compiles the plan ([`RegenPlanner`](super::planner::RegenPlanner)),
//!   runs [`RegenExecutor::run`](super::executor::RegenExecutor::run) against its
//!   own `RegenSession` + [`SnapshotPublisher`](super::snapshot::SnapshotPublisher),
//!   and reports the [`Outcome`] back. **R-WP10/11 wires that driver.**
//! * The scheduler is therefore generic over the driver `D`, not the engine. It
//!   never touches geometry, sessions, or the wire — it decides *when* to run,
//!   *what* to run, and *when to cancel*.
//!
//! ### Snapshot publication stays with the executor
//!
//! The executor mints a snapshot `generation` **atomically** with the session
//! commit, under the single-writer lock (see [`SnapshotPublisher::publish`]).
//! A snapshot cannot be correctly (re)published after the fact from here. So the
//! authoritative model stream is the executor's `SnapshotPublisher`; the
//! scheduler additionally **mirrors** the last [`Outcome::Published`] snapshot on
//! its own [`watch`] channel ([`SchedulerHandle::subscribe_snapshots`]) as a
//! convenience "latest model" subscription that rides alongside the status
//! side-channel — it forwards the very `Arc` the executor already published, so
//! the two never diverge.
//!
//! ## Policy (V1/V2 §5.2 / §14.1)
//!
//! | Incoming            | In-flight       | Action                                             |
//! |---------------------|-----------------|----------------------------------------------------|
//! | `ToStep` (preview)  | none            | (re)start ~120 ms debounce; coalesce the burst     |
//! | `ToStep` (preview)  | `ToEnd` running | cancel it *now* (preview > regen), then debounce   |
//! | `ToStep` (preview)  | preview running | cancel it (stale, latest-wins), then debounce      |
//! | `ToEnd`  (commit)   | none / pending  | supersede any pending preview; run **immediately** |
//! | `ToEnd`  (commit)   | any job running | cancel it (latest-wins), run the newest next       |
//!
//! Consequences, all enforced by tests:
//!
//! * **Coalescing** — a burst of N previews within the debounce window fires
//!   exactly **one** job (the last step wins; each new preview resets the timer,
//!   a different step just replaces it).
//! * **Single in-flight, always** — a superseding request fires the in-flight
//!   [`CancelToken`], then the scheduler **awaits that job's [`Outcome`]** before
//!   starting the next one (never two concurrent jobs).
//! * **Latest-wins** — three rapid `ToEnd`s ⇒ the first is cancelled, the last
//!   runs, the middle one is dropped (never started).
//! * **No auto-`ToEnd`** — when a preview job finishes with nothing newer queued,
//!   the scheduler goes [`Idle`](SchedulerStatus::Idle). Only a commit drives a
//!   `ToEnd`.
//!
//! ## Cancellation + the timeout guard
//!
//! Superseding fires the in-flight token and starts a **grace timer**
//! ([`SchedulerConfig::cancel_grace`], default 5 s). If the driver returns within
//! grace (a cooperative [`Outcome::Cancelled`]), the next job starts at once. If
//! it does **not**, the scheduler drops the stuck job and proceeds — surfacing an
//! [`EngineError::Timeout`] on the status channel. Actually **killing** a wedged
//! worker is the `WorkerManager`'s job (plan "WorkerManager"); the scheduler only
//! stops *waiting* on it so the pipeline never deadlocks.
//!
//! ## Runtime-agnostic
//!
//! Depends on `tokio` **sync + time only** — no `rt`, no `spawn`, no `process`.
//! [`RegenScheduler::run`] is one `select!` loop the **caller** spawns on its own
//! runtime; the scheduler never spawns a task itself.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time::{sleep_until, Instant};
use uuid::Uuid;

use crate::edit::outcome::{CommandOutcome, RegenHint};
use crate::ids::JobId;

use super::engine::EngineError;
use super::executor::{CancelToken, Outcome};
use super::planner::RegenRequest;
use super::snapshot::ModelSnapshot;

/// Debounce before a preview (`ToStep`) job fires, in milliseconds (V1/V2 §5.2).
pub const PREVIEW_DEBOUNCE_MS: u64 = 120;

/// Default grace after a cancel before the scheduler abandons a stuck driver and
/// proceeds (the timeout guard). Worker-kill itself is the `WorkerManager`'s job.
pub const CANCEL_GRACE_SECS: u64 = 5;

// ─────────────────────────────────────────────────────────────────────────────
// Driver seam
// ─────────────────────────────────────────────────────────────────────────────

/// What the scheduler hands the driver for one job: the fenced-later
/// [`RegenRequest`], the job's [`JobId`], and the [`CancelToken`] the scheduler
/// fires when the job is superseded. The driver (app-layer single writer)
/// compiles the plan and runs the executor against its own session.
#[derive(Debug, Clone)]
pub struct RegenDirective {
    /// Scheduler-minted id for this job (monotonic; deterministic in tests).
    pub job_id: JobId,
    /// What to regenerate — `ToStep(k)` (preview) or `ToEnd { from }` (commit).
    pub request: RegenRequest,
    /// Fired by the scheduler when a newer request supersedes this job. The
    /// driver's executor `select!`s on it and returns [`Outcome::Cancelled`].
    pub cancel: CancelToken,
}

/// The executor-driver seam: runs one [`RegenDirective`] to an [`Outcome`].
///
/// Blanket-implemented for any `Fn(RegenDirective) -> Future<Output = Outcome>`,
/// so the app passes a closure over its `DocumentSession` + `RegenExecutor` +
/// `SnapshotPublisher`. The scheduler owns none of those.
pub trait RegenDriver {
    /// The future one [`RegenDirective`] resolves to.
    type Future: Future<Output = Outcome>;
    /// Starts the job. Called synchronously; the returned future is polled by the
    /// scheduler loop and dropped if the job is abandoned after a cancel timeout.
    fn drive(&self, directive: RegenDirective) -> Self::Future;
}

impl<F, Fut> RegenDriver for F
where
    F: Fn(RegenDirective) -> Fut,
    Fut: Future<Output = Outcome>,
{
    type Future = Fut;
    fn drive(&self, directive: RegenDirective) -> Fut {
        self(directive)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Status side-channel
// ─────────────────────────────────────────────────────────────────────────────

/// Which flavour of job is (or was) in flight — carried on [`SchedulerStatus`]
/// and in the [`RegenDirective`] mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    /// A debounced preview to (and including) `step` (rollback-edit fast path).
    Preview { step: usize },
    /// A commit regen of `[from, applied_end]`.
    ToEnd { from: usize },
}

/// The scheduler's observable state, published on a [`watch`] channel
/// ([`SchedulerHandle::subscribe_status`]). This is the side-channel that surfaces
/// engine failures and cancel-timeouts, since a failed job publishes **no**
/// snapshot (the model is left unchanged).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchedulerStatus {
    /// No job running and nothing pending.
    Idle,
    /// A preview is waiting out its debounce window (no job running yet).
    Debouncing { step: usize },
    /// A job is executing.
    Running { job: JobId, kind: JobKind },
    /// The most recent job failed (or a cancel timed out). Sticky until the next
    /// job starts or a snapshot is published.
    LastError { job: JobId, error: EngineError },
}

// ─────────────────────────────────────────────────────────────────────────────
// Config + handle
// ─────────────────────────────────────────────────────────────────────────────

/// Tunables for the scheduler (both configurable per the plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchedulerConfig {
    /// Preview debounce window (default ~120 ms).
    pub debounce: Duration,
    /// Grace after a cancel before a stuck driver is abandoned (default 5 s).
    pub cancel_grace: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(PREVIEW_DEBOUNCE_MS),
            cancel_grace: Duration::from_secs(CANCEL_GRACE_SECS),
        }
    }
}

/// The cheap, cloneable control surface. The app's single writer calls
/// [`handle`](Self::handle) after each committed command; consumers subscribe to
/// the status and snapshot channels. Sends are non-blocking (unbounded, sync) so
/// the editor never stalls behind regen.
#[derive(Debug, Clone)]
pub struct SchedulerHandle {
    commands: mpsc::UnboundedSender<Command>,
    status_rx: watch::Receiver<SchedulerStatus>,
    snapshot_rx: watch::Receiver<Option<Arc<ModelSnapshot>>>,
}

impl SchedulerHandle {
    /// Maps a [`CommandOutcome`]'s [`RegenHint`] to a request and enqueues it
    /// (the plan's `handle(hint)` entrypoint — it also needs the outcome's
    /// [`DirtyRange`](crate::history::DirtyRange) to supply `ToEnd`'s `from`).
    ///
    /// * [`RegenHint::None`] — nothing enqueued (metadata-only edit).
    /// * [`RegenHint::PreviewTo(k)`] — a debounced `ToStep(k)` preview.
    /// * [`RegenHint::ToEnd`] — a commit `ToEnd { from = dirty.from }` (0 when the
    ///   command dirtied no step).
    pub fn handle(&self, outcome: &CommandOutcome) {
        match outcome.regen {
            RegenHint::None => {}
            RegenHint::PreviewTo(step) => self.request(RegenRequest::ToStep(step)),
            RegenHint::ToEnd => {
                let from = outcome.dirty.map_or(0, |d| d.from);
                self.request(RegenRequest::ToEnd { from });
            }
        }
    }

    /// Enqueues a request directly (bypassing the [`RegenHint`] mapping).
    pub fn request(&self, request: RegenRequest) {
        // Send failure ⇒ the loop stopped (shut down / dropped); nothing to run.
        let _ = self.commands.send(Command::Regen(request));
    }

    /// Signals the loop to cancel any in-flight job, drop pending work, and exit.
    pub fn shutdown(&self) {
        let _ = self.commands.send(Command::Shutdown);
    }

    /// Subscribes to the status side-channel (initial value: the latest status).
    #[must_use]
    pub fn subscribe_status(&self) -> watch::Receiver<SchedulerStatus> {
        self.status_rx.clone()
    }

    /// Subscribes to the "latest published model" mirror (initial value: the
    /// latest snapshot, or `None`).
    #[must_use]
    pub fn subscribe_snapshots(&self) -> watch::Receiver<Option<Arc<ModelSnapshot>>> {
        self.snapshot_rx.clone()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Internal loop state
// ─────────────────────────────────────────────────────────────────────────────

/// A control message on the request channel (not part of the public API).
enum Command {
    Regen(RegenRequest),
    Shutdown,
}

/// The next request to run once the pipeline frees up.
enum Pending {
    /// A preview, runnable at `ready_at` (its debounce deadline).
    Preview { step: usize, ready_at: Instant },
    /// A commit, runnable immediately (no debounce).
    ToEnd { from: usize },
}

/// The single in-flight job: its identity and the future being driven.
struct InFlight<F> {
    job_id: JobId,
    kind: JobKind,
    cancel: CancelToken,
    fut: Pin<Box<F>>,
}

/// What one loop iteration woke up for.
enum Event {
    CancelTimeout,
    Completed(Outcome),
    Command(Option<Command>),
    DebounceElapsed,
}

// ─────────────────────────────────────────────────────────────────────────────
// The scheduler
// ─────────────────────────────────────────────────────────────────────────────

/// The single-in-flight, latest-wins regen scheduler (loop task). Construct with
/// [`new`](Self::new); spawn [`run`](Self::run) on the caller's runtime; drive it
/// through the returned [`SchedulerHandle`].
pub struct RegenScheduler<D: RegenDriver> {
    driver: D,
    config: SchedulerConfig,
    commands: mpsc::UnboundedReceiver<Command>,
    status_tx: watch::Sender<SchedulerStatus>,
    snapshot_tx: watch::Sender<Option<Arc<ModelSnapshot>>>,
    /// Monotonic job counter (deterministic [`JobId`]s: `Uuid::from_u128(seq)`).
    seq: u64,
    in_flight: Option<InFlight<D::Future>>,
    pending: Option<Pending>,
    /// Deadline for the timeout guard, set when the in-flight job is cancelled.
    cancel_deadline: Option<Instant>,
    /// The most recent hard failure, surfaced as [`SchedulerStatus::LastError`]
    /// while idle. Cleared when a new job starts or a snapshot publishes.
    last_error: Option<(JobId, EngineError)>,
}

impl<D: RegenDriver> RegenScheduler<D> {
    /// Builds a scheduler (default [`SchedulerConfig`]) and its [`SchedulerHandle`].
    #[must_use]
    pub fn new(driver: D) -> (Self, SchedulerHandle) {
        Self::with_config(driver, SchedulerConfig::default())
    }

    /// Builds a scheduler with an explicit [`SchedulerConfig`].
    #[must_use]
    pub fn with_config(driver: D, config: SchedulerConfig) -> (Self, SchedulerHandle) {
        let (commands_tx, commands_rx) = mpsc::unbounded_channel();
        let (status_tx, status_rx) = watch::channel(SchedulerStatus::Idle);
        let (snapshot_tx, snapshot_rx) = watch::channel(None);
        let handle = SchedulerHandle {
            commands: commands_tx,
            status_rx,
            snapshot_rx,
        };
        let scheduler = Self {
            driver,
            config,
            commands: commands_rx,
            status_tx,
            snapshot_tx,
            seq: 0,
            in_flight: None,
            pending: None,
            cancel_deadline: None,
            last_error: None,
        };
        (scheduler, handle)
    }

    /// Runs the scheduling loop until [`shutdown`](SchedulerHandle::shutdown) (or
    /// every handle is dropped). Spawn this on the caller's runtime.
    pub async fn run(mut self) {
        loop {
            // (A) Start the pending request if the pipeline is idle and it is ready.
            self.try_start();
            // (B) Publish the resulting status.
            self.settle_status();
            // (C) Wait for the next event. Field borrows are split so the in-flight
            //     future and the command receiver can be polled in one `select!`.
            let event = {
                let has_in_flight = self.in_flight.is_some();
                let cancel_at = self.cancel_deadline;
                let debounce_at = if has_in_flight {
                    None
                } else {
                    match &self.pending {
                        Some(Pending::Preview { ready_at, .. }) => Some(*ready_at),
                        _ => None,
                    }
                };
                let in_flight = &mut self.in_flight;
                let commands = &mut self.commands;
                tokio::select! {
                    biased;
                    // Timeout guard first: a wedged job must never deadlock the loop.
                    () = sleep_until(cancel_at.unwrap_or_else(Instant::now)),
                        if cancel_at.is_some() && has_in_flight => Event::CancelTimeout,
                    // Commands next: draining the queue before starting the next job
                    // is what makes latest-wins work (a superseding request must be
                    // seen before the freed pipeline picks a job).
                    cmd = commands.recv() => Event::Command(cmd),
                    // Then the in-flight terminal.
                    outcome = poll_in_flight(in_flight), if has_in_flight => {
                        Event::Completed(outcome)
                    }
                    // Finally the debounce timer (idle + a not-yet-ready preview).
                    () = sleep_until(debounce_at.unwrap_or_else(Instant::now)),
                        if debounce_at.is_some() => Event::DebounceElapsed,
                }
            };
            match event {
                Event::CancelTimeout => self.on_cancel_timeout(),
                Event::Completed(outcome) => self.on_job_complete(outcome),
                Event::Command(Some(Command::Regen(req))) => self.on_request(req),
                Event::Command(Some(Command::Shutdown)) | Event::Command(None) => {
                    self.on_shutdown();
                    break;
                }
                // `try_start` at the top of the next iteration picks up the preview.
                Event::DebounceElapsed => {}
            }
        }
    }

    /// Starts the pending request iff the pipeline is idle and the request is
    /// ready (`ToEnd` immediately; a preview once its debounce deadline passed).
    fn try_start(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        let ready = match &self.pending {
            Some(Pending::ToEnd { .. }) => true,
            Some(Pending::Preview { ready_at, .. }) => Instant::now() >= *ready_at,
            None => false,
        };
        if ready {
            let pending = self.pending.take().expect("ready ⇒ Some");
            self.start_job(pending);
        }
    }

    /// Mints a job for `pending` and hands it to the driver.
    fn start_job(&mut self, pending: Pending) {
        self.seq += 1;
        let job_id = JobId(Uuid::from_u128(u128::from(self.seq)));
        let (kind, request) = match pending {
            Pending::Preview { step, .. } => {
                (JobKind::Preview { step }, RegenRequest::ToStep(step))
            }
            Pending::ToEnd { from } => (JobKind::ToEnd { from }, RegenRequest::ToEnd { from }),
        };
        let cancel = CancelToken::new();
        let fut = Box::pin(self.driver.drive(RegenDirective {
            job_id,
            request,
            cancel: cancel.clone(),
        }));
        // A fresh job supersedes any prior error and resets the cancel guard.
        self.last_error = None;
        self.cancel_deadline = None;
        self.in_flight = Some(InFlight {
            job_id,
            kind,
            cancel,
            fut,
        });
    }

    /// Applies one incoming request per the policy table.
    fn on_request(&mut self, request: RegenRequest) {
        match request {
            RegenRequest::ToStep(step) => {
                // Preview: cancel any in-flight job (preview > regen; a running
                // preview is stale), then (re)start the debounce with this step.
                self.cancel_in_flight();
                self.pending = Some(Pending::Preview {
                    step,
                    ready_at: Instant::now() + self.config.debounce,
                });
            }
            RegenRequest::ToEnd { from } => {
                // Commit: cancel any in-flight job, supersede any pending preview,
                // and run immediately (latest-wins, no debounce).
                self.cancel_in_flight();
                self.pending = Some(Pending::ToEnd { from });
            }
        }
    }

    /// Fires the in-flight cancel token and arms the timeout guard. No-op when
    /// nothing is running.
    fn cancel_in_flight(&mut self) {
        if let Some(inf) = &self.in_flight {
            if !inf.cancel.is_cancelled() {
                inf.cancel.cancel();
            }
            if self.cancel_deadline.is_none() {
                self.cancel_deadline = Some(Instant::now() + self.config.cancel_grace);
            }
        }
    }

    /// Handles the in-flight job's terminal [`Outcome`].
    fn on_job_complete(&mut self, outcome: Outcome) {
        let job = self.in_flight.as_ref().map(|i| i.job_id);
        self.in_flight = None;
        self.cancel_deadline = None;
        match outcome {
            // Mirror the executor's published snapshot on the convenience channel.
            Outcome::Published(snapshot) => {
                self.last_error = None;
                self.snapshot_tx.send_replace(Some(snapshot));
            }
            // A failure publishes NO snapshot — surface it on the status channel.
            Outcome::EngineFailed(err) => {
                if let Some(job) = job {
                    self.last_error = Some((job, err));
                }
            }
            // Superseded / cooperatively cancelled / no-op: the model is unchanged.
            Outcome::Superseded | Outcome::Cancelled | Outcome::NoOp => {}
        }
    }

    /// The cancel grace elapsed without the driver returning: drop the wedged job
    /// and proceed. Worker-kill is the `WorkerManager`'s concern; here we only
    /// stop waiting so the next request can run.
    fn on_cancel_timeout(&mut self) {
        let job = self.in_flight.as_ref().map(|i| i.job_id);
        self.in_flight = None; // drops the stuck future (worker cleanup is elsewhere).
        self.cancel_deadline = None;
        if let Some(job) = job {
            self.last_error = Some((
                job,
                EngineError::Timeout {
                    message: "regen driver did not return within the cancel grace window".into(),
                },
            ));
        }
    }

    /// Shutdown: cancel the in-flight job, drop pending work (nothing new runs),
    /// and let the loop break. The in-flight future is dropped with `self`.
    fn on_shutdown(&mut self) {
        self.cancel_in_flight();
        self.pending = None;
    }

    /// Recomputes and publishes the status (only notifying on a real change).
    fn settle_status(&self) {
        let status = if let Some(inf) = &self.in_flight {
            SchedulerStatus::Running {
                job: inf.job_id,
                kind: inf.kind,
            }
        } else if let Some(Pending::Preview { step, .. }) = &self.pending {
            SchedulerStatus::Debouncing { step: *step }
        } else if let Some((job, err)) = &self.last_error {
            SchedulerStatus::LastError {
                job: *job,
                error: err.clone(),
            }
        } else {
            SchedulerStatus::Idle
        };
        self.status_tx.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                *current = status.clone();
                true
            }
        });
    }
}

/// Awaits the in-flight future. Only called under an `in_flight.is_some()` guard.
async fn poll_in_flight<F: Future<Output = Outcome>>(
    in_flight: &mut Option<InFlight<F>>,
) -> Outcome {
    in_flight
        .as_mut()
        .expect("guarded by in_flight.is_some()")
        .fut
        .as_mut()
        .await
}
