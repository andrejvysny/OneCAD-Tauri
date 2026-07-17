//! Sketch entities (geometry primitives inside a sketch).
//!
//! Ports the OneCAD-CPP **topology model VERBATIM**: entities reference **point
//! entities by id** rather than inlining coordinates. Only [`Point`] carries a
//! position; a [`Line`] stores its two endpoint `EntityId`s, and an [`Arc`] /
//! [`Circle`] / [`Ellipse`] its center `EntityId`. This is the C++ pattern
//! (`SketchLine{startPointId,endPointId}`, `SketchArc{centerPointId,radius,
//! startAngle,endAngle}`, `SketchCircle{centerPointId,radius}`,
//! `SketchEllipse{centerPointId,majorRadius,minorRadius,rotation}` — see
//! `OneCAD-CPP/src/core/sketch/Sketch{Point,Line,Arc,Circle,Ellipse}.h`). Keep
//! it: shared point identity is what makes coincident endpoints and rubber-band
//! drags work.
//!
//! [`Point`]: SketchEntity::Point
//! [`Line`]: SketchEntity::Line
//! [`Arc`]: SketchEntity::Arc
//! [`Circle`]: SketchEntity::Circle
//! [`Ellipse`]: SketchEntity::Ellipse
//!
//! **Serde** — internally tagged on `"kind"`, camelCase variant values +
//! camelCase fields. Numeric fields reject `NaN`/`±Inf` both at the constructor
//! boundary (checked constructors return `Option`) and on deserialize (SCHEMA
//! §4). `construction` / `referenceLocked` default to `false` when absent.
//!
//! **Forward-compat** (see the note in [`crate::sketch`]): a tagged enum cannot
//! preserve an ALIEN variant (an unknown `kind` fails to deserialize) and drops
//! alien fields inside a known variant. Entity-schema evolution is therefore
//! fixture-gated (a snapshot bump), not silently round-tripped. This is the
//! documented decision — no per-variant `extra` catch-all is added (it would
//! complicate the internally-tagged codec for little gain).
//!
//! **DISCREPANCY** (report, don't edit — SCHEMA wins on wire naming): SCHEMA
//! §7.3 (lines 602-606, 622) tags entities on `"type"` with PascalCase values
//! and **inlines coordinates** — `{type:"Line", p0, p1}`, `{type:"Arc", center,
//! radius, start, end}` (start/end are points, not angles), `{type:"Circle",
//! center, radius}`. That is the worker-lane wire shape (carried opaquely by
//! `SketchOpParams`); this typed model is the authoritative document / sketch
//! file format. Bridging the two is the `onecad-protocol` adapter's job.
//! Additional C++ fidelity note: C++ carries `referenceLocked` on the *base*
//! `SketchEntity` (all kinds); the WP scopes it to `Point` only — kept as
//! specified.

use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::EntityId;
use crate::math::Vec2;

/// Rejects `NaN`/`±Inf` on a raw `f64` field during deserialization
/// (SCHEMA §4). Integer JSON values (e.g. `40`) are accepted and widened.
fn de_finite<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    let v = f64::deserialize(d)?;
    if v.is_finite() {
        Ok(v)
    } else {
        Err(serde::de::Error::custom("non-finite value"))
    }
}

/// A geometry primitive in a sketch.
///
/// Internally tagged on `"kind"` ∈ `point` | `line` | `arc` | `circle` |
/// `ellipse`. Non-`Point` kinds reference their defining points by `EntityId`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SketchEntity {
    /// A 2D point — the only entity carrying coordinates (`at`). Others
    /// reference points by id. DOF: 2 (C++ `SketchPoint`).
    Point {
        /// Entity identity.
        id: EntityId,
        /// Position in sketch-local coordinates (mm).
        at: Vec2,
        /// Construction geometry (reference only; cannot form faces).
        #[serde(default)]
        construction: bool,
        /// Locked host-face reference geometry (selectable but not editable).
        /// C++ keeps this on every entity; scoped to `Point` per the WP.
        #[serde(default)]
        reference_locked: bool,
    },
    /// A line segment between two point entities (C++ `SketchLine`).
    Line {
        /// Entity identity.
        id: EntityId,
        /// Start point entity id.
        start: EntityId,
        /// End point entity id.
        end: EntityId,
        /// Construction geometry.
        #[serde(default)]
        construction: bool,
    },
    /// A circular arc: center point + radius + CCW angular extent, angles in
    /// radians from +X (C++ `SketchArc`).
    Arc {
        /// Entity identity.
        id: EntityId,
        /// Center point entity id.
        center: EntityId,
        /// Radius (mm).
        #[serde(deserialize_with = "de_finite")]
        radius: f64,
        /// Start angle (radians, from +X, CCW).
        #[serde(deserialize_with = "de_finite")]
        start_angle: f64,
        /// End angle (radians, from +X, CCW).
        #[serde(deserialize_with = "de_finite")]
        end_angle: f64,
        /// Construction geometry.
        #[serde(default)]
        construction: bool,
    },
    /// A full circle: center point + radius (C++ `SketchCircle`).
    Circle {
        /// Entity identity.
        id: EntityId,
        /// Center point entity id.
        center: EntityId,
        /// Radius (mm).
        #[serde(deserialize_with = "de_finite")]
        radius: f64,
        /// Construction geometry.
        #[serde(default)]
        construction: bool,
    },
    /// An ellipse: center point + semi-major/minor radii + major-axis rotation
    /// (radians from +X) (C++ `SketchEllipse`).
    Ellipse {
        /// Entity identity.
        id: EntityId,
        /// Center point entity id.
        center: EntityId,
        /// Semi-major radius (mm).
        #[serde(deserialize_with = "de_finite")]
        major_r: f64,
        /// Semi-minor radius (mm).
        #[serde(deserialize_with = "de_finite")]
        minor_r: f64,
        /// Major-axis rotation (radians from +X).
        #[serde(deserialize_with = "de_finite")]
        rotation: f64,
        /// Construction geometry.
        #[serde(default)]
        construction: bool,
    },
}

impl SketchEntity {
    /// A point at `at` (already finite — [`Vec2`] rejects non-finite on
    /// construction).
    #[must_use]
    pub fn point(id: EntityId, at: Vec2, construction: bool, reference_locked: bool) -> Self {
        Self::Point {
            id,
            at,
            construction,
            reference_locked,
        }
    }

    /// A line between two point entities.
    #[must_use]
    pub fn line(id: EntityId, start: EntityId, end: EntityId, construction: bool) -> Self {
        Self::Line {
            id,
            start,
            end,
            construction,
        }
    }

    /// An arc, rejecting non-finite `radius`/`start_angle`/`end_angle`
    /// (SCHEMA §4).
    #[must_use]
    pub fn arc(
        id: EntityId,
        center: EntityId,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        construction: bool,
    ) -> Option<Self> {
        (radius.is_finite() && start_angle.is_finite() && end_angle.is_finite()).then_some(
            Self::Arc {
                id,
                center,
                radius,
                start_angle,
                end_angle,
                construction,
            },
        )
    }

    /// A circle, rejecting a non-finite `radius` (SCHEMA §4).
    #[must_use]
    pub fn circle(id: EntityId, center: EntityId, radius: f64, construction: bool) -> Option<Self> {
        radius.is_finite().then_some(Self::Circle {
            id,
            center,
            radius,
            construction,
        })
    }

    /// An ellipse, rejecting non-finite `major_r`/`minor_r`/`rotation`
    /// (SCHEMA §4).
    #[must_use]
    pub fn ellipse(
        id: EntityId,
        center: EntityId,
        major_r: f64,
        minor_r: f64,
        rotation: f64,
        construction: bool,
    ) -> Option<Self> {
        (major_r.is_finite() && minor_r.is_finite() && rotation.is_finite()).then_some(
            Self::Ellipse {
                id,
                center,
                major_r,
                minor_r,
                rotation,
                construction,
            },
        )
    }

    /// This entity's identity.
    #[must_use]
    pub fn id(&self) -> EntityId {
        match *self {
            Self::Point { id, .. }
            | Self::Line { id, .. }
            | Self::Arc { id, .. }
            | Self::Circle { id, .. }
            | Self::Ellipse { id, .. } => id,
        }
    }

    /// True for construction (reference-only) geometry.
    #[must_use]
    pub fn is_construction(&self) -> bool {
        match *self {
            Self::Point { construction, .. }
            | Self::Line { construction, .. }
            | Self::Arc { construction, .. }
            | Self::Circle { construction, .. }
            | Self::Ellipse { construction, .. } => construction,
        }
    }

    /// The **point-entity ids** this entity references (for dangling-ref
    /// validation + dependency tracking). `Point` references nothing; `Line` →
    /// `[start, end]`; `Arc`/`Circle`/`Ellipse` → `[center]`.
    #[must_use]
    pub fn referenced_entities(&self) -> Vec<EntityId> {
        match *self {
            Self::Point { .. } => Vec::new(),
            Self::Line { start, end, .. } => vec![start, end],
            Self::Arc { center, .. }
            | Self::Circle { center, .. }
            | Self::Ellipse { center, .. } => vec![center],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::EntityId;
    use uuid::Uuid;

    fn eid(n: u128) -> EntityId {
        EntityId(Uuid::from_u128(n))
    }

    #[test]
    fn checked_constructors_reject_non_finite() {
        let c = eid(1);
        assert!(SketchEntity::arc(eid(2), c, f64::NAN, 0.0, 1.0, false).is_none());
        assert!(SketchEntity::arc(eid(2), c, 5.0, f64::INFINITY, 1.0, false).is_none());
        assert!(SketchEntity::circle(eid(2), c, f64::NAN, false).is_none());
        assert!(SketchEntity::ellipse(eid(2), c, 5.0, 3.0, f64::NEG_INFINITY, false).is_none());
        assert!(SketchEntity::circle(eid(2), c, 3.0, false).is_some());
    }

    #[test]
    fn deserialize_rejects_non_finite_radius() {
        // `1e999` parses to +Inf; `de_finite` must reject it through the
        // internally-tagged enum.
        let json = r#"{ "kind": "circle", "id": "00000000-0000-0000-0000-000000000001",
            "center": "00000000-0000-0000-0000-000000000002", "radius": 1e999 }"#;
        assert!(serde_json::from_str::<SketchEntity>(json).is_err());
    }

    #[test]
    fn tagged_kind_round_trips_and_defaults_bools() {
        let json = serde_json::json!({
            "kind": "line",
            "id": "00000000-0000-0000-0000-000000000010",
            "start": "00000000-0000-0000-0000-000000000011",
            "end": "00000000-0000-0000-0000-000000000012"
            // construction omitted -> defaults false
        });
        let e: SketchEntity = serde_json::from_value(json).unwrap();
        assert!(!e.is_construction());
        assert_eq!(e.referenced_entities(), vec![eid(0x11), eid(0x12)]);
        // Serialize carries the `kind` tag.
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["kind"], serde_json::json!("line"));
    }

    #[test]
    fn alien_variant_is_not_preservable() {
        // Documented forward-compat limitation: an unknown `kind` fails.
        let json = serde_json::json!({ "kind": "spline", "id":
            "00000000-0000-0000-0000-000000000001" });
        assert!(serde_json::from_value::<SketchEntity>(json).is_err());
    }

    #[test]
    fn referenced_entities_per_kind() {
        let c = eid(0xC);
        assert_eq!(
            SketchEntity::point(eid(1), Vec2::new_unchecked(0.0, 0.0), false, false)
                .referenced_entities(),
            Vec::<EntityId>::new()
        );
        assert_eq!(
            SketchEntity::arc(eid(2), c, 5.0, 0.0, 1.0, false)
                .unwrap()
                .referenced_entities(),
            vec![c]
        );
    }
}
