//! Typed event channel names emitted by the backend to the webview.
//!
//! Projection stores in the frontend are written ONLY by these events (plan
//! "Frontend owns projection stores"). Payload shapes live in [`crate::dto`]:
//! [`DocumentChange`](crate::dto::DocumentChange) for [`DOCUMENT_CHANGED`],
//! [`DocumentProjection`](crate::dto::DocumentProjection) for [`PROJECTION_UPDATED`].

/// The active document's projection changed — carries the full
/// [`DocumentProjection`](crate::dto::DocumentProjection). Drives the document /
/// history / sketch stores (they are re-hydrated from one authoritative payload).
pub const PROJECTION_UPDATED: &str = "projection-updated";

/// A regen published new geometry — carries a
/// [`DocumentChange`](crate::dto::DocumentChange) (`{revision, changedBodies,
/// removedBodies}`) so the viewport pull-fetches meshes for visible bodies.
pub const DOCUMENT_CHANGED: &str = "document-changed";

/// Incremental regen progress for the current job (reserved; R-WP11 fills it).
pub const REGEN_PROGRESS: &str = "regen-progress";

/// The current regen job finished (published or discarded).
pub const REGEN_FINISHED: &str = "regen-finished";

/// One or more references need user repair.
pub const NEEDS_REPAIR: &str = "needs-repair";

/// Worker lifecycle status changed (starting/ready/failed) — R-WP11.
pub const WORKER_STATUS: &str = "worker-status";

/// The selection set changed.
pub const SELECTION_CHANGED: &str = "selection-changed";

/// An autosave completed (or a crash marker was written).
pub const AUTOSAVE: &str = "autosave";
