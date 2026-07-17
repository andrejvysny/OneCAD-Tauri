//! Typed references from operations to topological inputs.
//!
//! Every op resolves its inputs on its exact predecessor snapshot; the stored
//! input anchor is never overwritten with the op's own output (Invariant 3).
//! An [`ElementRef`] carries three complementary layers so the resolution
//! ladder can rebind after edits (SCHEMA §7.3 "Semantic reference", §10):
//!
//! * `primary` — the last-known identity binding (`bodyId`/`elementId`/`kind`).
//! * `intent`  — the frozen worker-owned descriptor (**evidence, never
//!   identity** — Invariant 2), versioned.
//! * `anchor`  — geometric selection intent (world point, surface UV, local
//!   frame, adjacency hint) used to narrow ambiguity.
//!
//! Serde: camelCase, no `deny_unknown_fields`; every ref carries `extra` so
//! unknown fields injected at a nested-ref level survive a round-trip.

use serde::{Deserialize, Serialize};

use crate::ids::{BodyId, ElementId, EntityId, RegionId, SketchId};
use crate::math::{Vec2, Vec3};

/// Free-form map that absorbs unknown JSON keys so they round-trip losslessly.
pub type Extra = serde_json::Map<String, serde_json::Value>;

/// The kind of a topological element (SCHEMA §7.3 `kind` ∈ `face`/`edge`/
/// `vertex`).
///
/// Note: SCHEMA `primary.kind` also allows `body` for whole-body references;
/// bodies are referenced directly via [`BodyId`] in op params, so `ElementKind`
/// intentionally models only sub-body elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElementKind {
    Face,
    Edge,
    Vertex,
}

/// Last-known identity binding of a referenced element (SCHEMA §7.3
/// `primary`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrimaryRef {
    #[serde(rename = "bodyId")]
    pub body: BodyId,
    #[serde(rename = "elementId")]
    pub element: ElementId,
    pub kind: ElementKind,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Frozen worker-owned descriptor captured when the ref was authored — evidence
/// for the ladder, never identity (Invariant 2; SCHEMA §7.3 `intent`, §10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntentQuery {
    /// Descriptor / resolver version the descriptor was computed under.
    pub version: u32,
    pub kind: ElementKind,
    /// Opaque, worker-owned descriptor payload (§10). The core never
    /// interprets it; it round-trips verbatim.
    pub descriptor: serde_json::Value,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// A local coordinate frame captured at selection time (SCHEMA §7.3
/// `anchor.localFrame`).
///
/// Not `Copy`: carries an `extra` map so unknown keys injected at the
/// `localFrame` level survive a round-trip.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalFrame {
    pub origin: Vec3,
    pub x: Vec3,
    pub y: Vec3,
    pub z: Vec3,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Geometric selection intent used to narrow ladder ambiguity (SCHEMA §7.3
/// `anchor`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnchorIntent {
    pub world_point: Vec3,
    /// Surface parameters at the pick point. Wire form is the array `[u, v]`
    /// (finite-checked via [`Vec2`]); identical shape to the former `[f64; 2]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub surface_uv: Option<Vec2>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_frame: Option<LocalFrame>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adjacency_hint: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// A reference to a topological element: identity + evidence + anchor.
///
/// All three layers are optional so a ref can be authored from any subset of
/// evidence, but a usable ref has at least one populated.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<PrimaryRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<IntentQuery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorIntent>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Reference to a closed sketch profile region (extrude/revolve profile;
/// SCHEMA `SketchRegionRef` — C++ `SketchRegionRef{sketchId, regionId}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchRegionRef {
    #[serde(rename = "sketchId")]
    pub sketch: SketchId,
    #[serde(rename = "regionId")]
    pub region: RegionId,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Reference to a sketch line (used as a revolve axis; C++
/// `SketchLineRef{sketchId, lineId}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchLineRef {
    #[serde(rename = "sketchId")]
    pub sketch: SketchId,
    #[serde(rename = "lineId")]
    pub line: EntityId,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Reference to a face plus its explicit coplanar patch members (C++
/// `FaceRef{bodyId, faceId, patchFaceIds}`, redesigned around [`ElementRef`]).
///
/// Discrepancy note: C++/SCHEMA `FaceRef` is `{bodyId, faceId, patchFaceIds[]}`
/// (flat ids). The plan's Rust core carries the richer typed [`ElementRef`]
/// (identity + evidence + anchor) instead, so the face survives edits via the
/// ladder. `patch` holds the explicit coplanar patch members (empty for legacy
/// single-face ops).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceRef {
    pub reference: ElementRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub patch: Vec<ElementRef>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// A revolve/pattern axis: either a sketch line or a body edge (C++
/// `RevolveParams::AxisRef = variant<monostate, SketchLineRef, EdgeRef>`;
/// SCHEMA §7.3 `axis.kind` ∈ `sketchLine` | `edge` | `none`).
///
/// The `none` case is modeled as `Option::<AxisRef>::None` (absent axis), not a
/// variant.
/// Each variant carries a per-variant `extra` map: `serde`'s internally-tagged
/// codec buffers each struct variant through a `Content` map and honours a
/// trailing `#[serde(flatten)]` field, so unknown keys injected at the axis
/// level (beside `kind`) round-trip losslessly. The `axis_ref_alien_keys_*`
/// round-trip tests pin this behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AxisRef {
    /// A line entity inside a sketch (SCHEMA `kind:"sketchLine"`).
    SketchLine {
        #[serde(rename = "sketchId")]
        sketch: SketchId,
        #[serde(rename = "lineId")]
        line: EntityId,
        #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
        extra: Extra,
    },
    /// A body edge (SCHEMA `kind:"edge"`; C++ `EdgeRef{bodyId, edgeId}`).
    #[serde(rename = "edge")]
    Element {
        #[serde(rename = "bodyId")]
        body: BodyId,
        #[serde(rename = "edgeId")]
        edge: ElementId,
        #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
        extra: Extra,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M2: an alien key beside `kind` on an `AxisRef` variant round-trips via the
    /// per-variant `extra` map (internally-tagged codec honours the flatten).
    #[test]
    fn axis_ref_alien_keys_round_trip() {
        for json in [
            r#"{"kind":"sketchLine","sketchId":"00000000-0000-0000-0000-000000000001","lineId":"00000000-0000-0000-0000-000000000002","alienAxisKey":{"keep":true}}"#,
            r#"{"kind":"edge","bodyId":"00000000-0000-0000-0000-000000000003","edgeId":"e:9","alienAxisKey":{"keep":true}}"#,
        ] {
            let axis: AxisRef = serde_json::from_str(json).unwrap();
            let reser = serde_json::to_value(&axis).unwrap();
            assert_eq!(
                reser.get("alienAxisKey"),
                Some(&serde_json::json!({ "keep": true })),
                "axis-level unknown key must survive round-trip: {json}"
            );
        }
    }

    /// M2: an alien key at the `localFrame` level round-trips via `LocalFrame.extra`.
    #[test]
    fn local_frame_alien_keys_round_trip() {
        let mut frame = LocalFrame {
            origin: Vec3::new_unchecked(0.0, 0.0, 0.0),
            x: Vec3::new_unchecked(1.0, 0.0, 0.0),
            y: Vec3::new_unchecked(0.0, 1.0, 0.0),
            z: Vec3::new_unchecked(0.0, 0.0, 1.0),
            extra: Extra::new(),
        };
        frame
            .extra
            .insert("alienFrameKey".into(), serde_json::json!(7));
        let back: LocalFrame =
            serde_json::from_value(serde_json::to_value(&frame).unwrap()).unwrap();
        assert_eq!(back, frame);
        assert_eq!(back.extra.get("alienFrameKey"), Some(&serde_json::json!(7)));
    }

    /// m6: `AnchorIntent.surface_uv` is finite-checked (a `Vec2`) yet keeps the
    /// bare `[u, v]` wire shape.
    #[test]
    fn anchor_surface_uv_is_finite_checked_array() {
        let json = r#"{"worldPoint":[1,2,3],"surfaceUv":[0.25,0.75]}"#;
        let a: AnchorIntent = serde_json::from_str(json).unwrap();
        assert_eq!(a.surface_uv.map(<[f64; 2]>::from), Some([0.25, 0.75]));
        assert_eq!(
            serde_json::to_value(&a).unwrap()["surfaceUv"],
            serde_json::json!([0.25, 0.75])
        );
        // Non-finite uv rejected on read (1e999 → +Inf).
        assert!(serde_json::from_str::<AnchorIntent>(
            r#"{"worldPoint":[1,2,3],"surfaceUv":[1e999,0]}"#
        )
        .is_err());
    }
}
