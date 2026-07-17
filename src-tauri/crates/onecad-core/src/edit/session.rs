//! [`DocumentSession`]: the single writer to the authoritative document.
//!
//! All mutations flow through one session (plan "DocumentSession single writer")
//! so ordering is deterministic and undo/redo stays consistent. Each
//! [`EditCommand`] is validated, applied, and paired with a memento inverse
//! ([`crate::edit::undo`]); a [`DependencyGraph`] is rebuilt on every structural
//! change and used for anti-time-travel (`produces_before`) validation.
//!
//! ## Dirty / regen table (per command)
//!
//! | command | dirty span | [`RegenHint`] |
//! |---|---|---|
//! | `AddOperation` (at cursor) | `[insertIndex, len)` | `ToEnd` |
//! | `AddOperation` (append draft) | `[index, len)` | `ToEnd` if applied, else `None` |
//! | `UpdateOperationParams` | `[step, len)` | `ToEnd` |
//! | `EditOperationInput` | `[step, len)` | `ToEnd` |
//! | `RemoveOperation` | `[removedIndex, len)` | `ToEnd` |
//! | `SetRollback` | span from `set_cursor` | `PreviewTo(cursor)` |
//! | `SetOperationSuppression` | `[step, len)` | `ToEnd` |
//! | `AddSketch` | first dependent op step (`None` if none) | `ToEnd` / `None` |
//! | `DeleteSketch` / `UpdateSketchAttachment` / `SketchEdit` / `SketchDragGesture` | `min(producing Sketch op, first dependent op)` | `ToEnd` / `None` |
//! | `RenameSketch` | — | `None` |
//! | `AddBody` / `DeleteBody` / `RenameBody` / `SetVisibility` | — | `None` |
//! | `AddDatumPlane` | — | `None` |
//! | `SetVariable` / `AddVariable` / `RemoveVariable` | `[0, len)` (conservative) | `ToEnd` / `None` |
//!
//! `UpdateOperationParams` keeps it simple (`ToEnd`) rather than a `PreviewTo`
//! interactive path (deferred; plan "keep simple"). Variable edits are
//! conservative: a bare-variable-name expression can drive any dimensioned op,
//! so any variable change dirties the whole applied timeline (a precise
//! variable→op dependency awaits the expression engine).
//!
//! A sketch edit dirties from `min(producing Sketch op, first dependent op)`: a
//! `Sketch` op is itself a timeline node whose regen re-runs region detection
//! (worker-side), so an edit must dirty from that producer, not only from the
//! first op consuming a region. A document sketch with no producing op falls back
//! to its first consumer.
//!
//! ## Reference existence is a regen-time concern (F7; C++ parity)
//!
//! The session validates *structural* invariants — duplicate ids, `opType`
//! immutability, fillet/chamfer lockstep, and anti-time-travel
//! (`produces_before`) — but does NOT check that a referenced sketch or body
//! actually *exists* when a command binds it (e.g. an `EditOperationInput`
//! profile pointing at an absent sketch, or an `AddOperation` whose params name a
//! body no op produces). Reference resolution/existence is deferred to regen by
//! design, matching C++: the command layer only rewrites the record, and the
//! worker's `RegenerationEngine` surfaces an unresolved reference as a
//! failure/`NeedsRepair` rather than the command rejecting it up front.

use crate::document::body::BodyMeta;
use crate::document::datum::DatumPlane;
use crate::document::record::{KnownOperation, Operation, OperationRecord};
use crate::document::refs::ElementRef;
use crate::document::variables::{Scalar, Variable, VariableTable};
use crate::document::Document;
use crate::error::DomainError;
use crate::history::{DependencyGraph, DirtyRange, Timeline};
use crate::ids::{BodyId, RecordId, SketchId};
use crate::math::Vec2;
use crate::sketch::{Constraint, Sketch, SketchEntity, SketchError};

use super::command::{EditCommand, InputPath, InputRef, SketchEditOp, VisibilityTarget};
use super::outcome::{CommandOutcome, ProjectionDelta, RegenHint};
use super::undo::{insert_record_at, replace_record_at, AppliedEdit, Inverse, Txn, UndoStack};

/// Owns the document and applies [`EditCommand`]s as the sole writer.
#[derive(Debug)]
pub struct DocumentSession {
    document: Document,
    undo: UndoStack,
    graph: DependencyGraph,
    open_txn: Option<Box<TxnState>>,
}

/// A transaction being accumulated between `begin_transaction`/`end_transaction`.
#[derive(Debug)]
struct TxnState {
    label: String,
    edits: Vec<AppliedEdit>,
    combined: CommandOutcome,
}

impl DocumentSession {
    /// A session over `document`, with a freshly built dependency graph.
    #[must_use]
    pub fn new(document: Document) -> Self {
        let mut graph = DependencyGraph::new();
        graph.rebuild_from_records(document.timeline.records());
        Self {
            document,
            undo: UndoStack::new(),
            graph,
            open_txn: None,
        }
    }

    /// The authoritative document (read-only; mutate only via [`apply`]).
    ///
    /// [`apply`]: DocumentSession::apply
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The derived dependency graph.
    #[must_use]
    pub fn graph(&self) -> &DependencyGraph {
        &self.graph
    }

    /// Consumes the session, returning the document.
    #[must_use]
    pub fn into_document(self) -> Document {
        self.document
    }

    /// Whether an undo step is available.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        self.undo.can_undo()
    }

    /// Whether a redo step is available.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        self.undo.can_redo()
    }

    /// Current undo depth.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.undo_depth()
    }

    /// Applies a command: validates, mutates the document, captures a memento
    /// inverse, and records it on the open transaction (or as a singleton undo
    /// step). Returns the per-command [`CommandOutcome`].
    ///
    /// # Errors
    /// A [`DomainError`] if the command fails validation; the document is left
    /// unchanged (apply is atomic). When a command fails **inside an open
    /// transaction**, the whole transaction is auto-cancelled first (its
    /// already-applied edits are rolled back in reverse and it is closed — C++
    /// `CommandProcessor::execute` `CommandProcessor.cpp:62-67`) before the
    /// error propagates.
    pub fn apply(&mut self, cmd: EditCommand) -> Result<CommandOutcome, DomainError> {
        let (outcome, inverse) = match self.apply_forward(&cmd) {
            Ok(v) => v,
            Err(e) => {
                // A failure mid-transaction cancels the whole batch (C++ parity).
                if self.open_txn.is_some() {
                    self.cancel_transaction();
                }
                return Err(e);
            }
        };
        let edit = AppliedEdit {
            forward: cmd,
            inverse,
        };
        match &mut self.open_txn {
            Some(txn) => {
                txn.edits.push(edit);
                merge_outcome(&mut txn.combined, &outcome);
            }
            None => {
                let label = edit.forward.label().to_string();
                self.undo.push_committed(Txn::single(label, edit));
            }
        }
        Ok(outcome)
    }

    /// Opens a transaction: subsequent [`apply`]s batch into one undo step until
    /// [`end_transaction`]. A no-op if one is already open.
    ///
    /// [`apply`]: DocumentSession::apply
    /// [`end_transaction`]: DocumentSession::end_transaction
    pub fn begin_transaction(&mut self, label: impl Into<String>) {
        if self.open_txn.is_none() {
            self.open_txn = Some(Box::new(TxnState {
                label: label.into(),
                edits: Vec::new(),
                combined: empty_outcome(),
            }));
        }
    }

    /// Commits the open transaction as one undo step and returns the combined
    /// outcome (merged deltas, unioned dirty span, strongest regen). Returns
    /// `None` if no transaction is open or it is empty.
    pub fn end_transaction(&mut self) -> Option<CommandOutcome> {
        let txn = self.open_txn.take()?;
        if txn.edits.is_empty() {
            return None;
        }
        let combined = txn.combined.clone();
        self.undo.push_committed(Txn {
            label: txn.label,
            edits: txn.edits,
        });
        Some(combined)
    }

    /// Rolls back and discards the open transaction (C++ `cancelTransaction`).
    pub fn cancel_transaction(&mut self) {
        if let Some(txn) = self.open_txn.take() {
            for edit in txn.edits.into_iter().rev() {
                edit.inverse.apply(&mut self.document);
            }
            self.rebuild_graph();
        }
    }

    /// Undoes the newest committed transaction (applies its inverses in reverse
    /// and moves it to the redo stack). Ignored while a transaction is open
    /// (C++ `CommandProcessor::undo`). Returns `true` if a step was undone.
    pub fn undo(&mut self) -> bool {
        if self.open_txn.is_some() {
            return false;
        }
        let Some(txn) = self.undo.pop_for_undo() else {
            return false;
        };
        for edit in txn.edits.iter().rev() {
            edit.inverse.clone().apply(&mut self.document);
        }
        self.rebuild_graph();
        self.undo.push_undone(txn);
        true
    }

    /// Redoes the newest undone transaction by **re-executing** its forward
    /// commands (recomputing fresh inverses). Ignored while a transaction is
    /// open. Returns `true` if a step was redone.
    ///
    /// # Errors
    /// A [`DomainError`] if a replayed command fails (the partial replay is
    /// rolled back and the step returned to the redo stack).
    pub fn redo(&mut self) -> Result<bool, DomainError> {
        if self.open_txn.is_some() {
            return Ok(false);
        }
        let Some(txn) = self.undo.pop_for_redo() else {
            return Ok(false);
        };
        let mut new_edits: Vec<AppliedEdit> = Vec::with_capacity(txn.edits.len());
        for edit in &txn.edits {
            match self.apply_forward(&edit.forward) {
                Ok((_, inverse)) => new_edits.push(AppliedEdit {
                    forward: edit.forward.clone(),
                    inverse,
                }),
                Err(e) => {
                    for done in new_edits.into_iter().rev() {
                        done.inverse.apply(&mut self.document);
                    }
                    self.rebuild_graph();
                    self.undo.push_undone(txn);
                    return Err(e);
                }
            }
        }
        self.undo.push_redone(Txn {
            label: txn.label,
            edits: new_edits,
        });
        Ok(true)
    }

    // ── Forward dispatch ─────────────────────────────────────────────────────

    /// Applies one command's forward effect, returning its outcome + inverse.
    /// Pure with respect to the undo stacks (used by `apply`, `redo`, cancel).
    fn apply_forward(
        &mut self,
        cmd: &EditCommand,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        match cmd {
            EditCommand::AddOperation { record, at_cursor } => {
                self.add_operation(record.clone(), *at_cursor)
            }
            EditCommand::UpdateOperationParams { record, op } => {
                self.update_operation_params(*record, op.clone())
            }
            EditCommand::EditOperationInput {
                record,
                path,
                reference,
            } => self.edit_operation_input(*record, path, reference),
            EditCommand::RemoveOperation { record } => self.remove_operation(*record),
            EditCommand::SetRollback { cursor } => self.set_rollback(*cursor),
            EditCommand::SetOperationSuppression {
                record,
                suppressed,
                cascade,
            } => self.set_suppression(*record, *suppressed, *cascade),
            EditCommand::AddSketch { sketch } => self.add_sketch(sketch.clone()),
            EditCommand::DeleteSketch { sketch } => self.delete_sketch(*sketch),
            EditCommand::RenameSketch { sketch, name } => self.rename_sketch(*sketch, name.clone()),
            EditCommand::UpdateSketchAttachment {
                sketch,
                plane,
                attachment,
            } => self.update_sketch_attachment(*sketch, *plane, attachment.clone()),
            EditCommand::SketchEdit { sketch, ops } => self.sketch_edit(*sketch, ops),
            EditCommand::SketchDragGesture { sketch, after, .. } => {
                self.sketch_drag(*sketch, after.clone())
            }
            EditCommand::AddBody { body } => self.add_body(body.clone()),
            EditCommand::DeleteBody { body } => self.delete_body(*body),
            EditCommand::RenameBody { body, name } => self.rename_body(*body, name.clone()),
            EditCommand::SetVisibility { target, visible } => {
                self.set_visibility(*target, *visible)
            }
            EditCommand::AddDatumPlane { datum } => self.add_datum(datum.clone()),
            EditCommand::SetVariable { variable, value } => {
                self.set_variable(*variable, value.clone())
            }
            EditCommand::AddVariable { variable } => self.add_variable(variable.clone()),
            EditCommand::RemoveVariable { variable } => self.remove_variable(*variable),
        }
    }

    // ── Timeline commands ────────────────────────────────────────────────────

    fn add_operation(
        &mut self,
        mut record: OperationRecord,
        at_cursor: bool,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let id = record.record_id;
        if self.document.timeline.record_by_id(id).is_some() {
            return Err(DomainError::Validation(format!("duplicate record id {id}")));
        }
        // F2: fillet/chamfer `edges`/`edge_ids` must be in lockstep (all entry paths).
        validate_fillet_lockstep(&record.op)?;
        // F8: re-derive the uniform input view for Known ops (self-healing — don't
        // trust caller-supplied `inputs`; mirrors `update_operation_params` and the
        // record deserialize path). An Opaque frozen node keeps its stored inputs.
        if matches!(record.op, Operation::Known(_)) {
            record.inputs = record.op.derive_inputs();
        }
        let len = self.document.timeline.len();
        let insert_index = if at_cursor {
            self.document.timeline.cursor().min(len)
        } else {
            len
        };
        // Anti-time-travel: validate on the would-be record list before mutating.
        let mut recs = self.document.timeline.records().to_vec();
        recs.insert(insert_index, record.clone());
        self.validate_temporal(&recs, id, &record.op.derive_inputs().bodies)?;

        let index = if at_cursor {
            self.document.timeline.insert_at_cursor(record)
        } else {
            let cursor = self.document.timeline.cursor();
            insert_record_at(&mut self.document.timeline, insert_index, record, cursor);
            insert_index
        };
        self.rebuild_graph();

        let new_len = self.document.timeline.len();
        let dirty = DirtyRange::new(index, new_len);
        let regen = if index < self.document.timeline.cursor() {
            RegenHint::ToEnd
        } else {
            RegenHint::None
        };
        let mut delta = ProjectionDelta::timeline();
        delta.cursor_changed = at_cursor;
        Ok((
            CommandOutcome {
                projection_delta: delta,
                dirty: Some(dirty),
                regen,
            },
            Inverse::RemoveRecord { record: id },
        ))
    }

    fn update_operation_params(
        &mut self,
        id: RecordId,
        op: Operation,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let index = self
            .document
            .timeline
            .index_of(id)
            .ok_or(DomainError::RecordNotFound(id))?;
        let prior = self.document.timeline.record(index).unwrap().clone();
        if !same_op_type(&prior.op, &op) {
            return Err(DomainError::Validation(
                "UpdateOperationParams may not change opType".into(),
            ));
        }
        // F2: fillet/chamfer `edges`/`edge_ids` must be in lockstep (all entry paths).
        validate_fillet_lockstep(&op)?;
        let mut nr = prior.clone();
        nr.op = op;
        nr.inputs = nr.op.derive_inputs();

        let mut recs = self.document.timeline.records().to_vec();
        recs[index] = nr.clone();
        self.validate_temporal(&recs, id, &nr.op.derive_inputs().bodies)?;

        replace_record_at(&mut self.document.timeline, index, nr);
        self.rebuild_graph();
        Ok(self.dirty_record_outcome(
            index,
            Inverse::RestoreRecord {
                index,
                record: Box::new(prior),
            },
        ))
    }

    fn edit_operation_input(
        &mut self,
        id: RecordId,
        path: &InputPath,
        reference: &InputRef,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let index = self
            .document
            .timeline
            .index_of(id)
            .ok_or(DomainError::RecordNotFound(id))?;
        let prior = self.document.timeline.record(index).unwrap().clone();
        let mut nr = prior.clone();
        set_input(&mut nr.op, path, reference)?;
        nr.inputs = nr.op.derive_inputs();

        let mut recs = self.document.timeline.records().to_vec();
        recs[index] = nr.clone();
        self.validate_temporal(&recs, id, &nr.op.derive_inputs().bodies)?;

        replace_record_at(&mut self.document.timeline, index, nr);
        self.rebuild_graph();
        Ok(self.dirty_record_outcome(
            index,
            Inverse::RestoreRecord {
                index,
                record: Box::new(prior),
            },
        ))
    }

    fn remove_operation(&mut self, id: RecordId) -> Result<(CommandOutcome, Inverse), DomainError> {
        let index = self
            .document
            .timeline
            .index_of(id)
            .ok_or(DomainError::RecordNotFound(id))?;
        let record = self.document.timeline.record(index).unwrap().clone();
        let cursor = self.document.timeline.cursor();
        let dirty = self.document.timeline.remove(id)?;
        self.rebuild_graph();
        let mut delta = ProjectionDelta::timeline();
        delta.cursor_changed = index < cursor;
        Ok((
            CommandOutcome {
                projection_delta: delta,
                dirty: Some(dirty),
                regen: RegenHint::ToEnd,
            },
            Inverse::InsertRecord {
                index,
                record: Box::new(record),
                cursor,
            },
        ))
    }

    fn set_rollback(&mut self, cursor: usize) -> Result<(CommandOutcome, Inverse), DomainError> {
        let len = self.document.timeline.len();
        if cursor > len {
            return Err(DomainError::Timeline(format!(
                "rollback cursor {cursor} exceeds op count {len}"
            )));
        }
        let prior = self.document.timeline.cursor();
        let dirty = self.document.timeline.set_cursor(cursor);
        let delta = ProjectionDelta {
            cursor_changed: true,
            ..ProjectionDelta::default()
        };
        Ok((
            CommandOutcome {
                projection_delta: delta,
                dirty: Some(dirty),
                regen: RegenHint::PreviewTo(cursor),
            },
            Inverse::RestoreCursor { cursor: prior },
        ))
    }

    fn set_suppression(
        &mut self,
        id: RecordId,
        suppressed: bool,
        cascade: bool,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let index = self
            .document
            .timeline
            .index_of(id)
            .ok_or(DomainError::RecordNotFound(id))?;
        // Affected = the op + (if cascading) its downstream closure.
        let mut affected: Vec<RecordId> = vec![id];
        if cascade {
            affected.extend(self.graph.downstream(id));
        }
        // Capture priors (for the composite inverse) and set the flag in one rebuild.
        let mut recs = self.document.timeline.records().to_vec();
        let mut inverses: Vec<Inverse> = Vec::new();
        for &rid in &affected {
            if let Some(i) = recs.iter().position(|r| r.record_id == rid) {
                inverses.push(Inverse::RestoreRecord {
                    index: i,
                    record: Box::new(recs[i].clone()),
                });
                recs[i].suppressed = suppressed;
            }
        }
        let cursor = self.document.timeline.cursor();
        self.document.timeline = Timeline::from_records(recs);
        self.document
            .timeline
            .set_cursor(cursor.min(self.document.timeline.len()));
        self.rebuild_graph();

        let dirty = DirtyRange::new(index, self.document.timeline.len());
        Ok((
            CommandOutcome {
                projection_delta: ProjectionDelta::timeline(),
                dirty: Some(dirty),
                regen: RegenHint::ToEnd,
            },
            Inverse::Composite(inverses),
        ))
    }

    // ── Sketch commands ──────────────────────────────────────────────────────

    fn add_sketch(&mut self, sketch: Sketch) -> Result<(CommandOutcome, Inverse), DomainError> {
        let id = sketch.id;
        if self.document.sketches.contains_key(&id) {
            return Err(DomainError::Validation(format!("duplicate sketch id {id}")));
        }
        self.document.sketches.insert(id, sketch);
        Ok((
            self.sketch_dirty_outcome(id),
            Inverse::RestoreSketch { id, prior: None },
        ))
    }

    fn delete_sketch(&mut self, id: SketchId) -> Result<(CommandOutcome, Inverse), DomainError> {
        let prior = self
            .document
            .sketches
            .remove(&id)
            .ok_or_else(|| DomainError::Validation(format!("sketch {id} not found")))?;
        let prior_vis = self.document.sketch_visibility.remove(&id);
        Ok((
            self.sketch_dirty_outcome(id),
            Inverse::Composite(vec![
                Inverse::RestoreSketch {
                    id,
                    prior: Some(Box::new(prior)),
                },
                Inverse::RestoreSketchVisibility {
                    id,
                    prior: prior_vis,
                },
            ]),
        ))
    }

    fn rename_sketch(
        &mut self,
        id: SketchId,
        name: String,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let sketch = self
            .document
            .sketches
            .get_mut(&id)
            .ok_or_else(|| DomainError::Validation(format!("sketch {id} not found")))?;
        let prior = sketch.clone();
        sketch.name = name;
        Ok((
            CommandOutcome::metadata_only(ProjectionDelta::sketch(id)),
            Inverse::RestoreSketch {
                id,
                prior: Some(Box::new(prior)),
            },
        ))
    }

    fn update_sketch_attachment(
        &mut self,
        id: SketchId,
        plane: crate::sketch::SketchPlane,
        attachment: crate::sketch::SketchAttachment,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let sketch = self
            .document
            .sketches
            .get_mut(&id)
            .ok_or_else(|| DomainError::Validation(format!("sketch {id} not found")))?;
        let prior = sketch.clone();
        sketch.plane = plane;
        sketch.attachment = attachment;
        Ok((
            self.sketch_dirty_outcome(id),
            Inverse::RestoreSketch {
                id,
                prior: Some(Box::new(prior)),
            },
        ))
    }

    fn sketch_edit(
        &mut self,
        id: SketchId,
        ops: &[SketchEditOp],
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let prior = self
            .document
            .sketches
            .get(&id)
            .cloned()
            .ok_or_else(|| DomainError::Validation(format!("sketch {id} not found")))?;
        let next = apply_sketch_ops(&prior, ops)?;
        self.document.sketches.insert(id, next);
        Ok((
            self.sketch_dirty_outcome(id),
            Inverse::RestoreSketch {
                id,
                prior: Some(Box::new(prior)),
            },
        ))
    }

    fn sketch_drag(
        &mut self,
        id: SketchId,
        after: Sketch,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let prior = self
            .document
            .sketches
            .get(&id)
            .cloned()
            .ok_or_else(|| DomainError::Validation(format!("sketch {id} not found")))?;
        self.document.sketches.insert(id, after);
        Ok((
            self.sketch_dirty_outcome(id),
            Inverse::RestoreSketch {
                id,
                prior: Some(Box::new(prior)),
            },
        ))
    }

    // ── Body commands ────────────────────────────────────────────────────────

    fn add_body(&mut self, meta: BodyMeta) -> Result<(CommandOutcome, Inverse), DomainError> {
        let id = meta.id;
        let prior = self.document.bodies.clone();
        if !self.document.bodies.register(meta) {
            return Err(DomainError::Validation(format!("duplicate body id {id}")));
        }
        Ok((
            CommandOutcome::metadata_only(ProjectionDelta::body(id)),
            Inverse::RestoreBodies {
                registry: Box::new(prior),
            },
        ))
    }

    fn delete_body(&mut self, id: BodyId) -> Result<(CommandOutcome, Inverse), DomainError> {
        if !self.document.bodies.contains(id) {
            return Err(DomainError::Validation(format!("body {id} not found")));
        }
        let prior = self.document.bodies.clone();
        self.document.bodies.remove(id);
        Ok((
            CommandOutcome::metadata_only(ProjectionDelta::body(id)),
            Inverse::RestoreBodies {
                registry: Box::new(prior),
            },
        ))
    }

    fn rename_body(
        &mut self,
        id: BodyId,
        name: String,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        if !self.document.bodies.contains(id) {
            return Err(DomainError::Validation(format!("body {id} not found")));
        }
        let prior = self.document.bodies.clone();
        self.document.bodies.set_name(id, name);
        Ok((
            CommandOutcome::metadata_only(ProjectionDelta::body(id)),
            Inverse::RestoreBodies {
                registry: Box::new(prior),
            },
        ))
    }

    fn set_visibility(
        &mut self,
        target: VisibilityTarget,
        visible: bool,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        match target {
            VisibilityTarget::Body(id) => {
                if !self.document.bodies.contains(id) {
                    return Err(DomainError::Validation(format!("body {id} not found")));
                }
                let prior = self.document.bodies.clone();
                self.document.bodies.set_visible(id, visible);
                Ok((
                    CommandOutcome::metadata_only(ProjectionDelta::body(id)),
                    Inverse::RestoreBodies {
                        registry: Box::new(prior),
                    },
                ))
            }
            VisibilityTarget::Sketch(id) => {
                if !self.document.sketches.contains_key(&id) {
                    return Err(DomainError::Validation(format!("sketch {id} not found")));
                }
                let prior = self.document.sketch_visibility.get(&id).copied();
                self.document.set_sketch_visible(id, visible);
                Ok((
                    CommandOutcome::metadata_only(ProjectionDelta::sketch(id)),
                    Inverse::RestoreSketchVisibility { id, prior },
                ))
            }
        }
    }

    fn add_datum(&mut self, datum: DatumPlane) -> Result<(CommandOutcome, Inverse), DomainError> {
        let id = datum.id;
        if self.document.datum_planes.contains_key(&id) {
            return Err(DomainError::Validation(format!("duplicate datum id {id}")));
        }
        self.document.datum_planes.insert(id, datum);
        Ok((
            CommandOutcome::metadata_only(ProjectionDelta::datum(id)),
            Inverse::RestoreDatum { id, prior: None },
        ))
    }

    // ── Variable commands ────────────────────────────────────────────────────

    fn set_variable(
        &mut self,
        var_id: crate::ids::VariableId,
        value: Scalar,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        let existing = self
            .document
            .variables
            .iter()
            .find(|v| v.id == var_id)
            .cloned()
            .ok_or_else(|| DomainError::Validation(format!("variable {var_id} not found")))?;
        let prior = self.document.variables.clone();
        self.document
            .variables
            .upsert(Variable { value, ..existing });
        Ok((
            self.variable_outcome(),
            Inverse::RestoreVariables {
                table: Box::new(prior),
            },
        ))
    }

    fn add_variable(&mut self, var: Variable) -> Result<(CommandOutcome, Inverse), DomainError> {
        if self.document.variables.get(&var.name).is_some() {
            return Err(DomainError::Validation(format!(
                "duplicate variable name {}",
                var.name
            )));
        }
        let prior = self.document.variables.clone();
        self.document.variables.upsert(var);
        Ok((
            self.variable_outcome(),
            Inverse::RestoreVariables {
                table: Box::new(prior),
            },
        ))
    }

    fn remove_variable(
        &mut self,
        var_id: crate::ids::VariableId,
    ) -> Result<(CommandOutcome, Inverse), DomainError> {
        if !self.document.variables.iter().any(|v| v.id == var_id) {
            return Err(DomainError::Validation(format!(
                "variable {var_id} not found"
            )));
        }
        let prior = self.document.variables.clone();
        let mut vt = VariableTable::new();
        for v in self.document.variables.iter() {
            if v.id != var_id {
                vt.upsert(v.clone());
            }
        }
        self.document.variables = vt;
        Ok((
            self.variable_outcome(),
            Inverse::RestoreVariables {
                table: Box::new(prior),
            },
        ))
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn rebuild_graph(&mut self) {
        self.graph
            .rebuild_from_records(self.document.timeline.records());
    }

    /// Validates that every input body of `record_id` is produced strictly
    /// before it (anti-time-travel; C++ `DependencyGraph::producesBefore`).
    fn validate_temporal(
        &self,
        records: &[OperationRecord],
        record_id: RecordId,
        input_bodies: &[BodyId],
    ) -> Result<(), DomainError> {
        let mut g = DependencyGraph::new();
        g.rebuild_from_records(records);
        for &b in input_bodies {
            if !g.produces_before(b, record_id) {
                return Err(DomainError::Timeline(format!(
                    "op {record_id} references body {b} produced by a later operation (anti-time-travel)"
                )));
            }
        }
        Ok(())
    }

    /// Outcome for a record-content edit at `index`: dirties `[index, len)`,
    /// regen to end.
    fn dirty_record_outcome(&self, index: usize, inverse: Inverse) -> (CommandOutcome, Inverse) {
        let dirty = DirtyRange::new(index, self.document.timeline.len());
        (
            CommandOutcome::dirty_to_end(ProjectionDelta::timeline(), dirty),
            inverse,
        )
    }

    /// Outcome for a sketch edit: dirties to the end from the earliest affected
    /// step (regen to end); metadata-only if nothing depends on the sketch.
    ///
    /// The earliest step is `min(producer, first consumer)` (F4). When a `Sketch`
    /// op produces this `SketchId` in the timeline, that op's own regen re-runs
    /// region detection worker-side, so an edit must dirty from the producer — not
    /// merely from the first op that consumes a region. With no producer op (a
    /// document sketch that never became a timeline node) it falls back to the
    /// first consumer, and to metadata-only when nothing references it.
    fn sketch_dirty_outcome(&self, id: SketchId) -> CommandOutcome {
        let delta = ProjectionDelta::sketch(id);
        let producer_step = self
            .graph
            .sketch_producer(id)
            .and_then(|rid| self.document.timeline.index_of(rid));
        let consumer_step = self.first_step_referencing_sketch(id);
        let start = match (producer_step, consumer_step) {
            (Some(p), Some(c)) => Some(p.min(c)),
            (Some(s), None) | (None, Some(s)) => Some(s),
            (None, None) => None,
        };
        match start {
            Some(step) => CommandOutcome::dirty_to_end(
                delta,
                DirtyRange::new(step, self.document.timeline.len()),
            ),
            None => CommandOutcome::metadata_only(delta),
        }
    }

    /// Conservative outcome for a variable edit: dirties the whole applied
    /// timeline (a bare-variable expression can drive any op).
    fn variable_outcome(&self) -> CommandOutcome {
        let delta = ProjectionDelta {
            variables_changed: true,
            ..ProjectionDelta::default()
        };
        let len = self.document.timeline.len();
        if len == 0 {
            CommandOutcome::metadata_only(delta)
        } else {
            CommandOutcome::dirty_to_end(delta, DirtyRange::new(0, len))
        }
    }

    /// The first timeline index whose op references `sketch` as an input.
    fn first_step_referencing_sketch(&self, sketch: SketchId) -> Option<usize> {
        self.document
            .timeline
            .records()
            .iter()
            .position(|r| r.op.derive_inputs().sketches.contains(&sketch))
    }
}

// ── Free helpers ─────────────────────────────────────────────────────────────

fn empty_outcome() -> CommandOutcome {
    CommandOutcome {
        projection_delta: ProjectionDelta::default(),
        dirty: None,
        regen: RegenHint::None,
    }
}

/// Merges `o` into `acc` (transaction batching).
fn merge_outcome(acc: &mut CommandOutcome, o: &CommandOutcome) {
    acc.projection_delta.merge(&o.projection_delta);
    acc.dirty = union_dirty(acc.dirty, o.dirty);
    acc.regen = stronger_regen(acc.regen, o.regen);
}

fn union_dirty(a: Option<DirtyRange>, b: Option<DirtyRange>) -> Option<DirtyRange> {
    match (a, b) {
        (Some(x), Some(y)) => Some(DirtyRange::new(x.from.min(y.from), x.to.max(y.to))),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

fn stronger_regen(a: RegenHint, b: RegenHint) -> RegenHint {
    use RegenHint::{None, PreviewTo, ToEnd};
    match (a, b) {
        (ToEnd, _) | (_, ToEnd) => ToEnd,
        (PreviewTo(x), PreviewTo(y)) => PreviewTo(x.max(y)),
        (PreviewTo(x), None) | (None, PreviewTo(x)) => PreviewTo(x),
        (None, None) => None,
    }
}

/// Validates fillet/chamfer edge consistency (F2): when the typed `edges` list
/// is populated it must stay in lockstep with the bare `edge_ids` — equal length,
/// and each typed ref's `primary.element` (when present) must equal the parallel
/// `edge_ids` entry. An intent-only ref (`primary` = `None`) is accepted
/// positionally (matched by count alone). An empty `edges` list is the legacy
/// bare-ids form and is always accepted (the operated body binds at regen time).
/// Non-fillet/chamfer ops and opaque ops are trivially valid.
fn validate_fillet_lockstep(op: &Operation) -> Result<(), DomainError> {
    let (edge_ids, edges) = match op {
        Operation::Known(KnownOperation::Fillet(p)) => (&p.edge_ids, &p.edges),
        Operation::Known(KnownOperation::Chamfer(p)) => (&p.edge_ids, &p.edges),
        _ => return Ok(()),
    };
    if edges.is_empty() {
        return Ok(());
    }
    if edges.len() != edge_ids.len() {
        return Err(DomainError::Validation(format!(
            "fillet/chamfer edges ({}) and edgeIds ({}) length mismatch",
            edges.len(),
            edge_ids.len()
        )));
    }
    for (i, e) in edges.iter().enumerate() {
        if let Some(primary) = &e.primary {
            if primary.element != edge_ids[i] {
                return Err(DomainError::Validation(format!(
                    "fillet/chamfer edge {i}: typed ref element {} != edgeIds[{i}] {}",
                    primary.element, edge_ids[i]
                )));
            }
        }
    }
    Ok(())
}

/// True iff two operations have the same `opType` (params update may not change it).
fn same_op_type(a: &Operation, b: &Operation) -> bool {
    match (a, b) {
        (Operation::Known(x), Operation::Known(y)) => {
            std::mem::discriminant(x) == std::mem::discriminant(y)
        }
        (Operation::Opaque(x), Operation::Opaque(y)) => x.raw.get("opType") == y.raw.get("opType"),
        _ => false,
    }
}

/// Writes `reference` into the op input slot named by `path`, keeping fillet
/// `edge_ids`/`edges` in lockstep. See [`crate::edit::command`] for the divergence.
fn set_input(
    op: &mut Operation,
    path: &InputPath,
    reference: &InputRef,
) -> Result<(), DomainError> {
    let known = match op {
        Operation::Known(k) => k,
        Operation::Opaque(_) => {
            return Err(DomainError::InvalidReference(
                "cannot edit inputs of a frozen (opaque) op".into(),
            ))
        }
    };
    match (path, known) {
        (InputPath::ExtrudeProfile, KnownOperation::Extrude(p)) => {
            p.profile = Some(want_region(reference)?);
        }
        (InputPath::ExtrudeTargetFace { second }, KnownOperation::Extrude(p)) => {
            let er = want_element(reference)?;
            if *second {
                p.target_face2 = Some(er);
            } else {
                p.target_face = Some(er);
            }
        }
        (InputPath::RevolveAxis, KnownOperation::Revolve(p)) => {
            p.axis = Some(want_axis(reference)?);
        }
        (InputPath::ExtrudeProfile, KnownOperation::Revolve(p)) => {
            p.profile = Some(want_region(reference)?);
        }
        (InputPath::FilletEdges { index }, KnownOperation::Fillet(p)) => {
            set_fillet_edge(
                &mut p.edges,
                &mut p.edge_ids,
                *index,
                want_element(reference)?,
            )?;
        }
        (InputPath::FilletEdges { index }, KnownOperation::Chamfer(p)) => {
            set_fillet_edge(
                &mut p.edges,
                &mut p.edge_ids,
                *index,
                want_element(reference)?,
            )?;
        }
        (InputPath::BooleanTarget, KnownOperation::Boolean(p)) => {
            p.target_body = want_body(reference)?;
        }
        (InputPath::BooleanTool, KnownOperation::Boolean(p)) => {
            p.tool_body = want_body(reference)?;
        }
        _ => {
            return Err(DomainError::InvalidReference(
                "input path does not match the operation type / reference kind".into(),
            ))
        }
    }
    Ok(())
}

/// Sets fillet/chamfer edge `index`, keeping the typed `edges` ref and the bare
/// `edge_ids` element id consistent (both must be populated — see the
/// `FilletParams` doc). The typed ref must carry a `primary` element id.
fn set_fillet_edge(
    edges: &mut Vec<ElementRef>,
    edge_ids: &mut Vec<crate::ids::ElementId>,
    index: usize,
    reference: ElementRef,
) -> Result<(), DomainError> {
    let element = reference
        .primary
        .as_ref()
        .map(|p| p.element.clone())
        .ok_or_else(|| {
            DomainError::InvalidReference(
                "a fillet/chamfer edge ref must carry a primary element id".into(),
            )
        })?;
    // The bare `edge_ids` and typed `edges` lists must stay the same length and
    // in lockstep (see `FilletParams`). Overwrite an existing slot or append one;
    // a gap beyond the end is an out-of-range edit.
    let len = edges.len().max(edge_ids.len());
    if index > len {
        return Err(DomainError::InvalidReference(format!(
            "fillet edge index {index} out of range (len {len})"
        )));
    }
    if index == edges.len() {
        edges.push(reference);
    } else {
        edges[index] = reference;
    }
    if index == edge_ids.len() {
        edge_ids.push(element);
    } else {
        edge_ids[index] = element;
    }
    Ok(())
}

fn want_element(r: &InputRef) -> Result<ElementRef, DomainError> {
    match r {
        InputRef::Element(e) => Ok(e.clone()),
        _ => Err(DomainError::InvalidReference(
            "expected an element ref".into(),
        )),
    }
}
fn want_region(r: &InputRef) -> Result<crate::document::refs::SketchRegionRef, DomainError> {
    match r {
        InputRef::Region(reg) => Ok(reg.clone()),
        _ => Err(DomainError::InvalidReference(
            "expected a region ref".into(),
        )),
    }
}
fn want_axis(r: &InputRef) -> Result<crate::document::refs::AxisRef, DomainError> {
    match r {
        InputRef::Axis(a) => Ok(a.clone()),
        _ => Err(DomainError::InvalidReference("expected an axis ref".into())),
    }
}
fn want_body(r: &InputRef) -> Result<BodyId, DomainError> {
    match r {
        InputRef::Body(b) => Ok(*b),
        _ => Err(DomainError::InvalidReference("expected a body ref".into())),
    }
}

/// Applies a batch of sketch edits to a copy of `prior`, preserving entity /
/// constraint order (SetDimension/SetEntityPositions mutate in place). Validation
/// (dup id / dangling ref) runs on the rebuilt sketch via its `add_*` API.
fn apply_sketch_ops(prior: &Sketch, ops: &[SketchEditOp]) -> Result<Sketch, DomainError> {
    let mut entities: Vec<SketchEntity> = prior.entities().to_vec();
    let mut constraints: Vec<Constraint> = prior.constraints().to_vec();

    for op in ops {
        match op {
            SketchEditOp::AddEntity { entity } => entities.push(entity.clone()),
            SketchEditOp::RemoveEntity { entity } => {
                entities = cascade_remove_entity(&entities, &mut constraints, *entity);
            }
            SketchEditOp::AddConstraint { constraint } => constraints.push(constraint.clone()),
            SketchEditOp::RemoveConstraint { constraint } => {
                constraints.retain(|c| c.id() != *constraint);
            }
            SketchEditOp::SetDimension { constraint, value } => {
                let c = constraints
                    .iter_mut()
                    .find(|c| c.id() == *constraint)
                    .ok_or_else(|| {
                        DomainError::Validation(format!("constraint {constraint} not found"))
                    })?;
                *c = constraint_with_value(c, value.clone()).ok_or_else(|| {
                    DomainError::Validation(format!("constraint {constraint} is not dimensional"))
                })?;
            }
            SketchEditOp::SetEntityPositions { positions } => {
                for (eid, at) in positions {
                    let e = entities
                        .iter_mut()
                        .find(|e| e.id() == *eid)
                        .ok_or_else(|| {
                            DomainError::Validation(format!("entity {eid} not found"))
                        })?;
                    *e = entity_with_position(e, *at).ok_or_else(|| {
                        DomainError::Validation(format!("entity {eid} is not a point"))
                    })?;
                }
            }
        }
    }
    rebuild_sketch(prior, entities, constraints)
}

/// Removes `seed` and, to a fixpoint, every entity that transitively references
/// it, then drops any constraint that referenced a removed entity.
fn cascade_remove_entity(
    entities: &[SketchEntity],
    constraints: &mut Vec<Constraint>,
    seed: crate::ids::EntityId,
) -> Vec<SketchEntity> {
    let mut removed = vec![seed];
    loop {
        let newly: Vec<_> = entities
            .iter()
            .filter(|e| {
                !removed.contains(&e.id())
                    && e.referenced_entities().iter().any(|r| removed.contains(r))
            })
            .map(SketchEntity::id)
            .collect();
        if newly.is_empty() {
            break;
        }
        removed.extend(newly);
    }
    constraints.retain(|c| !c.entities().iter().any(|r| removed.contains(r)));
    entities
        .iter()
        .filter(|e| !removed.contains(&e.id()))
        .cloned()
        .collect()
}

/// Rebuilds a validated [`Sketch`] from working entity/constraint vecs,
/// preserving `prior`'s identity/plane/attachment/regions.
fn rebuild_sketch(
    prior: &Sketch,
    entities: Vec<SketchEntity>,
    constraints: Vec<Constraint>,
) -> Result<Sketch, DomainError> {
    let mut s = Sketch::new(prior.id, prior.name.clone(), prior.attachment.clone());
    s.plane = prior.plane;
    s.regions = prior.regions.clone();
    s.extra = prior.extra.clone();
    for e in entities {
        s.add_entity(e).map_err(sketch_err)?;
    }
    for c in constraints {
        s.add_constraint(c).map_err(sketch_err)?;
    }
    Ok(s)
}

fn sketch_err(e: SketchError) -> DomainError {
    DomainError::Validation(e.to_string())
}

/// Returns a clone of `c` with its dimensional value replaced, or `None` for a
/// geometric constraint.
fn constraint_with_value(c: &Constraint, value: Scalar) -> Option<Constraint> {
    Some(match c {
        Constraint::Distance {
            id,
            entity1,
            entity2,
            ..
        } => Constraint::Distance {
            id: *id,
            entity1: *entity1,
            entity2: *entity2,
            value,
        },
        Constraint::HorizontalDistance {
            id, point1, point2, ..
        } => Constraint::HorizontalDistance {
            id: *id,
            point1: *point1,
            point2: *point2,
            value,
        },
        Constraint::VerticalDistance {
            id, point1, point2, ..
        } => Constraint::VerticalDistance {
            id: *id,
            point1: *point1,
            point2: *point2,
            value,
        },
        Constraint::Angle {
            id, line1, line2, ..
        } => Constraint::Angle {
            id: *id,
            line1: *line1,
            line2: *line2,
            value,
        },
        Constraint::Radius { id, entity, .. } => Constraint::Radius {
            id: *id,
            entity: *entity,
            value,
        },
        Constraint::Diameter { id, entity, .. } => Constraint::Diameter {
            id: *id,
            entity: *entity,
            value,
        },
        _ => return None,
    })
}

/// Returns a clone of `e` at position `at`, or `None` if `e` is not a point.
fn entity_with_position(e: &SketchEntity, at: Vec2) -> Option<SketchEntity> {
    match e {
        SketchEntity::Point {
            id,
            construction,
            reference_locked,
            ..
        } => Some(SketchEntity::Point {
            id: *id,
            at,
            construction: *construction,
            reference_locked: *reference_locked,
        }),
        _ => None,
    }
}
