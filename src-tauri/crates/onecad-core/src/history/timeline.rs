//! Strict-linear operation timeline with a rollback cursor.
//!
//! The timeline is the authoritative ordered list of [`OperationRecord`]s
//! (V1/V2 §0.3: "Timeline is linear"). Two orthogonal pieces of state live
//! alongside the records:
//!
//! * `cursor` — the **applied op count** (OneCAD-CPP `Document::appliedOpCount_`,
//!   `Document.cpp:918-940`). Records `[0, cursor)` are *applied* (part of the
//!   active regen); records `[cursor, len)` are *drafts* that live beyond the
//!   rollback bar without being deleted. `cursor ≤ len` is a hard invariant.
//!   Rollback is a cursor move (plan "Rust core specifics": *rollback = cursor,
//!   NOT the C++ suppression conflation*).
//! * `states` — one [`StepState`] per record (V1/V2 §5.1 timeline UI states).
//!   `states.len() == records.len()` is a hard invariant, enforced at every
//!   mutation.
//!
//! Insert-at-rollback (V1/V2 §5.3) is [`Timeline::insert_at_cursor`]: a new node
//! is inserted at the cursor and the applied prefix grows to include it; steps
//! after it become [`StepState::Dirty`] and are no longer editable
//! ([`Timeline::is_editable`]).

use crate::document::record::OperationRecord;
use crate::error::DomainError;
use crate::ids::RecordId;

use super::DirtyRange;

/// State of a single timeline step (V1/V2 §5.1).
///
/// `NeedsRepair` is a state, never an `Err` (plan error taxonomy;
/// `crate::error`). `Suppressed` steps are intentionally skipped by regen and
/// are preserved across dirty-marking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepState {
    /// Regenerated successfully and up to date.
    Valid,
    /// Pending regeneration (inserted / edited / downstream of a change).
    Dirty,
    /// The op failed to regenerate; carries a human-facing reason.
    Error {
        /// Why the step failed (worker / validation message).
        reason: String,
    },
    /// A reference could not be bound with confidence (resolution ladder gave
    /// up). Distinct from `Error` — the model is intact, the binding is not.
    NeedsRepair,
    /// The op is suppressed and skipped during regen.
    Suppressed,
}

/// The strict-linear operation timeline: ordered records + rollback cursor +
/// per-step states.
#[derive(Debug, Default, Clone)]
pub struct Timeline {
    records: Vec<OperationRecord>,
    /// Applied op count: `records[0, cursor)` applied, `[cursor, len)` drafts.
    cursor: usize,
    states: Vec<StepState>,
}

impl Timeline {
    /// An empty timeline (cursor 0, no records).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a timeline from a loaded record list. The cursor is placed at the
    /// end (all records applied) and every step starts [`StepState::Dirty`]
    /// (a freshly loaded document must be regenerated before it is trusted).
    #[must_use]
    pub fn from_records(records: Vec<OperationRecord>) -> Self {
        let cursor = records.len();
        let states = vec![StepState::Dirty; records.len()];
        Self {
            records,
            cursor,
            states,
        }
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    /// Number of records (applied + drafts).
    #[must_use]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// True iff there are no records.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// The applied op count (rollback cursor). `records[0, cursor)` are applied.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// All records in timeline order.
    #[must_use]
    pub fn records(&self) -> &[OperationRecord] {
        &self.records
    }

    /// All step states in timeline order (parallel to [`Timeline::records`]).
    #[must_use]
    pub fn states(&self) -> &[StepState] {
        &self.states
    }

    /// The record at `index`, if in range.
    #[must_use]
    pub fn record(&self, index: usize) -> Option<&OperationRecord> {
        self.records.get(index)
    }

    /// The record with the given id, if present.
    #[must_use]
    pub fn record_by_id(&self, id: RecordId) -> Option<&OperationRecord> {
        self.index_of(id).map(|i| &self.records[i])
    }

    /// The timeline index of a record id, if present.
    #[must_use]
    pub fn index_of(&self, id: RecordId) -> Option<usize> {
        self.records.iter().position(|r| r.record_id == id)
    }

    /// The state of step `step`, if in range.
    #[must_use]
    pub fn state(&self, step: usize) -> Option<&StepState> {
        self.states.get(step)
    }

    /// Whether `step` may be edited. Editing is allowed only within the applied
    /// prefix (`step < cursor`); draft/dirty steps beyond the rollback bar are
    /// disabled until valid (V1/V2 §5.3: *"editing later steps disabled until
    /// valid"*).
    #[must_use]
    pub fn is_editable(&self, step: usize) -> bool {
        step < self.cursor && step < self.records.len()
    }

    // ── Mutations ────────────────────────────────────────────────────────────

    /// Inserts a record at the rollback cursor and returns its new index
    /// (OneCAD-CPP `AddOperationCommand::execute`,
    /// `AddOperationCommand.cpp:33-37`: `insertIndex = min(appliedOpCount, len)`;
    /// `setAppliedOpCount(insertIndex + 1)`).
    ///
    /// Effects (V1/V2 §5.3 insert-at-rollback): the new node is placed at
    /// `cursor`, the applied prefix grows to include it (`cursor += 1`), and the
    /// inserted node plus every step after it become [`StepState::Dirty`]
    /// (pending regen). `Suppressed` steps keep their state.
    pub fn insert_at_cursor(&mut self, record: OperationRecord) -> usize {
        let index = self.cursor.min(self.records.len());
        self.records.insert(index, record);
        self.states.insert(index, StepState::Dirty);
        // Applied prefix now covers the inserted op (cpp: setAppliedOpCount(idx+1)).
        self.cursor = index + 1;
        // Inserted node + tail are pending (spec §5.3: steps after become Dirty;
        // the inserted node is new so it is Dirty too).
        self.mark_dirty_from_internal(index);
        debug_assert_eq!(self.records.len(), self.states.len());
        index
    }

    /// Moves the rollback cursor to `k` (clamped to `[0, len]`) and returns the
    /// span of steps whose applied-status changed.
    ///
    /// * Forward (`k > cursor`) promotes drafts `[cursor, k)` into the applied
    ///   prefix; they are marked [`StepState::Dirty`] (need regen). This is the
    ///   "regen-to-end recovers drafts" path (OneCAD-CPP
    ///   `setAppliedOpCount(operations().size())`).
    /// * Backward (`k < cursor`) is a rollback: steps `[k, cursor)` leave the
    ///   applied prefix and become drafts (their [`StepState`] is left intact —
    ///   rollback discards their live bodies without editing the records).
    pub fn set_cursor(&mut self, k: usize) -> DirtyRange {
        let k = k.min(self.records.len());
        let old = self.cursor;
        self.cursor = k;
        if k > old {
            for s in old..k {
                self.set_dirty_preserving_suppressed(s);
            }
            DirtyRange::new(old, k)
        } else if k < old {
            DirtyRange::new(k, old)
        } else {
            DirtyRange::empty(k)
        }
    }

    /// Marks `step` and every later step [`StepState::Dirty`] (skipping
    /// `Suppressed` steps) and returns the affected span. This is the
    /// dirty-cascade primitive used when an upstream step changes.
    pub fn mark_dirty_from(&mut self, step: usize) -> DirtyRange {
        self.mark_dirty_from_internal(step)
    }

    /// Sets the state of a single step.
    ///
    /// # Errors
    /// [`DomainError::Timeline`] if `step` is out of range.
    pub fn mark_state(&mut self, step: usize, state: StepState) -> Result<(), DomainError> {
        if step >= self.states.len() {
            return Err(DomainError::Timeline(format!(
                "mark_state: step {step} out of bounds (len {})",
                self.states.len()
            )));
        }
        self.states[step] = state;
        Ok(())
    }

    /// Removes the record with the given id, keeping the cursor consistent
    /// (OneCAD-CPP `Document::removeOperation`, `Document.cpp:966-990`: decrement
    /// the applied cursor iff the removed index is inside the applied prefix,
    /// then clamp to `len`). Returns the span invalidated by the removal
    /// (`[removed_index, new_len)`), with those steps marked [`StepState::Dirty`].
    ///
    /// # Errors
    /// [`DomainError::RecordNotFound`] if no record has that id.
    pub fn remove(&mut self, record_id: RecordId) -> Result<DirtyRange, DomainError> {
        let index = self
            .index_of(record_id)
            .ok_or(DomainError::RecordNotFound(record_id))?;
        self.records.remove(index);
        self.states.remove(index);
        // cpp:978-980: only shift the cursor when the removed op was applied.
        if index < self.cursor && self.cursor > 0 {
            self.cursor -= 1;
        }
        // cpp:981-983: clamp.
        if self.cursor > self.records.len() {
            self.cursor = self.records.len();
        }
        let dirty = self.mark_dirty_from_internal(index);
        debug_assert_eq!(self.records.len(), self.states.len());
        Ok(dirty)
    }

    /// Validates the timeline invariants: `states.len() == records.len()` and
    /// `cursor ≤ len`.
    ///
    /// # Errors
    /// [`DomainError::Timeline`] describing the first violated invariant.
    pub fn validate(&self) -> Result<(), DomainError> {
        if self.states.len() != self.records.len() {
            return Err(DomainError::Timeline(format!(
                "states.len()={} != records.len()={}",
                self.states.len(),
                self.records.len()
            )));
        }
        if self.cursor > self.records.len() {
            return Err(DomainError::Timeline(format!(
                "cursor {} exceeds len {}",
                self.cursor,
                self.records.len()
            )));
        }
        Ok(())
    }

    // ── Internals ────────────────────────────────────────────────────────────

    fn mark_dirty_from_internal(&mut self, step: usize) -> DirtyRange {
        let start = step.min(self.records.len());
        for s in start..self.records.len() {
            self.set_dirty_preserving_suppressed(s);
        }
        DirtyRange::new(start, self.records.len())
    }

    fn set_dirty_preserving_suppressed(&mut self, step: usize) {
        // A suppressed step is intentionally skipped by regen; it is not "dirty".
        if self.states[step] != StepState::Suppressed {
            self.states[step] = StepState::Dirty;
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
    use crate::ids::BodyId;
    use uuid::Uuid;

    fn extrude_record(seed: u128, distance: f64) -> OperationRecord {
        let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
            profile: None,
            distance: Scalar::new(distance),
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
        let mut rec = OperationRecord::new(RecordId(Uuid::from_u128(seed)), 0, "Extrude", op);
        rec.outputs = vec![BodyId(Uuid::from_u128(0xB000 + seed))];
        rec
    }

    #[test]
    fn insert_grows_applied_prefix_and_dirties_tail() {
        let mut tl = Timeline::new();
        let i0 = tl.insert_at_cursor(extrude_record(1, 10.0));
        let i1 = tl.insert_at_cursor(extrude_record(2, 5.0));
        assert_eq!((i0, i1), (0, 1));
        assert_eq!(tl.cursor(), 2);
        assert_eq!(tl.len(), 2);
        tl.validate().unwrap();
    }

    #[test]
    fn is_editable_tracks_applied_prefix() {
        let mut tl = Timeline::new();
        tl.insert_at_cursor(extrude_record(1, 10.0));
        tl.insert_at_cursor(extrude_record(2, 5.0));
        tl.set_cursor(1); // rollback: op index 1 becomes a draft
        assert!(tl.is_editable(0));
        assert!(!tl.is_editable(1)); // draft beyond the cursor is disabled
    }

    #[test]
    fn mark_state_out_of_bounds_errs() {
        let mut tl = Timeline::new();
        assert!(tl.mark_state(0, StepState::Valid).is_err());
    }
}
