//! The authoritative document (Rust-owned) and its sub-models.
//!
//! Aggregates the operation [`Timeline`], [`BodyRegistry`], datum planes,
//! variables, sketches and [`RepairState`] (V1/V2 §2.1 `Document` root +
//! `GlobalRegistry`). This is the single source of truth; the
//! [`DocumentSession`](crate::edit::DocumentSession) is its only writer.
//!
//! ## Serde (`document.json`)
//!
//! `Document` (de)serializes through [`DocumentData`] (the `#[serde(from/into)]`
//! mirror, same pattern as [`Sketch`]). Two deliberate choices:
//!
//! * **Deterministic collection order** — sketches and datum planes are
//!   `BTreeMap`s keyed by their (UUID) ids, so the JSON object key order is
//!   stable across runs (V1/V2 §0.1 invariant 5).
//! * **Regen-derived timeline state is NOT persisted.** The timeline serializes
//!   as `{records, cursor}` only; per-step [`StepState`](crate::history::StepState)s
//!   are *derived* regen status (a freshly loaded document is all-`Dirty` until
//!   regenerated — [`Timeline::from_records`]), never authoritative document
//!   content. Excluding them keeps `document.json` a function of the authoritative
//!   inputs and lets the edit layer compute exact memento inverses over
//!   `{records, cursor}` alone (see [`crate::edit::undo`]).

pub mod body;
pub mod datum;
pub mod element_index;
pub mod record;
pub mod refs;
pub mod repair;
pub mod variables;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::document::body::BodyRegistry;
use crate::document::datum::DatumPlane;
use crate::document::element_index::ElementIndex;
use crate::document::record::OperationRecord;
use crate::document::refs::Extra;
use crate::document::repair::RepairState;
use crate::document::variables::{Unit, VariableTable};
use crate::history::{StepState, Timeline};
use crate::ids::{DatumPlaneId, DocumentId, ElementId, SketchId};
use crate::sketch::Sketch;

/// The document-schema version stamped on freshly authored documents (V1/V2 §2.1
/// `GlobalSchemaVersion`).
pub const DOCUMENT_SCHEMA_VERSION: u32 = 1;

/// Document-wide settings (V1/V2 §2.1: units, tolerance policy, OCCT fingerprint).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSettings {
    /// Document units (V1: millimetres only).
    #[serde(default)]
    pub units: Unit,
    /// Hash of the modeling-tolerance policy (determinism gate).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tolerance_policy_hash: String,
    /// The OCCT fingerprint the document was last regenerated under (V1/V2 §8);
    /// `None` until a worker has regenerated it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occt_fingerprint: Option<String>,
}

impl Default for DocumentSettings {
    fn default() -> Self {
        Self {
            units: Unit::Mm,
            tolerance_policy_hash: String::new(),
            occt_fingerprint: None,
        }
    }
}

/// A named, reusable selection set (V1/V2 §2.1 `NamedSelections`). Minimal for
/// V1: a name plus the referenced element ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedSelection {
    /// User-facing name.
    pub name: String,
    /// Referenced element ids.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<ElementId>,
    /// Unknown keys, preserved verbatim.
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Root of an open document (V1/V2 §2.1). The single authoritative model owned
/// by Rust; mutated only through [`DocumentSession`](crate::edit::DocumentSession).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(from = "DocumentData", into = "DocumentData")]
pub struct Document {
    /// Stable document identity.
    pub id: DocumentId,
    /// Document-schema version (currently [`DOCUMENT_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Document-wide settings.
    pub settings: DocumentSettings,
    /// Named parameters / variables (ordered).
    pub variables: VariableTable,
    /// Sketches, keyed by id in deterministic (UUID) order.
    pub sketches: BTreeMap<SketchId, Sketch>,
    /// Per-sketch visibility overrides. Absent ⇒ visible (the default). Sketch
    /// visibility is document-owned because [`Sketch`] itself carries no
    /// `visible` field (mirrors OneCAD-CPP `Document::isSketchVisible`).
    pub sketch_visibility: BTreeMap<SketchId, bool>,
    /// Datum planes, keyed by id in deterministic order.
    pub datum_planes: BTreeMap<DatumPlaneId, DatumPlane>,
    /// The strict-linear operation timeline (+ rollback cursor).
    pub timeline: Timeline,
    /// Body registry (`BodyId` lifecycle).
    pub bodies: BodyRegistry,
    /// Minted-element partition index (`ElementId -> {body, kind}`; V1/V2 §3.2).
    /// Added in R-WP7 for regen element-map folding + `AcquireElementIds`
    /// minting; serialized only when non-empty (existing fixtures unaffected).
    pub elements: ElementIndex,
    /// Topological-naming repair state.
    pub repair: RepairState,
    /// Named selection sets (minimal).
    pub named_selections: Vec<NamedSelection>,
    /// Document-level unknown keys, preserved verbatim.
    pub extra: Extra,
}

impl Document {
    /// A fresh, empty document with the current schema version.
    #[must_use]
    pub fn new(id: DocumentId) -> Self {
        Self {
            id,
            schema_version: DOCUMENT_SCHEMA_VERSION,
            settings: DocumentSettings::default(),
            variables: VariableTable::new(),
            sketches: BTreeMap::new(),
            sketch_visibility: BTreeMap::new(),
            datum_planes: BTreeMap::new(),
            timeline: Timeline::new(),
            bodies: BodyRegistry::new(),
            elements: ElementIndex::new(),
            repair: RepairState::new(),
            named_selections: Vec::new(),
            extra: Extra::new(),
        }
    }

    /// A sketch by id.
    #[must_use]
    pub fn sketch(&self, id: SketchId) -> Option<&Sketch> {
        self.sketches.get(&id)
    }

    /// A mutable sketch by id.
    pub fn sketch_mut(&mut self, id: SketchId) -> Option<&mut Sketch> {
        self.sketches.get_mut(&id)
    }

    /// A datum plane by id.
    #[must_use]
    pub fn datum(&self, id: DatumPlaneId) -> Option<&DatumPlane> {
        self.datum_planes.get(&id)
    }

    /// Whether a sketch is visible (absent override ⇒ `true`).
    #[must_use]
    pub fn sketch_visible(&self, id: SketchId) -> bool {
        self.sketch_visibility.get(&id).copied().unwrap_or(true)
    }

    /// Sets a sketch's visibility override.
    pub fn set_sketch_visible(&mut self, id: SketchId, visible: bool) {
        self.sketch_visibility.insert(id, visible);
    }
}

// ── Serde mirror ────────────────────────────────────────────────────────────

/// Serde form of a [`Timeline`]: its authoritative `{records, cursor}` only
/// (regen-derived per-step states are not persisted — see the module docs).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TimelineWire {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    records: Vec<OperationRecord>,
    #[serde(default)]
    cursor: usize,
}

impl From<&Timeline> for TimelineWire {
    fn from(t: &Timeline) -> Self {
        Self {
            records: t.records().to_vec(),
            cursor: t.cursor(),
        }
    }
}

impl From<TimelineWire> for Timeline {
    fn from(w: TimelineWire) -> Self {
        // `from_records` places the cursor at the end and marks every step Dirty
        // (a loaded document must regenerate before it is trusted); restore the
        // stored cursor. A backward cursor move leaves the states Dirty, which is
        // the correct just-loaded state.
        let mut t = Timeline::from_records(w.records);
        t.set_cursor(w.cursor);
        t
    }
}

/// Serde mirror of [`Document`] (the persisted fields).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DocumentData {
    id: DocumentId,
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    settings: DocumentSettings,
    #[serde(default)]
    variables: VariableTable,
    #[serde(default)]
    sketches: BTreeMap<SketchId, Sketch>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sketch_visibility: BTreeMap<SketchId, bool>,
    #[serde(default)]
    datum_planes: BTreeMap<DatumPlaneId, DatumPlane>,
    #[serde(default)]
    timeline: TimelineWire,
    #[serde(default)]
    bodies: BodyRegistry,
    #[serde(default, skip_serializing_if = "ElementIndex::is_empty")]
    elements: ElementIndex,
    #[serde(default)]
    repair: RepairState,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    named_selections: Vec<NamedSelection>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    extra: Extra,
}

impl From<Document> for DocumentData {
    fn from(d: Document) -> Self {
        Self {
            id: d.id,
            schema_version: d.schema_version,
            settings: d.settings,
            variables: d.variables,
            sketches: d.sketches,
            sketch_visibility: d.sketch_visibility,
            datum_planes: d.datum_planes,
            timeline: TimelineWire::from(&d.timeline),
            bodies: d.bodies,
            elements: d.elements,
            repair: d.repair,
            named_selections: d.named_selections,
            extra: d.extra,
        }
    }
}

impl From<DocumentData> for Document {
    fn from(w: DocumentData) -> Self {
        let schema_version = if w.schema_version == 0 {
            DOCUMENT_SCHEMA_VERSION
        } else {
            w.schema_version
        };
        // `from_records` marks every step Dirty; a loaded `RepairState` means the
        // last regen surfaced unresolved refs at those steps, so reflect that as
        // `NeedsRepair` (F3). `mark_state` is bounds-checked — a step_index past
        // the record count (tampered/stale) is silently ignored.
        let mut timeline = Timeline::from(w.timeline);
        for item in w.repair.items() {
            let _ = timeline.mark_state(item.step_index, StepState::NeedsRepair);
        }
        Self {
            id: w.id,
            schema_version,
            settings: w.settings,
            variables: w.variables,
            sketches: w.sketches,
            sketch_visibility: w.sketch_visibility,
            datum_planes: w.datum_planes,
            timeline,
            bodies: w.bodies,
            elements: w.elements,
            repair: w.repair,
            named_selections: w.named_selections,
            extra: w.extra,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::record::{
        BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation,
    };
    use crate::document::variables::Scalar;
    use crate::ids::RecordId;
    use crate::sketch::WorldPlane;
    use uuid::Uuid;

    fn extrude(seed: u128) -> OperationRecord {
        let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
            profile: None,
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
        OperationRecord::new(RecordId(Uuid::from_u128(seed)), 0, "Extrude", op)
    }

    #[test]
    fn document_round_trips_and_preserves_cursor_not_states() {
        let mut doc = Document::new(DocumentId(Uuid::from_u128(1)));
        doc.sketches.insert(
            SketchId(Uuid::from_u128(0x5C01)),
            Sketch::on_world_plane(
                SketchId(Uuid::from_u128(0x5C01)),
                "Sketch 1",
                WorldPlane::XY,
            ),
        );
        doc.timeline.insert_at_cursor(extrude(0x10));
        doc.timeline.insert_at_cursor(extrude(0x11));
        doc.timeline.set_cursor(1); // rollback

        let json = serde_json::to_value(&doc).unwrap();
        // Timeline serializes as {records, cursor}, no `states`.
        assert!(json["timeline"].get("states").is_none());
        assert_eq!(json["timeline"]["cursor"], 1);
        assert_eq!(json["schemaVersion"], 1);

        let back: Document = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(serde_json::to_value(&back).unwrap(), json);
        assert_eq!(back.timeline.cursor(), 1);
        assert_eq!(back.timeline.len(), 2);
    }

    #[test]
    fn loaded_repair_state_seeds_needs_repair_step() {
        use crate::document::repair::{LadderLevel, RepairItem, RepairReason};
        use crate::history::StepState;

        let mut doc = Document::new(DocumentId(Uuid::from_u128(7)));
        doc.timeline.insert_at_cursor(extrude(0x10));
        doc.timeline.insert_at_cursor(extrude(0x11));
        doc.timeline.insert_at_cursor(extrude(0x12));
        // A NeedsRepair unresolved ref surfaced at step 1.
        doc.repair.set_step(
            1,
            vec![RepairItem {
                step_index: 1,
                ref_id: "op_1.input0".into(),
                element_id: None,
                ladder_failed: LadderLevel::Descriptor,
                reason: RepairReason::Ambiguous,
                candidates: vec![],
                anchor: None,
                ui_label: String::new(),
                scoring_version: None,
            }],
        );

        // Save → load: the repaired step re-seeds NeedsRepair; the rest stay Dirty.
        let json = serde_json::to_value(&doc).unwrap();
        let back: Document = serde_json::from_value(json).unwrap();
        assert_eq!(back.timeline.state(1), Some(&StepState::NeedsRepair));
        assert_eq!(back.timeline.state(0), Some(&StepState::Dirty));
        assert_eq!(back.timeline.state(2), Some(&StepState::Dirty));
    }

    #[test]
    fn sketch_visibility_defaults_true() {
        let mut doc = Document::new(DocumentId(Uuid::from_u128(1)));
        let sid = SketchId(Uuid::from_u128(2));
        assert!(doc.sketch_visible(sid));
        doc.set_sketch_visible(sid, false);
        assert!(!doc.sketch_visible(sid));
    }
}
