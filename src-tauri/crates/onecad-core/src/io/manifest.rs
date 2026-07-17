//! Container manifest (`manifest.json`) — the archive index.
//!
//! The manifest carries the format identity (magic + versions), document identity
//! and provenance timestamps (RFC3339, supplied by the caller — the pure core does
//! not read the wall clock), the `opsHash` cache-freshness token, and a per-entry
//! SHA-256 table used to verify section integrity on read.
//!
//! Verification split (plan task 3): a hash mismatch on an **authoritative** entry
//! (`document.json`, `timeline/ops.jsonl`) is [`IoError::Corrupt`] — the payload is
//! not trustworthy; a mismatch on a **cache** entry (geometry/meshes/checkpoints/
//! preview) is a *stale cache*, reported and skipped (Invariant 7: a cache
//! degrades performance, never correctness).

use serde::{Deserialize, Serialize};

use crate::document::refs::Extra;
use crate::ids::DocumentId;

use super::{IoError, IoResult};

/// The container magic string stamped in every manifest.
pub const MAGIC: &str = "ONECAD";

/// The container format version this build reads/writes (exact match required for
/// now — a different version is [`IoError::UnsupportedVersion`]).
pub const CONTAINER_VERSION: u32 = 2;

/// The global document-schema version this build authors. Older values trigger a
/// migration chain; newer values open read-only (see [`super::migrate`]).
pub const GLOBAL_SCHEMA_VERSION: u32 = 1;

/// The authoritative section path. A hash mismatch on it is [`IoError::Corrupt`]
/// (the payload is untrustworthy).
///
/// `timeline/ops.jsonl` is deliberately **not** here: it is a derived projection,
/// so a hash mismatch or content divergence downgrades to a `Warning` and
/// `document.json` wins (see [`super::history_io::cross_validate`]). Everything
/// else (`geometry/`, `meshes/`, `checkpoints/`, `preview.png`) is a cache — a
/// mismatch there is a *stale cache*, skipped (Invariant 7).
pub const AUTHORITATIVE_PATHS: [&str; 1] = [super::container::DOCUMENT_PATH];

/// One archive entry's integrity record: its path and the SHA-256 (lowercase hex)
/// of its uncompressed bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestEntry {
    /// Archive-relative path (forward slashes, never absolute, never `..`).
    pub path: String,
    /// SHA-256 of the entry's uncompressed bytes, lowercase hex.
    pub sha256: String,
}

/// Top-level container manifest (`manifest.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Format magic — must equal [`MAGIC`].
    pub magic: String,
    /// Container format version — must equal [`CONTAINER_VERSION`].
    pub container_version: u32,
    /// Global document-schema version of the stored `document.json`.
    pub global_schema_version: u32,
    /// The authoring app version (provenance only).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub app_version: String,
    /// The OCCT fingerprint the caches were produced under, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occt_fingerprint: Option<String>,
    /// The stored document's identity (must match `document.json`'s `id`).
    pub document_id: DocumentId,
    /// RFC3339 creation timestamp (caller-supplied).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created: String,
    /// RFC3339 last-modified timestamp (caller-supplied).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub modified: String,
    /// SHA-256 (hex) of the canonical timeline-records JSON — the cache-freshness
    /// token (see [`super::history_io::ops_hash`]). Caches are valid only while
    /// this matches the loaded document's records.
    pub ops_hash: String,
    /// Per-entry integrity table (excludes `manifest.json` itself).
    #[serde(default)]
    pub entries: Vec<ManifestEntry>,
    /// Unknown top-level keys, preserved verbatim (forward-compat).
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

impl Manifest {
    /// Validates the format identity (magic + container version).
    ///
    /// # Errors
    /// * [`IoError::BadMagic`] if the magic is not [`MAGIC`].
    /// * [`IoError::UnsupportedVersion`] if the container version is not
    ///   [`CONTAINER_VERSION`].
    pub fn validate_identity(&self) -> IoResult<()> {
        if self.magic != MAGIC {
            return Err(IoError::BadMagic);
        }
        if self.container_version != CONTAINER_VERSION {
            return Err(IoError::UnsupportedVersion {
                found: self.container_version,
                expected: CONTAINER_VERSION,
            });
        }
        Ok(())
    }

    /// The manifest entry for `path`, if present.
    #[must_use]
    pub fn entry(&self, path: &str) -> Option<&ManifestEntry> {
        self.entries.iter().find(|e| e.path == path)
    }

    /// True iff `path` is one of the authoritative (non-cache) sections.
    #[must_use]
    pub fn is_authoritative(path: &str) -> bool {
        AUTHORITATIVE_PATHS.contains(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn manifest() -> Manifest {
        Manifest {
            magic: MAGIC.into(),
            container_version: CONTAINER_VERSION,
            global_schema_version: GLOBAL_SCHEMA_VERSION,
            app_version: "0.1.0".into(),
            occt_fingerprint: None,
            document_id: DocumentId(Uuid::from_u128(1)),
            created: "2026-07-16T00:00:00Z".into(),
            modified: "2026-07-16T00:00:00Z".into(),
            ops_hash: "abc".into(),
            entries: vec![ManifestEntry {
                path: super::super::container::DOCUMENT_PATH.into(),
                sha256: "dd".into(),
            }],
            extra: Extra::new(),
        }
    }

    #[test]
    fn validate_identity_accepts_good_manifest() {
        assert!(manifest().validate_identity().is_ok());
    }

    #[test]
    fn validate_identity_rejects_bad_magic() {
        let mut m = manifest();
        m.magic = "NOPE".into();
        assert!(matches!(m.validate_identity(), Err(IoError::BadMagic)));
    }

    #[test]
    fn validate_identity_rejects_wrong_version() {
        let mut m = manifest();
        m.container_version = 99;
        assert!(matches!(
            m.validate_identity(),
            Err(IoError::UnsupportedVersion {
                found: 99,
                expected: 2
            })
        ));
    }

    #[test]
    fn authoritative_classification() {
        // Only document.json is integrity-critical; ops.jsonl is a derived
        // projection, caches are caches.
        assert!(Manifest::is_authoritative("document.json"));
        assert!(!Manifest::is_authoritative("timeline/ops.jsonl"));
        assert!(!Manifest::is_authoritative("geometry/x.brep"));
    }

    #[test]
    fn manifest_round_trips_and_preserves_unknown_keys() {
        let json = r#"{
            "magic":"ONECAD","containerVersion":2,"globalSchemaVersion":1,
            "documentId":"00000000-0000-0000-0000-000000000001",
            "opsHash":"abc","entries":[],"futureKey":{"x":1}
        }"#;
        let m: Manifest = serde_json::from_str(json).unwrap();
        assert!(m.extra.contains_key("futureKey"));
        let reser = serde_json::to_value(&m).unwrap();
        assert_eq!(reser["futureKey"], serde_json::json!({"x":1}));
    }
}
