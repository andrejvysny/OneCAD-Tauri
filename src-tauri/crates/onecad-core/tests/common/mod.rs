//! Shared deterministic fixtures for the schema-freeze + round-trip tests.
//!
//! All identities are minted from fixed bytes so JSON snapshots are stable.

#![allow(dead_code)]

use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, BooleanOp, BooleanParams, ChamferParams, CircularPatternParams,
    DeterminismSettings, ExtrudeMode, ExtrudeParams, FilletParams, KnownOperation,
    LinearPatternParams, LoftParams, MirrorBodyParams, OpaqueOperation, Operation, OperationRecord,
    PlaneKind, RevolveParams, ShellParams, SketchOpParams, SketchPlaneRef, SweepParams,
};
use onecad_core::document::refs::{
    AnchorIntent, AxisRef, ElementKind, ElementRef, IntentQuery, LocalFrame, PrimaryRef,
    SketchRegionRef,
};
use onecad_core::document::variables::Scalar;
use onecad_core::ids::{BodyId, ElementId, EntityId, RecordId, RegionId, SketchId};
use onecad_core::math::{Vec2, Vec3};

/// Deterministic UUID from a small integer seed.
pub fn uid(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

pub fn body_a() -> BodyId {
    BodyId(uid(0xB0D1))
}
pub fn body_b() -> BodyId {
    BodyId(uid(0xB0D2))
}
pub fn sketch_1() -> SketchId {
    SketchId(uid(0x5C01))
}
pub fn region_0() -> RegionId {
    RegionId::new("r0")
}
pub fn line_e1() -> EntityId {
    EntityId(uid(0xE001))
}
pub fn elem_e14() -> ElementId {
    ElementId::new("e:14")
}
pub fn elem_e15() -> ElementId {
    ElementId::new("e:15")
}

pub fn v3(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3::new(x, y, z).expect("finite")
}

pub fn profile() -> SketchRegionRef {
    SketchRegionRef {
        sketch: sketch_1(),
        region: region_0(),
        extra: Default::default(),
    }
}

/// A fully-populated element ref (identity + evidence + anchor) for the
/// nested-ref preservation test.
pub fn face_ref() -> ElementRef {
    ElementRef {
        primary: Some(PrimaryRef {
            body: body_a(),
            element: ElementId::new("el_face_1"),
            kind: ElementKind::Face,
            extra: Default::default(),
        }),
        intent: Some(IntentQuery {
            version: 1,
            kind: ElementKind::Face,
            descriptor: serde_json::json!({ "surfaceType": "plane", "areaQ": 120000 }),
            extra: Default::default(),
        }),
        anchor: Some(AnchorIntent {
            world_point: v3(12.0, 3.5, 0.0),
            surface_uv: Some(Vec2::new_unchecked(0.25, 0.75)),
            local_frame: Some(LocalFrame {
                origin: v3(12.0, 3.5, 0.0),
                x: v3(1.0, 0.0, 0.0),
                y: v3(0.0, 1.0, 0.0),
                z: v3(0.0, 0.0, 1.0),
                extra: Default::default(),
            }),
            adjacency_hint: Some("d41d8cd98f00b204".into()),
            extra: Default::default(),
        }),
        extra: Default::default(),
    }
}

pub fn determinism() -> DeterminismSettings {
    DeterminismSettings {
        parallel: false,
        occt_options: serde_json::Map::new(),
        occt_options_hash: "cbf29ce484222325".into(),
        tolerance_policy_hash: "b2c9000000000000".into(),
        solver_policy_hash: "3e9a000000000000".into(),
        extra: Default::default(),
    }
}

// ── Per-variant Operation builders ──────────────────────────────────────────

pub fn op_sketch() -> Operation {
    Operation::Known(KnownOperation::Sketch(SketchOpParams {
        sketch: sketch_1(),
        plane: SketchPlaneRef {
            kind: PlaneKind::Xy,
            origin: v3(0.0, 0.0, 0.0),
            x_axis: v3(0.0, 1.0, 0.0),
            y_axis: v3(-1.0, 0.0, 0.0),
            normal: v3(0.0, 0.0, 1.0),
            extra: Default::default(),
        },
        entities: vec![
            serde_json::json!({ "id": "e1", "type": "Line", "p0": [0, 0], "p1": [40, 0] }),
        ],
        constraints: vec![
            serde_json::json!({ "id": "c1", "type": "Horizontal", "entities": ["e1"] }),
        ],
        extra: Default::default(),
    }))
}

pub fn op_extrude_newbody() -> Operation {
    Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: Some(profile()),
        distance: Scalar::new(25.0),
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
    }))
}

/// Extrude that Cuts into an existing body (exercises the `booleanMode !=
/// NewBody` ⇒ target-body dependency rule) and references a ToFace target.
pub fn op_extrude_cut() -> Operation {
    Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: Some(profile()),
        distance: Scalar::new(10.0),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::ToFace,
        boolean_mode: BooleanMode::Cut,
        target_body: Some(body_a()),
        target_face: Some(face_ref()),
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    }))
}

pub fn op_revolve() -> Operation {
    Operation::Known(KnownOperation::Revolve(RevolveParams {
        profile: Some(profile()),
        angle_deg: Scalar::new(360.0),
        axis: Some(AxisRef::SketchLine {
            sketch: sketch_1(),
            line: line_e1(),
            extra: Default::default(),
        }),
        boolean_mode: BooleanMode::NewBody,
        target_body: None,
        extra: Default::default(),
    }))
}

pub fn op_fillet() -> Operation {
    Operation::Known(KnownOperation::Fillet(FilletParams {
        radius: Scalar::new(2.0),
        edge_ids: vec![elem_e14(), elem_e15()],
        edges: vec![],
        chain_tangent_edges: true,
        extra: Default::default(),
    }))
}

pub fn op_chamfer() -> Operation {
    Operation::Known(KnownOperation::Chamfer(ChamferParams {
        radius: Scalar::new(1.0),
        edge_ids: vec![elem_e14()],
        edges: vec![],
        chain_tangent_edges: true,
        extra: Default::default(),
    }))
}

/// A fillet whose edges are typed [`ElementRef`]s (M5): two edges on the SAME
/// body (`body_a`) plus one intent-only edge (no `primary`). Exercises operated-
/// body dedup + the "intent-only ⇒ no body" rule in `derive_inputs`.
pub fn op_fillet_with_edge_refs() -> Operation {
    let edge_on_body_a = |el: &str| ElementRef {
        primary: Some(PrimaryRef {
            body: body_a(),
            element: ElementId::new(el),
            kind: ElementKind::Edge,
            extra: Default::default(),
        }),
        intent: None,
        anchor: None,
        extra: Default::default(),
    };
    let intent_only = ElementRef {
        primary: None,
        intent: Some(IntentQuery {
            version: 1,
            kind: ElementKind::Edge,
            descriptor: serde_json::json!({ "curveType": "line" }),
            extra: Default::default(),
        }),
        anchor: None,
        extra: Default::default(),
    };
    Operation::Known(KnownOperation::Fillet(FilletParams {
        radius: Scalar::new(2.0),
        edge_ids: vec![],
        edges: vec![
            edge_on_body_a("el_e14"),
            edge_on_body_a("el_e15"),
            intent_only,
        ],
        chain_tangent_edges: true,
        extra: Default::default(),
    }))
}

pub fn op_shell() -> Operation {
    Operation::Known(KnownOperation::Shell(ShellParams {
        thickness: Scalar::new(1.5),
        open_faces: vec![ElementId::new("f:3")],
        target_body: Some(body_a()),
        extra: Default::default(),
    }))
}

pub fn op_boolean() -> Operation {
    Operation::Known(KnownOperation::Boolean(BooleanParams {
        operation: BooleanOp::Union,
        target_body: body_a(),
        tool_body: body_b(),
        extra: Default::default(),
    }))
}

pub fn op_linear_pattern() -> Operation {
    Operation::Known(KnownOperation::LinearPattern(LinearPatternParams {
        source_body: Some(body_a()),
        direction: v3(1.0, 0.0, 0.0),
        spacing: Scalar::new(10.0),
        count: 3,
        fuse_result: true,
        extra: Default::default(),
    }))
}

pub fn op_circular_pattern() -> Operation {
    Operation::Known(KnownOperation::CircularPattern(CircularPatternParams {
        source_body: Some(body_a()),
        axis_origin: v3(0.0, 0.0, 0.0),
        axis_direction: v3(0.0, 0.0, 1.0),
        angle_deg: Scalar::new(360.0),
        count: 4,
        fuse_result: true,
        extra: Default::default(),
    }))
}

pub fn op_loft() -> Operation {
    Operation::Known(KnownOperation::Loft(LoftParams {
        profiles: vec![
            profile(),
            SketchRegionRef {
                sketch: SketchId(uid(0x5C02)),
                region: RegionId::new("r1"),
                extra: Default::default(),
            },
        ],
        is_solid: true,
        is_ruled: false,
        boolean_mode: BooleanMode::NewBody,
        extra: Default::default(),
    }))
}

pub fn op_sweep() -> Operation {
    Operation::Known(KnownOperation::Sweep(SweepParams {
        profile: Some(profile()),
        path_sketch: Some(SketchId(uid(0x5C03))),
        path_edge: None,
        boolean_mode: BooleanMode::NewBody,
        extra: Default::default(),
    }))
}

pub fn op_mirror() -> Operation {
    Operation::Known(KnownOperation::MirrorBody(MirrorBodyParams {
        source_body: Some(body_a()),
        plane_point: v3(0.0, 0.0, 0.0),
        plane_normal: v3(1.0, 0.0, 0.0),
        fuse_with_original: false,
        extra: Default::default(),
    }))
}

/// An unknown-`opType` op, captured as a frozen node.
pub fn op_opaque() -> Operation {
    let mut raw = serde_json::Map::new();
    raw.insert(
        "opType".into(),
        serde_json::Value::String("FlangeBend".into()),
    );
    raw.insert(
        "params".into(),
        serde_json::json!({ "angleDeg": 90.0, "reliefType": "rectangular" }),
    );
    raw.insert("vendorHint".into(), serde_json::json!({ "tool": "future" }));
    Operation::Opaque(OpaqueOperation { raw })
}

/// Wraps an op in a canonical record with a fixed record id.
pub fn record(step: u32, name: &str, op: Operation) -> OperationRecord {
    let mut rec = OperationRecord::new(RecordId(uid(0x2EC0 + step as u128)), step, name, op);
    rec.determinism = determinism();
    rec.outputs = vec![body_a()];
    rec
}

/// The full ordered set of canonical records (one per Operation variant + Opaque).
pub fn canonical_records() -> Vec<(&'static str, OperationRecord)> {
    vec![
        ("sketch", record(0, "Sketch 1", op_sketch())),
        (
            "extrude_newbody",
            record(1, "Extrude 1", op_extrude_newbody()),
        ),
        ("extrude_cut", record(2, "Cut 1", op_extrude_cut())),
        ("revolve", record(3, "Revolve 1", op_revolve())),
        ("fillet", record(4, "Fillet 1", op_fillet())),
        ("chamfer", record(5, "Chamfer 1", op_chamfer())),
        ("shell", record(6, "Shell 1", op_shell())),
        ("boolean", record(7, "Boolean 1", op_boolean())),
        (
            "linear_pattern",
            record(8, "Linear Pattern 1", op_linear_pattern()),
        ),
        (
            "circular_pattern",
            record(9, "Circular Pattern 1", op_circular_pattern()),
        ),
        ("loft", record(10, "Loft 1", op_loft())),
        ("sweep", record(11, "Sweep 1", op_sweep())),
        ("mirror_body", record(12, "Mirror 1", op_mirror())),
        ("opaque_frozen", record(13, "Frozen 1", op_opaque())),
    ]
}
