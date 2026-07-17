//! Typed event channel names emitted by the backend to the webview.
//!
//! Projection stores in the frontend are written ONLY by these events.

/// The active document changed (carries `{body_id, mesh_key}` for pull fetch).
pub const DOCUMENT_CHANGED: &str = "document-changed";

/// Incremental regen progress for the current job.
pub const REGEN_PROGRESS: &str = "regen-progress";

/// The current regen job finished (published or discarded).
pub const REGEN_FINISHED: &str = "regen-finished";

/// One or more references need user repair.
pub const NEEDS_REPAIR: &str = "needs-repair";

/// Worker lifecycle status changed (starting/ready/failed).
pub const WORKER_STATUS: &str = "worker-status";

/// The selection set changed.
pub const SELECTION_CHANGED: &str = "selection-changed";

/// An autosave completed (or a crash marker was written).
pub const AUTOSAVE: &str = "autosave";
