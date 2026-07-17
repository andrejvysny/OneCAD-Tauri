//! v2 `.onecad` container IO, per-section codecs, migration and crash recovery.
//!
//! # Layout (v2.0)
//!
//! A `.onecad` file is a ZIP archive with this section layout:
//!
//! ```text
//! manifest.json          index: magic, versions, documentId, opsHash, entry hashes
//! document.json          DocumentData — the AUTHORITATIVE payload
//! timeline/ops.jsonl     one OperationRecord per line — a DERIVED readable projection
//! geometry/<bodyId>.brep opaque BREP cache (from the worker)
//! meshes/<bodyId>.<lod>.mesh   MESH1 cache
//! checkpoints/<step>.json + .bin   optional checkpoint artifacts
//! preview.png            optional thumbnail
//! ```
//!
//! ## Design decisions (flagged for orchestrator review)
//!
//! * **`document.json` is the single source of truth; `timeline/ops.jsonl` is a
//!   derived, human-readable projection.** On load both are read and
//!   cross-validated; on any divergence `document.json` wins and a `Warning`
//!   diagnostic is emitted (never an error). Rationale: a single authoritative
//!   payload avoids the dual-source reconciliation problem — the timeline records
//!   already live inside `document.json` (`DocumentData.timeline.records`), so
//!   splitting them into a second authoritative section would create two things
//!   that must agree. `ops.jsonl` exists only so the history is greppable/diffable
//!   outside the app.
//! * **Sketches stay INLINE in `document.json`; the v2.0 container has NO
//!   `sketches/` directory.** The plan sketched a `sketches/<uuid>.json` layout,
//!   but [`Document`](crate::document::Document) holds sketches inline in a
//!   `BTreeMap<SketchId, Sketch>`, so extracting them would (a) duplicate a source
//!   of truth and (b) churn the already-frozen `document.json` shape. Divergence
//!   from the plan is deliberate and recorded here + in [`sketch_io`]. Flag for
//!   orchestrator review.
//!
//! ## File attack surface (Codex red-team; plan "File attack surface")
//!
//! Every entry read is bounded. The container is treated as adversarial input;
//! a malformed or hostile archive yields a typed [`IoError`], **never a panic**.
//! See [`container`] for the caps table (per-section decompression caps, total
//! container cap, entry-count cap) and the zip path-traversal guard. JSON nesting
//! depth is bounded by `serde_json`'s default recursion limit (128), which returns
//! an `Err` rather than overflowing the stack (verified by
//! `hostile_deeply_nested_json_errors`). Hostile STEP is out of scope here — it is
//! handled in the isolated worker.

pub mod container;
pub mod document_io;
pub mod history_io;
pub mod manifest;
pub mod migrate;
pub mod recovery;
pub mod sketch_io;

use thiserror::Error;

/// Typed failures raised by the container IO layer.
///
/// Hostile or malformed input maps to one of these variants — the layer never
/// panics on adversarial bytes (plan "File attack surface"; test
/// `hostile_*`). `NeedsRepair` is a document *state*, never an IO error (see
/// [`crate::error`]).
#[derive(Debug, Error)]
pub enum IoError {
    /// Filesystem / zip transport failure.
    #[error("io: {0}")]
    Io(String),

    /// The manifest magic string was absent or not `"ONECAD"`.
    #[error("bad magic: not a OneCAD container")]
    BadMagic,

    /// The container version is outside the supported range (v2 exact for now).
    #[error("unsupported container version {found} (this build reads {expected})")]
    UnsupportedVersion {
        /// The version found in the manifest.
        found: u32,
        /// The version this build reads.
        expected: u32,
    },

    /// The archive, an authoritative section, or an authoritative entry hash is
    /// corrupt / malformed.
    #[error("corrupt container: {0}")]
    Corrupt(String),

    /// A decompressed section, or the whole container, exceeded its size cap
    /// (decompression-bomb guard).
    #[error("too large: {0}")]
    TooLarge(String),

    /// A zip entry name escaped the archive root (`../`, absolute path, or a
    /// path the platform would resolve outside the extraction dir).
    #[error("path traversal blocked: {0}")]
    PathTraversal(String),
}

/// Convenience result alias for the container IO layer.
pub type IoResult<T> = Result<T, IoError>;

impl From<std::io::Error> for IoError {
    fn from(e: std::io::Error) -> Self {
        IoError::Io(e.to_string())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Diagnostics
// ─────────────────────────────────────────────────────────────────────────────

/// Severity of a non-fatal load [`Diagnostic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Informational (e.g. a lossless migration ran).
    Info,
    /// Something is off but the load continues (e.g. `ops.jsonl` diverged, caches
    /// went stale, a low-confidence migration forced read-only).
    Warning,
}

/// A non-fatal observation surfaced during a load. Diagnostics never fail the
/// load; they are collected and reported to the app (plan §9 "guided migration
/// report" + the cache/ops.jsonl reconciliation warnings).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// How serious the observation is.
    pub severity: Severity,
    /// A stable machine code (e.g. `"ops-jsonl-divergence"`) for tests / UI.
    pub code: &'static str,
    /// Human-facing detail.
    pub message: String,
}

impl Diagnostic {
    /// A `Warning`-severity diagnostic.
    #[must_use]
    pub fn warning(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code,
            message: message.into(),
        }
    }

    /// An `Info`-severity diagnostic.
    #[must_use]
    pub fn info(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Info,
            code,
            message: message.into(),
        }
    }
}

/// Lowercase-hex-encodes bytes (SHA-256 digests, entry/ops hashes). Shared by the
/// IO codecs so the file format has one hex convention.
#[must_use]
pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

/// SHA-256 of `bytes` as a lowercase-hex string (the file format's content-hash
/// convention; SCHEMA §2).
#[must_use]
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}
