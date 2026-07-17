//! The result of applying an [`EditCommand`](crate::edit::EditCommand): what
//! changed, what regen must recompute, and where.
//!
//! This is deliberately **transport-agnostic** — it names document-level change
//! flags and the ids that moved, NOT a UI DTO. The full projection DTO
//! (frontend-facing) is minted from these deltas in a later WP (R-WP10); the
//! scheduler consumes [`RegenHint`] + [`DirtyRange`] to plan regen.

use crate::history::DirtyRange;
use crate::ids::{BodyId, DatumPlaneId, SketchId};

/// How regen should react to a command (plan "Rust core specifics":
/// `PreviewToStep` on interactive edit, `ToEnd` on commit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegenHint {
    /// No regen needed (metadata-only edit: rename, visibility, …).
    None,
    /// Fast preview regen up to (and including) the given step — the debounced
    /// rollback-edit path.
    PreviewTo(usize),
    /// Full regen to the end of the applied timeline (commit path).
    ToEnd,
}

/// Which parts of the document a command touched. Union-mergeable so a batched
/// transaction reports one combined delta.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionDelta {
    /// The timeline's records changed (insert / remove / params / input / suppression).
    pub timeline_changed: bool,
    /// The rollback cursor moved.
    pub cursor_changed: bool,
    /// Bodies whose metadata/lifecycle changed.
    pub bodies: Vec<BodyId>,
    /// Sketches added / edited / renamed / retargeted / removed.
    pub sketches: Vec<SketchId>,
    /// Datum planes added / changed.
    pub datums: Vec<DatumPlaneId>,
    /// The variable table changed.
    pub variables_changed: bool,
    /// The repair state changed. **Reserved for a later WP** (F12): no edit
    /// command produces a repair delta yet — repair transitions arrive from the
    /// regen/ladder path, wired in a later WP. Kept in the delta shape so the flag
    /// is stable once that path lands.
    pub repair_changed: bool,
    /// A named selection changed. **Reserved for a later WP** (F12): named
    /// selections are modeled ([`NamedSelection`](crate::document::NamedSelection))
    /// but no edit command mutates them yet, so nothing sets this flag today.
    pub named_selections_changed: bool,
}

impl ProjectionDelta {
    /// An empty delta.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A delta marking a single body changed.
    #[must_use]
    pub fn body(id: BodyId) -> Self {
        Self {
            bodies: vec![id],
            ..Self::default()
        }
    }

    /// A delta marking a single sketch changed.
    #[must_use]
    pub fn sketch(id: SketchId) -> Self {
        Self {
            sketches: vec![id],
            ..Self::default()
        }
    }

    /// A delta marking a single datum changed.
    #[must_use]
    pub fn datum(id: DatumPlaneId) -> Self {
        Self {
            datums: vec![id],
            ..Self::default()
        }
    }

    /// A delta marking the timeline records changed.
    #[must_use]
    pub fn timeline() -> Self {
        Self {
            timeline_changed: true,
            ..Self::default()
        }
    }

    /// Unions `other` into `self` (transaction batching). Id lists are merged
    /// without duplicates, preserving first-seen order.
    pub fn merge(&mut self, other: &ProjectionDelta) {
        self.timeline_changed |= other.timeline_changed;
        self.cursor_changed |= other.cursor_changed;
        self.variables_changed |= other.variables_changed;
        self.repair_changed |= other.repair_changed;
        self.named_selections_changed |= other.named_selections_changed;
        merge_unique(&mut self.bodies, &other.bodies);
        merge_unique(&mut self.sketches, &other.sketches);
        merge_unique(&mut self.datums, &other.datums);
    }

    /// True iff nothing changed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.timeline_changed
            && !self.cursor_changed
            && !self.variables_changed
            && !self.repair_changed
            && !self.named_selections_changed
            && self.bodies.is_empty()
            && self.sketches.is_empty()
            && self.datums.is_empty()
    }
}

fn merge_unique<T: Copy + PartialEq>(dst: &mut Vec<T>, src: &[T]) {
    for &item in src {
        if !dst.contains(&item) {
            dst.push(item);
        }
    }
}

/// The outcome of one applied command (or one committed transaction).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutcome {
    /// What changed in the document.
    pub projection_delta: ProjectionDelta,
    /// The contiguous span of timeline steps whose regen result was invalidated
    /// (`None` for edits that touch no timeline step — rename/visibility/…).
    pub dirty: Option<DirtyRange>,
    /// How the scheduler should regenerate.
    pub regen: RegenHint,
}

impl CommandOutcome {
    /// An outcome for a metadata-only edit: no dirty span, no regen.
    #[must_use]
    pub fn metadata_only(projection_delta: ProjectionDelta) -> Self {
        Self {
            projection_delta,
            dirty: None,
            regen: RegenHint::None,
        }
    }

    /// An outcome that dirties `dirty` and regenerates to the end (commit path).
    #[must_use]
    pub fn dirty_to_end(projection_delta: ProjectionDelta, dirty: DirtyRange) -> Self {
        Self {
            projection_delta,
            dirty: Some(dirty),
            regen: RegenHint::ToEnd,
        }
    }
}
