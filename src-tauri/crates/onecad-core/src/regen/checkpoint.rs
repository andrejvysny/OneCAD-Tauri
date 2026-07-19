//! Regen checkpoints and their versioned envelopes (SCHEMA §7.7; V1/V2 §4.2).
//!
//! A checkpoint is an **atomic artifact set** for a step (BREP blobs via
//! BinTools, the ElementMap partition JSON, the three signatures and the
//! `historyPrefixHash`), each wrapped in a Rust-readable [`CheckpointEnvelope`].
//! The core stores the **opaque bytes plus the parsed envelope metadata** (SCHEMA
//! §7.7: "core stores bytes + parsed envelope metadata").
//!
//! **Checkpoints are disposable caches (Invariant 7).** An envelope whose
//! versions/fingerprint are incompatible, or whose stored history-prefix hash no
//! longer matches the timeline, is discarded — the planner falls back to an
//! earlier checkpoint or replays from empty. A checkpoint never blocks opening
//! the authoritative JSON, and **correctness never depends on the cache**.

use std::collections::BTreeMap;

use crate::document::body::BodyRegistry;
use crate::document::element_index::ElementIndex;
use crate::ids::{BodyId, SnapshotId};

use super::engine::StepSignatures;
use super::planner::{HistoryPrefixHash, PlanContext};

/// The current checkpoint artifact-schema version (SCHEMA §7.7
/// `artifactSchemaVersion`).
pub const ARTIFACT_SCHEMA_VERSION: u32 = 1;

/// A checkpoint id (SCHEMA §7.7 `checkpointId`, e.g. `"ckpt_9"`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CheckpointId(pub String);

impl CheckpointId {
    /// Wraps a checkpoint id string.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// The raw id string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A reference to a checkpoint used as a plan's base (SCHEMA §7.2
/// `baseCheckpoint`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRef {
    pub step_index: usize,
    pub checkpoint_id: CheckpointId,
}

/// A checkpoint artifact envelope (SCHEMA §7.7). Per-artifact (per body), but the
/// **version axes and fingerprint are shared** across a checkpoint's artifacts,
/// so a single representative envelope drives compatibility in [`CheckpointMeta`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckpointEnvelope {
    pub artifact_schema_version: u32,
    /// The body this artifact serializes.
    pub body: BodyId,
    pub step: usize,
    /// History-prefix hash of the checkpoint's base state.
    pub history_prefix_hash: HistoryPrefixHash,
    pub brep_content_hash: String,
    /// OCCT fingerprint (governs BREP/checkpoint compatibility).
    pub occt_fingerprint: String,
    pub descriptor_version: u32,
    pub resolver_version: u32,
    pub quantization_version: u32,
    pub signature_version: u32,
    /// The codec that produced the bytes (e.g. `"brep-bintools"`).
    pub codec: String,
    pub size: u64,
    /// Content hash of the bytes (integrity).
    pub content_hash: String,
}

impl CheckpointEnvelope {
    /// Whether this envelope is compatible with the current policy/fingerprint
    /// (SCHEMA §7.7 / §13). An incompatible envelope is discarded — the cache
    /// degrades to replay, never to a wrong result (Invariant 7).
    #[must_use]
    pub fn is_compatible(&self, ctx: &PlanContext) -> bool {
        self.artifact_schema_version == ARTIFACT_SCHEMA_VERSION
            && self.occt_fingerprint == ctx.occt_fingerprint
            && self.descriptor_version == ctx.policy_versions.descriptor
            && self.resolver_version == ctx.policy_versions.resolver
            && self.quantization_version == ctx.policy_versions.quantization
            && self.signature_version == ctx.policy_versions.signature
    }
}

/// One stored artifact: the parsed envelope + the opaque bytes (SCHEMA §7.7 —
/// "core stores bytes + parsed envelope metadata").
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckpointArtifact {
    pub envelope: CheckpointEnvelope,
    /// The opaque codec bytes (BREP blob, etc.). The core never interprets them.
    pub bytes: Vec<u8>,
}

/// The atomic artifact set emitted by `SaveCheckpoint` (SCHEMA §7.7).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CheckpointArtifacts {
    pub step: usize,
    /// Per-body artifacts (BREP + envelope).
    pub artifacts: Vec<CheckpointArtifact>,
    /// The ElementMap partition JSON bytes (SCHEMA §7.7 `elementMapPartition`).
    pub element_map_partition: Vec<u8>,
    /// The three step signatures at this checkpoint.
    pub signatures: StepSignatures,
    /// History-prefix hash of the checkpoint's base state.
    pub history_prefix_hash: HistoryPrefixHash,
}

impl CheckpointArtifacts {
    /// A representative envelope for compatibility checks (the first artifact's).
    /// All of a checkpoint's artifacts share version axes + fingerprint, so any
    /// one is representative. `None` for a checkpoint with no BREP artifacts.
    #[must_use]
    pub fn representative_envelope(&self) -> Option<&CheckpointEnvelope> {
        self.artifacts.first().map(|a| &a.envelope)
    }
}

/// Cache-level metadata for a stored checkpoint (SCHEMA §7.7 — what the planner
/// browses to pick a base). Carries the representative envelope for compatibility
/// and the stored history-prefix hash for staleness detection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointMeta {
    pub id: CheckpointId,
    pub step: usize,
    pub history_prefix_hash: HistoryPrefixHash,
    pub envelope: CheckpointEnvelope,
}

/// Which signature drifted on restore (SCHEMA §7.7 `driftDetail.signature`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftSignature {
    Geometry,
    BodyLifecycle,
    ReferencedBinding,
}

/// Restore drift detail (SCHEMA §7.7 `driftDetail`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftDetail {
    pub signature: DriftSignature,
    pub expected: String,
    pub actual: String,
}

/// `RestoreCheckpoint` result (SCHEMA §7.7).
///
/// Carries the **reconstructed base state** so the executor seeds its scratch
/// from the checkpoint artifacts, not from live session state (review F3): the
/// worker restores the BREP and the ElementMap partition, then returns the body
/// registry and element index they represent. The `base_registry` lifecycle log
/// ends at `checkpoint_step`, so folding the plan's remaining steps onto it
/// appends (never duplicates — the body.rs clear-before-replay contract). A
/// `restored == false` or `drift_detected == true` means the checkpoint is
/// unusable; the executor then falls back to replay-from-0 (review F12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestoreResult {
    pub restored: bool,
    pub snapshot_id: SnapshotId,
    pub drift_detected: bool,
    pub drift_detail: Option<DriftDetail>,
    /// The step the checkpoint represents (its base covers steps `0..=step`).
    pub checkpoint_step: usize,
    /// The restored body registry (log truncated to `checkpoint_step`).
    pub base_registry: BodyRegistry,
    /// The restored element partition index at the checkpoint.
    pub base_elements: ElementIndex,
}

impl RestoreResult {
    /// A successful, drift-free restore of `step` carrying the reconstructed base
    /// registry + element index.
    #[must_use]
    pub fn ok(
        snapshot_id: SnapshotId,
        step: usize,
        base_registry: BodyRegistry,
        base_elements: ElementIndex,
    ) -> Self {
        Self {
            restored: true,
            snapshot_id,
            drift_detected: false,
            drift_detail: None,
            checkpoint_step: step,
            base_registry,
            base_elements,
        }
    }
}

/// A checkpoint loaded from the store (its full artifact set + meta).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCheckpoint {
    pub meta: CheckpointMeta,
    pub artifacts: CheckpointArtifacts,
}

// ─────────────────────────────────────────────────────────────────────────────
// Store
// ─────────────────────────────────────────────────────────────────────────────

/// A checkpoint cache. Implementations are disposable stores keyed by step; the
/// planner reads [`list`](CheckpointStore::list) to pick an accelerated base.
pub trait CheckpointStore {
    /// All stored checkpoint metadata (order unspecified; the planner picks by
    /// step + compatibility).
    fn list(&self) -> Vec<CheckpointMeta>;

    /// Stores a checkpoint's artifacts at `step`, returning its minted id. A
    /// later save at the same step supersedes the earlier one.
    fn save(&mut self, step: usize, artifacts: CheckpointArtifacts) -> CheckpointId;

    /// Loads the checkpoint stored at `step`, if any.
    fn load(&self, step: usize) -> Option<StoredCheckpoint>;
}

/// A naive in-memory [`CheckpointStore`] — the vertical-slice default. With **no
/// checkpoints saved, plans replay from 0** (the correct, cache-free baseline).
#[derive(Debug, Default)]
pub struct InMemoryCheckpointStore {
    by_step: BTreeMap<usize, StoredCheckpoint>,
    next_id: u64,
}

impl InMemoryCheckpointStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl CheckpointStore for InMemoryCheckpointStore {
    fn list(&self) -> Vec<CheckpointMeta> {
        self.by_step.values().map(|c| c.meta.clone()).collect()
    }

    fn save(&mut self, step: usize, artifacts: CheckpointArtifacts) -> CheckpointId {
        let id = CheckpointId::new(format!("ckpt_{}", self.next_id));
        self.next_id += 1;
        // Representative envelope for the meta (all artifacts share version axes);
        // synthesize a minimal one when a checkpoint carries no BREP artifacts.
        let envelope = artifacts
            .representative_envelope()
            .cloned()
            .unwrap_or_else(|| CheckpointEnvelope {
                artifact_schema_version: ARTIFACT_SCHEMA_VERSION,
                body: BodyId(uuid::Uuid::nil()),
                step,
                history_prefix_hash: artifacts.history_prefix_hash.clone(),
                brep_content_hash: String::new(),
                occt_fingerprint: String::new(),
                descriptor_version: 1,
                resolver_version: 1,
                quantization_version: 1,
                signature_version: 1,
                codec: String::new(),
                size: 0,
                content_hash: String::new(),
            });
        let meta = CheckpointMeta {
            id: id.clone(),
            step,
            history_prefix_hash: artifacts.history_prefix_hash.clone(),
            envelope,
        };
        self.by_step
            .insert(step, StoredCheckpoint { meta, artifacts });
        id
    }

    fn load(&self, step: usize) -> Option<StoredCheckpoint> {
        self.by_step.get(&step).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::regen::engine::{Signature, StepSignatures};
    use crate::regen::planner::PlanContext;

    fn sigs() -> StepSignatures {
        StepSignatures {
            geometry: Signature::new("g"),
            body_lifecycle: Signature::new("b"),
            referenced_binding: Signature::new("r"),
        }
    }

    fn envelope(fp: &str, desc: u32) -> CheckpointEnvelope {
        CheckpointEnvelope {
            artifact_schema_version: ARTIFACT_SCHEMA_VERSION,
            body: BodyId(uuid::Uuid::from_u128(1)),
            step: 2,
            history_prefix_hash: HistoryPrefixHash::new("abc"),
            brep_content_hash: "aa".into(),
            occt_fingerprint: fp.into(),
            descriptor_version: desc,
            resolver_version: 1,
            quantization_version: 1,
            signature_version: 1,
            codec: "brep-bintools".into(),
            size: 10,
            content_hash: "bb".into(),
        }
    }

    fn ctx() -> PlanContext {
        PlanContext {
            policy_versions: Default::default(),
            occt_fingerprint: "fp".into(),
        }
    }

    #[test]
    fn compatible_matches_versions_and_fingerprint() {
        assert!(envelope("fp", 1).is_compatible(&ctx()));
        assert!(
            !envelope("other", 1).is_compatible(&ctx()),
            "fingerprint mismatch"
        );
        assert!(
            !envelope("fp", 2).is_compatible(&ctx()),
            "descriptor version mismatch"
        );
    }

    #[test]
    fn store_save_list_load_round_trip() {
        let mut store = InMemoryCheckpointStore::new();
        let artifacts = CheckpointArtifacts {
            step: 2,
            artifacts: vec![CheckpointArtifact {
                envelope: envelope("fp", 1),
                bytes: vec![1, 2, 3],
            }],
            element_map_partition: vec![],
            signatures: sigs(),
            history_prefix_hash: HistoryPrefixHash::new("abc"),
        };
        let id = store.save(2, artifacts);
        let metas = store.list();
        assert_eq!(metas.len(), 1);
        assert_eq!(metas[0].id, id);
        assert_eq!(metas[0].step, 2);
        let loaded = store.load(2).unwrap();
        assert_eq!(loaded.artifacts.artifacts[0].bytes, vec![1, 2, 3]);
        assert!(store.load(5).is_none());
    }
}
