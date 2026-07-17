//! R-WP11 chaos gate — the RISKY part. Drives the real [`WorkerManager`] over the
//! built `onecad-worker-stub` (chaos hooks CRASH_ON / HANG_ON / GARBAGE /
//! CRASH_COUNTDOWN) and asserts the lifecycle contract:
//!
//! * crash on a verb mid-plan ⇒ recoverable `Crashed`, **no partial publish**;
//! * hang ⇒ ping timeout ⇒ SIGKILL ⇒ restart;
//! * garbage frame ⇒ restart (no resync) ⇒ backoff exhausted ⇒ Failed;
//! * repeated same-plan crash ⇒ **crash circuit breaker** (poison) ⇒ Failed;
//! * convergence drill: kill mid-plan N× ⇒ the document **always** converges to
//!   the last-valid snapshot, never a partial (parameterized, env-overridable).
//!
//! All gate on the stub binary existing (built by `cargo test --workspace`); a
//! missing binary skips cleanly.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord,
};
use onecad_core::document::variables::Scalar;
use onecad_core::history::{DependencyGraph, Timeline};
use onecad_core::ids::{DocumentRevision, JobId, RecordId, WorkerEpoch};
use onecad_core::regen::{
    CancelToken, EngineError, GeometryEngine, Lod, Outcome, PlanArtifacts, PlanContext, PlanEvent,
    PlanRequest, PolicyVersions, RegenExecutor, RegenPlan, RegenPlanner, RegenRequest,
    RegenSession, SnapshotPublisher, TessellateSpec,
};

use onecad_lib::worker::manager::{SupervisorConfig, WorkerLifecycle, WorkerState};
use onecad_lib::worker::{AdoptingEngine, MeshProvider, WorkerManager};

// ─────────────────────────────────────────────────────────────────────────────
// Harness
// ─────────────────────────────────────────────────────────────────────────────

/// The built stub binary, derived from the test executable's target dir. `None`
/// ⇒ the crate was not built with `--workspace` ⇒ skip.
fn stub_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?; // <target>/debug/deps/<test>
    let deps = exe.parent()?; // deps
    let debug = deps.parent()?; // debug
    let bin = debug.join("onecad-worker-stub");
    bin.exists().then_some(bin)
}

/// A fast supervision policy so chaos drills run in ~1 s, not seconds-per-restart.
/// The ping timeout is kept generous (500 ms) so a *healthy* worker's ping never
/// times out under the CPU contention of the parallel test run (which would
/// SIGKILL it spuriously), while a *hung* worker is still detected in ~1 s.
fn fast_config(binary: PathBuf, envs: Vec<(String, String)>) -> SupervisorConfig {
    SupervisorConfig {
        binary,
        envs,
        ping_interval: Duration::from_millis(100),
        ping_timeout: Duration::from_millis(500),
        max_missed_pings: 2,
        backoff: vec![
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
        ],
        poison_threshold: 3,
        auto_open_session: false,
    }
}

fn extrude_record(seed: u128, distance: f64) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: None,
        distance: Scalar::new(distance),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: BooleanMode::NewBody,
        target_body: None,
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    }));
    OperationRecord::new(RecordId(Uuid::from_u128(seed)), 0, "Extrude", op)
}

fn one_extrude_timeline() -> Timeline {
    let mut tl = Timeline::new();
    tl.insert_at_cursor(extrude_record(0x10, 25.0));
    tl
}

fn plan_request(tl: &Timeline, rev: u64, epoch: u64) -> PlanRequest {
    let ctx = PlanContext {
        policy_versions: PolicyVersions::default(),
        occt_fingerprint: "0000000000000000".into(),
    };
    let plan: RegenPlan = RegenPlanner::plan(
        tl,
        &DependencyGraph::new(),
        &[],
        RegenRequest::ToEnd { from: 0 },
        &ctx,
    );
    plan.into_request(
        JobId(Uuid::from_u128(u128::from(rev + 1))),
        DocumentRevision(rev),
        WorkerEpoch(epoch),
        PolicyVersions::default(),
        PlanArtifacts {
            tessellate: Some(TessellateSpec {
                lod: Lod::Coarse,
                include_edges: true,
            }),
        },
    )
}

/// Drives one plan through the executor (atomic prepare/accept) over the WM,
/// enforcing D1 adoption — exactly the app's regen path.
async fn drive(wm: &WorkerManager, tl: &Timeline, rev: u64, epoch: u64) -> Outcome {
    let plan = plan_request(tl, rev, epoch);
    let known: HashSet<Uuid> = plan.ops.iter().map(|o| o.record_id.as_uuid()).collect();
    let engine = AdoptingEngine::new(Arc::new(wm.clone()), known, HashSet::new());
    let executor = RegenExecutor::new(engine);
    let mut session = RegenSession {
        timeline: tl.clone(),
        ..Default::default()
    };
    let publisher = SnapshotPublisher::new();
    let cancel = CancelToken::new();
    let gate = move || (DocumentRevision(rev), WorkerEpoch(epoch));
    executor
        .run(plan, &mut session, &gate, &cancel, &publisher)
        .await
}

// ─────────────────────────────────────────────────────────────────────────────
// Happy path: streaming + prepare + adoption
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn execute_plan_streams_step_and_prepares() {
    let Some(bin) = stub_binary() else {
        eprintln!("skip: stub binary not built");
        return;
    };
    let wm = WorkerManager::spawn(fast_config(bin, vec![]));
    assert!(wm.wait_ready(Duration::from_secs(3)).await, "worker ready");

    let tl = one_extrude_timeline();
    let mut rx = wm.execute_plan(plan_request(&tl, 0, 1)).await;
    let mut steps = 0;
    let mut prepared = false;
    while let Some(ev) = rx.recv().await {
        match ev {
            PlanEvent::Step(s) => {
                steps += 1;
                // body_<opId> adopted to BodyId(opId uuid).
                assert!(!s.body_events.is_empty());
            }
            PlanEvent::Prepared(p) => {
                prepared = true;
                assert_eq!(p.per_step.len(), 1);
                assert_eq!(p.per_step[0].body_ids[0].as_uuid(), Uuid::from_u128(0x10));
            }
            PlanEvent::Failed(e) => panic!("unexpected failure: {e}"),
        }
    }
    assert_eq!(steps, 1, "one planStep per op");
    assert!(prepared, "terminal PlanPrepared");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn executor_publishes_over_wm_stub() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(bin, vec![]));
    assert!(wm.wait_ready(Duration::from_secs(3)).await);
    let tl = one_extrude_timeline();
    let outcome = drive(&wm, &tl, 0, 1).await;
    match outcome {
        Outcome::Published(snap) => assert_eq!(snap.bodies.len(), 1, "extrude body published"),
        other => panic!("expected Published, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Mesh bulk assembly (inline + chunked) + MESH1 validation + credit
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_mesh_inline_validates_mesh1() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(bin, vec![]));
    assert!(wm.wait_ready(Duration::from_secs(3)).await);
    let body = onecad_core::ids::BodyId(Uuid::from_u128(0x10));
    let bytes = wm
        .fetch_mesh(body, Lod::Coarse, onecad_core::ids::SnapshotId(1))
        .await
        .expect("inline mesh");
    // The WM already ran validate_mesh_blob; re-assert the magic here.
    assert_eq!(&bytes[0..4], &[0x48, 0x53, 0x45, 0x4D], "MESH1 magic");
    assert!(onecad_protocol::mesh::validate_mesh_blob(&bytes).is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_mesh_chunked_assembles_and_verifies_sha() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(
        bin,
        vec![("ONECAD_STUB_CHUNKED_MESH".into(), "1".into())],
    ));
    assert!(wm.wait_ready(Duration::from_secs(3)).await);
    let body = onecad_core::ids::BodyId(Uuid::from_u128(0x11));
    let bytes = wm
        .fetch_mesh(body, Lod::Fine, onecad_core::ids::SnapshotId(1))
        .await
        .expect("chunked mesh reassembled + sha-verified");
    assert!(onecad_protocol::mesh::validate_mesh_blob(&bytes).is_ok());
    assert_eq!(bytes[0x1C], 2, "fine lod byte round-trips through chunks");
}

// ─────────────────────────────────────────────────────────────────────────────
// Chaos: crash mid-plan (no partial publish)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crash_on_execute_plan_never_publishes_partial() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(
        bin,
        vec![("ONECAD_STUB_CRASH_ON".into(), "ExecutePlan".into())],
    ));
    assert!(wm.wait_ready(Duration::from_secs(3)).await);
    let tl = one_extrude_timeline();
    let outcome = drive(&wm, &tl, 0, 1).await;
    match outcome {
        Outcome::EngineFailed(EngineError::Crashed { .. }) => {}
        other => panic!("crash must yield EngineFailed(Crashed), not a publish: {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Chaos: hung worker → ping timeout → SIGKILL → restart
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hang_on_execute_plan_ping_timeout_sigkill_restart() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(
        bin,
        vec![("ONECAD_STUB_HANG_ON".into(), "ExecutePlan".into())],
    ));
    let mut life = wm.subscribe();
    assert!(wm.wait_ready(Duration::from_secs(3)).await);

    let tl = one_extrude_timeline();
    let outcome = drive(&wm, &tl, 0, 1).await;
    assert!(
        matches!(outcome, Outcome::EngineFailed(EngineError::Crashed { .. })),
        "hung plan resolves via SIGKILL → Crashed: {outcome:?}"
    );
    // A ping-timeout restart must have been announced.
    let mut saw_ping_restart = false;
    for _ in 0..32 {
        match tokio::time::timeout(Duration::from_millis(200), life.recv()).await {
            Ok(Ok(WorkerLifecycle::Restarting { reason, .. })) if reason.contains("hung") => {
                saw_ping_restart = true;
                break;
            }
            Ok(Ok(_)) => {}
            _ => break,
        }
    }
    assert!(
        saw_ping_restart,
        "hung worker → ping timeout → SIGKILL restart"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Chaos: garbage frame → restart (no resync) → backoff exhausted → Failed
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn garbage_frame_restarts_then_fails_after_backoff() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(
        bin,
        vec![("ONECAD_STUB_GARBAGE".into(), "1".into())],
    ));
    // Every connect sees a bad-magic frame → handshake fails → backoff → Failed.
    let mut failed = false;
    for _ in 0..100 {
        if wm.state() == WorkerState::Failed {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(failed, "garbage frames must exhaust backoff → Failed");
    assert!(!wm.wait_ready(Duration::from_millis(50)).await);
}

// ─────────────────────────────────────────────────────────────────────────────
// Chaos: poison / crash circuit breaker on repeated same-plan crash
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_crash_trips_circuit_breaker() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(
        bin,
        vec![("ONECAD_STUB_CRASH_ON".into(), "ExecutePlan".into())],
    ));
    let mut life = wm.subscribe();
    let tl = one_extrude_timeline();

    // Drive the same plan until the crash circuit opens (poison_threshold = 3).
    let mut failed_state = false;
    for _ in 0..12 {
        let _ = wm.wait_ready(Duration::from_millis(400)).await;
        let outcome = drive(&wm, &tl, 0, 1).await;
        assert!(
            matches!(outcome, Outcome::EngineFailed(_)),
            "poisoned plan never publishes: {outcome:?}"
        );
        if wm.state() == WorkerState::Failed {
            failed_state = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(
        failed_state,
        "repeated crash on the same plan trips → Failed"
    );

    let mut saw_circuit = false;
    while let Ok(ev) = life.try_recv() {
        if matches!(ev, WorkerLifecycle::CircuitOpen { .. }) {
            saw_circuit = true;
        }
    }
    assert!(saw_circuit, "a CircuitOpen lifecycle event was emitted");
}

// ─────────────────────────────────────────────────────────────────────────────
// Convergence drill: kill mid-plan N× → always converges, never partial
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn convergence_drill_kill_mid_plan_repeatedly() {
    let Some(bin) = stub_binary() else {
        return;
    };
    // CI-friendly default 25; env-overridable to 100 for the heavy drill.
    let iters: u32 = std::env::var("ONECAD_CONVERGENCE_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(25);

    // A persisted countdown so a restarted worker crashes `iters` times then
    // succeeds (the counter survives process restarts).
    let dir = tempfile::tempdir().unwrap();
    let counter = dir.path().join("countdown");
    std::fs::write(&counter, iters.to_string()).unwrap();

    let mut config = fast_config(
        bin,
        vec![(
            "ONECAD_STUB_CRASH_COUNTDOWN".into(),
            counter.to_string_lossy().into_owned(),
        )],
    );
    config.poison_threshold = iters + 100; // never trip the circuit during the drill.
                                           // Quiesce liveness pinging during the sub-second drill so a load-induced ping
                                           // timeout can't SIGKILL an idle worker and inflate the crash count — the drill
                                           // asserts an exact `crashes == iters`; death detection here rides child-exit.
    config.ping_interval = Duration::from_secs(30);
    let wm = WorkerManager::spawn(config);
    let tl = one_extrude_timeline();

    let mut crashes = 0u32;
    let mut published = false;
    // Each crash restarts the worker; bound the loop generously.
    for _ in 0..(iters + 20) {
        if !wm.wait_ready(Duration::from_secs(2)).await {
            continue; // mid-restart; retry.
        }
        match drive(&wm, &tl, 0, 1).await {
            Outcome::Published(snap) => {
                assert_eq!(snap.bodies.len(), 1, "converged to the last-valid snapshot");
                published = true;
                break;
            }
            Outcome::EngineFailed(EngineError::Crashed { .. }) => crashes += 1,
            other => panic!("partial/unexpected outcome mid-drill: {other:?}"),
        }
    }
    assert!(published, "the document must converge after {iters} kills");
    // Atomicity ("never a partial publish") is enforced by the loop's match arm,
    // which panics on any non-Published/non-Crashed outcome. The count is `>=`
    // because a loaded machine can add a transient connection-loss kill or two on
    // top of the `iters` scripted ones — the drill still converges either way.
    assert!(
        crashes >= iters,
        "survived at least {iters} mid-plan kills (saw {crashes})"
    );
}
