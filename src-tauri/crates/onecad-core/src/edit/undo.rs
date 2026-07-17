//! Undo/redo via **inverse mementos** with transaction batching.
//!
//! Design (plan "Rust core specifics"; C++ `CommandProcessor`):
//!
//! * Each applied [`EditCommand`] captures an [`Inverse`] — a *memento* that
//!   restores the exact prior state, **never** a replayed inverse geometry op
//!   (plan invariant: "undo = history change + replay, never inverse OCCT
//!   mutation"). Because `document.json` persists only `{records, cursor}` (regen
//!   states are derived — see [`crate::document`]), the mementos here restore the
//!   authoritative state exactly.
//! * [`Txn`] groups several applied edits into one user-visible undo step
//!   (C++ `beginTransaction`/`endTransaction`; a group undoes its edits in
//!   reverse).
//! * [`UndoStack`] is bounded to [`UNDO_CAP`] (200) and evicts the oldest step
//!   on overflow (C++ `CommandProcessor.cpp:79`, `undoStack_.erase(begin())`);
//!   the redo stack is cleared on every new apply.
//!
//! Undo restores state via [`Inverse::apply`]; redo **re-executes** the forward
//! command (recomputing a fresh inverse), matching C++ `redo()` calling
//! `execute()` again.

use crate::document::body::BodyRegistry;
use crate::document::datum::DatumPlane;
use crate::document::record::OperationRecord;
use crate::document::repair::RepairState;
use crate::document::variables::VariableTable;
use crate::document::Document;
use crate::history::Timeline;
use crate::ids::{DatumPlaneId, RecordId, SketchId};
use crate::sketch::Sketch;

use super::command::EditCommand;

/// Maximum retained undo steps (C++ `kMaxUndoDepth`, `CommandProcessor.cpp:79`).
pub const UNDO_CAP: usize = 200;

/// A memento that restores the exact document state prior to one applied
/// command (or one composite sub-step).
///
/// Timeline-structural inverses operate over `{records, cursor}` (the persisted
/// authoritative fields). Subsystem inverses (`RestoreBodies`,
/// `RestoreVariables`) memento the whole small collection — exact and cheap
/// (bodies/variables are few per document). Where a single command touches
/// several subsystems its inverse is a [`Inverse::Composite`], applied in
/// reverse.
#[derive(Debug, Clone)]
pub enum Inverse {
    /// Undo of `AddOperation`: delete the added record (Timeline.remove restores
    /// the cursor).
    RemoveRecord {
        /// Id of the record to remove.
        record: RecordId,
    },
    /// Undo of `RemoveOperation`: re-insert the record at its original index,
    /// then restore the cursor.
    InsertRecord {
        /// Original timeline index.
        index: usize,
        /// The removed record.
        record: Box<OperationRecord>,
        /// Cursor to restore after re-insertion.
        cursor: usize,
    },
    /// Undo of a record-content edit (params / input / suppression): replace the
    /// record at `index` with its prior full form.
    RestoreRecord {
        /// Timeline index to overwrite.
        index: usize,
        /// The prior record.
        record: Box<OperationRecord>,
    },
    /// Undo of `SetRollback`: restore the cursor.
    RestoreCursor {
        /// Cursor to restore.
        cursor: usize,
    },
    /// Undo of a sketch add/delete/edit/rename/retarget/drag: restore (`Some`)
    /// or remove (`None` — undo of an add) the sketch.
    RestoreSketch {
        /// Target sketch id.
        id: SketchId,
        /// The prior sketch, or `None` if it did not exist.
        prior: Option<Box<Sketch>>,
    },
    /// Undo of a sketch-visibility change.
    RestoreSketchVisibility {
        /// Target sketch id.
        id: SketchId,
        /// The prior override (`None` = no override = visible default).
        prior: Option<bool>,
    },
    /// Undo of a body add/delete/rename/visibility: restore the whole registry.
    RestoreBodies {
        /// The prior body registry.
        registry: Box<BodyRegistry>,
    },
    /// Undo of `AddDatumPlane` (or a datum change): restore (`Some`) or remove
    /// (`None`) the datum.
    RestoreDatum {
        /// Target datum id.
        id: DatumPlaneId,
        /// The prior datum, or `None` if it did not exist.
        prior: Option<Box<DatumPlane>>,
    },
    /// Undo of an add/remove/set variable: restore the whole variable table.
    RestoreVariables {
        /// The prior variable table.
        table: Box<VariableTable>,
    },
    /// Undo of an edit that changed the repair state.
    RestoreRepair {
        /// The prior repair state.
        state: Box<RepairState>,
    },
    /// Several inverses, applied in **reverse** order (multi-subsystem command).
    Composite(Vec<Inverse>),
    /// Nothing to undo.
    Noop,
}

impl Inverse {
    /// Restores the mementoed prior state onto `doc`.
    pub(crate) fn apply(self, doc: &mut Document) {
        match self {
            Inverse::RemoveRecord { record } => {
                // The memento is applied against the exact state it was captured
                // over, so the record must be present. Assert in debug to catch a
                // future memento bug; release keeps the graceful no-op on absence.
                let removed = doc.timeline.remove(record);
                debug_assert!(
                    removed.is_ok(),
                    "RemoveRecord inverse: record {record} not found"
                );
            }
            Inverse::InsertRecord {
                index,
                record,
                cursor,
            } => insert_record_at(&mut doc.timeline, index, *record, cursor),
            Inverse::RestoreRecord { index, record } => {
                replace_record_at(&mut doc.timeline, index, *record);
            }
            Inverse::RestoreCursor { cursor } => {
                doc.timeline.set_cursor(cursor);
            }
            Inverse::RestoreSketch { id, prior } => match prior {
                Some(s) => {
                    doc.sketches.insert(id, *s);
                }
                None => {
                    doc.sketches.remove(&id);
                }
            },
            Inverse::RestoreSketchVisibility { id, prior } => match prior {
                Some(v) => {
                    doc.sketch_visibility.insert(id, v);
                }
                None => {
                    doc.sketch_visibility.remove(&id);
                }
            },
            Inverse::RestoreBodies { registry } => doc.bodies = *registry,
            Inverse::RestoreDatum { id, prior } => match prior {
                Some(d) => {
                    doc.datum_planes.insert(id, *d);
                }
                None => {
                    doc.datum_planes.remove(&id);
                }
            },
            Inverse::RestoreVariables { table } => doc.variables = *table,
            Inverse::RestoreRepair { state } => doc.repair = *state,
            Inverse::Composite(list) => {
                for inv in list.into_iter().rev() {
                    inv.apply(doc);
                }
            }
            Inverse::Noop => {}
        }
    }
}

/// One applied command: its forward intent plus the memento that undoes it.
#[derive(Debug, Clone)]
pub struct AppliedEdit {
    /// The forward command (replayed on redo).
    pub forward: EditCommand,
    /// The memento that undoes it.
    pub inverse: Inverse,
}

/// A batch of applied edits that undo/redo as one user-visible step.
#[derive(Debug, Clone)]
pub struct Txn {
    /// Human-facing label.
    pub label: String,
    /// The applied edits, in application order.
    pub edits: Vec<AppliedEdit>,
}

impl Txn {
    /// A transaction with a single edit.
    #[must_use]
    pub fn single(label: impl Into<String>, edit: AppliedEdit) -> Self {
        Self {
            label: label.into(),
            edits: vec![edit],
        }
    }
}

/// The bounded undo/redo stacks (data structure only — the session drives the
/// state restoration).
#[derive(Debug, Default, Clone)]
pub struct UndoStack {
    undo: Vec<Txn>,
    redo: Vec<Txn>,
}

impl UndoStack {
    /// An empty stack.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether an undo step is available.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Whether a redo step is available.
    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Current undo depth.
    #[must_use]
    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    /// Current redo depth.
    #[must_use]
    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }

    /// Records a newly committed transaction: clears the redo stack and evicts
    /// the oldest undo step past [`UNDO_CAP`].
    pub fn push_committed(&mut self, txn: Txn) {
        self.undo.push(txn);
        self.redo.clear();
        self.enforce_cap();
    }

    /// Pops the newest undo transaction (the session applies its inverses).
    pub fn pop_for_undo(&mut self) -> Option<Txn> {
        self.undo.pop()
    }

    /// Pushes an undone transaction onto the redo stack.
    pub fn push_undone(&mut self, txn: Txn) {
        self.redo.push(txn);
    }

    /// Pops the newest redo transaction (the session replays its forwards).
    pub fn pop_for_redo(&mut self) -> Option<Txn> {
        self.redo.pop()
    }

    /// Pushes a redone transaction back onto the undo stack **without** clearing
    /// redo (redo is a navigation, not a new edit); still caps.
    pub fn push_redone(&mut self, txn: Txn) {
        self.undo.push(txn);
        self.enforce_cap();
    }

    /// Clears both stacks.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    fn enforce_cap(&mut self) {
        while self.undo.len() > UNDO_CAP {
            self.undo.remove(0);
        }
    }
}

// ── Timeline mutation helpers (public-API-only; see `crate::document`) ────────

/// Replaces the record at `index` (record-content edits) and restores the
/// cursor. Uses only the public [`Timeline`] API (rebuild via
/// [`Timeline::from_records`]); regen states reset to `Dirty`, which is
/// irrelevant because they are not persisted.
pub(crate) fn replace_record_at(tl: &mut Timeline, index: usize, record: OperationRecord) {
    let cursor = tl.cursor();
    let mut recs = tl.records().to_vec();
    // Mementos carry the exact index they were captured at, so it is always in
    // range; assert in debug to catch a future memento bug, guard in release.
    debug_assert!(
        index < recs.len(),
        "replace_record_at: index {index} out of bounds (len {})",
        recs.len()
    );
    if index < recs.len() {
        recs[index] = record;
    }
    *tl = Timeline::from_records(recs);
    tl.set_cursor(cursor.min(tl.len()));
}

/// Re-inserts `record` at `index` and restores `cursor` (undo of a removal).
pub(crate) fn insert_record_at(
    tl: &mut Timeline,
    index: usize,
    record: OperationRecord,
    cursor: usize,
) {
    let at = index.min(tl.len());
    tl.set_cursor(at);
    tl.insert_at_cursor(record); // inserts at min(cursor, len) == `at`
    tl.set_cursor(cursor.min(tl.len()));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::DocumentId;
    use uuid::Uuid;

    #[test]
    fn cap_evicts_oldest_and_redo_cleared_on_new_apply() {
        let mut s = UndoStack::new();
        for i in 0..(UNDO_CAP + 5) {
            s.push_committed(Txn {
                label: format!("t{i}"),
                edits: vec![],
            });
        }
        assert_eq!(s.undo_depth(), UNDO_CAP, "capped at 200");
        // Undo one, then a new apply clears redo.
        let t = s.pop_for_undo().unwrap();
        s.push_undone(t);
        assert!(s.can_redo());
        s.push_committed(Txn {
            label: "new".into(),
            edits: vec![],
        });
        assert!(!s.can_redo(), "redo cleared on new apply");
    }

    #[test]
    fn composite_applies_in_reverse() {
        // A composite that inserts then renames a sketch visibility flag; undo
        // must reverse the order. Here we just check apply order via visibility.
        let mut doc = Document::new(DocumentId(Uuid::from_u128(1)));
        let sid = SketchId(Uuid::from_u128(9));
        doc.set_sketch_visible(sid, false);
        let inv = Inverse::Composite(vec![
            Inverse::RestoreSketchVisibility {
                id: sid,
                prior: Some(false),
            },
            Inverse::RestoreSketchVisibility {
                id: sid,
                prior: None,
            },
        ]);
        // Reverse order: the None (remove) is applied first, then Some(false)
        // last → final state visible=false.
        inv.apply(&mut doc);
        assert!(!doc.sketch_visible(sid));
    }
}
