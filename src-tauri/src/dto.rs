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

/// The `worker-status` event payload (`types.ts` `WorkerStatus`) — the sidecar
/// lifecycle the status bar surfaces. `state` is one of
/// `starting`|`ready`|`restarting`|`failed`; `epoch` is the worker epoch that
/// transition belongs to (`0` when unknown, e.g. terminal failure).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerStatusDto {
    pub state: String,
    pub epoch: u64,
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
    /// The published [`ModelSnapshot`](onecad_core::regen::ModelSnapshot) id this
    /// geometry belongs to (SCHEMA §7.5). The frontend forwards it to
    /// `promoteSelection` so a picked TopoKey resolves against the exact snapshot
    /// the mesh was tessellated at (Invariant 4 — bodies/maps/meshes share one id).
    pub snapshot_id: u64,
    pub changed_bodies: Vec<BodyMeshRef>,
    pub removed_bodies: Vec<String>,
}

// ── Solver-lane DTOs (SCHEMA §7.4) — mirror `src/ipc/types.ts` sketch shapes ──
//
// These MIRROR the frontend `localSolver`/`types.ts` sketch shapes so the F-WP9
// swap (mock lane → real tauri commands) is a drop-in: `SketchSolveStatus` matches
// `types.ts SketchSolveStatus` (the four PascalCase tokens the worker's SketchUpsert
// `state` returns verbatim), `SketchSessionDto` == `SketchSession`, `SketchUpsertDto`
// == `SketchUpsertResult`, `SketchRegionDto` == `SketchRegion`.

/// Solver state (SCHEMA §7.4 `SketchUpsert.state`; `types.ts SketchSolveStatus`).
/// Serializes as the bare PascalCase token the worker emits (variant name).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SketchSolveStatus {
    UnderConstrained,
    FullyConstrained,
    OverConstrained,
    Conflicting,
}

impl SketchSolveStatus {
    /// Parses the worker's `state` string; unknown ⇒ `UnderConstrained` (the safe
    /// "not solved" default).
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "FullyConstrained" => Self::FullyConstrained,
            "OverConstrained" => Self::OverConstrained,
            "Conflicting" => Self::Conflicting,
            _ => Self::UnderConstrained,
        }
    }

    /// The tree-projection [`SketchStatus`] for this solve state.
    #[must_use]
    pub fn tree_status(self) -> SketchStatus {
        match self {
            Self::FullyConstrained => SketchStatus::Ok,
            Self::UnderConstrained => SketchStatus::Under,
            Self::OverConstrained => SketchStatus::Over,
            Self::Conflicting => SketchStatus::Error,
        }
    }
}

/// A live sketch session (`enterSketch` result; `types.ts SketchSession`). The
/// `plane`/`entities`/`constraints` carry the SCHEMA §7.3/§7.4 wire JSON verbatim
/// (identical to the frontend wire form) so the seam is a drop-in.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchSessionDto {
    pub sketch_id: String,
    pub plane: serde_json::Value,
    pub entities: serde_json::Value,
    pub constraints: serde_json::Value,
    pub dof: u32,
    pub status: SketchSolveStatus,
}

/// A re-solve result (`sketchUpsert`/`endGesture`; `types.ts SketchUpsertResult`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchUpsertDto {
    pub sketch_id: String,
    pub sketch_revision: u64,
    pub dof: u32,
    pub status: SketchSolveStatus,
    /// CHANGED point coordinates after the solve, keyed by the point entity id.
    pub solved_positions: std::collections::BTreeMap<String, [f64; 2]>,
}

/// A `BeginGesture` acknowledgement (SCHEMA §7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BeginGestureDto {
    pub gesture_id: u64,
    pub ready: bool,
}

/// One incremental drag solve (SCHEMA §7.4 `SolveDrag` result). `status` is the
/// worker token (`success`|`partial`|`conflicting`|`redundant`), plus the
/// client-side `superseded` terminal a stale `seq` may receive (latest-wins).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragSolveDto {
    pub gesture_id: u64,
    pub seq: u64,
    pub status: String,
    pub dof: u32,
    pub conflicting: Vec<String>,
    /// CHANGED point coordinates, keyed by point entity id.
    pub positions: std::collections::BTreeMap<String, [f64; 2]>,
    pub solve_micros: u64,
    /// True when this `seq` was superseded by a newer drag (positions empty).
    pub superseded: bool,
}

/// One closed profile region (`finishSketch`; `types.ts SketchRegion`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchRegionDto {
    pub region_id: String,
    pub outer_loop: Vec<String>,
    pub holes: Vec<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_triangles: Option<PreviewTrianglesDto>,
}

/// A region's triangulated fill in plane (u,v) coordinates (`types.ts
/// SketchRegion.previewTriangles`): flat `positions` (u,v pairs) + `indices`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTrianglesDto {
    pub positions: Vec<f64>,
    pub indices: Vec<u32>,
}

/// `finishSketch` result (`types.ts FinishSketchResult`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinishSketchDto {
    pub regions: Vec<SketchRegionDto>,
}

/// One promoted element (`promoteSelection`; SCHEMA §7.5 `AcquireElementIds`
/// result — Rust-minted `elementId`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotedElementDto {
    pub topo_key: String,
    pub element_id: String,
    pub kind: String,
    pub body_id: String,
}

/// One repair candidate surfaced to the M4b repair UI (`resolveRefs` →
/// `needsRepair`; SCHEMA §9 `candidates[]`). Carries the evidence handle
/// (`topoKey`), the normalized score + margin, a geometric hint (`worldPos` centre),
/// and a human-usable `summary` (e.g. `"planar face, area≈120mm²"`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveCandidateDto {
    pub topo_key: String,
    pub score: f64,
    pub margin: f64,
    /// Candidate centre in world coords — a geometric hint for highlighting.
    pub world_pos: [f64; 3],
    pub summary: String,
    /// Per-feature score contributions (SCHEMA §9 `featureContributions`), when the
    /// worker carried them (rides in the candidate's `extra`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feature_contributions: Option<serde_json::Value>,
}

/// One dry-run resolution (`resolveRefs`; SCHEMA §7.5 `ResolveRefs` result) — the
/// **full** ladder result the M4b repair UI consumes (un-lossy: the older DTO
/// dropped candidates/reason/anchor).
///
/// Per ref: `outcome` (status), `elementId` (the bound id — `autoBind`/`unchanged`,
/// or the last-known id on `needsRepair`), and — on `needsRepair` — the ranked
/// `candidates[]` plus `ladderFailed`/`reason`/`scoringVersion`/`uiLabel`/`anchor`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolveRefDto {
    pub ref_id: String,
    /// `autoBind` | `needsRepair` | `unchanged`.
    pub outcome: String,
    /// The bound `ElementId` (empty ⇒ omitted). For `autoBind`/`unchanged` this is
    /// the resolved id; for `needsRepair` it is the ref's last-known id, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub element_id: Option<String>,
    /// The bound element's `TopoKey` evidence (SCHEMA §9) — present on `autoBind`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topo_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<f64>,
    /// `history` | `descriptor` — the ladder level that could not decide (needsRepair).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ladder_failed: Option<String>,
    /// `ambiguous` | `no-candidates` | `low-confidence` (needsRepair).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The `resolverVersion` the scores were computed under (SCHEMA §9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scoring_version: Option<u32>,
    /// UI-friendly label (SCHEMA §9 `uiLabel`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_label: Option<String>,
    /// The selection intent captured when the ref was authored (SCHEMA §9 `anchor`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<serde_json::Value>,
    /// Ranked candidates (needsRepair), sorted by score descending.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<ResolveCandidateDto>,
}

impl ResolveRefDto {
    /// Maps a core [`RefResolution`](onecad_core::regen::RefResolution) to the full
    /// repair-UI DTO (SCHEMA §7.5/§9). Un-lossy: the `needsRepair` path carries the
    /// ranked candidates + reason + anchor the old DTO dropped.
    #[must_use]
    pub fn from_resolution(r: onecad_core::regen::RefResolution) -> Self {
        use onecad_core::regen::ResolveOutcome;
        let non_empty = |id: onecad_core::ids::ElementId| {
            let s = id.as_str().to_string();
            (!s.is_empty()).then_some(s)
        };
        let base = |ref_id: String, outcome: &str| ResolveRefDto {
            ref_id,
            outcome: outcome.to_string(),
            element_id: None,
            topo_key: None,
            score: None,
            margin: None,
            ladder_failed: None,
            reason: None,
            scoring_version: None,
            ui_label: None,
            anchor: None,
            candidates: Vec::new(),
        };
        match r.outcome {
            ResolveOutcome::AutoBind {
                element_id,
                score,
                margin,
                topo_key,
            } => ResolveRefDto {
                element_id: non_empty(element_id),
                topo_key: topo_key.map(|k| k.as_str().to_string()),
                score: Some(score),
                margin: Some(margin),
                ..base(r.ref_id, "autoBind")
            },
            ResolveOutcome::Unchanged { element_id } => ResolveRefDto {
                element_id: element_id.and_then(non_empty),
                ..base(r.ref_id, "unchanged")
            },
            ResolveOutcome::NeedsRepair(item) => ResolveRefDto {
                element_id: item.element_id.and_then(non_empty),
                ladder_failed: Some(ladder_level_str(item.ladder_failed).to_string()),
                reason: Some(repair_reason_str(item.reason).to_string()),
                scoring_version: item.scoring_version,
                ui_label: (!item.ui_label.is_empty()).then_some(item.ui_label),
                anchor: item
                    .anchor
                    .as_ref()
                    .and_then(|a| serde_json::to_value(a).ok()),
                candidates: item.candidates.into_iter().map(candidate_dto).collect(),
                ..base(r.ref_id, "needsRepair")
            },
        }
    }
}

fn ladder_level_str(l: onecad_core::document::repair::LadderLevel) -> &'static str {
    use onecad_core::document::repair::LadderLevel;
    match l {
        LadderLevel::History => "history",
        LadderLevel::Descriptor => "descriptor",
    }
}

fn repair_reason_str(r: onecad_core::document::repair::RepairReason) -> &'static str {
    use onecad_core::document::repair::RepairReason;
    match r {
        RepairReason::Ambiguous => "ambiguous",
        RepairReason::NoCandidates => "no-candidates",
        RepairReason::LowConfidence => "low-confidence",
    }
}

fn candidate_dto(c: onecad_core::document::repair::RepairCandidate) -> ResolveCandidateDto {
    ResolveCandidateDto {
        topo_key: c.topo_key.as_str().to_string(),
        score: c.score,
        margin: c.margin,
        world_pos: [c.world_pos.x, c.world_pos.y, c.world_pos.z],
        summary: c.summary,
        feature_contributions: c.extra.get("featureContributions").cloned(),
    }
}

/// The `regen-finished` event payload (`{revision, outcome}`) so the frontend
/// correlation resolves promptly without the 8 s fallback (F-WP8 flag 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegenFinished {
    pub revision: u64,
    /// `published` | `superseded` | `failed` | `cancelled` | `noop`.
    pub outcome: String,
}

/// One entry in the `needs-repair` event — a **lean** summary of a step left in
/// NeedsRepair (SCHEMA §9). The repair panel fetches the full candidate evidence via
/// `resolveRefs` on demand, so this carries only what the banner/badge needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedsRepairItemDto {
    /// The op record id (`RecordId`) of the step needing repair.
    pub op_id: String,
    /// The op-input ref identity (SCHEMA §9 `refId`, e.g. `"op_5.input0"`).
    pub ref_id: String,
    /// `ambiguous` | `no-candidates` | `low-confidence` (SCHEMA §9 `reason`).
    pub reason: String,
    /// The `resolverVersion` the candidate scores were computed under (SCHEMA §9).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scoring_version: Option<u32>,
    /// How many candidates the ladder surfaced (0 ⇒ `no-candidates`).
    pub candidate_count: usize,
}

/// The `needs-repair` event payload (`{revision, items}`). Emitted after **every**
/// published regen; an EMPTY `items` means repairs cleared, so the frontend can drop
/// the banner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NeedsRepairEvent {
    pub revision: u64,
    pub items: Vec<NeedsRepairItemDto>,
}

/// Builds the lean [`NeedsRepairItemDto`] for one repair item + its op record id.
#[must_use]
pub fn needs_repair_item_dto(
    op_id: String,
    item: &onecad_core::document::repair::RepairItem,
) -> NeedsRepairItemDto {
    NeedsRepairItemDto {
        op_id,
        ref_id: item.ref_id.clone(),
        reason: repair_reason_str(item.reason).to_string(),
        scoring_version: item.scoring_version,
        candidate_count: item.candidates.len(),
    }
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

    // ── M4a deliverable 3: the un-lossy resolve_refs DTO mapping ──────────────

    #[test]
    fn resolve_ref_dto_autobind_carries_id_and_topokey_evidence() {
        use onecad_core::ids::{ElementId, TopoKey};
        use onecad_core::regen::{RefResolution, ResolveOutcome};
        let dto = ResolveRefDto::from_resolution(RefResolution {
            ref_id: "op_5.input0".into(),
            outcome: ResolveOutcome::AutoBind {
                element_id: ElementId::new("el_top"),
                score: 0.94,
                margin: 0.31,
                topo_key: Some(TopoKey::new("f:1")),
            },
        });
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["outcome"], "autoBind");
        assert_eq!(v["elementId"], "el_top");
        assert_eq!(v["topoKey"], "f:1");
        assert_eq!(v["score"], 0.94);
        assert_eq!(v["margin"], 0.31);
        assert!(v.get("candidates").is_none(), "no candidates on autoBind");

        // Unminted dry-run autoBind: empty id ⇒ elementId omitted, topoKey kept.
        let dto = ResolveRefDto::from_resolution(RefResolution {
            ref_id: "op_5.input1".into(),
            outcome: ResolveOutcome::AutoBind {
                element_id: ElementId::new(""),
                score: 0.9,
                margin: 0.2,
                topo_key: Some(TopoKey::new("f:3")),
            },
        });
        let v = serde_json::to_value(&dto).unwrap();
        assert!(v.get("elementId").is_none(), "empty id ⇒ omitted");
        assert_eq!(v["topoKey"], "f:3");
    }

    #[test]
    fn resolve_ref_dto_needs_repair_carries_full_candidate_evidence() {
        use onecad_core::document::repair::{
            LadderLevel, RepairCandidate, RepairItem, RepairReason,
        };
        use onecad_core::ids::TopoKey;
        use onecad_core::math::Vec3;
        use onecad_core::regen::{RefResolution, ResolveOutcome};

        let mut extra = onecad_core::document::refs::Extra::new();
        extra.insert(
            "featureContributions".into(),
            serde_json::json!({ "area": 0.25 }),
        );
        let item = RepairItem {
            step_index: 6,
            ref_id: "op_6.input0".into(),
            element_id: Some(onecad_core::ids::ElementId::new("el_last")),
            ladder_failed: LadderLevel::Descriptor,
            reason: RepairReason::Ambiguous,
            candidates: vec![
                RepairCandidate {
                    topo_key: TopoKey::new("f:31"),
                    score: 0.91,
                    margin: 0.0,
                    world_pos: Vec3::new_unchecked(12.0, 3.5, 0.0),
                    summary: "planar face".into(),
                    extra,
                },
                RepairCandidate {
                    topo_key: TopoKey::new("f:44"),
                    score: 0.91,
                    margin: 0.0,
                    world_pos: Vec3::new_unchecked(12.0, -3.5, 0.0),
                    summary: "planar face".into(),
                    extra: Default::default(),
                },
            ],
            scoring_version: Some(1),
            anchor: None,
            ui_label: "Fillet edge".into(),
        };
        let dto = ResolveRefDto::from_resolution(RefResolution {
            ref_id: "op_6.input0".into(),
            outcome: ResolveOutcome::NeedsRepair(item),
        });
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["outcome"], "needsRepair");
        assert_eq!(v["elementId"], "el_last");
        assert_eq!(v["ladderFailed"], "descriptor");
        assert_eq!(v["reason"], "ambiguous");
        assert_eq!(v["scoringVersion"], 1);
        assert_eq!(v["uiLabel"], "Fillet edge");
        // The candidate evidence the OLD DTO dropped is now carried in full.
        assert_eq!(v["candidates"].as_array().unwrap().len(), 2);
        assert_eq!(v["candidates"][0]["topoKey"], "f:31");
        assert_eq!(v["candidates"][0]["score"], 0.91);
        assert_eq!(
            v["candidates"][0]["worldPos"],
            serde_json::json!([12.0, 3.5, 0.0])
        );
        assert_eq!(
            v["candidates"][0]["featureContributions"],
            serde_json::json!({ "area": 0.25 })
        );
    }

    #[test]
    fn needs_repair_item_dto_shape() {
        use onecad_core::document::repair::{LadderLevel, RepairItem, RepairReason};
        let item = RepairItem {
            step_index: 2,
            ref_id: "op_2.input0".into(),
            element_id: None,
            ladder_failed: LadderLevel::Descriptor,
            reason: RepairReason::NoCandidates,
            candidates: Vec::new(),
            scoring_version: Some(1),
            anchor: None,
            ui_label: String::new(),
        };
        let dto = needs_repair_item_dto("rec-1".into(), &item);
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["opId"], "rec-1");
        assert_eq!(v["refId"], "op_2.input0");
        assert_eq!(v["reason"], "no-candidates");
        assert_eq!(v["scoringVersion"], 1);
        assert_eq!(v["candidateCount"], 0);
    }
}
