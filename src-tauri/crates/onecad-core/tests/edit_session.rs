//! R-WP5 edit/session integration tests.
//!
//! Covers (task point 10): (a) per-command apply→undo→redo structural equality
//! across ALL `EditCommand` variants, (b) proptest random valid sequences
//! (apply-all → undo-all == initial; stack cap 200), (c) transactions as a
//! single undo unit with combined outcome, (d) the RegenHint/DirtyRange table,
//! (e) anti-time-travel rejection, (g) a session insta snapshot.
//!
//! (f) selection rules live in the `selection` module's unit tests.

use proptest::prelude::*;
use uuid::Uuid;

use onecad_core::document::body::BodyMeta;
use onecad_core::document::datum::DatumPlane;
use onecad_core::document::record::PlaneKind;
use onecad_core::document::record::{
    BooleanMode, BooleanOp, BooleanParams, ExtrudeMode, ExtrudeParams, FilletParams,
    KnownOperation, Operation, OperationRecord, RevolveParams, SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::refs::{AxisRef, ElementKind, ElementRef, PrimaryRef, SketchRegionRef};
use onecad_core::document::variables::{Scalar, Unit, Variable};
use onecad_core::document::Document;
use onecad_core::edit::{
    DocumentSession, EditCommand, InputPath, InputRef, RegenHint, SketchEditOp, VisibilityTarget,
};
use onecad_core::history::{DirtyRange, Timeline};
use onecad_core::ids::{
    BodyId, ConstraintId, DatumPlaneId, DocumentId, ElementId, EntityId, RecordId, RegionId,
    SketchId, VariableId,
};
use onecad_core::math::{Vec2, Vec3};
use onecad_core::sketch::{Constraint, Sketch, SketchAttachment, SketchEntity, WorldPlane};

// ── id helpers ───────────────────────────────────────────────────────────────

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn rid(n: u128) -> RecordId {
    RecordId(u(0x2E00 + n))
}
fn bid(n: u128) -> BodyId {
    BodyId(u(0xB000 + n))
}
fn sid(n: u128) -> SketchId {
    SketchId(u(0x5C00 + n))
}
fn eid(n: u128) -> EntityId {
    EntityId(u(0xE000 + n))
}
fn cid(n: u128) -> ConstraintId {
    ConstraintId(u(0xC000 + n))
}
fn vid(n: u128) -> VariableId {
    VariableId(u(0x7000 + n))
}
fn did(n: u128) -> DatumPlaneId {
    DatumPlaneId(u(0xD000 + n))
}

const S1: fn() -> SketchId = || sid(1);
const BX: fn() -> BodyId = || bid(1);
const BY: fn() -> BodyId = || bid(2);
const B0: fn() -> BodyId = || bid(0);

fn s(v: f64) -> Scalar {
    Scalar::new(v)
}

// ── op-record builders ───────────────────────────────────────────────────────

fn profile() -> SketchRegionRef {
    SketchRegionRef {
        sketch: S1(),
        region: RegionId::new("r0"),
        extra: Default::default(),
    }
}

fn record(id: RecordId, name: &str, op: Operation, outputs: Vec<BodyId>) -> OperationRecord {
    let mut r = OperationRecord::new(id, 0, name, op);
    r.outputs = outputs;
    r
}

fn sketch_op(id: RecordId, sketch: SketchId) -> OperationRecord {
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
    record(id, "Sketch", op, vec![])
}

fn extrude_newbody(id: RecordId, distance: f64, out: BodyId) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: Some(profile()),
        distance: s(distance),
        draft_angle_deg: s(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: BooleanMode::NewBody,
        target_body: None,
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: s(0.0),
        target_face2: None,
        extra: Default::default(),
    }));
    record(id, "Extrude", op, vec![out])
}

fn extrude_cut(id: RecordId, target: BodyId, out: BodyId) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: Some(profile()),
        distance: s(5.0),
        draft_angle_deg: s(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: BooleanMode::Cut,
        target_body: Some(target),
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: s(0.0),
        target_face2: None,
        extra: Default::default(),
    }));
    record(id, "Cut", op, vec![out])
}

fn boolean(id: RecordId, target: BodyId, tool: BodyId, out: BodyId) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Boolean(BooleanParams {
        operation: BooleanOp::Union,
        target_body: target,
        tool_body: tool,
        extra: Default::default(),
    }));
    record(id, "Boolean", op, vec![out])
}

fn edge_ref(body: BodyId, el: &str) -> ElementRef {
    ElementRef {
        primary: Some(PrimaryRef {
            body,
            element: ElementId::new(el),
            kind: ElementKind::Edge,
            extra: Default::default(),
        }),
        intent: None,
        anchor: None,
        extra: Default::default(),
    }
}

fn fillet(id: RecordId, body: BodyId, el: &str, out: BodyId) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Fillet(FilletParams {
        radius: s(2.0),
        edge_ids: vec![ElementId::new(el)],
        edges: vec![edge_ref(body, el)],
        chain_tangent_edges: true,
        extra: Default::default(),
    }));
    record(id, "Fillet", op, vec![out])
}

fn revolve(id: RecordId, out: BodyId) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Revolve(RevolveParams {
        profile: Some(profile()),
        angle_deg: s(360.0),
        axis: Some(AxisRef::SketchLine {
            sketch: S1(),
            line: eid(0x20),
            extra: Default::default(),
        }),
        boolean_mode: BooleanMode::NewBody,
        target_body: None,
        extra: Default::default(),
    }));
    record(id, "Revolve", op, vec![out])
}

// ── base document ────────────────────────────────────────────────────────────

/// A sketch with two points, a line and a distance constraint (so SketchEdit
/// SetDimension / SetEntityPositions have valid targets).
fn base_sketch() -> Sketch {
    let mut sk = Sketch::on_world_plane(S1(), "Sketch 1", WorldPlane::XY);
    sk.add_entity(SketchEntity::point(
        eid(1),
        Vec2::new_unchecked(0.0, 0.0),
        false,
        false,
    ))
    .unwrap();
    sk.add_entity(SketchEntity::point(
        eid(2),
        Vec2::new_unchecked(40.0, 0.0),
        false,
        false,
    ))
    .unwrap();
    sk.add_entity(SketchEntity::line(eid(0x20), eid(1), eid(2), false))
        .unwrap();
    sk.add_constraint(Constraint::Distance {
        id: cid(1),
        entity1: eid(1),
        entity2: eid(2),
        value: s(40.0),
    })
    .unwrap();
    sk
}

fn base_document() -> Document {
    let mut doc = Document::new(DocumentId(u(1)));
    doc.settings.tolerance_policy_hash = "tol".into();
    doc.sketches.insert(S1(), base_sketch());
    doc.datum_planes.insert(
        did(1),
        DatumPlane::offset_from_plane(did(1), "Datum 1", "XY", 0.0),
    );
    doc.bodies.register(BodyMeta::new(B0(), "Imported", rid(0)));
    doc.variables.upsert(Variable {
        id: vid(1),
        name: "width".into(),
        value: s(40.0),
        unit: Unit::Mm,
    });
    doc.timeline = Timeline::from_records(vec![
        sketch_op(rid(0), S1()),
        extrude_newbody(rid(1), 25.0, BX()),
        extrude_newbody(rid(2), 10.0, BY()),
        boolean(rid(3), BX(), BY(), BX()),
        fillet(rid(4), BX(), "el_e1", BX()),
    ]);
    doc
}

fn json(doc: &Document) -> serde_json::Value {
    serde_json::to_value(doc).unwrap()
}

// ── (a) per-command apply → undo → redo across ALL variants ──────────────────

/// One representative, valid command per `EditCommand` variant.
fn all_variant_commands() -> Vec<(&'static str, EditCommand)> {
    let after_sketch = {
        let mut sk = Sketch::on_world_plane(S1(), "Sketch 1", WorldPlane::XY);
        sk.add_entity(SketchEntity::point(
            eid(1),
            Vec2::new_unchecked(5.0, 5.0),
            false,
            false,
        ))
        .unwrap();
        sk
    };
    vec![
        (
            "AddOperation",
            EditCommand::AddOperation {
                record: extrude_newbody(rid(0x99), 3.0, bid(0x99)),
                at_cursor: true,
            },
        ),
        (
            "UpdateOperationParams",
            EditCommand::UpdateOperationParams {
                record: rid(1),
                op: extrude_newbody(rid(1), 99.0, BX()).op,
            },
        ),
        (
            "EditOperationInput",
            EditCommand::EditOperationInput {
                record: rid(4),
                path: InputPath::FilletEdges { index: 0 },
                reference: InputRef::Element(edge_ref(BX(), "el_e_new")),
            },
        ),
        (
            "RemoveOperation",
            EditCommand::RemoveOperation { record: rid(1) },
        ),
        ("SetRollback", EditCommand::SetRollback { cursor: 2 }),
        (
            "SetOperationSuppression",
            EditCommand::SetOperationSuppression {
                record: rid(4),
                suppressed: true,
                cascade: false,
            },
        ),
        (
            "AddSketch",
            EditCommand::AddSketch {
                sketch: Sketch::on_world_plane(sid(2), "Sketch 2", WorldPlane::XZ),
            },
        ),
        ("DeleteSketch", EditCommand::DeleteSketch { sketch: S1() }),
        (
            "RenameSketch",
            EditCommand::RenameSketch {
                sketch: S1(),
                name: "Renamed".into(),
            },
        ),
        (
            "UpdateSketchAttachment",
            EditCommand::UpdateSketchAttachment {
                sketch: S1(),
                plane: WorldPlane::XZ.plane(),
                attachment: SketchAttachment::World {
                    plane: WorldPlane::XZ,
                },
            },
        ),
        (
            "SketchEdit",
            EditCommand::SketchEdit {
                sketch: S1(),
                ops: vec![
                    SketchEditOp::SetDimension {
                        constraint: cid(1),
                        value: s(55.0),
                    },
                    SketchEditOp::SetEntityPositions {
                        positions: vec![(eid(1), Vec2::new_unchecked(1.0, 2.0))],
                    },
                    SketchEditOp::AddEntity {
                        entity: SketchEntity::point(
                            eid(9),
                            Vec2::new_unchecked(9.0, 9.0),
                            false,
                            false,
                        ),
                    },
                ],
            },
        ),
        (
            "SketchDragGesture",
            EditCommand::SketchDragGesture {
                sketch: S1(),
                before: base_sketch(),
                after: after_sketch,
            },
        ),
        (
            "AddBody",
            EditCommand::AddBody {
                body: BodyMeta::new(bid(0x50), "New Body", rid(0)),
            },
        ),
        ("DeleteBody", EditCommand::DeleteBody { body: B0() }),
        (
            "RenameBody",
            EditCommand::RenameBody {
                body: B0(),
                name: "Renamed Body".into(),
            },
        ),
        (
            "SetVisibilityBody",
            EditCommand::SetVisibility {
                target: VisibilityTarget::Body(B0()),
                visible: false,
            },
        ),
        (
            "SetVisibilitySketch",
            EditCommand::SetVisibility {
                target: VisibilityTarget::Sketch(S1()),
                visible: false,
            },
        ),
        (
            "AddDatumPlane",
            EditCommand::AddDatumPlane {
                datum: DatumPlane::offset_from_plane(did(2), "Datum 2", "XZ", 10.0),
            },
        ),
        (
            "SetVariable",
            EditCommand::SetVariable {
                variable: vid(1),
                value: s(88.0),
            },
        ),
        (
            "AddVariable",
            EditCommand::AddVariable {
                variable: Variable {
                    id: vid(2),
                    name: "height".into(),
                    value: s(20.0),
                    unit: Unit::Mm,
                },
            },
        ),
        (
            "RemoveVariable",
            EditCommand::RemoveVariable { variable: vid(1) },
        ),
    ]
}

#[test]
fn every_command_apply_undo_restores_and_redo_reapplies() {
    let base = base_document();
    let initial = json(&base);
    let mut count = 0;
    for (name, cmd) in all_variant_commands() {
        count += 1;
        let mut sess = DocumentSession::new(base.clone());
        sess.apply(cmd)
            .unwrap_or_else(|e| panic!("{name}: apply failed: {e}"));
        let after = json(sess.document());
        assert_ne!(after, initial, "{name}: apply must change the document");

        assert!(sess.undo(), "{name}: undo available");
        assert_eq!(
            json(sess.document()),
            initial,
            "{name}: undo must restore exactly"
        );

        assert!(sess.redo().unwrap(), "{name}: redo available");
        assert_eq!(
            json(sess.document()),
            after,
            "{name}: redo must reapply exactly"
        );

        // undo→redo is idempotent for state.
        assert!(sess.undo());
        assert_eq!(
            json(sess.document()),
            initial,
            "{name}: second undo restores"
        );
    }
    assert_eq!(count, 21, "all 21 EditCommand variants covered");
}

// ── EditOperationInput: every InputPath branch ───────────────────────────────

#[test]
fn edit_operation_input_all_paths() {
    // A document with an extrude, a boolean and a revolve to exercise each path.
    let mut doc = Document::new(DocumentId(u(2)));
    doc.timeline = Timeline::from_records(vec![
        extrude_newbody(rid(1), 25.0, BX()),
        extrude_newbody(rid(2), 10.0, BY()),
        boolean(rid(3), BX(), BY(), BX()),
        fillet(rid(4), BX(), "el_e1", BX()),
        revolve(rid(5), bid(3)),
    ]);
    let initial = json(&doc);

    let cases: Vec<(&str, RecordId, InputPath, InputRef)> = vec![
        (
            "extrude profile",
            rid(1),
            InputPath::ExtrudeProfile,
            InputRef::Region(SketchRegionRef {
                sketch: sid(9),
                region: RegionId::new("r9"),
                extra: Default::default(),
            }),
        ),
        (
            "fillet edge",
            rid(4),
            InputPath::FilletEdges { index: 0 },
            InputRef::Element(edge_ref(BX(), "el_edited")),
        ),
        (
            // swap target BX -> BY (produced before R3, so not time-travel)
            "boolean target",
            rid(3),
            InputPath::BooleanTarget,
            InputRef::Body(BY()),
        ),
        (
            // swap tool BY -> BX
            "boolean tool",
            rid(3),
            InputPath::BooleanTool,
            InputRef::Body(BX()),
        ),
        (
            "revolve axis",
            rid(5),
            InputPath::RevolveAxis,
            InputRef::Axis(AxisRef::Element {
                body: BX(),
                edge: ElementId::new("el_axis"),
                extra: Default::default(),
            }),
        ),
    ];

    for (name, rec, path, reference) in cases {
        let mut sess = DocumentSession::new(doc.clone());
        sess.apply(EditCommand::EditOperationInput {
            record: rec,
            path,
            reference,
        })
        .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_ne!(json(sess.document()), initial, "{name} changed the doc");
        assert!(sess.undo());
        assert_eq!(json(sess.document()), initial, "{name} undo restored");
    }
}

#[test]
fn fillet_edit_keeps_edge_ids_and_edges_in_lockstep() {
    let mut doc = Document::new(DocumentId(u(3)));
    doc.timeline = Timeline::from_records(vec![fillet(rid(4), BX(), "el_old", BX())]);
    let mut sess = DocumentSession::new(doc);
    sess.apply(EditCommand::EditOperationInput {
        record: rid(4),
        path: InputPath::FilletEdges { index: 0 },
        reference: InputRef::Element(edge_ref(BX(), "el_fresh")),
    })
    .unwrap();
    let v = json(sess.document());
    let params = &v["timeline"]["records"][0]["params"];
    assert_eq!(params["edgeIds"][0], "el_fresh", "bare edge_ids updated");
    assert_eq!(
        params["edges"][0]["primary"]["elementId"], "el_fresh",
        "typed edges ref updated in lockstep"
    );
}

// ── (e) anti-time-travel ─────────────────────────────────────────────────────

#[test]
fn add_operation_referencing_later_body_is_rejected() {
    let mut doc = Document::new(DocumentId(u(4)));
    doc.timeline = Timeline::from_records(vec![
        extrude_newbody(rid(1), 25.0, bid(0xA)),
        extrude_newbody(rid(2), 10.0, bid(0xC)), // produces bodyC at index 1
    ]);
    doc.timeline.set_cursor(1); // insert before the op that produces bodyC
    let mut sess = DocumentSession::new(doc);
    // A Cut that targets bodyC (produced by a LATER op) is anti-time-travel.
    let err = sess
        .apply(EditCommand::AddOperation {
            record: extrude_cut(rid(9), bid(0xC), bid(0xD)),
            at_cursor: true,
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("anti-time-travel"),
        "expected anti-time-travel rejection, got: {err}"
    );
}

// ── (d) RegenHint / DirtyRange table ─────────────────────────────────────────

#[test]
fn regen_hint_and_dirty_range_table() {
    let base = base_document();
    let len = base.timeline.len(); // 5

    let check = |cmd: EditCommand, want_dirty: Option<DirtyRange>, want_regen: RegenHint| {
        let mut sess = DocumentSession::new(base.clone());
        let out = sess.apply(cmd).unwrap();
        assert_eq!(out.dirty, want_dirty, "dirty mismatch");
        assert_eq!(out.regen, want_regen, "regen mismatch");
    };

    // AddOperation at cursor (=len): inserts at len, dirties [len, len+1), ToEnd.
    check(
        EditCommand::AddOperation {
            record: extrude_newbody(rid(0x99), 3.0, bid(0x99)),
            at_cursor: true,
        },
        Some(DirtyRange::new(len, len + 1)),
        RegenHint::ToEnd,
    );
    // UpdateOperationParams on index 1: [1, len), ToEnd.
    check(
        EditCommand::UpdateOperationParams {
            record: rid(1),
            op: extrude_newbody(rid(1), 99.0, BX()).op,
        },
        Some(DirtyRange::new(1, len)),
        RegenHint::ToEnd,
    );
    // RemoveOperation index 1: [1, len-1), ToEnd.
    check(
        EditCommand::RemoveOperation { record: rid(1) },
        Some(DirtyRange::new(1, len - 1)),
        RegenHint::ToEnd,
    );
    // SetRollback to 2 (backward from len): span [2, len), PreviewTo(2).
    check(
        EditCommand::SetRollback { cursor: 2 },
        Some(DirtyRange::new(2, len)),
        RegenHint::PreviewTo(2),
    );
    // SetVisibility body: metadata only.
    check(
        EditCommand::SetVisibility {
            target: VisibilityTarget::Body(B0()),
            visible: false,
        },
        None,
        RegenHint::None,
    );
    // RenameSketch: metadata only.
    check(
        EditCommand::RenameSketch {
            sketch: S1(),
            name: "x".into(),
        },
        None,
        RegenHint::None,
    );
    // SetVariable: conservative whole-timeline dirty, ToEnd.
    check(
        EditCommand::SetVariable {
            variable: vid(1),
            value: s(1.0),
        },
        Some(DirtyRange::new(0, len)),
        RegenHint::ToEnd,
    );
    // SketchEdit on S1: produced by the Sketch op at index 0 (regen re-runs region
    // detection there), so it dirties from the producer, not the first consumer at
    // index 1 (F4): [0, len), ToEnd.
    check(
        EditCommand::SketchEdit {
            sketch: S1(),
            ops: vec![SketchEditOp::AddEntity {
                entity: SketchEntity::point(eid(9), Vec2::new_unchecked(1.0, 1.0), false, false),
            }],
        },
        Some(DirtyRange::new(0, len)),
        RegenHint::ToEnd,
    );
    // AddSketch (no dependents yet): metadata only.
    check(
        EditCommand::AddSketch {
            sketch: Sketch::on_world_plane(sid(5), "S5", WorldPlane::XY),
        },
        None,
        RegenHint::None,
    );
}

// ── (c) transactions ─────────────────────────────────────────────────────────

#[test]
fn transaction_is_single_undo_unit_with_combined_outcome() {
    let base = base_document();
    let initial = json(&base);
    let mut sess = DocumentSession::new(base);

    sess.begin_transaction("batch");
    sess.apply(EditCommand::UpdateOperationParams {
        record: rid(1),
        op: extrude_newbody(rid(1), 99.0, BX()).op,
    })
    .unwrap();
    sess.apply(EditCommand::UpdateOperationParams {
        record: rid(3),
        op: boolean(rid(3), BX(), BY(), BX()).op,
    })
    .unwrap();
    let combined = sess.end_transaction().unwrap();

    // One undo unit.
    assert_eq!(sess.undo_depth(), 1, "batched into a single undo step");
    // Combined dirty is the union: [1, len) ∪ [3, len) = [1, len).
    assert_eq!(
        combined.dirty,
        Some(DirtyRange::new(1, base_document().timeline.len()))
    );
    assert_eq!(combined.regen, RegenHint::ToEnd);
    assert!(combined.projection_delta.timeline_changed);

    // A single undo restores BOTH edits.
    assert!(sess.undo());
    assert_eq!(
        json(sess.document()),
        initial,
        "one undo reverts the whole batch"
    );
}

#[test]
fn cancel_transaction_rolls_back_without_undo_entry() {
    let base = base_document();
    let initial = json(&base);
    let mut sess = DocumentSession::new(base);
    sess.begin_transaction("batch");
    sess.apply(EditCommand::RenameBody {
        body: B0(),
        name: "temp".into(),
    })
    .unwrap();
    sess.cancel_transaction();
    assert_eq!(json(sess.document()), initial, "cancel rolls back");
    assert_eq!(sess.undo_depth(), 0, "cancelled txn leaves no undo entry");
}

// ── (F1) transaction failure auto-cancels the whole batch ────────────────────

#[test]
fn failed_apply_in_transaction_auto_cancels_and_undo_hits_previous_txn() {
    let base = base_document();
    let initial = json(&base);
    let mut sess = DocumentSession::new(base);

    // A first, committed transaction (its own single undo step).
    sess.apply(EditCommand::RenameBody {
        body: B0(),
        name: "Committed".into(),
    })
    .unwrap();
    let after_committed = json(sess.document());
    assert_ne!(after_committed, initial);
    assert_eq!(sess.undo_depth(), 1);

    // Open a transaction, apply one good edit, then one that fails validation.
    sess.begin_transaction("batch");
    sess.apply(EditCommand::RenameBody {
        body: B0(),
        name: "PartialBatch".into(),
    })
    .unwrap();
    let err = sess
        .apply(EditCommand::RemoveOperation { record: rid(999) }) // not present
        .unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("not found")
            || err.to_string().to_lowercase().contains("recordnotfound")
            || matches!(err, onecad_core::error::DomainError::RecordNotFound(_)),
        "expected a not-found error, got: {err}"
    );

    // The whole transaction rolled back to the pre-transaction state.
    assert_eq!(
        json(sess.document()),
        after_committed,
        "auto-cancel restores the pre-transaction state (partial batch edit reverted)"
    );

    // The transaction is closed: undo runs again (it is ignored while a txn is
    // open) and the cancelled batch left no undo entry.
    assert_eq!(
        sess.undo_depth(),
        1,
        "cancelled batch adds no undo entry; only the committed txn remains"
    );
    assert!(sess.undo(), "undo runs — no open transaction");
    assert_eq!(
        json(sess.document()),
        initial,
        "undo reverts the PREVIOUS committed txn, not the cancelled batch"
    );
}

// ── (F2) fillet/chamfer edge lockstep on add/update entry paths ──────────────

fn fillet_op(edge_ids: &[&str], edges: Vec<ElementRef>) -> Operation {
    Operation::Known(KnownOperation::Fillet(FilletParams {
        radius: s(2.0),
        edge_ids: edge_ids.iter().map(|e| ElementId::new(*e)).collect(),
        edges,
        chain_tangent_edges: true,
        extra: Default::default(),
    }))
}

#[test]
fn add_operation_validates_fillet_edge_lockstep() {
    let fresh = || {
        let mut d = Document::new(DocumentId(u(0x5A)));
        d.bodies.register(BodyMeta::new(BX(), "b", rid(0)));
        d
    };
    let add = |op: Operation| EditCommand::AddOperation {
        record: record(rid(1), "Fillet", op, vec![BX()]),
        at_cursor: true,
    };

    // Mismatched count rejected (2 ids, 1 typed edge).
    let mut sess = DocumentSession::new(fresh());
    let err = sess
        .apply(add(fillet_op(&["e1", "e2"], vec![edge_ref(BX(), "e1")])))
        .unwrap_err();
    assert!(
        err.to_string().contains("mismatch"),
        "count mismatch: {err}"
    );

    // Primary element disagreeing with the parallel edge_ids entry rejected.
    let mut sess = DocumentSession::new(fresh());
    let err = sess
        .apply(add(fillet_op(&["e1"], vec![edge_ref(BX(), "eX")])))
        .unwrap_err();
    assert!(
        err.to_string().contains("!="),
        "primary disagreement: {err}"
    );

    // Consistent (equal length, matching primary) accepted.
    let mut sess = DocumentSession::new(fresh());
    sess.apply(add(fillet_op(&["e1"], vec![edge_ref(BX(), "e1")])))
        .expect("consistent fillet accepted");

    // Empty typed edges (legacy bare-ids form) accepted.
    let mut sess = DocumentSession::new(fresh());
    sess.apply(add(fillet_op(&["e1"], vec![])))
        .expect("bare-ids fillet accepted");
}

#[test]
fn update_operation_params_validates_fillet_edge_lockstep() {
    let mut doc = Document::new(DocumentId(u(0x5B)));
    doc.bodies.register(BodyMeta::new(BX(), "b", rid(0)));
    doc.timeline = Timeline::from_records(vec![record(
        rid(1),
        "Fillet",
        fillet_op(&["e1"], vec![]),
        vec![BX()],
    )]);
    let mut sess = DocumentSession::new(doc);
    // Update to a mismatched typed-edge set is rejected.
    let err = sess
        .apply(EditCommand::UpdateOperationParams {
            record: rid(1),
            op: fillet_op(&["e1", "e2"], vec![edge_ref(BX(), "e1")]),
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("mismatch"),
        "update mismatch: {err}"
    );
    // Update to a consistent set is accepted.
    sess.apply(EditCommand::UpdateOperationParams {
        record: rid(1),
        op: fillet_op(&["e9"], vec![edge_ref(BX(), "e9")]),
    })
    .expect("consistent update accepted");
}

// ── (b) proptest: random valid sequences ─────────────────────────────────────

/// Builds a valid command from an action selector + a unique index.
fn action_command(kind: u8, i: usize) -> EditCommand {
    let n = i as u128;
    match kind % 5 {
        0 => EditCommand::AddVariable {
            variable: Variable {
                id: vid(0x100 + n),
                name: format!("v{i}"),
                value: s(i as f64),
                unit: Unit::Mm,
            },
        },
        1 => EditCommand::AddSketch {
            sketch: Sketch::on_world_plane(sid(0x100 + n), format!("S{i}"), WorldPlane::XY),
        },
        2 => EditCommand::AddOperation {
            record: extrude_newbody(rid(0x100 + n), i as f64, bid(0x100 + n)),
            at_cursor: true,
        },
        3 => EditCommand::RenameBody {
            body: B0(),
            name: format!("b{i}"),
        },
        _ => EditCommand::SetVisibility {
            target: VisibilityTarget::Body(B0()),
            visible: i.is_multiple_of(2),
        },
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Apply a random valid sequence, then undo everything: the document JSON
    /// returns exactly to its initial value, and the stack never exceeds 200.
    #[test]
    fn apply_all_then_undo_all_restores_initial(actions in prop::collection::vec(0u8..5, 0..80)) {
        let base = base_document();
        let initial = json(&base);
        let mut sess = DocumentSession::new(base);

        for (i, &k) in actions.iter().enumerate() {
            sess.apply(action_command(k, i)).unwrap();
            prop_assert!(sess.undo_depth() <= 200, "stack cap holds");
        }
        while sess.can_undo() {
            prop_assert!(sess.undo());
        }
        prop_assert_eq!(json(sess.document()), initial);
    }
}

#[test]
fn undo_stack_caps_at_200() {
    let base = base_document();
    let mut sess = DocumentSession::new(base);
    for i in 0..250 {
        sess.apply(action_command(0, i)).unwrap(); // AddVariable, unique
    }
    assert_eq!(sess.undo_depth(), 200, "capped at 200, oldest evicted");
}

// ── (g) session snapshot ─────────────────────────────────────────────────────

#[test]
fn session_snapshot_after_scripted_sequence() {
    let mut sess = DocumentSession::new(base_document());
    sess.apply(EditCommand::AddVariable {
        variable: Variable {
            id: vid(2),
            name: "height".into(),
            value: s(20.0),
            unit: Unit::Mm,
        },
    })
    .unwrap();
    sess.apply(EditCommand::AddOperation {
        record: extrude_newbody(rid(0x30), 12.5, bid(0x30)),
        at_cursor: true,
    })
    .unwrap();
    sess.apply(EditCommand::RenameBody {
        body: B0(),
        name: "Base Plate".into(),
    })
    .unwrap();
    sess.apply(EditCommand::SetRollback { cursor: 3 }).unwrap();
    sess.apply(EditCommand::AddDatumPlane {
        datum: DatumPlane::offset_from_plane(did(2), "Datum 2", "XZ", 5.0),
    })
    .unwrap();

    insta::assert_json_snapshot!("session_after_sequence", sess.document());
}
