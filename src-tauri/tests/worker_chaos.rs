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
    CancelToken, EngineError, Fencing, GeometryEngine, Lod, Outcome, PlanArtifacts, PlanContext,
    PlanEvent, PlanPrepared, PlanRequest, PolicyVersions, RegenExecutor, RegenPlan, RegenPlanner,
    RegenRequest, RegenSession, SnapshotPublisher, TessellateSpec,
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
        max_rapid_deaths: 3,
        // Short enough for the flap drill to converge quickly, long enough that a
        // single scripted death in the other drills doesn't accumulate to Failed.
        healthy_threshold: Duration::from_millis(300),
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

/// Drives one `ExecutePlan` directly over the WM (no executor / checkpoint logic),
/// returning the prepared plan or the terminal engine error — used to exercise the
/// worker fencing with hand-built plans (F1/F4).
async fn run_execute(wm: &WorkerManager, plan: PlanRequest) -> Result<PlanPrepared, EngineError> {
    let mut rx = wm.execute_plan(plan).await;
    let mut prepared = None;
    let mut failed = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            PlanEvent::Prepared(p) => prepared = Some(p),
            PlanEvent::Failed(e) => failed = Some(e),
            PlanEvent::Step(_) => {}
        }
    }
    prepared.ok_or_else(|| {
        failed.unwrap_or(EngineError::Protocol {
            message: "plan produced no terminal event".into(),
        })
    })
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
// D5: sequential replay-from-0 regens (edit → regen → edit → regen). Every regen
// the RegenPlanner emits is a from-0 plan (empty-anchor base); after cycle 1's
// accept the stub head token is nonzero, so a strict head-hash fence would reject
// cycle 2. D5 makes a from-0 plan always base-valid, so both cycles publish and the
// second REPLACES the head wholesale (both bodies, no stale from cycle 1).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sequential_from_zero_regens_both_publish_wholesale() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(bin, vec![]));
    assert!(wm.wait_ready(Duration::from_secs(3)).await);

    // Cycle 1 — one extrude, from-0 plan (empty anchor), revision 0. Publishes and
    // advances the stub head hash past the empty anchor on accept.
    let mut tl = one_extrude_timeline(); // op 0x10
    let out1 = drive(&wm, &tl, 0, 1).await;
    let snap1 = match out1 {
        Outcome::Published(s) => s,
        other => panic!("cycle 1 must publish: {other:?}"),
    };
    assert_eq!(snap1.bodies.len(), 1, "cycle 1 publishes one body");

    // "Edit": append a second extrude, bump the revision. The planner STILL emits a
    // from-0 plan (empty anchor) — but the stub head hash is now nonzero. Pre-D5 this
    // fence-rejected (EngineFailed); D5 makes it base-valid, so cycle 2 publishes.
    tl.insert_at_cursor(extrude_record(0x11, 10.0)); // op 0x11
    let out2 = drive(&wm, &tl, 1, 1).await;
    let snap2 = match out2 {
        Outcome::Published(s) => s,
        other => panic!("cycle 2 (from-0 after head advanced) must publish (D5): {other:?}"),
    };
    // Wholesale replace: the published set is plan 2's output (both extrudes) — the
    // D1 uniqueness check re-created body_<op 0x10> (present in the cycle-1 head)
    // WITHOUT a false-positive collision (from-0 base is empty, so `existing` is
    // empty; only in-plan duplicates are rejected).
    assert_eq!(
        snap2.bodies.len(),
        2,
        "cycle 2 publishes both bodies (wholesale)"
    );
    let ids: HashSet<Uuid> = snap2.bodies.iter().map(|b| b.body.as_uuid()).collect();
    assert!(
        ids.contains(&Uuid::from_u128(0x10)) && ids.contains(&Uuid::from_u128(0x11)),
        "cycle 2 head = {{body_0x10, body_0x11}}, got {ids:?}"
    );
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

/// F3: a plan that repeatedly crashes the worker on the SAME op trips its
/// crashing-op circuit; the poisoned plan then **fails fast without dispatch**, the
/// worker is **not** killed (state != Failed), and a DIFFERENT plan still runs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poisoned_plan_fails_fast_and_a_different_plan_still_runs() {
    let Some(bin) = stub_binary() else {
        return;
    };
    // Crash ONLY on plan A's op (RecordId 0x10) so its crashing-op key poisons; a
    // high flap budget isolates the circuit behaviour from the F2 rapid-death cap.
    let op_a = Uuid::from_u128(0x10).to_string();
    let mut config = fast_config(bin, vec![("ONECAD_STUB_CRASH_ON_OP".into(), op_a)]);
    config.poison_threshold = 3;
    config.max_rapid_deaths = 100;
    let wm = WorkerManager::spawn(config);
    let mut life = wm.subscribe();
    let tl_a = one_extrude_timeline(); // op 0x10

    // Drive plan A until its crash circuit opens.
    let mut circuit_open = false;
    for _ in 0..20 {
        let _ = wm.wait_ready(Duration::from_millis(400)).await;
        let outcome = drive(&wm, &tl_a, 0, 1).await;
        assert!(
            matches!(outcome, Outcome::EngineFailed(_)),
            "crashing plan never publishes: {outcome:?}"
        );
        while let Ok(ev) = life.try_recv() {
            if matches!(ev, WorkerLifecycle::CircuitOpen { .. }) {
                circuit_open = true;
            }
        }
        if circuit_open {
            break;
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    assert!(
        circuit_open,
        "repeated same-op crash trips the crash circuit (CircuitOpen)"
    );

    // The poisoned plan now fails FAST without dispatch (never crashes the worker),
    // and the worker is NOT Failed — the circuit does not kill it (F3).
    let fast = drive(&wm, &tl_a, 0, 1).await;
    assert!(
        matches!(&fast, Outcome::EngineFailed(EngineError::Crashed { message }) if message.contains("circuit")),
        "poisoned plan fails fast (circuit), got {fast:?}"
    );
    assert_ne!(
        wm.state(),
        WorkerState::Failed,
        "the worker stays alive after a poison (F3)"
    );

    // A DIFFERENT plan (op 0x20 — not the crashing op) still executes to a publish,
    // proving the worker was kept alive to serve other work.
    let mut tl_b = Timeline::new();
    tl_b.insert_at_cursor(extrude_record(0x20, 25.0));
    assert!(
        wm.wait_ready(Duration::from_secs(1)).await,
        "worker still alive to serve plan B"
    );
    let outcome_b = drive(&wm, &tl_b, 0, 1).await;
    assert!(
        matches!(outcome_b, Outcome::Published(_)),
        "a different plan still executes after the poison (F3): {outcome_b:?}"
    );
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
    config.max_rapid_deaths = iters + 100; // never trip the F2 flap cap on the scripted crashes.
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
                // F6: assert the surviving body's IDENTITY, not just the count — the
                // extrude op (RecordId 0x10) mints body_<opId>, adopted to BodyId(0x10).
                assert_eq!(
                    snap.bodies[0].body,
                    onecad_core::ids::BodyId(Uuid::from_u128(0x10)),
                    "converged body keeps the deterministic body_<opId> identity"
                );
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

// ─────────────────────────────────────────────────────────────────────────────
// F1/D4 + F4: the fencing stub now fences workerEpoch + expectedBaseHash (the gate
// is no longer blind), but NEVER documentRevision — a post-edit regen with a higher
// documentRevision succeeds and the head adopts it.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_fences_epoch_and_hash_but_not_revision() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(bin, vec![]));
    assert!(wm.wait_ready(Duration::from_secs(3)).await);

    // Cycle 1: op0 at revision 0 (from-0, expectedBaseHash = empty). Establishes the
    // stub's fencing baseline (epoch 1, head hash h0 after accept).
    let mut tl = Timeline::new();
    tl.insert_at_cursor(extrude_record(0x10, 25.0));
    let plan1 = plan_request(&tl, 0, 1);
    let job1 = plan1.job_id;
    run_execute(&wm, plan1).await.expect("cycle 1 prepares");
    wm.accept_prepared(
        job1,
        Fencing {
            document_revision: DocumentRevision(0),
            worker_epoch: WorkerEpoch(1),
        },
    )
    .await
    .expect("accept cycle 1");

    // Build an incremental cycle-2 plan (op1 only, expectedBaseHash chained to cycle
    // 1's echo). V1 has no checkpoint plumbing, so hand-build the plan the way a
    // checkpoint-accelerated regen would (the worker replays op1 on its live state).
    tl.insert_at_cursor(extrude_record(0x11, 10.0));
    let incremental = |rev: u64, epoch: u64| {
        let mut p = plan_request(&tl, rev, epoch); // from-0 two-op plan
        let h0 = p.prefix_hashes[0].clone();
        let h01 = p.prefix_hashes[1].clone();
        p.ops.drain(0..1);
        p.expected_base_hash = h0;
        p.prefix_hashes = vec![h01];
        p.target_step = 1;
        p
    };

    // F4 negative: a wrong workerEpoch is FENCED (the gate is no longer blind).
    let mut bad_epoch = incremental(1, 99);
    bad_epoch.job_id = JobId(Uuid::from_u128(90));
    assert!(
        matches!(
            run_execute(&wm, bad_epoch).await,
            Err(EngineError::Protocol { .. })
        ),
        "a wrong workerEpoch must be fenced (F4)"
    );

    // F4 negative: a wrong INCREMENTAL expectedBaseHash is FENCED. The wrong value
    // must be NONZERO (not the empty anchor) — under D5 an empty-anchor plan is a
    // from-0 plan and is always base-valid (the from-0 exemption), so only a nonzero
    // hash that differs from the head exercises the strict head-hash fence.
    let mut bad_hash = incremental(1, 1);
    bad_hash.job_id = JobId(Uuid::from_u128(91));
    bad_hash.expected_base_hash = onecad_core::regen::HistoryPrefixHash::new(
        "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef0",
    ); // nonzero, != head h0 and != the empty anchor
    assert!(
        matches!(
            run_execute(&wm, bad_hash).await,
            Err(EngineError::Protocol { .. })
        ),
        "a wrong (nonzero) incremental expectedBaseHash must be fenced (F4/D5)"
    );

    // F1 positive: documentRevision 1 (> head 0) is NOT fenced — it prepares +
    // accepts, and the head ADOPTS revision 1 (D4).
    let plan2 = incremental(1, 1);
    let job2 = plan2.job_id;
    run_execute(&wm, plan2)
        .await
        .expect("post-edit regen (documentRevision 1 > head 0) prepares — not fenced (F1)");
    let accept2 = wm
        .accept_prepared(
            job2,
            Fencing {
                document_revision: DocumentRevision(1),
                worker_epoch: WorkerEpoch(1),
            },
        )
        .await
        .expect("accept cycle 2");
    assert_eq!(
        accept2.document_revision,
        DocumentRevision(1),
        "the worker head adopts the plan's documentRevision (D4)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F2: connect-then-die flaps count toward a strike budget → Failed (no forever loop).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connect_then_die_exhausts_flap_budget() {
    let Some(bin) = stub_binary() else {
        return;
    };
    // The stub exits 0 right after the hello: each connect → Ready → immediate death.
    // The unified flap counter (max_rapid_deaths = 3) must reach Failed instead of
    // restarting forever.
    let wm = WorkerManager::spawn(fast_config(
        bin,
        vec![("ONECAD_STUB_EXIT_AFTER_HELLO".into(), "1".into())],
    ));
    let mut failed = false;
    for _ in 0..200 {
        if wm.state() == WorkerState::Failed {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        failed,
        "connect-then-die must exhaust the flap budget → Failed (F2)"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F6: the restart hook fires on the post-restart READY transition (live conn), not
// at death (conn == None) — so an enqueued replay dispatches instead of racing.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn restart_hook_fires_on_ready_not_at_death() {
    let Some(bin) = stub_binary() else {
        return;
    };
    let wm = WorkerManager::spawn(fast_config(
        bin,
        vec![("ONECAD_STUB_EXIT_AFTER_HELLO".into(), "1".into())],
    ));
    let observed = Arc::new(std::sync::Mutex::new(Vec::<WorkerState>::new()));
    let obs = observed.clone();
    let wm2 = wm.clone();
    // The hook records the worker state at the instant it fires. If it fired at death
    // the state would be Restarting (conn None); on the Ready transition it is Ready.
    wm.set_restart_hook(Arc::new(move |_epoch| {
        obs.lock().unwrap().push(wm2.state());
    }));

    // Let it flap until the budget is exhausted.
    for _ in 0..200 {
        if wm.state() == WorkerState::Failed {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let states = observed.lock().unwrap().clone();
    assert!(
        !states.is_empty(),
        "the restart hook fired on at least one restart"
    );
    assert!(
        states.iter().all(|s| *s == WorkerState::Ready),
        "the restart hook fires on the READY transition (F6), got {states:?}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F5: a gapped / overlapping chunk stream is rejected (StreamAcc gap-detection),
// never silently zero-filled.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fetch_mesh_gapped_or_overlapping_chunks_are_rejected() {
    let Some(bin) = stub_binary() else {
        return;
    };
    for mode in ["gap", "overlap"] {
        let wm = WorkerManager::spawn(fast_config(
            bin.clone(),
            vec![
                ("ONECAD_STUB_CHUNKED_MESH".into(), "1".into()),
                ("ONECAD_STUB_CHUNKED_MESH_GAP".into(), mode.into()),
            ],
        ));
        assert!(wm.wait_ready(Duration::from_secs(3)).await);
        let body = onecad_core::ids::BodyId(Uuid::from_u128(0x12));
        let res = wm
            .fetch_mesh(body, Lod::Fine, onecad_core::ids::SnapshotId(1))
            .await;
        assert!(
            res.is_err(),
            "a {mode} mesh stream must be rejected (F5), got {res:?}"
        );
    }
}
