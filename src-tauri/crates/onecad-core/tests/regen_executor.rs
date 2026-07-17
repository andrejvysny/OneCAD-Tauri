//! R-WP7 golden fixtures for the [`RegenExecutor`], driven by the scripted
//! [`FakeEngine`] (`tests/support`). These enforce the ExecutePlan contract
//! (SCHEMA §7.2), the failure policy (V1/V2 §4.4) and the invariants (SCHEMA §11):
//!
//! * (a) happy path — all `Valid`, one snapshot published, registry folded,
//!   accept fenced correctly;
//! * (b) forced ambiguity — `NeedsRepair` STATE (NEVER an `Err`), repair state
//!   populated, downstream `Dirty`, `m−1` snapshot accepted (corpus case `f`);
//! * (c) op failure at `m` — `Error` at `m`, `m−1` snapshot, downstream `Dirty`;
//! * (d) engine crash — discard, all plan steps `Dirty`, session JSON intact;
//! * (e) revision superseded — discard + `Superseded`, nothing published;
//! * (f) cancellation — pre-cancel and mid-stream (`select!` wakeup);
//! * (j) body identity across split/merge (V1/V2 §2.2).

mod support;

use std::collections::BTreeMap;
use std::sync::Arc;

use uuid::Uuid;

use onecad_core::document::body::BodyLifecycleEvent;
use onecad_core::document::refs::ElementKind;
use onecad_core::document::repair::{LadderLevel, RepairCandidate, RepairItem, RepairReason};
use onecad_core::document::Document;
use onecad_core::history::StepState;
use onecad_core::ids::{BodyId, DocumentId, DocumentRevision, JobId, TopoKey, WorkerEpoch};
use onecad_core::math::Vec3;
use onecad_core::regen::{
    CancelToken, ElementMapDelta, ElementMapEntry, EngineError, Fencing, ModelSnapshot,
    OpFailureCode, Outcome, RegenExecutor, RegenRequest, RegenSession, SnapshotPublisher,
    StoppedReason,
};

use support::*;

const JOB: JobId = JobId(Uuid::from_u128(0x105));
const REV: DocumentRevision = DocumentRevision(7);
const EPOCH: WorkerEpoch = WorkerEpoch(3);

/// A symmetric-tie NeedsRepair item (corpus case `f`: 0.91/0.91, margin 0.00 ⇒
/// NeedsRepair, never a guess).
fn symmetric_repair_item(step: usize, ref_id: &str) -> RepairItem {
    RepairItem {
        step_index: step,
        ref_id: ref_id.into(),
        element_id: None,
        ladder_failed: LadderLevel::Descriptor,
        reason: RepairReason::Ambiguous,
        candidates: vec![
            RepairCandidate {
                topo_key: TopoKey::new("f:31"),
                score: 0.91,
                margin: 0.0,
                world_pos: Vec3::new_unchecked(12.0, 3.5, 0.0),
                summary: "left twin".into(),
                extra: Default::default(),
            },
            RepairCandidate {
                topo_key: TopoKey::new("f:44"),
                score: 0.91,
                margin: 0.0,
                world_pos: Vec3::new_unchecked(12.0, -3.5, 0.0),
                summary: "right twin".into(),
                extra: Default::default(),
            },
        ],
        anchor: None,
        ui_label: "Fillet edge on right pocket".into(),
        scoring_version: None,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// (a) happy path
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn happy_path_three_ops_all_valid() {
    let tl = timeline_of(3);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let exec = RegenExecutor::new(FakeEngine::all_ok());
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    let snap = match out {
        Outcome::Published(s) => s,
        other => panic!("expected Published, got {other:?}"),
    };
    assert_eq!(snap.stopped_reason, StoppedReason::Completed);
    assert_eq!(snap.step_index, Some(2));
    assert_eq!(snap.bodies.len(), 3, "registry folded 3 bodies");
    // All bodies share the publish generation (Invariant 4).
    for b in &snap.bodies {
        assert_eq!(b.mesh_key.generation, snap.generation);
    }
    // Timeline all Valid; registry has 3 bodies.
    for s in 0..3 {
        assert_eq!(
            session.timeline.state(s),
            Some(&StepState::Valid),
            "step {s}"
        );
    }
    assert_eq!(session.bodies.len(), 3);

    // Accept called once with the plan's fencing tokens; no discard.
    let log = exec.engine().log();
    assert_eq!(log.plans.len(), 1);
    assert_eq!(log.accepts.len(), 1);
    assert_eq!(
        log.accepts[0].1,
        Fencing {
            document_revision: REV,
            worker_epoch: EPOCH
        }
    );
    assert!(log.discards.is_empty());
    assert!(publisher.latest().is_some(), "snapshot published");
    // The plan the worker received carried the empty-base expected hash.
    assert_eq!(
        log.plans[0].expected_base_hash,
        onecad_core::regen::HistoryPrefixHash::empty()
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// (b) forced ambiguity → NeedsRepair STATE (never Err)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn needs_repair_at_step_two_is_state_never_error() {
    let tl = timeline_of(4);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let mut scripts = BTreeMap::new();
    scripts.insert(
        2,
        StepScript::NeedsRepair {
            items: vec![symmetric_repair_item(2, "op_2.input0")],
            body_events: vec![],
        },
    );
    let exec = RegenExecutor::new(FakeEngine::new(scripts));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    // NEVER an Err/EngineFailed — NeedsRepair is STATE, still a Published prepare.
    let snap = match out {
        Outcome::Published(s) => s,
        other => panic!("NeedsRepair must be Published state, got {other:?}"),
    };
    assert_eq!(snap.stopped_reason, StoppedReason::NeedsRepair);
    assert_eq!(snap.step_index, Some(1), "accepted the m-1 snapshot");

    assert_eq!(session.timeline.state(0), Some(&StepState::Valid));
    assert_eq!(session.timeline.state(1), Some(&StepState::Valid));
    assert_eq!(session.timeline.state(2), Some(&StepState::NeedsRepair));
    assert_eq!(session.timeline.state(3), Some(&StepState::Dirty));

    // RepairState populated at step 2 with the symmetric-tie candidates.
    let items = session.repair.items_for_step(2);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].candidates.len(), 2);
    assert_eq!(items[0].candidates[0].margin, 0.0, "symmetric tie");
    assert_eq!(snap.repair_summary.needs_repair_count, 1);
    assert_eq!(snap.repair_summary.steps, vec![2]);

    // The m-1 prepared snapshot was still accepted (SCHEMA §8 failure contract).
    assert_eq!(exec.engine().log().accepts.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// (c) op failure at m
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn op_failure_marks_error_and_publishes_m_minus_one() {
    let tl = timeline_of(3);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let mut scripts = BTreeMap::new();
    scripts.insert(
        1,
        StepScript::Fail {
            code: OpFailureCode::GeometryInvalid,
        },
    );
    let exec = RegenExecutor::new(FakeEngine::new(scripts));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    let snap = match out {
        Outcome::Published(s) => s,
        other => panic!("expected Published (accepted m-1), got {other:?}"),
    };
    assert_eq!(snap.stopped_reason, StoppedReason::OpFailed);
    assert_eq!(
        snap.step_index,
        Some(0),
        "m-1 snapshot for a failure at m=1"
    );

    assert_eq!(session.timeline.state(0), Some(&StepState::Valid));
    assert!(
        matches!(session.timeline.state(1), Some(StepState::Error { .. })),
        "failed step is Error"
    );
    assert_eq!(session.timeline.state(2), Some(&StepState::Dirty));
    assert_eq!(exec.engine().log().accepts.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// (d) engine crash mid-plan → discard, all Dirty, session intact
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn engine_crash_discards_and_leaves_session_intact() {
    // Build an authoritative Document and snapshot its JSON.
    let mut doc = Document::new(DocumentId(Uuid::from_u128(0x9)));
    for i in 0..3u128 {
        doc.timeline
            .insert_at_cursor(extrude_record(0x10 + i, 5.0 + i as f64));
    }
    let before = serde_json::to_value(&doc).unwrap();

    let req = plan_request(
        &doc.timeline,
        RegenRequest::ToEnd { from: 0 },
        JOB,
        REV,
        EPOCH,
    );
    let mut scripts = BTreeMap::new();
    scripts.insert(1, StepScript::Crash);
    let exec = RegenExecutor::new(FakeEngine::new(scripts));

    let mut session = RegenSession {
        bodies: doc.bodies.clone(),
        timeline: doc.timeline.clone(),
        repair: doc.repair.clone(),
        elements: doc.elements.clone(),
    };
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    assert!(
        matches!(out, Outcome::EngineFailed(_)),
        "crash → EngineFailed, got {out:?}"
    );
    // Discard called, NO accept, nothing published.
    let log = exec.engine().log();
    assert!(log.discards.contains(&JOB));
    assert!(log.accepts.is_empty());
    assert!(publisher.latest().is_none());

    // All plan steps marked Dirty; body registry untouched (no scratch commit).
    for s in 0..3 {
        assert_eq!(
            session.timeline.state(s),
            Some(&StepState::Dirty),
            "step {s}"
        );
    }
    assert!(session.bodies.is_empty(), "no bodies committed on crash");

    // Session JSON intact (write the session pieces back — states are not
    // persisted, so a crash leaves document.json byte-identical).
    doc.bodies = session.bodies;
    doc.repair = session.repair;
    doc.elements = session.elements;
    doc.timeline = session.timeline;
    let after = serde_json::to_value(&doc).unwrap();
    assert_eq!(before, after, "document JSON must be intact after a crash");
}

// ─────────────────────────────────────────────────────────────────────────────
// (e) revision superseded
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn revision_superseded_discards_without_publishing() {
    let tl = timeline_of(2);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let exec = RegenExecutor::new(FakeEngine::all_ok());
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    // The document advanced under us (rev 8 != plan rev 7).
    let gate = || (DocumentRevision(8), EPOCH);
    let cancel = CancelToken::new();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    assert!(matches!(out, Outcome::Superseded), "got {out:?}");
    let log = exec.engine().log();
    assert!(log.discards.contains(&JOB));
    assert!(log.accepts.is_empty(), "must NOT accept a stale prepare");
    assert!(publisher.latest().is_none());
    assert!(session.bodies.is_empty(), "no commit on supersede");
}

// ─────────────────────────────────────────────────────────────────────────────
// (f) cancellation
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn pre_cancel_takes_cancel_path() {
    let tl = timeline_of(3);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let exec = RegenExecutor::new(FakeEngine::all_ok());
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    cancel.cancel(); // pre-cancel: preempts even buffered events.

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    assert!(matches!(out, Outcome::Cancelled), "got {out:?}");
    let log = exec.engine().log();
    assert_eq!(log.cancels, vec![JOB], "engine.cancel(job) called");
    assert!(log.discards.contains(&JOB));
    assert!(log.accepts.is_empty());
    assert!(publisher.latest().is_none());
}

#[tokio::test]
async fn mid_stream_cancel_wakes_from_await() {
    // Steps 0,1 succeed then step 2 hangs (holds the sender). The executor drains
    // 0,1, parks awaiting the terminal, and is woken by the cancel token.
    let tl = timeline_of(3);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);
    let mut scripts = BTreeMap::new();
    scripts.insert(2, StepScript::Hang);
    let exec = RegenExecutor::new(FakeEngine::new(scripts));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let fut = exec.run(req, &mut session, &gate, &cancel, &publisher);
    tokio::pin!(fut);
    let out = loop {
        tokio::select! {
            biased;
            o = &mut fut => break o,
            _ = tokio::task::yield_now() => cancel.cancel(),
        }
    };

    assert!(matches!(out, Outcome::Cancelled), "got {out:?}");
    let log = exec.engine().log();
    assert!(log.cancels.contains(&JOB));
    assert!(log.accepts.is_empty());
    assert!(publisher.latest().is_none());
}

// ─────────────────────────────────────────────────────────────────────────────
// (j) body identity across split/merge (V1/V2 §2.2)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn body_identity_survives_split_then_merge() {
    let body_a = BodyId(Uuid::from_u128(0xA));
    let body_b = BodyId(Uuid::from_u128(0xB));

    let tl = timeline_of(3);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let mut scripts = BTreeMap::new();
    // step 0: create A.
    scripts.insert(
        0,
        StepScript::Ok {
            body_events: vec![BodyLifecycleEvent::Created { body: body_a }],
            deltas: ElementMapDelta::default(),
            signatures: sigs(0),
        },
    );
    // step 1: split A -> [A (survivor, keeps id), B (new)].
    scripts.insert(
        1,
        StepScript::Ok {
            body_events: vec![BodyLifecycleEvent::Split {
                parent: body_a,
                children: vec![body_a, body_b],
            }],
            deltas: ElementMapDelta::default(),
            signatures: sigs(1),
        },
    );
    // step 2: merge [A, B] -> winner A (B retired/aliased to A).
    scripts.insert(
        2,
        StepScript::Ok {
            body_events: vec![BodyLifecycleEvent::Merged {
                inputs: vec![body_a, body_b],
                winner: body_a,
            }],
            deltas: ElementMapDelta::default(),
            signatures: sigs(2),
        },
    );

    let exec = RegenExecutor::new(FakeEngine::new(scripts));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;
    assert!(matches!(out, Outcome::Published(_)), "got {out:?}");

    // A survived split (kept its id) and won the merge; B is retired, aliased to A.
    assert!(
        session.bodies.contains(body_a),
        "A keeps identity through split+merge"
    );
    assert!(!session.bodies.contains(body_b), "B retired by the merge");
    assert_eq!(
        session.bodies.resolve(body_b),
        body_a,
        "B aliased to the winner A"
    );
    assert_eq!(session.bodies.len(), 1);
    // Lifecycle log recorded all three events in order.
    assert_eq!(session.bodies.log().len(), 3);
}

// ─────────────────────────────────────────────────────────────────────────────
// element-map delta → document ElementIndex (partition folding)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn element_map_delta_folds_into_partition_index() {
    let tl = timeline_of(1);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    // Step 0 creates a body and promotes one face element into the partition.
    let body = default_body_of(rid(0x10));
    let el = elem("el_face_1");
    let mut scripts = BTreeMap::new();
    scripts.insert(
        0,
        StepScript::Ok {
            body_events: vec![created(body)],
            deltas: ElementMapDelta {
                added: vec![ElementMapEntry {
                    element_id: el.clone(),
                    topo_key: TopoKey::new("f:0"),
                    kind: ElementKind::Face,
                    body,
                }],
                removed: vec![],
                relabeled: vec![],
            },
            signatures: sigs(0),
        },
    );

    let exec = RegenExecutor::new(FakeEngine::new(scripts));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;
    assert!(matches!(out, Outcome::Published(_)), "got {out:?}");

    // The element was folded into the document partition index, mapping to the
    // step's body (Invariant 1: the id itself is stable; only partition moves).
    assert_eq!(session.elements.body_of(&el), Some(body));
    assert_eq!(session.elements.len(), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// F19: multi-body step → elements partition by their delta entry's body
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn multi_body_step_partitions_elements_by_entry_body() {
    // One step creates TWO bodies and promotes one element into EACH. Under the old
    // "last-created body" heuristic both elements landed in the last body; F19 makes
    // each element land in the body named by its own delta entry.
    let tl = timeline_of(1);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let body_a = BodyId(Uuid::from_u128(0xA));
    let body_b = BodyId(Uuid::from_u128(0xB));
    let el_a = elem("el_a");
    let el_b = elem("el_b");
    let mut scripts = BTreeMap::new();
    scripts.insert(
        0,
        StepScript::Ok {
            body_events: vec![
                BodyLifecycleEvent::Created { body: body_a },
                BodyLifecycleEvent::Created { body: body_b },
            ],
            deltas: ElementMapDelta {
                added: vec![
                    ElementMapEntry {
                        element_id: el_a.clone(),
                        topo_key: TopoKey::new("f:0"),
                        kind: ElementKind::Face,
                        body: body_a,
                    },
                    ElementMapEntry {
                        element_id: el_b.clone(),
                        topo_key: TopoKey::new("f:1"),
                        kind: ElementKind::Face,
                        body: body_b,
                    },
                ],
                removed: vec![],
                relabeled: vec![],
            },
            signatures: sigs(0),
        },
    );

    let exec = RegenExecutor::new(FakeEngine::new(scripts));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;
    assert!(matches!(out, Outcome::Published(_)), "got {out:?}");

    assert_eq!(session.bodies.len(), 2);
    assert_eq!(
        session.elements.body_of(&el_a),
        Some(body_a),
        "el_a → body_a"
    );
    assert_eq!(
        session.elements.body_of(&el_b),
        Some(body_b),
        "el_b → body_b"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// F6: a failing / NeedsRepair step's body/element events are gated out
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn needs_repair_step_body_events_excluded_from_accepted_registry() {
    // Step 1 needs repair BUT also emits a body event. The accepted registry must
    // exclude that body — only steps ≤ last_valid (= step 0) are committed (F6).
    let tl = timeline_of(3);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let ghost = BodyId(Uuid::from_u128(0xDEAD));
    let mut scripts = BTreeMap::new();
    scripts.insert(
        1,
        StepScript::NeedsRepair {
            items: vec![symmetric_repair_item(1, "op_1.input0")],
            body_events: vec![BodyLifecycleEvent::Created { body: ghost }],
        },
    );

    let exec = RegenExecutor::new(FakeEngine::new(scripts));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    let snap = match out {
        Outcome::Published(s) => s,
        other => panic!("expected Published, got {other:?}"),
    };
    assert_eq!(snap.stopped_reason, StoppedReason::NeedsRepair);
    assert_eq!(snap.step_index, Some(0), "accepted m-1 = step 0");
    // F6: the NeedsRepair step's body event must NOT reach the accepted registry.
    assert!(
        !session.bodies.contains(ghost),
        "failing step's body must be gated out"
    );
    assert_eq!(session.bodies.len(), 1, "only step 0's body committed");
    // The repair state IS still recorded (that is the point of surfacing repair).
    assert_eq!(session.repair.items_for_step(1).len(), 1);
}

// ─────────────────────────────────────────────────────────────────────────────
// F7: cancel wins over a prepared terminal, deterministically
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn cancel_wins_over_buffered_terminal() {
    // Cancel is requested; every event (steps + the prepared terminal) is in flight.
    // The executor MUST return Cancelled — never accept/publish the terminal.
    let tl = timeline_of(2);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let exec = RegenExecutor::new(FakeEngine::all_ok());
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    cancel.cancel();

    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    assert!(matches!(out, Outcome::Cancelled), "got {out:?}");
    let log = exec.engine().log();
    assert!(log.cancels.contains(&JOB), "engine.cancel(job) called");
    assert!(log.accepts.is_empty(), "must NOT accept under cancel");
    assert!(publisher.latest().is_none(), "nothing published");
    assert!(session.bodies.is_empty(), "no commit under cancel");
}

// ─────────────────────────────────────────────────────────────────────────────
// F2: accept_prepared rejection → discard + Dirty + EngineFailed
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn accept_rejection_discards_marks_dirty_and_fails() {
    let tl = timeline_of(2);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let exec = RegenExecutor::new(FakeEngine::all_ok().with_accept_rejection(
        EngineError::Protocol {
            message: "stale fencing token".into(),
        },
    ));
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    assert!(
        matches!(out, Outcome::EngineFailed(EngineError::Protocol { .. })),
        "accept rejection surfaces as EngineFailed, got {out:?}"
    );
    let log = exec.engine().log();
    assert_eq!(log.accepts.len(), 1, "accept was attempted");
    assert!(log.discards.contains(&JOB), "discard after accept failure");
    assert!(publisher.latest().is_none(), "nothing published");
    assert_eq!(session.timeline.state(0), Some(&StepState::Dirty));
    assert_eq!(session.timeline.state(1), Some(&StepState::Dirty));
    assert!(session.bodies.is_empty(), "no commit on accept failure");
}

// ─────────────────────────────────────────────────────────────────────────────
// F10 / X-WP1 item 2: bad opaque history-prefix echo → PROTOCOL_ERROR
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn bad_history_prefix_echo_is_protocol_error() {
    let tl = timeline_of(2);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);

    let exec = RegenExecutor::new(FakeEngine::all_ok().with_forced_bad_hash());
    let mut session = RegenSession::with_timeline(tl);
    let publisher = SnapshotPublisher::new();
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    let out = exec
        .run(req, &mut session, &gate, &cancel, &publisher)
        .await;

    assert!(
        matches!(out, Outcome::EngineFailed(EngineError::Protocol { .. })),
        "echo mismatch is a PROTOCOL_ERROR, got {out:?}"
    );
    let log = exec.engine().log();
    assert!(log.accepts.is_empty(), "must NOT accept on echo mismatch");
    assert!(log.discards.contains(&JOB), "discard on echo mismatch");
    assert!(publisher.latest().is_none(), "nothing published");
}

// ─────────────────────────────────────────────────────────────────────────────
// F21: determinism — same plan twice → identical snapshot except generation
// ─────────────────────────────────────────────────────────────────────────────

async fn run_for_determinism(publisher: &SnapshotPublisher) -> Arc<ModelSnapshot> {
    let tl = timeline_of(3);
    let req = plan_request(&tl, RegenRequest::ToEnd { from: 0 }, JOB, REV, EPOCH);
    let exec = RegenExecutor::new(FakeEngine::all_ok());
    let mut session = RegenSession::with_timeline(tl);
    let gate = move || (REV, EPOCH);
    let cancel = CancelToken::new();
    match exec.run(req, &mut session, &gate, &cancel, publisher).await {
        Outcome::Published(s) => s,
        other => panic!("expected Published, got {other:?}"),
    }
}

#[tokio::test]
async fn same_plan_twice_publishes_identical_snapshot_excluding_generation() {
    // A shared publisher makes the two publishes differ ONLY in the monotonic
    // generation. Everything geometry/state-bearing must be byte-identical
    // (Invariant 5): stopped reason, last-valid step, per-step states, the three
    // signatures, the repair summary, and the bodies (registry) modulo generation.
    let publisher = SnapshotPublisher::new();
    let a = run_for_determinism(&publisher).await;
    let b = run_for_determinism(&publisher).await;

    assert_ne!(
        a.generation, b.generation,
        "generation is the ONE difference"
    );
    assert_eq!(a.stopped_reason, b.stopped_reason);
    assert_eq!(a.step_index, b.step_index);
    assert_eq!(a.step_states, b.step_states);
    assert_eq!(a.signatures, b.signatures);
    assert_eq!(a.repair_summary, b.repair_summary);
    // Bodies (registry JSON) modulo the per-publish generation on the mesh key.
    let bodies = |s: &Arc<ModelSnapshot>| {
        s.bodies
            .iter()
            .map(|bd| (bd.body, bd.signature.clone(), bd.visible, bd.mesh_key.lod))
            .collect::<Vec<_>>()
    };
    assert_eq!(bodies(&a), bodies(&b), "identical bodies modulo generation");
}
