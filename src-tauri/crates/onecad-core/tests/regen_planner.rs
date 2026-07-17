//! R-WP7 golden fixtures for the pure [`RegenPlanner`] + checkpoint selection:
//!
//! * (i) determinism — same inputs ⇒ identical plan + history-prefix hash;
//! * (g) checkpoint restore — a compatible, hash-matching checkpoint accelerates
//!   the base, and the executor folds the remaining steps onto it (lifecycle
//!   correctness across restart);
//! * (h) corrupt/incompatible checkpoint — fallback to replay-from-0
//!   (correctness never depends on the cache — Invariant 7).

mod support;

use std::collections::BTreeMap;

use uuid::Uuid;

use onecad_core::document::body::{BodyLifecycleEvent, BodyRegistry};
use onecad_core::document::element_index::ElementIndex;
use onecad_core::history::{DependencyGraph, StepState, Timeline};
use onecad_core::ids::{BodyId, DocumentRevision, JobId, WorkerEpoch};
use onecad_core::regen::{
    history_prefix_hash, CancelToken, CheckpointArtifact, CheckpointArtifacts, CheckpointEnvelope,
    CheckpointStore, HistoryPrefixHash, InMemoryCheckpointStore, OpFailureCode, Outcome,
    PlanArtifacts, PolicyVersions, RegenExecutor, RegenPlanner, RegenRequest, RegenSession,
    SnapshotPublisher, StoppedReason,
};

use support::*;

const JOB: JobId = JobId(Uuid::from_u128(0x205));
const REV: DocumentRevision = DocumentRevision(7);
const EPOCH: WorkerEpoch = WorkerEpoch(3);

/// Builds a compatible checkpoint envelope for `step` whose stored history-prefix
/// hash matches `records[0..=step]` (so the planner accepts it).
fn compatible_envelope(step: usize, tl: &Timeline) -> CheckpointEnvelope {
    CheckpointEnvelope {
        artifact_schema_version: 1,
        body: BodyId(Uuid::from_u128(0xB0)),
        step,
        history_prefix_hash: history_prefix_hash(&tl.records()[0..=step]),
        brep_content_hash: "aa".into(),
        occt_fingerprint: "fp".into(), // matches default_ctx()
        descriptor_version: 1,
        resolver_version: 1,
        quantization_version: 1,
        signature_version: 1,
        codec: "brep-bintools".into(),
        size: 10,
        content_hash: "bb".into(),
    }
}

fn artifacts_for(step: usize, envelope: CheckpointEnvelope) -> CheckpointArtifacts {
    CheckpointArtifacts {
        step,
        artifacts: vec![CheckpointArtifact {
            envelope: envelope.clone(),
            bytes: vec![1, 2, 3],
        }],
        element_map_partition: vec![],
        signatures: sigs(step),
        history_prefix_hash: envelope.history_prefix_hash,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// (i) determinism
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn planner_is_deterministic() {
    let tl = timeline_of(4);
    let g = DependencyGraph::new();
    let a = RegenPlanner::plan(
        &tl,
        &g,
        &[],
        RegenRequest::ToEnd { from: 0 },
        &default_ctx(),
    );
    let b = RegenPlanner::plan(
        &tl,
        &g,
        &[],
        RegenRequest::ToEnd { from: 0 },
        &default_ctx(),
    );
    assert_eq!(a, b, "same inputs → identical plan");
    assert_eq!(a.expected_base_hash, b.expected_base_hash);

    // The hash is prefix-sensitive and stable.
    let r = tl.records();
    assert_eq!(history_prefix_hash(&r[0..2]), history_prefix_hash(&r[0..2]));
    assert_ne!(history_prefix_hash(&r[0..2]), history_prefix_hash(&r[0..3]));
}

// ─────────────────────────────────────────────────────────────────────────────
// (g) checkpoint restore path
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn planner_selects_compatible_checkpoint_as_base() {
    let tl = timeline_of(4);
    let mut store = InMemoryCheckpointStore::new();
    // A checkpoint after step 1, compatible + hash-matching.
    store.save(1, artifacts_for(1, compatible_envelope(1, &tl)));

    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &store.list(),
        RegenRequest::ToEnd { from: 2 },
        &default_ctx(),
    );

    // Restored from the step-1 checkpoint ⇒ execute [2, 3] only.
    let restore = plan.restore.as_ref().expect("checkpoint chosen");
    assert_eq!(restore.step_index, 1);
    assert_eq!(plan.start_step, 2);
    assert_eq!(plan.target_step, 3);
    assert_eq!(plan.planned_ops.len(), 2);
    assert_eq!(plan.planned_ops[0].step_index, 2);
    // expected_base_hash is over records[0..2] (Invariant: what the checkpoint stored).
    assert_eq!(
        plan.expected_base_hash,
        history_prefix_hash(&tl.records()[0..2])
    );
}

/// Builds the two-body restored base a step-1 checkpoint represents (its lifecycle
/// log ends at the checkpoint step). The executor seeds scratch from THIS (via
/// `restore_checkpoint`), not from live session state (review F3).
fn restored_base(a: BodyId, b: BodyId) -> BodyRegistry {
    let mut reg = BodyRegistry::new();
    reg.fold(0, rid(0x10), BodyLifecycleEvent::Created { body: a });
    reg.fold(1, rid(0x11), BodyLifecycleEvent::Created { body: b });
    reg
}

#[tokio::test]
async fn executor_folds_remaining_steps_onto_restored_base() {
    // The checkpoint plan starts at step 2; the executor reconstructs the base from
    // the checkpoint artifacts (restore_checkpoint) and folds only steps 2, 3.
    // F3: re-running the same plan does NOT duplicate lifecycle-log entries,
    // because each run reseeds from the immutable restore result — not from the
    // (now-ahead) live registry.
    let tl = timeline_of(4);
    let mut store = InMemoryCheckpointStore::new();
    store.save(1, artifacts_for(1, compatible_envelope(1, &tl)));

    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &store.list(),
        RegenRequest::ToEnd { from: 2 },
        &default_ctx(),
    );
    assert!(plan.restore.is_some(), "precondition: checkpoint chosen");

    let restored_a = BodyId(Uuid::from_u128(0xA0));
    let restored_b = BodyId(Uuid::from_u128(0xB0));
    let restore = RestoreConfig {
        restored: true,
        drift: false,
        base_registry: restored_base(restored_a, restored_b),
        base_elements: ElementIndex::new(),
    };
    let exec = RegenExecutor::new(FakeEngine::all_ok().with_restore(restore));

    // The live session is EMPTY — the executor must reconstruct the base from the
    // checkpoint, not from the session.
    let mut session = RegenSession::with_timeline(tl.clone());
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let req = plan.clone().into_request(
        JOB,
        REV,
        EPOCH,
        PolicyVersions::default(),
        PlanArtifacts::default(),
    );
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;
    assert!(matches!(out, Outcome::Published(_)), "got {out:?}");
    // Restored 2 + folded 2 = 4 active bodies; lifecycle log appended (not
    // duplicated — clear-before-replay respected via the restored base).
    assert_eq!(session.bodies.len(), 4);
    assert_eq!(session.bodies.log().len(), 4, "2 restored + 2 replayed");
    assert!(session.bodies.contains(restored_a));
    assert_eq!(session.timeline.state(2), Some(&StepState::Valid));
    assert_eq!(session.timeline.state(3), Some(&StepState::Valid));

    // F3: re-run the SAME checkpoint plan. The live session is now ahead (4 bodies,
    // log 4), but the executor reseeds from the restore result ⇒ still 4 / log 4,
    // NOT 6 / log 6.
    let req2 = plan.into_request(
        JOB,
        REV,
        EPOCH,
        PolicyVersions::default(),
        PlanArtifacts::default(),
    );
    let out2 = exec
        .run(req2, &mut session, &gate, &cancel, &publisher)
        .await;
    assert!(matches!(out2, Outcome::Published(_)), "got {out2:?}");
    assert_eq!(session.bodies.len(), 4, "no body accumulation on re-run");
    assert_eq!(
        session.bodies.log().len(),
        4,
        "no lifecycle-log duplication on re-run (F3)"
    );
    assert_eq!(exec.engine().restore_calls(), 2, "restore per run");
}

// ─────────────────────────────────────────────────────────────────────────────
// (h) corrupt / incompatible checkpoint → replay-from-0
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn incompatible_fingerprint_checkpoint_falls_back_to_replay_from_zero() {
    let tl = timeline_of(4);
    let mut store = InMemoryCheckpointStore::new();
    // Fingerprint mismatch (worker rebuilt under a different OCCT) ⇒ discard.
    let mut env = compatible_envelope(1, &tl);
    env.occt_fingerprint = "STALE_FP".into();
    store.save(1, artifacts_for(1, env));

    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &store.list(),
        RegenRequest::ToEnd { from: 2 },
        &default_ctx(),
    );

    assert!(plan.restore.is_none(), "incompatible checkpoint discarded");
    assert_eq!(plan.start_step, 0, "replay from empty base");
    assert_eq!(plan.expected_base_hash, HistoryPrefixHash::empty());
    assert_eq!(plan.planned_ops.len(), 4);
}

#[test]
fn stale_prefix_hash_checkpoint_falls_back_to_replay_from_zero() {
    let tl = timeline_of(4);
    let mut store = InMemoryCheckpointStore::new();
    // Envelope compatible, but the stored history-prefix hash no longer matches
    // records[0..=1] (an upstream record was edited) ⇒ stale ⇒ discard.
    let mut env = compatible_envelope(1, &tl);
    env.history_prefix_hash = HistoryPrefixHash::new("0000stalehash0000");
    let mut artifacts = artifacts_for(1, env.clone());
    artifacts.history_prefix_hash = env.history_prefix_hash.clone();
    store.save(1, artifacts);

    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &store.list(),
        RegenRequest::ToEnd { from: 2 },
        &default_ctx(),
    );

    assert!(plan.restore.is_none(), "stale checkpoint discarded");
    assert_eq!(plan.start_step, 0);
    assert_eq!(plan.planned_ops.len(), 4);
}

#[tokio::test]
async fn replay_from_zero_produces_same_result_as_the_naive_baseline() {
    // Invariant 7: an unusable cache degrades performance, never correctness. The
    // fallback plan replays everything and still succeeds identically.
    let tl = timeline_of(3);
    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &[], // no checkpoints
        RegenRequest::ToEnd { from: 0 },
        &default_ctx(),
    );
    let req = plan.into_request(
        JOB,
        REV,
        EPOCH,
        PolicyVersions::default(),
        PlanArtifacts::default(),
    );

    let exec = RegenExecutor::new(FakeEngine::all_ok());
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    match out {
        Outcome::Published(s) => {
            assert_eq!(s.stopped_reason, StoppedReason::Completed);
            assert_eq!(session.bodies.len(), 3);
        }
        other => panic!("expected Published, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// (F12) Invariant 7: a broken checkpoint transparently replays from 0 (once)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn checkpoint_restore_drift_retries_from_zero() {
    // A compatible, hash-matching checkpoint is chosen, but the restore reports
    // DRIFT (worker's restored base disagrees) ⇒ the checkpoint is unusable ⇒ the
    // executor strips it and replays from 0, transparently succeeding (F12).
    let tl = timeline_of(4);
    let mut store = InMemoryCheckpointStore::new();
    store.save(1, artifacts_for(1, compatible_envelope(1, &tl)));
    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &store.list(),
        RegenRequest::ToEnd { from: 2 },
        &default_ctx(),
    );
    assert!(plan.restore.is_some(), "precondition: checkpoint chosen");
    let req = plan.into_request(
        JOB,
        REV,
        EPOCH,
        PolicyVersions::default(),
        PlanArtifacts::default(),
    );

    let restore = RestoreConfig {
        restored: true,
        drift: true,
        base_registry: BodyRegistry::new(),
        base_elements: ElementIndex::new(),
    };
    let exec = RegenExecutor::new(FakeEngine::all_ok().with_restore(restore));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    assert!(
        matches!(out, Outcome::Published(_)),
        "from-0 retry succeeds transparently, got {out:?}"
    );
    // Replay-from-0 executed all 4 steps ⇒ 4 bodies.
    assert_eq!(session.bodies.len(), 4);
    let log = exec.engine().log();
    // The checkpoint attempt aborted at restore (never reached execute_plan); only
    // the from-0 retry did — and it carried NO checkpoint.
    assert_eq!(
        log.plans.len(),
        1,
        "only the from-0 retry reached execute_plan"
    );
    assert!(
        log.plans[0].base_checkpoint.is_none(),
        "retry stripped the checkpoint"
    );
    assert_eq!(log.plans[0].ops.len(), 4, "retry replays all 4 steps");
    assert_eq!(exec.engine().restore_calls(), 1, "restore attempted once");
}

#[tokio::test]
async fn checkpoint_op_failure_does_not_retry_from_zero() {
    // A real op failure (a step event WAS received) must NOT trigger the from-0
    // retry — it surfaces as an accepted m-1 snapshot (F12: retries are only for
    // failures BEFORE any step event).
    let tl = timeline_of(4);
    let mut store = InMemoryCheckpointStore::new();
    store.save(1, artifacts_for(1, compatible_envelope(1, &tl)));
    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &store.list(),
        RegenRequest::ToEnd { from: 2 },
        &default_ctx(),
    );
    let req = plan.into_request(
        JOB,
        REV,
        EPOCH,
        PolicyVersions::default(),
        PlanArtifacts::default(),
    );

    let restore = RestoreConfig {
        restored: true,
        drift: false,
        base_registry: restored_base(BodyId(Uuid::from_u128(0xA0)), BodyId(Uuid::from_u128(0xB0))),
        base_elements: ElementIndex::new(),
    };
    let mut scripts = BTreeMap::new();
    scripts.insert(
        3,
        StepScript::Fail {
            code: OpFailureCode::GeometryInvalid,
        },
    );
    let exec = RegenExecutor::new(FakeEngine::new(scripts).with_restore(restore));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    let snap = match out {
        Outcome::Published(s) => s,
        other => panic!("op failure → accepted m-1, got {other:?}"),
    };
    assert_eq!(snap.stopped_reason, StoppedReason::OpFailed);
    let log = exec.engine().log();
    assert_eq!(log.plans.len(), 1, "no from-0 retry on a real op failure");
    assert!(
        log.plans[0].base_checkpoint.is_some(),
        "still the checkpoint plan (not stripped)"
    );
    assert_eq!(exec.engine().restore_calls(), 1, "restored once, no retry");
}
