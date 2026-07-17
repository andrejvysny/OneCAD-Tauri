//! Frontend-facing projection DTOs (camelCase serde).
//!
//! These MIRROR the zustand store shapes the frontend already renders from
//! (`src/stores/documentStore.ts` — `DocumentProjection`/`BodyMeta`/`SketchMeta`/
//! `FeatureMeta` — and `src/ipc/types.ts`). Projection stores are written **only
//! by backend events** (plan "Frontend owns projection stores"); the app crate
//! mints these DTOs from the authoritative [`onecad_core`] document + the latest
//! regen [`ModelSnapshot`], and hands them to the webview via commands + events.
//!
//! The DTO layer lives in the app crate so `onecad-core` stays tauri-free and its
//! frozen file-format serde is never coupled to a UI wire shape.

use serde::Serialize;

use onecad_core::document::record::{KnownOperation, Operation};
use onecad_core::history::StepState;

/// Whether a document is open (`src/stores/documentStore.ts` `DocStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DocStatus {
    /// No document open.
    Empty,
    /// A document is loading.
    Loading,
    /// A document is open and ready.
    Ready,
}

/// One body in the tree (`documentStore.ts` `BodyMeta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyDto {
    pub id: String,
    pub name: String,
    pub visible: bool,
}

/// Sketch solve status (`documentStore.ts` `SketchStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SketchStatus {
    Ok,
    Under,
    Over,
    Error,
}

/// One sketch in the tree (`documentStore.ts` `SketchMeta`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchDto {
    pub id: String,
    pub name: String,
    pub visible: bool,
    pub dof: u32,
    pub status: SketchStatus,
}

/// Feature-timeline entry kind (`documentStore.ts` `FeatureKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FeatureKind {
    Sketch,
    Extrude,
    Revolve,
    Fillet,
    Boolean,
}

/// Feature regen status (`documentStore.ts` `FeatureStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FeatureStatus {
    Ok,
    Dirty,
    Error,
    NeedsRepair,
}

/// One feature-timeline entry (`documentStore.ts` `FeatureMeta`; identical shape
/// to `types.ts` `FeatureRecord` so a controller maps it 1:1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeatureDto {
    pub id: String,
    pub kind: FeatureKind,
    pub label: String,
    /// Mono value shown on the right of the history chip (e.g. `"25.0 mm"`).
    pub value_text: String,
    pub status: FeatureStatus,
}

/// The full document projection (`documentStore.ts` `DocumentProjection`).
///
/// `bodies`/`sketches` serialize as JSON objects keyed by id (the store's
/// `Record<string, …>`); a `BTreeMap` keeps the key order deterministic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentProjection {
    pub status: DocStatus,
    pub revision: u64,
    pub title: String,
    pub dirty: bool,
    pub bodies: std::collections::BTreeMap<String, BodyDto>,
    pub sketches: std::collections::BTreeMap<String, SketchDto>,
    pub features: Vec<FeatureDto>,
}

impl DocumentProjection {
    /// The projection for "no document open".
    #[must_use]
    pub fn empty() -> Self {
        Self {
            status: DocStatus::Empty,
            revision: 0,
            title: String::new(),
            dirty: false,
            bodies: std::collections::BTreeMap::new(),
            sketches: std::collections::BTreeMap::new(),
            features: Vec::new(),
        }
    }
}

/// A handle returned by open/new/close (`src/ipc/types.ts` `DocumentSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSnapshotDto {
    pub document_id: String,
    pub title: String,
}

/// One recent-project entry for the start screen (`types.ts` `RecentProject`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProjectDto {
    pub id: String,
    pub name: String,
    pub path: String,
    /// ISO-8601 last-modified timestamp.
    pub modified_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
}

/// One changed body in a `document-changed` event (`types.ts` `BodyMeshRef`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyMeshRef {
    pub body_id: String,
    /// Mirrors the Rust `MeshCache` key `(BodyId, Lod, generation)`, rendered
    /// `"<bodyId>:<lod>:<generation>"` (matches the mock's `mockMeshKey`).
    pub mesh_key: String,
}

/// The `document-changed` event payload (`types.ts` `DocumentChange`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentChange {
    pub revision: u64,
    pub changed_bodies: Vec<BodyMeshRef>,
    pub removed_bodies: Vec<String>,
}

// ── Mappers (op → feature kind / value; regen state → feature status) ─────────

/// Maps a timeline [`Operation`] to its frontend feature kind.
///
/// Ops outside the vertical slice (Shell/patterns/Loft/Sweep/Mirror) and opaque
/// frozen nodes fall back to the nearest slice kind — they never appear in a V1
/// document but keep the projection total.
#[must_use]
pub fn feature_kind(op: &Operation) -> FeatureKind {
    match op {
        Operation::Known(k) => match k {
            KnownOperation::Sketch(_) => FeatureKind::Sketch,
            KnownOperation::Extrude(_) | KnownOperation::Loft(_) | KnownOperation::Sweep(_) => {
                FeatureKind::Extrude
            }
            KnownOperation::Revolve(_) => FeatureKind::Revolve,
            KnownOperation::Fillet(_) | KnownOperation::Chamfer(_) | KnownOperation::Shell(_) => {
                FeatureKind::Fillet
            }
            KnownOperation::Boolean(_)
            | KnownOperation::LinearPattern(_)
            | KnownOperation::CircularPattern(_)
            | KnownOperation::MirrorBody(_) => FeatureKind::Boolean,
        },
        Operation::Opaque(_) => FeatureKind::Extrude,
    }
}

/// The mono value text for a feature chip (e.g. `"25.0 mm"` / `"360.0°"`). Empty
/// for dimensionless features (sketch/boolean).
#[must_use]
pub fn feature_value_text(op: &Operation) -> String {
    let Operation::Known(k) = op else {
        return String::new();
    };
    match k {
        KnownOperation::Extrude(p) => format!("{:.1} mm", p.distance.value.abs()),
        KnownOperation::Revolve(p) => format!("{:.1}°", p.angle_deg.value),
        KnownOperation::Fillet(p) => format!("{:.1} mm", p.radius.value),
        KnownOperation::Chamfer(p) => format!("{:.1} mm", p.radius.value),
        KnownOperation::Shell(p) => format!("{:.1} mm", p.thickness.value),
        _ => String::new(),
    }
}

/// The default label for a feature kind (used when a record carries no name).
#[must_use]
pub fn default_label(kind: FeatureKind) -> &'static str {
    match kind {
        FeatureKind::Sketch => "Sketch",
        FeatureKind::Extrude => "Extrude",
        FeatureKind::Revolve => "Revolve",
        FeatureKind::Fillet => "Fillet",
        FeatureKind::Boolean => "Boolean",
    }
}

/// Maps a regen [`StepState`] to a frontend feature status.
#[must_use]
pub fn feature_status(state: &StepState) -> FeatureStatus {
    match state {
        StepState::Valid => FeatureStatus::Ok,
        StepState::Error { .. } => FeatureStatus::Error,
        StepState::NeedsRepair => FeatureStatus::NeedsRepair,
        // Dirty (awaiting regen) and Suppressed (skipped) both read as inactive.
        StepState::Dirty | StepState::Suppressed => FeatureStatus::Dirty,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use onecad_core::document::record::{
        BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation,
    };
    use onecad_core::document::variables::Scalar;

    fn extrude(dist: f64) -> Operation {
        Operation::Known(KnownOperation::Extrude(ExtrudeParams {
            profile: None,
            distance: Scalar::new(dist),
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

    #[test]
    fn projection_serializes_camelcase_mirroring_the_store() {
        let mut bodies = std::collections::BTreeMap::new();
        bodies.insert(
            "b1".to_string(),
            BodyDto {
                id: "b1".into(),
                name: "Body 1".into(),
                visible: true,
            },
        );
        let proj = DocumentProjection {
            status: DocStatus::Ready,
            revision: 5,
            title: "Bracket".into(),
            dirty: false,
            bodies,
            sketches: std::collections::BTreeMap::new(),
            features: vec![FeatureDto {
                id: "f1".into(),
                kind: FeatureKind::Extrude,
                label: "Extrude".into(),
                value_text: "25.0 mm".into(),
                status: FeatureStatus::Ok,
            }],
        };
        let v = serde_json::to_value(&proj).unwrap();
        assert_eq!(v["status"], "ready");
        assert_eq!(v["revision"], 5);
        assert_eq!(v["bodies"]["b1"]["visible"], true);
        assert_eq!(v["features"][0]["kind"], "extrude");
        assert_eq!(v["features"][0]["valueText"], "25.0 mm");
        assert_eq!(v["features"][0]["status"], "ok");
    }

    #[test]
    fn extrude_value_text_and_kind() {
        let op = extrude(25.0);
        assert_eq!(feature_kind(&op), FeatureKind::Extrude);
        assert_eq!(feature_value_text(&op), "25.0 mm");
    }

    #[test]
    fn step_state_maps_to_feature_status() {
        assert_eq!(feature_status(&StepState::Valid), FeatureStatus::Ok);
        assert_eq!(feature_status(&StepState::Dirty), FeatureStatus::Dirty);
        assert_eq!(
            feature_status(&StepState::NeedsRepair),
            FeatureStatus::NeedsRepair
        );
        assert_eq!(
            feature_status(&StepState::Error {
                reason: "boom".into()
            }),
            FeatureStatus::Error
        );
    }
}
