//! History: the linear timeline plus a derived dependency graph.
//!
//! * [`timeline`] — the authoritative strict-linear [`Timeline`] with a rollback
//!   cursor (= "applied op count") and a per-step [`StepState`] (V1/V2 §0.3,
//!   §5). Rollback is a cursor move, NOT the C++ suppression conflation
//!   (plan "Rust core specifics").
//! * [`graph`] — a hand-rolled [`DependencyGraph`] port of the OneCAD-CPP
//!   `DependencyGraph` (deterministic Kahn tie-break by creation index,
//!   `produces_before` anti-time-travel, suppression propagation, failure
//!   tracking).
//!
//! [`DirtyRange`] lives here (not in `edit/outcome.rs`, which the plan reserves
//! for the richer `EditOutcome` dirty-closure landed in a later WP). Cursor
//! moves, inserts and removes return a `DirtyRange` describing the contiguous
//! span of steps regen must recompute.

pub mod graph;
pub mod timeline;

pub use graph::DependencyGraph;
pub use timeline::{StepState, Timeline};

/// A contiguous, half-open span of timeline steps `[from, to)` whose regen
/// result was invalidated by a mutation (insert / remove / cursor move).
///
/// `to` is exclusive and is usually `records.len()` (dirtiness cascades to the
/// tail of the timeline). An empty range (`from == to`) means "nothing changed".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRange {
    /// First affected step (inclusive).
    pub from: usize,
    /// One past the last affected step (exclusive).
    pub to: usize,
}

impl DirtyRange {
    /// A range `[from, to)`. Normalizes an inverted input to empty.
    #[must_use]
    pub fn new(from: usize, to: usize) -> Self {
        Self {
            from,
            to: to.max(from),
        }
    }

    /// The empty range anchored at `at` (`[at, at)`).
    #[must_use]
    pub fn empty(at: usize) -> Self {
        Self { from: at, to: at }
    }

    /// True iff the range covers no steps.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.from >= self.to
    }

    /// Number of steps in the range.
    #[must_use]
    pub fn len(&self) -> usize {
        self.to.saturating_sub(self.from)
    }

    /// True iff `step` is inside the range.
    #[must_use]
    pub fn contains(&self, step: usize) -> bool {
        step >= self.from && step < self.to
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_range_basics() {
        let r = DirtyRange::new(2, 5);
        assert_eq!(r.len(), 3);
        assert!(!r.is_empty());
        assert!(r.contains(2) && r.contains(4) && !r.contains(5));
        assert!(DirtyRange::empty(3).is_empty());
        // Inverted inputs normalize to empty rather than panicking.
        assert!(DirtyRange::new(5, 2).is_empty());
    }
}
