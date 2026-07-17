//! Sketch constraints — the 18 kinds ported field-for-field from OneCAD-CPP
//! (`SketchTypes.h ConstraintType` + the `constraints/Constraints.h` classes +
//! `Sketch.h` add* helpers).
//!
//! Each variant carries a [`ConstraintId`] and the exact entity/point ids its
//! C++ class references. Dimensional constraints (Distance, HorizontalDistance,
//! VerticalDistance, Angle, Radius, Diameter — the C++ `DimensionalConstraint`
//! subclasses) carry a [`Scalar`] `value` (the expression slot; V1 expr = bare
//! variable name). `Fixed` is NOT dimensional in C++ (plain `fixedX`/`fixedY`),
//! so it carries a [`Vec2`] `at`, not a `Scalar`.
//!
//! **Angle** stores **radians** (C++ `AngleConstraint::value()` is radians;
//! `angleDegrees()` is a derived accessor). **HorizontalDistance/VerticalDistance**
//! are *signed* (`x2 − x1` / `y2 − y1`).
//!
//! **Horizontal / Vertical are line-form only.** Verified in
//! `OneCAD-CPP/src/core/sketch/Sketch.cpp`: `addHorizontal(lineOrPoint1,
//! point2)` with two points is merely an *input-resolution convenience* — it
//! searches for the existing line whose endpoints are those two points and
//! constructs a `HorizontalConstraint(lineId)`. The constraint CLASS
//! (`Constraints.h`) stores a single `m_lineId` and serializes `{"line": …}`.
//! There is no stored two-point form, so the domain model carries one `line`.
//!
//! **Serde** — internally tagged on `"kind"`, camelCase variant values +
//! camelCase fields. Same forward-compat rules as [`crate::sketch::entity`]:
//! alien variants are not preservable, alien fields inside a known variant are
//! dropped (fixture-gated schema evolution).
//!
//! **DISCREPANCY** (report, don't edit — SCHEMA wins on wire naming): SCHEMA
//! §7.3 (lines 608-611, 623-627) models a constraint generically as
//! `{id, type:"<PascalCase>", entities:[ids], value?, positions?}` — a flat
//! `entities` array under a `"type"` tag. This typed domain model instead names
//! each reference per its C++ role (`point1`/`line`/`entity1`/…) under a
//! `"kind"` tag, and exposes [`Constraint::entities`] to recover the flat id
//! list the solver adapter + dependency tracker need. Bridging is the
//! `onecad-protocol` adapter's job.

use serde::{Deserialize, Serialize};

use crate::document::variables::Scalar;
use crate::ids::{ConstraintId, EntityId};
use crate::math::Vec2;

/// Where a `PointOnCurve` (`OnCurve`) constraint pins the point on its curve
/// (C++ `CurvePosition`).
///
/// Serde: camelCase (`start`/`end`/`arbitrary`). (SCHEMA §7.3 spells the
/// analogous `positions` tokens PascalCase — reported divergence.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CurvePosition {
    /// Pin to the curve's start point.
    Start,
    /// Pin to the curve's end point.
    End,
    /// Pin anywhere on the curve (point may slide; C++ default).
    #[default]
    Arbitrary,
}

/// A geometric or dimensional constraint (18 kinds; C++ `ConstraintType`).
///
/// Internally tagged on `"kind"`. Every variant carries a [`ConstraintId`]
/// (`id`); [`Constraint::entities`] returns the referenced entity ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Constraint {
    /// Two points share a location (C++ `CoincidentConstraint`).
    Coincident {
        /// Constraint identity.
        id: ConstraintId,
        /// First point entity.
        point1: EntityId,
        /// Second point entity.
        point2: EntityId,
    },
    /// A line is horizontal (C++ `HorizontalConstraint`, line-form only).
    Horizontal {
        /// Constraint identity.
        id: ConstraintId,
        /// The line entity.
        line: EntityId,
    },
    /// A line is vertical (C++ `VerticalConstraint`, line-form only).
    Vertical {
        /// Constraint identity.
        id: ConstraintId,
        /// The line entity.
        line: EntityId,
    },
    /// A point is pinned to fixed coordinates (C++ `FixedConstraint`;
    /// `fixedX`/`fixedY`). Not a dimensional constraint — carries a [`Vec2`].
    Fixed {
        /// Constraint identity.
        id: ConstraintId,
        /// The pinned point entity.
        point: EntityId,
        /// Fixed position in sketch coordinates.
        at: Vec2,
    },
    /// A point lies at a line's midpoint (C++ `MidpointConstraint`).
    Midpoint {
        /// Constraint identity.
        id: ConstraintId,
        /// The constrained point entity.
        point: EntityId,
        /// The line entity.
        line: EntityId,
    },
    /// A point lies on a curve (line/arc/circle) at `position`
    /// (C++ `PointOnCurveConstraint`; `ConstraintType::OnCurve`).
    OnCurve {
        /// Constraint identity.
        id: ConstraintId,
        /// The constrained point entity.
        point: EntityId,
        /// The curve entity.
        curve: EntityId,
        /// Where on the curve the point is pinned.
        #[serde(default)]
        position: CurvePosition,
    },
    /// Two lines are parallel (C++ `ParallelConstraint`).
    Parallel {
        /// Constraint identity.
        id: ConstraintId,
        /// First line entity.
        line1: EntityId,
        /// Second line entity.
        line2: EntityId,
    },
    /// Two lines are perpendicular (C++ `PerpendicularConstraint`).
    Perpendicular {
        /// Constraint identity.
        id: ConstraintId,
        /// First line entity.
        line1: EntityId,
        /// Second line entity.
        line2: EntityId,
    },
    /// A curve is tangent to another entity (C++ `TangentConstraint`).
    Tangent {
        /// Constraint identity.
        id: ConstraintId,
        /// First entity.
        entity1: EntityId,
        /// Second entity.
        entity2: EntityId,
    },
    /// Two arcs/circles share a center (C++ `ConcentricConstraint`).
    Concentric {
        /// Constraint identity.
        id: ConstraintId,
        /// First arc/circle entity.
        entity1: EntityId,
        /// Second arc/circle entity.
        entity2: EntityId,
    },
    /// Two entities are equal in size (length/radius) (C++ `EqualConstraint`).
    Equal {
        /// Constraint identity.
        id: ConstraintId,
        /// First entity.
        entity1: EntityId,
        /// Second entity.
        entity2: EntityId,
    },
    /// Fixed distance between two entities (C++ `DistanceConstraint`; supports
    /// point-point, point-line, line-line).
    Distance {
        /// Constraint identity.
        id: ConstraintId,
        /// First entity.
        entity1: EntityId,
        /// Second entity.
        entity2: EntityId,
        /// Distance value (mm), expression-capable.
        value: Scalar,
    },
    /// Signed horizontal distance `x2 − x1` between two points
    /// (C++ `HorizontalDistanceConstraint`).
    HorizontalDistance {
        /// Constraint identity.
        id: ConstraintId,
        /// First point entity.
        point1: EntityId,
        /// Second point entity.
        point2: EntityId,
        /// Signed distance value (mm), expression-capable.
        value: Scalar,
    },
    /// Signed vertical distance `y2 − y1` between two points
    /// (C++ `VerticalDistanceConstraint`).
    VerticalDistance {
        /// Constraint identity.
        id: ConstraintId,
        /// First point entity.
        point1: EntityId,
        /// Second point entity.
        point2: EntityId,
        /// Signed distance value (mm), expression-capable.
        value: Scalar,
    },
    /// Angle between two lines (C++ `AngleConstraint`; value in **radians**).
    Angle {
        /// Constraint identity.
        id: ConstraintId,
        /// First line entity.
        line1: EntityId,
        /// Second line entity.
        line2: EntityId,
        /// Angle value in **radians**, expression-capable.
        value: Scalar,
    },
    /// Radius of an arc/circle (C++ `RadiusConstraint`).
    Radius {
        /// Constraint identity.
        id: ConstraintId,
        /// The arc/circle entity.
        entity: EntityId,
        /// Radius value (mm), expression-capable.
        value: Scalar,
    },
    /// Diameter of an arc/circle (C++ `DiameterConstraint`).
    Diameter {
        /// Constraint identity.
        id: ConstraintId,
        /// The arc/circle entity.
        entity: EntityId,
        /// Diameter value (mm), expression-capable.
        value: Scalar,
    },
    /// Two points mirrored about an axis line (C++ `SymmetricConstraint`).
    Symmetric {
        /// Constraint identity.
        id: ConstraintId,
        /// First point entity.
        point1: EntityId,
        /// Second point entity.
        point2: EntityId,
        /// The mirror-axis line entity (C++ `axisLine`).
        axis: EntityId,
    },
}

impl Constraint {
    /// This constraint's identity.
    #[must_use]
    pub fn id(&self) -> ConstraintId {
        match *self {
            Self::Coincident { id, .. }
            | Self::Horizontal { id, .. }
            | Self::Vertical { id, .. }
            | Self::Fixed { id, .. }
            | Self::Midpoint { id, .. }
            | Self::OnCurve { id, .. }
            | Self::Parallel { id, .. }
            | Self::Perpendicular { id, .. }
            | Self::Tangent { id, .. }
            | Self::Concentric { id, .. }
            | Self::Equal { id, .. }
            | Self::Distance { id, .. }
            | Self::HorizontalDistance { id, .. }
            | Self::VerticalDistance { id, .. }
            | Self::Angle { id, .. }
            | Self::Radius { id, .. }
            | Self::Diameter { id, .. }
            | Self::Symmetric { id, .. } => id,
        }
    }

    /// The referenced entity ids (for the solver adapter + dependency tracking).
    /// Order follows the C++ `referencedEntities()` ordering.
    #[must_use]
    pub fn entities(&self) -> Vec<EntityId> {
        match *self {
            Self::Horizontal { line, .. } | Self::Vertical { line, .. } => vec![line],
            Self::Fixed { point, .. } => vec![point],
            Self::Radius { entity, .. } | Self::Diameter { entity, .. } => vec![entity],
            Self::Coincident { point1, point2, .. }
            | Self::HorizontalDistance { point1, point2, .. }
            | Self::VerticalDistance { point1, point2, .. } => vec![point1, point2],
            Self::Midpoint { point, line, .. } => vec![point, line],
            Self::OnCurve { point, curve, .. } => vec![point, curve],
            Self::Parallel { line1, line2, .. }
            | Self::Perpendicular { line1, line2, .. }
            | Self::Angle { line1, line2, .. } => vec![line1, line2],
            Self::Tangent {
                entity1, entity2, ..
            }
            | Self::Concentric {
                entity1, entity2, ..
            }
            | Self::Equal {
                entity1, entity2, ..
            }
            | Self::Distance {
                entity1, entity2, ..
            } => vec![entity1, entity2],
            Self::Symmetric {
                point1,
                point2,
                axis,
                ..
            } => vec![point1, point2, axis],
        }
    }

    /// The dimensional value, when this is a dimensional constraint (Distance,
    /// HorizontalDistance, VerticalDistance, Angle, Radius, Diameter). `None`
    /// for geometric constraints (including `Fixed`, whose position is a
    /// [`Vec2`], not a `Scalar`).
    #[must_use]
    pub fn value(&self) -> Option<&Scalar> {
        match self {
            Self::Distance { value, .. }
            | Self::HorizontalDistance { value, .. }
            | Self::VerticalDistance { value, .. }
            | Self::Angle { value, .. }
            | Self::Radius { value, .. }
            | Self::Diameter { value, .. } => Some(value),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn cid(n: u128) -> ConstraintId {
        ConstraintId(Uuid::from_u128(n))
    }
    fn eid(n: u128) -> EntityId {
        EntityId(Uuid::from_u128(n))
    }

    /// Table: every kind's `entities()` matches the C++ `referencedEntities()`.
    #[test]
    fn entities_table_matches_cpp() {
        let (p1, p2, ax, l1, l2, e1, e2, cu) = (
            eid(1),
            eid(2),
            eid(3),
            eid(4),
            eid(5),
            eid(6),
            eid(7),
            eid(8),
        );
        let cases: Vec<(Constraint, Vec<EntityId>)> = vec![
            (
                Constraint::Coincident {
                    id: cid(1),
                    point1: p1,
                    point2: p2,
                },
                vec![p1, p2],
            ),
            (
                Constraint::Horizontal {
                    id: cid(2),
                    line: l1,
                },
                vec![l1],
            ),
            (
                Constraint::Vertical {
                    id: cid(3),
                    line: l1,
                },
                vec![l1],
            ),
            (
                Constraint::Fixed {
                    id: cid(4),
                    point: p1,
                    at: Vec2::new_unchecked(1.0, 2.0),
                },
                vec![p1],
            ),
            (
                Constraint::Midpoint {
                    id: cid(5),
                    point: p1,
                    line: l1,
                },
                vec![p1, l1],
            ),
            (
                Constraint::OnCurve {
                    id: cid(6),
                    point: p1,
                    curve: cu,
                    position: CurvePosition::Arbitrary,
                },
                vec![p1, cu],
            ),
            (
                Constraint::Parallel {
                    id: cid(7),
                    line1: l1,
                    line2: l2,
                },
                vec![l1, l2],
            ),
            (
                Constraint::Perpendicular {
                    id: cid(8),
                    line1: l1,
                    line2: l2,
                },
                vec![l1, l2],
            ),
            (
                Constraint::Tangent {
                    id: cid(9),
                    entity1: e1,
                    entity2: e2,
                },
                vec![e1, e2],
            ),
            (
                Constraint::Concentric {
                    id: cid(10),
                    entity1: e1,
                    entity2: e2,
                },
                vec![e1, e2],
            ),
            (
                Constraint::Equal {
                    id: cid(11),
                    entity1: e1,
                    entity2: e2,
                },
                vec![e1, e2],
            ),
            (
                Constraint::Distance {
                    id: cid(12),
                    entity1: e1,
                    entity2: e2,
                    value: Scalar::new(40.0),
                },
                vec![e1, e2],
            ),
            (
                Constraint::HorizontalDistance {
                    id: cid(13),
                    point1: p1,
                    point2: p2,
                    value: Scalar::new(10.0),
                },
                vec![p1, p2],
            ),
            (
                Constraint::VerticalDistance {
                    id: cid(14),
                    point1: p1,
                    point2: p2,
                    value: Scalar::new(10.0),
                },
                vec![p1, p2],
            ),
            (
                Constraint::Angle {
                    id: cid(15),
                    line1: l1,
                    line2: l2,
                    value: Scalar::new(std::f64::consts::FRAC_PI_2),
                },
                vec![l1, l2],
            ),
            (
                Constraint::Radius {
                    id: cid(16),
                    entity: cu,
                    value: Scalar::new(5.0),
                },
                vec![cu],
            ),
            (
                Constraint::Diameter {
                    id: cid(17),
                    entity: cu,
                    value: Scalar::new(10.0),
                },
                vec![cu],
            ),
            (
                Constraint::Symmetric {
                    id: cid(18),
                    point1: p1,
                    point2: p2,
                    axis: ax,
                },
                vec![p1, p2, ax],
            ),
        ];
        assert_eq!(cases.len(), 18, "all 18 constraint kinds covered");
        for (c, want) in cases {
            assert_eq!(c.entities(), want, "entities() mismatch for {:?}", c.id());
        }
    }

    #[test]
    fn value_only_on_dimensional_kinds() {
        let dim = Constraint::Radius {
            id: cid(1),
            entity: eid(1),
            value: Scalar::new(5.0),
        };
        assert_eq!(dim.value().unwrap().value, 5.0);
        let geo = Constraint::Horizontal {
            id: cid(2),
            line: eid(1),
        };
        assert!(geo.value().is_none());
        // Fixed is geometric: no Scalar value.
        let fixed = Constraint::Fixed {
            id: cid(3),
            point: eid(1),
            at: Vec2::new_unchecked(0.0, 0.0),
        };
        assert!(fixed.value().is_none());
    }

    #[test]
    fn distance_value_parses_bare_number_from_schema_wire() {
        // SCHEMA §7.3 line 611 sends `"value": 40.0` as a bare number; Scalar
        // accepts it. Tag stays `kind` in the domain model (SCHEMA uses `type`).
        let json = serde_json::json!({
            "kind": "distance",
            "id": "00000000-0000-0000-0000-000000000001",
            "entity1": "00000000-0000-0000-0000-000000000002",
            "entity2": "00000000-0000-0000-0000-000000000003",
            "value": 40.0
        });
        let c: Constraint = serde_json::from_value(json).unwrap();
        assert_eq!(c.value().unwrap().value, 40.0);
    }
}
