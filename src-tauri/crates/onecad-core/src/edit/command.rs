//! The [`EditCommand`] enum: the single mutation vocabulary for the document.
//!
//! One enum doubles as the `apply_command` payload (serde, internally tagged on
//! `"cmd"`, camelCase). Each variant ports the semantics of a OneCAD-CPP
//! `src/app/commands/*Command` (cited per variant). Undo is **memento-based**
//! (see [`crate::edit::undo`]), never command-replay, so a variant carries only
//! the *forward* intent; the session captures the inverse when it applies.
//!
//! ## `EditOperationInput.reference` — reported divergence
//!
//! The WP spec typed this field `ElementRef`, but the [`InputPath`] set it must
//! service includes a sketch-region profile, a revolve axis and boolean body
//! slots — none of which an `ElementRef` can express. The field is therefore an
//! [`InputRef`] union (element / region / axis / body). C++
//! `EditOperationInputCommand` carries a `std::variant` `OperationInput`
//! (`SketchRegionRef` / `FaceRef` / `BodyRef`); the region, element and body arms
//! port it faithfully. The **axis** arm has NO C++ `OperationInput` analogue — it
//! is a Rust extension. C++ re-binds a revolve axis by rewriting the axis in the
//! params via `UpdateOperationParamsCommand`, not through
//! `EditOperationInputCommand`. See the report.
//!
//! ## Fillet/Chamfer edge consistency
//!
//! An [`InputPath::FilletEdges`] edit populates **both** the bare `edge_ids`
//! entry and the typed `edges` entry of the fillet/chamfer params (they must
//! stay in lockstep — see [`crate::document::record::FilletParams`]); the
//! session enforces this.

use serde::{Deserialize, Serialize};

use crate::document::body::BodyMeta;
use crate::document::datum::DatumPlane;
use crate::document::record::{Operation, OperationRecord};
use crate::document::refs::{AxisRef, ElementRef, SketchRegionRef};
use crate::document::variables::{Scalar, Variable};
use crate::ids::{BodyId, ConstraintId, EntityId, RecordId, SketchId, VariableId};
use crate::math::Vec2;
use crate::sketch::{Constraint, Sketch, SketchAttachment, SketchEntity, SketchPlane};

/// The full document mutation vocabulary (serde tag `"cmd"`, camelCase).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum EditCommand {
    /// Append an op at the rollback cursor and grow the applied prefix
    /// (C++ `AddOperationCommand`). `at_cursor=false` appends at the end.
    AddOperation {
        /// The full record to insert.
        record: OperationRecord,
        /// Insert at the rollback cursor (`true`) or the timeline end (`false`).
        at_cursor: bool,
    },
    /// Replace an op's parameters (C++ `UpdateOperationParamsCommand`). `op` is
    /// the new operation payload; its `opType` must equal the target's.
    UpdateOperationParams {
        /// Target record.
        record: RecordId,
        /// Replacement operation (same `opType`).
        op: Operation,
    },
    /// Re-bind a single input reference inside an op's params — the topological
    /// repair path (C++ `EditOperationInputCommand`).
    EditOperationInput {
        /// Target record.
        record: RecordId,
        /// Which input slot to rebind.
        path: InputPath,
        /// The new reference (see [`InputRef`]).
        reference: InputRef,
    },
    /// Remove an op from the timeline (C++ `RemoveOperationCommand`).
    RemoveOperation {
        /// Target record.
        record: RecordId,
    },
    /// Move the rollback cursor (C++ `RollbackCommand`, modeled as a cursor move
    /// — NOT the C++ suppression conflation; plan "Rust core specifics").
    SetRollback {
        /// New applied-op count / cursor position.
        cursor: usize,
    },
    /// Suppress / un-suppress an op, optionally cascading downstream
    /// (C++ `SetOperationSuppressionCommand` + `RollbackCommand::suppressDownstream`).
    SetOperationSuppression {
        /// Target record.
        record: RecordId,
        /// Whether the op is suppressed.
        suppressed: bool,
        /// Cascade the flag to downstream ops.
        cascade: bool,
    },
    /// Add a sketch (C++ `AddSketchCommand`).
    AddSketch {
        /// The sketch to add.
        sketch: Sketch,
    },
    /// Delete a sketch (C++ `DeleteSketchCommand`).
    DeleteSketch {
        /// Target sketch.
        sketch: SketchId,
    },
    /// Rename a sketch (C++ `RenameSketchCommand`).
    RenameSketch {
        /// Target sketch.
        sketch: SketchId,
        /// New name.
        name: String,
    },
    /// Re-derive a host-attached sketch's plane/attachment from its host face's
    /// current geometry (C++ `UpdateSketchAttachmentCommand`). The core carries
    /// the already-resolved `plane`/`attachment` (the worker computed them).
    UpdateSketchAttachment {
        /// Target sketch.
        sketch: SketchId,
        /// Resolved plane frame.
        plane: SketchPlane,
        /// Updated attachment.
        attachment: SketchAttachment,
    },
    /// Apply a batch of in-place sketch edits (C++ sketch tool mutations feeding
    /// `SketchDragGestureCommand`; here made explicit as typed ops).
    SketchEdit {
        /// Target sketch.
        sketch: SketchId,
        /// Ordered sketch mutations.
        ops: Vec<SketchEditOp>,
    },
    /// Commit a drag gesture as a before/after sketch memento
    /// (C++ `SketchDragGestureCommand`, which snapshots whole sketches).
    SketchDragGesture {
        /// Target sketch.
        sketch: SketchId,
        /// Sketch before the gesture.
        before: Sketch,
        /// Sketch after the gesture.
        after: Sketch,
    },
    /// Register a body (C++ `AddBodyCommand`). Geometry lives in the worker; the
    /// core tracks [`BodyMeta`] only.
    AddBody {
        /// The body metadata to register.
        body: BodyMeta,
    },
    /// Delete a body (C++ `DeleteBodyCommand`).
    DeleteBody {
        /// Target body.
        body: BodyId,
    },
    /// Rename a body (C++ `RenameBodyCommand`).
    RenameBody {
        /// Target body.
        body: BodyId,
        /// New name.
        name: String,
    },
    /// Set body or sketch visibility (C++ `ToggleVisibilityCommand`, whose
    /// `ItemType` is `Body` | `Sketch`).
    SetVisibility {
        /// What to show/hide.
        target: VisibilityTarget,
        /// Visible (`true`) or hidden (`false`).
        visible: bool,
    },
    /// Add a datum plane (C++ `AddDatumPlaneCommand`).
    AddDatumPlane {
        /// The datum to add.
        datum: DatumPlane,
    },
    /// Set an existing variable's value.
    SetVariable {
        /// Target variable.
        variable: VariableId,
        /// New value.
        value: Scalar,
    },
    /// Add a new variable.
    AddVariable {
        /// The variable to add.
        variable: Variable,
    },
    /// Remove a variable.
    RemoveVariable {
        /// Target variable.
        variable: VariableId,
    },
    // ImportStep — DEFERRED (C++ `ImportStepCommand`). It adds imported bodies +
    // per-face colors from worker BREP payloads, which are out of this WP's
    // pure-core scope. Slot reserved for a later WP.
}

impl EditCommand {
    /// A short, stable, human-facing label (mirrors the C++ `label()`), used as
    /// the default transaction label.
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::AddOperation { .. } => "Add Operation",
            Self::UpdateOperationParams { .. } => "Update Operation",
            Self::EditOperationInput { .. } => "Re-profile Operation",
            Self::RemoveOperation { .. } => "Remove Operation",
            Self::SetRollback { .. } => "Rollback",
            Self::SetOperationSuppression { .. } => "Toggle Suppression",
            Self::AddSketch { .. } => "Add Sketch",
            Self::DeleteSketch { .. } => "Delete Sketch",
            Self::RenameSketch { .. } => "Rename Sketch",
            Self::UpdateSketchAttachment { .. } => "Update Sketch Attachment",
            Self::SketchEdit { .. } => "Edit Sketch",
            Self::SketchDragGesture { .. } => "Sketch Drag Gesture",
            Self::AddBody { .. } => "Add Body",
            Self::DeleteBody { .. } => "Delete Body",
            Self::RenameBody { .. } => "Rename Body",
            Self::SetVisibility { .. } => "Toggle Visibility",
            Self::AddDatumPlane { .. } => "Create Datum Plane",
            Self::SetVariable { .. } => "Set Variable",
            Self::AddVariable { .. } => "Add Variable",
            Self::RemoveVariable { .. } => "Remove Variable",
        }
    }
}

/// A single in-place sketch mutation inside a [`EditCommand::SketchEdit`] batch.
///
/// Serde: internally tagged on `"op"`, camelCase.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SketchEditOp {
    /// Add an entity (validated on apply — dup id / dangling ref rejected).
    AddEntity {
        /// The entity to add.
        entity: SketchEntity,
    },
    /// Remove an entity and cascade-drop anything that would dangle.
    RemoveEntity {
        /// Target entity.
        entity: EntityId,
    },
    /// Add a constraint (validated on apply).
    AddConstraint {
        /// The constraint to add.
        constraint: Constraint,
    },
    /// Remove a constraint.
    RemoveConstraint {
        /// Target constraint.
        constraint: ConstraintId,
    },
    /// Set a dimensional constraint's value (order-preserving).
    SetDimension {
        /// Target constraint (must be dimensional).
        constraint: ConstraintId,
        /// New dimension value.
        value: Scalar,
    },
    /// Move point entities to new positions (drag; order-preserving).
    SetEntityPositions {
        /// `(point entity, new position)` pairs.
        positions: Vec<(EntityId, Vec2)>,
    },
}

/// A typed path to one input reference inside an op's params (extensible).
///
/// Serde: internally tagged on `"path"`, camelCase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "path",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[non_exhaustive]
pub enum InputPath {
    /// The extrude/revolve profile region (`params.profile`).
    ExtrudeProfile,
    /// A ToFace extrude target face; `second` selects `targetFace2`.
    ExtrudeTargetFace {
        /// `false` = `targetFace`, `true` = `targetFace2`.
        second: bool,
    },
    /// A fillet/chamfer edge at `index` (into `edge_ids`/`edges`).
    FilletEdges {
        /// Index of the edge to rebind.
        index: usize,
    },
    /// A boolean op's target body (`params.targetBodyId`).
    BooleanTarget,
    /// A boolean op's tool body (`params.toolBodyId`).
    BooleanTool,
    /// A revolve axis (`params.axis`).
    RevolveAxis,
}

/// The payload written by [`EditCommand::EditOperationInput`] — a union over the
/// shapes an op input slot can take (see the module-level divergence note).
///
/// Serde: externally tagged, camelCase (`{"element": {…}}` / `{"region": {…}}` /
/// `{"axis": {…}}` / `{"body": "<uuid>"}`).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InputRef {
    /// A topological element reference (face/edge — fillet/ToFace).
    Element(ElementRef),
    /// A sketch-region profile reference (extrude/revolve profile).
    Region(SketchRegionRef),
    /// A revolve/pattern axis reference.
    Axis(AxisRef),
    /// A whole-body reference (boolean target/tool).
    Body(BodyId),
}

/// What a [`EditCommand::SetVisibility`] targets (C++ `ToggleVisibilityCommand::ItemType`).
///
/// Serde: externally tagged, camelCase (`{"body": "<uuid>"}` / `{"sketch": "<uuid>"}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VisibilityTarget {
    /// A solid body.
    Body(BodyId),
    /// A sketch.
    Sketch(SketchId),
}
