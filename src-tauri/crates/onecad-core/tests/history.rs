//! History WP4 tests: linear timeline (rollback/dirty) + hand-rolled
//! `DependencyGraph` (Kahn determinism, `produces_before`, suppression,
//! failure, cycle).
//!
//! Behaviors are cited against `OneCAD-CPP/src/app/history/DependencyGraph.cpp`
//! and the corpus case `corpus/cases/h_rollback_dirty_timeline.json`
//! (recorded in `corpus/expected-values/proto_timeline_rollback_dirty.txt`).

use proptest::prelude::*;
use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord, PlaneKind,
    SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::refs::SketchRegionRef;
use onecad_core::document::variables::Scalar;
use onecad_core::error::DomainError;
use onecad_core::history::{DependencyGraph, StepState, Timeline};
use onecad_core::ids::{BodyId, RecordId, RegionId, SketchId};
use onecad_core::math::Vec3;

// ── Builders ─────────────────────────────────────────────────────────────────

fn rid(n: u128) -> RecordId {
    RecordId(Uuid::from_u128(0x2EC0_0000 + n))
}
fn bid(n: u128) -> BodyId {
    BodyId(Uuid::from_u128(0xB0D0_0000 + n))
}
fn sid(n: u128) -> SketchId {
    SketchId(Uuid::from_u128(0x5C00_0000 + n))
}

/// An Extrude/NewBody record: no upstream deps, produces `out_body`.
fn extrude_newbody(rec: RecordId, out_body: BodyId, distance: f64) -> OperationRecord {
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
    let mut r = OperationRecord::new(rec, 0, "Extrude", op);
    r.outputs = vec![out_body];
    r
}

/// An Extrude/Cut record: consumes `target_body` (forms a dependency edge to its
/// producer), produces `out_body`.
fn extrude_cut(
    rec: RecordId,
    target_body: BodyId,
    out_body: BodyId,
    distance: f64,
) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: None,
        distance: Scalar::new(distance),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: BooleanMode::Cut,
        target_body: Some(target_body),
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    }));
    let mut r = OperationRecord::new(rec, 0, "Cut", op);
    r.outputs = vec![out_body];
    r
}

/// A Sketch op producing `sketch`.
fn sketch_op(rec: RecordId, sketch: SketchId) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Sketch(SketchOpParams {
        sketch,
        plane: SketchPlaneRef {
            kind: PlaneKind::Xy,
            origin: Vec3::new_unchecked(0.0, 0.0, 0.0),
            x_axis: Vec3::new_unchecked(0.0, 1.0, 0.0),
            y_axis: Vec3::new_unchecked(-1.0, 0.0, 0.0),
            normal: Vec3::new_unchecked(0.0, 0.0, 1.0),
            extra: Default::default(),
        },
        entities: vec![],
        constraints: vec![],
        extra: Default::default(),
    }));
    OperationRecord::new(rec, 0, "Sketch", op)
}

/// An Extrude that consumes `sketch` as its profile (forms a sketch→op edge in
/// the Rust port).
fn extrude_on_sketch(rec: RecordId, sketch: SketchId, out_body: BodyId) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: Some(SketchRegionRef {
            sketch,
            region: RegionId::new("r0"),
            extra: Default::default(),
        }),
        distance: Scalar::new(10.0),
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
    let mut r = OperationRecord::new(rec, 0, "Extrude", op);
    r.outputs = vec![out_body];
    r
}

// ── (a) Corpus case h: rollback + dirty timeline ─────────────────────────────

/// Reproduces `corpus/cases/h_rollback_dirty_timeline.json` on the new linear
/// timeline: op1/op2 added, rollback to op1, op3 inserted AT the cursor
/// (order op1/op3/op2), full regen-to-end. The applied-op-count (cursor)
/// transitions must match the recorded values 2 → 1 → 2 → 3
/// (`proto_timeline_rollback_dirty.cpp:92,102,126,138`).
#[test]
fn corpus_h_rollback_dirty_timeline() {
    let (op1, op2, op3) = (rid(1), rid(2), rid(3));
    let mut tl = Timeline::new();

    // Add op1, op2 (AddOperationCommand inserts at min(applied,len); applied+1).
    let i1 = tl.insert_at_cursor(extrude_newbody(op1, bid(1), 10.0));
    let i2 = tl.insert_at_cursor(extrude_newbody(op2, bid(2), 5.0));
    assert_eq!((i1, i2), (0, 1));
    // afterTwoExtrudes: opCount==2 AND appliedOpCount==2.
    assert_eq!(tl.len(), 2);
    assert_eq!(tl.cursor(), 2, "appliedOpCount after two extrudes");

    // Mark the applied prefix Valid to model a completed regen.
    tl.mark_state(0, StepState::Valid).unwrap();
    tl.mark_state(1, StepState::Valid).unwrap();

    // RollbackCommand(op1): appliedOpCount = index(op1)+1 = 1; ops NOT deleted.
    let dirty = tl.set_cursor(tl.index_of(op1).unwrap() + 1);
    assert_eq!(tl.cursor(), 1, "appliedOpCount after rollback");
    assert_eq!(tl.len(), 2, "rollback does not delete ops");
    assert_eq!(dirty.from, 1); // op2 left the applied prefix

    // Insert op3 AT the rollback cursor (between op1 and op2).
    let i3 = tl.insert_at_cursor(extrude_newbody(op3, bid(3), 2.5));
    assert_eq!(i3, 1);
    // afterInsertAtCursor: opCount==3; order op1@0,op3@1,op2@2; appliedOpCount==2.
    assert_eq!(tl.len(), 3);
    assert_eq!(tl.index_of(op1), Some(0));
    assert_eq!(tl.index_of(op3), Some(1));
    assert_eq!(tl.index_of(op2), Some(2));
    assert_eq!(tl.cursor(), 2, "appliedOpCount covers only inserted prefix");

    // §5.3: editing later steps disabled — op2 (beyond cursor) is not editable.
    assert!(tl.is_editable(0) && tl.is_editable(1));
    assert!(!tl.is_editable(2));
    // op3 and op2 are Dirty/pending; op1 stays Valid.
    assert_eq!(tl.state(0), Some(&StepState::Valid));
    assert_eq!(tl.state(1), Some(&StepState::Dirty));
    assert_eq!(tl.state(2), Some(&StepState::Dirty));

    // Full regen-to-end: setAppliedOpCount(len) then regen recovers all 3.
    let dirty_full = tl.set_cursor(tl.len());
    assert_eq!(tl.cursor(), 3, "appliedOpCount after full regen");
    assert_eq!((dirty_full.from, dirty_full.to), (2, 3)); // op2 promoted+dirtied
    for s in 0..tl.len() {
        tl.mark_state(s, StepState::Valid).unwrap();
    }
    tl.validate().unwrap();

    // The dependency graph over these NewBody ops has no edges
    // (expected-values: forwardEdgeCount=0 backwardEdgeCount=0). Topo == order.
    let mut g = DependencyGraph::new();
    g.rebuild_from_records(tl.records());
    let topo = g.topological_sort().unwrap();
    assert_eq!(topo, vec![op1, op3, op2]);
}

// ── (b) Kahn determinism / tie-break by creation index ───────────────────────

/// Independent records (no edges): the topo order equals creation order and is
/// stable across repeated calls, regardless of the process's HashMap iteration
/// randomization. Verified for every permutation of a 4-record set
/// (C++ comparator `indexA > indexB`, `DependencyGraph.cpp:126-138`).
#[test]
fn kahn_tie_break_is_deterministic_by_creation_index() {
    let base = [rid(10), rid(11), rid(12), rid(13)];
    let bodies = [bid(10), bid(11), bid(12), bid(13)];
    let idxs = [0usize, 1, 2, 3];

    // All 24 permutations of insertion order.
    let perms = permutations(&idxs);
    for perm in perms {
        let mut g = DependencyGraph::new();
        for &i in &perm {
            g.add_record(&extrude_newbody(base[i], bodies[i], 1.0));
        }
        let expected: Vec<RecordId> = g.creation_order().to_vec();
        let first = g.topological_sort().unwrap();
        // Tie-break => output tracks creation index, never hash iteration order.
        assert_eq!(first, expected, "topo must equal creation order");
        // Stable across many calls despite HashMap randomization.
        for _ in 0..64 {
            assert_eq!(g.topological_sort().unwrap(), first);
        }
    }
}

/// With a shared root producing a body consumed by several independent children,
/// the children emerge in creation-index order after the root (the tie-break).
#[test]
fn kahn_tie_break_orders_independent_children_by_creation_index() {
    let root = rid(20);
    let children = [rid(21), rid(22), rid(23)];
    let mut g = DependencyGraph::new();
    g.add_record(&extrude_newbody(root, bid(20), 1.0));
    for (k, &c) in children.iter().enumerate() {
        // Each child cuts the root body → edge root→child; children independent.
        g.add_record(&extrude_cut(c, bid(20), bid(21 + k as u128), 1.0));
    }
    let topo = g.topological_sort().unwrap();
    assert_eq!(topo, vec![root, children[0], children[1], children[2]]);
    // Every child depends on the root.
    for &c in &children {
        assert!(g.upstream(c).contains(&root));
    }
}

// ── (d) produces_before anti-time-travel ─────────────────────────────────────

/// `produces_before` (C++ `DependencyGraph.cpp:391-431`): a record may only
/// consume outputs of records earlier in creation order.
#[test]
fn produces_before_rejects_time_travel() {
    let (a, b, c) = (rid(30), rid(31), rid(32));
    let (bx, by, bz) = (bid(30), bid(31), bid(32));
    let mut g = DependencyGraph::new();
    g.add_record(&extrude_newbody(a, bx, 1.0)); // idx 0 produces X
    g.add_record(&extrude_newbody(b, by, 1.0)); // idx 1 produces Y
    g.add_record(&extrude_newbody(c, bz, 1.0)); // idx 2 produces Z

    // X produced before b/c → valid target.
    assert!(g.produces_before(bx, b));
    assert!(g.produces_before(bx, c));
    // Y produced by a LATER op than a → time travel → rejected.
    assert!(!g.produces_before(by, a));
    // A body no op produces (base/external) is always a valid target.
    assert!(g.produces_before(bid(999), a));
    // An op modifying a body it also produces, with no later producer → valid.
    assert!(g.produces_before(bz, c));
    // body_producer returns the most recent producer.
    assert_eq!(g.body_producer(bx), Some(a));
    assert_eq!(g.body_producer(bid(999)), None);
    // An untracked consumer is never blocked (cpp:400-402).
    assert!(g.produces_before(bx, rid(1234)));
}

/// The Rust port additionally links sketch inputs to their producing `Sketch`
/// op (divergence 1) — an extrude-on-sketch depends on the sketch node.
#[test]
fn sketch_op_produces_edge_to_consumer() {
    let (sk, ex) = (rid(40), rid(41));
    let sketch = sid(40);
    let mut g = DependencyGraph::new();
    g.add_record(&sketch_op(sk, sketch));
    g.add_record(&extrude_on_sketch(ex, sketch, bid(40)));
    assert_eq!(g.sketch_producer(sketch), Some(sk));
    assert!(g.upstream(ex).contains(&sk));
    assert!(g.downstream(sk).contains(&ex));
    assert_eq!(g.topological_sort().unwrap(), vec![sk, ex]);
}

// ── (e) Suppression cascade + snapshot/restore round-trip ────────────────────

/// `set_suppressed(.., cascade)` propagates to the downstream closure
/// (C++ `suppressDownstream`, `DependencyGraph.cpp:197-201`); snapshot/restore
/// round-trips (`getSuppressionState`/`setSuppressionState`, cpp:203-215).
#[test]
fn suppression_cascade_and_restore_round_trip() {
    // Chain A -> B -> C via bodies.
    let (a, b, c) = (rid(50), rid(51), rid(52));
    let (ba, bb, bc) = (bid(50), bid(51), bid(52));
    let mut g = DependencyGraph::new();
    g.add_record(&extrude_newbody(a, ba, 1.0));
    g.add_record(&extrude_cut(b, ba, bb, 1.0)); // B consumes A's body
    g.add_record(&extrude_cut(c, bb, bc, 1.0)); // C consumes B's body

    let snap_clean = g.suppression_snapshot();
    assert!(!g.is_suppressed(a) && !g.is_suppressed(b) && !g.is_suppressed(c));

    // Cascade-suppress from A → A, B, C all suppressed.
    g.set_suppressed(a, true, true);
    assert!(g.is_suppressed(a) && g.is_suppressed(b) && g.is_suppressed(c));
    let snap_suppressed = g.suppression_snapshot();

    // Restore the clean snapshot → all un-suppressed again.
    g.restore_suppression(&snap_clean);
    assert!(!g.is_suppressed(a) && !g.is_suppressed(b) && !g.is_suppressed(c));

    // Restore the suppressed snapshot → all suppressed again (round-trip).
    g.restore_suppression(&snap_suppressed);
    assert!(g.is_suppressed(a) && g.is_suppressed(b) && g.is_suppressed(c));

    // Cascade un-suppress (symmetric extension over C++).
    g.set_suppressed(a, false, true);
    assert!(!g.is_suppressed(a) && !g.is_suppressed(b) && !g.is_suppressed(c));
}

// ── Failure tracking + is_blocked ────────────────────────────────────────────

#[test]
fn failure_tracking_and_blocking() {
    let (a, b, c) = (rid(60), rid(61), rid(62));
    let (ba, bb, bc) = (bid(60), bid(61), bid(62));
    let mut g = DependencyGraph::new();
    g.add_record(&extrude_newbody(a, ba, 1.0));
    g.add_record(&extrude_cut(b, ba, bb, 1.0));
    g.add_record(&extrude_cut(c, bb, bc, 1.0));

    g.mark_failed(a, "boolean failed");
    assert!(g.is_failed(a));
    assert_eq!(g.failure_reason(a), Some("boolean failed"));
    assert_eq!(g.failed_ops(), vec![a]); // creation order
                                         // is_blocked = an upstream is failed.
    assert!(!g.is_blocked(a)); // a itself failed, but no upstream of a
    assert!(g.is_blocked(b)); // upstream a failed
    assert!(g.is_blocked(c)); // upstream a failed (transitive)

    g.clear_failures();
    assert!(!g.is_failed(a) && g.failed_ops().is_empty());
    assert!(!g.is_blocked(b));
}

// ── (f) Cycle detection ──────────────────────────────────────────────────────

/// The producer-index model cannot itself create a cycle (edges point
/// creation-order-forward), so an explicit back-edge is injected to exercise the
/// defensive cycle branch (`DependencyGraph.cpp:159-162`).
#[test]
fn topological_sort_reports_cycle_members() {
    let (a, b) = (rid(70), rid(71));
    let mut g = DependencyGraph::new();
    g.add_record(&extrude_newbody(a, bid(70), 1.0));
    g.add_record(&extrude_newbody(b, bid(71), 1.0));
    g.add_edge(a, b).unwrap();
    g.add_edge(b, a).unwrap(); // A -> B -> A

    assert!(g.has_cycle());
    match g.topological_sort() {
        Err(DomainError::Cycle(msg)) => {
            assert!(msg.contains(&a.to_string()), "cycle lists A: {msg}");
            assert!(msg.contains(&b.to_string()), "cycle lists B: {msg}");
        }
        other => panic!("expected Cycle error, got {other:?}"),
    }

    // add_edge on an absent endpoint errors.
    assert!(matches!(
        g.add_edge(a, rid(9999)),
        Err(DomainError::RecordNotFound(_))
    ));
}

#[test]
fn remove_keeps_cursor_and_states_consistent() {
    let (a, b, c) = (rid(80), rid(81), rid(82));
    let mut tl = Timeline::new();
    tl.insert_at_cursor(extrude_newbody(a, bid(80), 1.0));
    tl.insert_at_cursor(extrude_newbody(b, bid(81), 1.0));
    tl.insert_at_cursor(extrude_newbody(c, bid(82), 1.0));
    assert_eq!(tl.cursor(), 3);

    // Remove the middle applied op → cursor shifts down by one (cpp:978-980).
    let dirty = tl.remove(b).unwrap();
    assert_eq!(tl.len(), 2);
    assert_eq!(tl.cursor(), 2);
    assert_eq!(dirty.from, 1);
    assert_eq!(tl.index_of(a), Some(0));
    assert_eq!(tl.index_of(c), Some(1));
    tl.validate().unwrap();

    assert!(matches!(
        tl.remove(rid(9999)),
        Err(DomainError::RecordNotFound(_))
    ));
}

// ── (c) Property-based invariants ────────────────────────────────────────────

#[derive(Debug, Clone)]
enum Action {
    InsertIndep,
    InsertDep(u8),
    SetCursor(u8),
    Suppress(u8),
}

fn action_strategy() -> impl Strategy<Value = Action> {
    prop_oneof![
        Just(Action::InsertIndep),
        any::<u8>().prop_map(Action::InsertDep),
        any::<u8>().prop_map(Action::SetCursor),
        any::<u8>().prop_map(Action::Suppress),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// Random insert / rollback / suppress sequences preserve every timeline and
    /// graph invariant: `states.len() == records.len()`, `cursor ≤ len`, the topo
    /// sort respects every edge, and `downstream(x)` never contains `x`.
    #[test]
    fn invariants_hold_under_random_edits(actions in prop::collection::vec(action_strategy(), 0..16)) {
        let mut tl = Timeline::new();
        let mut bodies: Vec<BodyId> = Vec::new();
        let mut next: u128 = 0;
        let mut suppress_reqs: Vec<usize> = Vec::new();

        for act in &actions {
            match act {
                Action::InsertIndep => {
                    let out = bid(next);
                    tl.insert_at_cursor(extrude_newbody(rid(next), out, 1.0));
                    bodies.push(out);
                    next += 1;
                }
                Action::InsertDep(j) => {
                    if bodies.is_empty() {
                        let out = bid(next);
                        tl.insert_at_cursor(extrude_newbody(rid(next), out, 1.0));
                        bodies.push(out);
                    } else {
                        let target = bodies[(*j as usize) % bodies.len()];
                        let out = bid(next);
                        tl.insert_at_cursor(extrude_cut(rid(next), target, out, 1.0));
                        bodies.push(out);
                    }
                    next += 1;
                }
                Action::SetCursor(k) => {
                    let len = tl.len();
                    tl.set_cursor(if len == 0 { 0 } else { (*k as usize) % (len + 1) });
                }
                Action::Suppress(j) => {
                    if !tl.is_empty() {
                        suppress_reqs.push((*j as usize) % tl.len());
                    }
                }
            }
            // Timeline invariants after every step.
            prop_assert!(tl.validate().is_ok());
            prop_assert!(tl.cursor() <= tl.len());
            prop_assert_eq!(tl.states().len(), tl.records().len());
        }

        // Build the graph from the final record list and apply suppressions.
        let mut g = DependencyGraph::new();
        g.rebuild_from_records(tl.records());
        for &i in &suppress_reqs {
            if let Some(r) = tl.record(i) {
                g.set_suppressed(r.record_id, true, true);
            }
        }

        // The producer-index graph is always a DAG → sort succeeds.
        let topo = g.topological_sort().expect("producer-index graph is acyclic");
        prop_assert_eq!(topo.len(), tl.records().len());

        let pos: std::collections::HashMap<RecordId, usize> =
            topo.iter().enumerate().map(|(i, id)| (*id, i)).collect();

        for r in tl.records() {
            let x = r.record_id;
            // downstream(x) never contains x.
            prop_assert!(!g.downstream(x).contains(&x));
            // Every upstream producer precedes x in the topo order (edges respected).
            for u in g.upstream(x) {
                prop_assert!(pos[&u] < pos[&x]);
            }
        }
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

/// All permutations of a small index slice (Heap's algorithm, iterative-ish).
fn permutations(items: &[usize]) -> Vec<Vec<usize>> {
    let mut out = Vec::new();
    let mut cur = items.to_vec();
    permute(&mut cur, 0, &mut out);
    out
}

fn permute(cur: &mut Vec<usize>, k: usize, out: &mut Vec<Vec<usize>>) {
    if k == cur.len() {
        out.push(cur.clone());
        return;
    }
    for i in k..cur.len() {
        cur.swap(k, i);
        permute(cur, k + 1, out);
        cur.swap(k, i);
    }
}
