//! Datum (reference) planes usable as sketch planes.
//!
//! Ported faithfully from OneCAD-CPP `src/app/document/DatumPlane.h`. A datum
//! plane is defined **parametrically** (offset/angle relative to a base plane,
//! or offset from a model face) and carries a **cached resolved frame**. A
//! sketch placed on a datum copies the resolved frame at creation (frozen, like
//! sketch-on-face). The frame is re-derived by the worker in the regen epilogue
//! (V1/V2 §4.3); the core stores the parametric definition + the last resolved
//! frame.
//!
//! Serde: camelCase, no `deny_unknown_fields`; carries an `extra` flatten so
//! unknown fields round-trip. The typed-id fields (`base_body`, `base_face`,
//! `axis_edge`) are `Option`s — C++ stores them as (possibly empty) strings.

use serde::{Deserialize, Serialize};

use crate::document::refs::Extra;
use crate::ids::{BodyId, DatumPlaneId, ElementId};
use crate::sketch::SketchPlane;

/// How a datum plane's frame is derived (C++ `DatumPlane::Kind`). Serialized as
/// the PascalCase token (`datumPlaneKindName` in C++).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum DatumKind {
    /// Offset along a base origin/datum plane normal.
    #[default]
    OffsetFromPlane,
    /// Offset from a model face (frozen snapshot).
    OffsetFromFace,
    /// Rotate a base plane about an edge by `angle_deg`.
    AngledFromEdge,
    /// Frame from three points (reserved).
    ThreePoint,
}

/// A user-created reference plane (C++ `DatumPlane`).
///
/// The definition fields are parametric and re-derivable; `resolved_plane` /
/// `resolved_valid` cache the last resolved frame (the source of truth for
/// sketches placed on this datum).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatumPlane {
    /// Stable datum identity (C++ `id`, a UUID string).
    pub id: DatumPlaneId,
    /// Human-facing name.
    pub name: String,
    /// How the frame is derived.
    pub kind: DatumKind,

    // ── Parametric definition (re-derivable) ────────────────────────────────
    /// `"XY"`/`"XZ"`/`"YZ"` or another datum id (Offset/Angled). C++
    /// `basePlaneId` — a heterogeneous reference, kept as a raw string.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub base_plane_id: String,
    /// Owner body of `base_face` (`OffsetFromFace`). C++ `baseBodyId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_body: Option<BodyId>,
    /// The model face this datum is offset from (`OffsetFromFace`, frozen). C++
    /// `baseFaceId` (an ElementMap id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_face: Option<ElementId>,
    /// The edge this datum is rotated about (`AngledFromEdge`). C++ `axisEdgeId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis_edge: Option<ElementId>,
    /// Offset distance (mm).
    pub offset: f64,
    /// Rotation angle (degrees).
    pub angle_deg: f64,

    // ── Cached resolved frame ───────────────────────────────────────────────
    /// The resolved coordinate frame (source of truth for placed sketches).
    pub resolved_plane: SketchPlane,
    /// Whether `resolved_plane` is currently valid.
    pub resolved_valid: bool,

    /// Unknown keys, preserved verbatim.
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

impl DatumPlane {
    /// An `OffsetFromPlane` datum offset from a named world plane
    /// (`"XY"`/`"XZ"`/`"YZ"`), with an as-yet-unresolved frame.
    #[must_use]
    pub fn offset_from_plane(
        id: DatumPlaneId,
        name: impl Into<String>,
        base_plane_id: impl Into<String>,
        offset: f64,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            kind: DatumKind::OffsetFromPlane,
            base_plane_id: base_plane_id.into(),
            base_body: None,
            base_face: None,
            axis_edge: None,
            offset,
            angle_deg: 0.0,
            resolved_plane: SketchPlane::xy(),
            resolved_valid: false,
            extra: Extra::new(),
        }
    }

    /// Sets the cached resolved frame (worker regen epilogue).
    pub fn set_resolved(&mut self, plane: SketchPlane) {
        self.resolved_plane = plane;
        self.resolved_valid = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn did(n: u128) -> DatumPlaneId {
        DatumPlaneId(Uuid::from_u128(n))
    }

    #[test]
    fn datum_round_trips_and_kind_is_pascal_case() {
        let mut d = DatumPlane::offset_from_plane(did(1), "Datum 1", "XY", 10.0);
        d.set_resolved(SketchPlane::xz());
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["kind"], "OffsetFromPlane");
        assert_eq!(v["basePlaneId"], "XY");
        assert_eq!(v["resolvedValid"], true);
        let back: DatumPlane = serde_json::from_value(v).unwrap();
        assert_eq!(d, back);
    }

    #[test]
    fn offset_from_face_carries_typed_refs() {
        let mut d = DatumPlane::offset_from_plane(did(2), "Datum 2", "", 5.0);
        d.kind = DatumKind::OffsetFromFace;
        d.base_body = Some(BodyId(Uuid::from_u128(0xB0)));
        d.base_face = Some(ElementId::new("el_face_9"));
        let back: DatumPlane = serde_json::from_value(serde_json::to_value(&d).unwrap()).unwrap();
        assert_eq!(d, back);
    }
}
