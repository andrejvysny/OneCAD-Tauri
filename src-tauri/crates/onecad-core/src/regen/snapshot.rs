//! Published regen snapshots (V1/V2 §1.3).
//!
//! A [`ModelSnapshot`] is the **atomic publication unit**: the bodies, mesh keys,
//! signatures, step states, diagnostics and repair summary produced together all
//! share one [`SnapshotId`] and one monotonic `generation` (Invariant 4). A
//! snapshot is **immutable once published** — it is handed out behind an `Arc`.
//!
//! [`SnapshotPublisher`] publishes snapshots over a `tokio::sync::watch` channel
//! so render/pick consumers always observe the latest snapshot (double-buffer
//! swap, V1/V2 §11.2) and never a torn intermediate state.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::watch;

use crate::history::StepState;
use crate::ids::{BodyId, SnapshotId};

use super::engine::{Diagnostic, Signature, StepSignatures, StoppedReason};

/// Tessellation level of detail (SCHEMA §7.6 `lod`; V1/V2 §11.1 tiers). Core-level
/// mirror of the protocol `Lod` (the core stays transport-agnostic).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lod {
    Coarse,
    Medium,
    Fine,
}

/// The cache key for a body's mesh (plan "Mesh transfer": LRU `MeshCache` keyed
/// `(BodyId, Lod, generation)`). The `generation` pins the mesh to the snapshot
/// that produced it, so a stale mesh is never shown for a newer snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeshKey {
    pub body: BodyId,
    pub lod: Lod,
    pub generation: u64,
}

/// One body in a published snapshot. Its `mesh_key` addresses the (separately
/// cached) MESH1 buffer; `signature` is the body's geometry signature (drift
/// detection, Invariant 5).
#[derive(Debug, Clone, PartialEq)]
pub struct BodySnapshot {
    pub body: BodyId,
    pub mesh_key: MeshKey,
    pub signature: Signature,
    pub visible: bool,
}

/// A compact repair summary carried on a snapshot (the document `NeedsRepair`
/// badge + which steps need attention). The full [`RepairItem`]s live in the
/// document [`RepairState`](crate::document::repair::RepairState).
///
/// [`RepairItem`]: crate::document::repair::RepairItem
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepairSummary {
    pub needs_repair_count: usize,
    /// The (sorted, de-duplicated) steps with unresolved refs.
    pub steps: Vec<usize>,
}

/// One published, immutable regen result (V1/V2 §1.3 `ModelSnapshot`).
///
/// Everything here shares `id` + `generation` (Invariant 4). `step_index` is the
/// last **valid** step the snapshot represents (Invariant 6: a failure at `m`
/// publishes `≤ m−1`).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelSnapshot {
    /// The shared snapshot id (from `AcceptPrepared`).
    pub id: SnapshotId,
    /// Monotonic publication generation (assigned by the [`SnapshotPublisher`]).
    pub generation: u64,
    /// The last valid timeline step this snapshot represents, or `None` when only
    /// the base is valid — the first executed step failed / needs repair
    /// (review F14: an explicit `Option` instead of a `start − 1` sentinel).
    pub step_index: Option<usize>,
    /// The bodies of the snapshot (with mesh keys + signatures).
    pub bodies: Vec<BodySnapshot>,
    /// Why the producing plan stopped (`Completed` on full success; `OpFailed` /
    /// `NeedsRepair` on an accepted early-stop at `m−1`).
    pub stopped_reason: StoppedReason,
    /// The per-step states of the executed span (parallel-indexed to the
    /// timeline). Carried immutably so consumers read a consistent view.
    pub step_states: Vec<(usize, StepState)>,
    /// The three signatures of the last valid step, if any were collected
    /// (Invariant 5 drift detection).
    pub signatures: Option<StepSignatures>,
    /// Accumulated diagnostics across the executed span.
    pub diagnostics: Vec<Diagnostic>,
    /// Compact repair summary.
    pub repair_summary: RepairSummary,
}

/// Publishes immutable [`ModelSnapshot`]s over a `watch` channel and mints their
/// monotonic `generation`s.
///
/// Cloneable publish/subscribe handle: consumers `subscribe()`; the executor
/// `publish(...)`. The `watch` channel keeps only the latest snapshot (render/pick
/// want *current*, not a backlog).
#[derive(Debug)]
pub struct SnapshotPublisher {
    tx: watch::Sender<Option<Arc<ModelSnapshot>>>,
    generation: AtomicU64,
}

impl Default for SnapshotPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl SnapshotPublisher {
    /// A publisher with no snapshot yet (generation counter starts at 0; the
    /// first published snapshot gets generation 1).
    #[must_use]
    pub fn new() -> Self {
        let (tx, _rx) = watch::channel(None);
        Self {
            tx,
            generation: AtomicU64::new(0),
        }
    }

    /// A fresh subscriber. Its initial value is the latest snapshot (or `None`).
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Option<Arc<ModelSnapshot>>> {
        self.tx.subscribe()
    }

    /// The latest published snapshot, if any.
    #[must_use]
    pub fn latest(&self) -> Option<Arc<ModelSnapshot>> {
        self.tx.borrow().clone()
    }

    /// The most recently assigned generation (0 before the first publish).
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Publishes a snapshot built with the freshly-minted generation.
    ///
    /// `build` receives the next monotonic generation and returns the snapshot;
    /// the publisher wraps it in an `Arc`, sends it to all subscribers, and
    /// returns the `Arc`. Assigning the generation inside the closure guarantees
    /// the snapshot's `generation` field and its bodies' `MeshKey` generations
    /// agree with what subscribers observe.
    pub fn publish(&self, build: impl FnOnce(u64) -> ModelSnapshot) -> Arc<ModelSnapshot> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let snapshot = Arc::new(build(generation));
        // `send_replace` (not `send`) always stores the value and notifies —
        // `send` would drop it when there are momentarily no subscribers, so a
        // later subscriber must still see the latest snapshot.
        self.tx.send_replace(Some(snapshot.clone()));
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn snap(gen: u64) -> ModelSnapshot {
        ModelSnapshot {
            id: SnapshotId(gen),
            generation: gen,
            step_index: Some(0),
            bodies: vec![],
            stopped_reason: StoppedReason::Completed,
            step_states: vec![],
            signatures: None,
            diagnostics: vec![],
            repair_summary: RepairSummary::default(),
        }
    }

    #[test]
    fn publish_assigns_monotonic_generations() {
        let pub_ = SnapshotPublisher::new();
        assert_eq!(pub_.generation(), 0);
        let a = pub_.publish(snap);
        let b = pub_.publish(snap);
        assert_eq!(a.generation, 1);
        assert_eq!(b.generation, 2);
        assert_eq!(pub_.generation(), 2);
        assert_eq!(pub_.latest().unwrap().generation, 2);
    }

    #[tokio::test]
    async fn subscriber_observes_latest_snapshot() {
        let pub_ = SnapshotPublisher::new();
        let mut rx = pub_.subscribe();
        pub_.publish(snap);
        rx.changed().await.unwrap();
        assert_eq!(rx.borrow().as_ref().unwrap().generation, 1);
    }

    #[test]
    fn mesh_key_pins_generation() {
        let body = BodyId(Uuid::from_u128(1));
        let k1 = MeshKey {
            body,
            lod: Lod::Coarse,
            generation: 1,
        };
        let k2 = MeshKey {
            body,
            lod: Lod::Coarse,
            generation: 2,
        };
        assert_ne!(
            k1, k2,
            "same body/lod but different generation is a distinct key"
        );
    }
}
