//! Schema-freeze contract for the authoritative **sketch** file-format model.
//!
//! ⚠️  THESE SNAPSHOTS ARE THE `sketches/*.json` FILE-FORMAT CONTRACT. Changing
//! any `.snap` here is a schema change and REQUIRES SCHEMA-REVIEW SIGN-OFF + a
//! `protocol/` fixture bump (SCHEMA §13). Do not `cargo insta accept` a diff
//! without it. Same discipline as `schema_freeze` (the OperationRecord contract).
//!
//! Covers a canonical JSON snapshot of: (1) every `SketchEntity` kind, (2) every
//! `Constraint` kind (all 18), (3) a full `Sketch` document (plane + attachment +
//! entities + constraints + region cache + extra flatten). All identities are
//! minted from fixed byte seeds so the JSON is deterministic.

use onecad_core::document::refs::{ElementKind, ElementRef, PrimaryRef};
use onecad_core::document::variables::Scalar;
use onecad_core::ids::{BodyId, ConstraintId, ElementId, EntityId, RegionId, SketchId};
use onecad_core::math::Vec2;
use onecad_core::sketch::constraint::{Constraint, CurvePosition};
use onecad_core::sketch::entity::SketchEntity;
use onecad_core::sketch::{RegionInfo, Sketch, SketchAttachment, WorldPlane};
use uuid::Uuid;

fn eid(n: u128) -> EntityId {
    EntityId(Uuid::from_u128(n))
}
fn cid(n: u128) -> ConstraintId {
    ConstraintId(Uuid::from_u128(n))
}
fn v2(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y).expect("finite")
}

// Fixed point ids used across entity/constraint fixtures.
const P0: u128 = 0x0E00;
const P1: u128 = 0x0E01;
const P2: u128 = 0x0E02;
const CENTER: u128 = 0x0EC0;
const LINE_A: u128 = 0x0100;
const LINE_B: u128 = 0x0101;
const CIRCLE: u128 = 0x0C10;

/// One entity of every kind (referencing fixed point ids).
fn canonical_entities() -> Vec<(&'static str, SketchEntity)> {
    vec![
        (
            "point",
            SketchEntity::point(eid(P0), v2(0.0, 0.0), false, true),
        ),
        (
            "line",
            SketchEntity::line(eid(LINE_A), eid(P0), eid(P1), false),
        ),
        (
            "arc",
            SketchEntity::arc(
                eid(0x0A00),
                eid(CENTER),
                40.0,
                0.0,
                std::f64::consts::FRAC_PI_2,
                false,
            )
            .unwrap(),
        ),
        (
            "circle",
            SketchEntity::circle(eid(CIRCLE), eid(CENTER), 3.0, false).unwrap(),
        ),
        (
            "ellipse",
            SketchEntity::ellipse(eid(0x0E10), eid(CENTER), 10.0, 6.0, 0.25, true).unwrap(),
        ),
    ]
}

/// One constraint of every kind (all 18).
fn canonical_constraints() -> Vec<(&'static str, Constraint)> {
    vec![
        (
            "coincident",
            Constraint::Coincident {
                id: cid(1),
                point1: eid(P0),
                point2: eid(P1),
            },
        ),
        (
            "horizontal",
            Constraint::Horizontal {
                id: cid(2),
                line: eid(LINE_A),
            },
        ),
        (
            "vertical",
            Constraint::Vertical {
                id: cid(3),
                line: eid(LINE_B),
            },
        ),
        (
            "fixed",
            Constraint::Fixed {
                id: cid(4),
                point: eid(P0),
                at: v2(0.0, 0.0),
            },
        ),
        (
            "midpoint",
            Constraint::Midpoint {
                id: cid(5),
                point: eid(P2),
                line: eid(LINE_A),
            },
        ),
        (
            "on_curve",
            Constraint::OnCurve {
                id: cid(6),
                point: eid(P2),
                curve: eid(CIRCLE),
                position: CurvePosition::Arbitrary,
            },
        ),
        (
            "parallel",
            Constraint::Parallel {
                id: cid(7),
                line1: eid(LINE_A),
                line2: eid(LINE_B),
            },
        ),
        (
            "perpendicular",
            Constraint::Perpendicular {
                id: cid(8),
                line1: eid(LINE_A),
                line2: eid(LINE_B),
            },
        ),
        (
            "tangent",
            Constraint::Tangent {
                id: cid(9),
                entity1: eid(LINE_A),
                entity2: eid(CIRCLE),
            },
        ),
        (
            "concentric",
            Constraint::Concentric {
                id: cid(10),
                entity1: eid(CIRCLE),
                entity2: eid(0x0C11),
            },
        ),
        (
            "equal",
            Constraint::Equal {
                id: cid(11),
                entity1: eid(LINE_A),
                entity2: eid(LINE_B),
            },
        ),
        (
            "distance",
            Constraint::Distance {
                id: cid(12),
                entity1: eid(P0),
                entity2: eid(P1),
                value: Scalar::new(40.0),
            },
        ),
        (
            "horizontal_distance",
            Constraint::HorizontalDistance {
                id: cid(13),
                point1: eid(P0),
                point2: eid(P1),
                value: Scalar::new(40.0),
            },
        ),
        (
            "vertical_distance",
            Constraint::VerticalDistance {
                id: cid(14),
                point1: eid(P0),
                point2: eid(P2),
                value: Scalar::new(20.0),
            },
        ),
        (
            "angle",
            Constraint::Angle {
                id: cid(15),
                line1: eid(LINE_A),
                line2: eid(LINE_B),
                // radians (C++ AngleConstraint value is radians)
                value: Scalar::with_expr(std::f64::consts::FRAC_PI_2, "rightAngle"),
            },
        ),
        (
            "radius",
            Constraint::Radius {
                id: cid(16),
                entity: eid(CIRCLE),
                value: Scalar::new(3.0),
            },
        ),
        (
            "diameter",
            Constraint::Diameter {
                id: cid(17),
                entity: eid(CIRCLE),
                value: Scalar::new(6.0),
            },
        ),
        (
            "symmetric",
            Constraint::Symmetric {
                id: cid(18),
                point1: eid(P0),
                point2: eid(P1),
                axis: eid(LINE_B),
            },
        ),
    ]
}

/// A full, valid sketch document exercising plane + attachment + a region cache
/// + document-level extra flatten.
fn canonical_sketch() -> Sketch {
    let mut s = Sketch::on_world_plane(
        SketchId(Uuid::from_u128(0x5C01)),
        "Sketch 1",
        WorldPlane::XY,
    );

    // Points first (entities reference them by id).
    for (id, at) in [
        (P0, v2(0.0, 0.0)),
        (P1, v2(40.0, 0.0)),
        (P2, v2(40.0, 20.0)),
        (CENTER, v2(10.0, 10.0)),
    ] {
        s.add_entity(SketchEntity::point(eid(id), at, false, false))
            .unwrap();
    }
    s.add_entity(SketchEntity::line(eid(LINE_A), eid(P0), eid(P1), false))
        .unwrap();
    s.add_entity(SketchEntity::line(eid(LINE_B), eid(P1), eid(P2), false))
        .unwrap();
    s.add_entity(SketchEntity::circle(eid(CIRCLE), eid(CENTER), 3.0, true).unwrap())
        .unwrap();

    s.add_constraint(Constraint::Horizontal {
        id: cid(1),
        line: eid(LINE_A),
    })
    .unwrap();
    s.add_constraint(Constraint::Coincident {
        id: cid(2),
        point1: eid(P1),
        point2: eid(P1),
    })
    .unwrap();
    s.add_constraint(Constraint::Distance {
        id: cid(3),
        entity1: eid(P0),
        entity2: eid(P1),
        value: Scalar::new(40.0),
    })
    .unwrap();

    // Host-face attachment with a fully-formed primary ElementRef.
    s.attachment = SketchAttachment::HostFace {
        face: ElementRef {
            primary: Some(PrimaryRef {
                body: BodyId(Uuid::from_u128(0xB0D1)),
                element: ElementId::new("el_face_1"),
                kind: ElementKind::Face,
                extra: Default::default(),
            }),
            intent: None,
            anchor: None,
            extra: Default::default(),
        },
        projected_boundary_version: 2,
    };

    // Region cache (NOT authoritative) — one region with a deterministic id.
    s.set_regions(vec![RegionInfo {
        id: RegionId::new("r_fbf1e34acfb51ba4"),
        outer: vec![eid(LINE_A), eid(LINE_B)],
        holes: vec![vec![eid(CIRCLE)]],
        extra: Default::default(),
    }]);

    // Document-level unknown key survives via the top-level extra flatten.
    s.extra
        .insert("uiHint".into(), serde_json::json!({ "color": "#3af" }));
    s
}

#[test]
fn every_entity_kind_is_snapshot_locked() {
    for (key, entity) in canonical_entities() {
        insta::assert_json_snapshot!(format!("entity_{key}"), entity);
    }
}

#[test]
fn every_constraint_kind_is_snapshot_locked() {
    let all = canonical_constraints();
    assert_eq!(all.len(), 18, "all 18 constraint kinds must be covered");
    for (key, constraint) in all {
        insta::assert_json_snapshot!(format!("constraint_{key}"), constraint);
    }
}

#[test]
fn full_sketch_document_is_snapshot_locked() {
    insta::assert_json_snapshot!("sketch_document", canonical_sketch());
}

/// The full document round-trips byte-stably (serialize → parse → serialize).
#[test]
fn full_sketch_round_trips_byte_stable() {
    let s = canonical_sketch();
    let json1 = serde_json::to_value(&s).unwrap();
    let back: Sketch = serde_json::from_value(json1.clone()).unwrap();
    let json2 = serde_json::to_value(&back).unwrap();
    assert_eq!(
        json1, json2,
        "sketch document round-trip must be byte-stable"
    );
    assert_eq!(s, back);
}
