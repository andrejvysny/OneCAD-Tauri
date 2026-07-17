//! The per-document runtime — the app's **single writer**, Tauri-free.
//!
//! V1 = one open document. [`DocumentRuntime`] owns the authoritative
//! [`DocumentSession`] (edits + undo/redo), a mirror [`RegenSession`] (the geometry
//! outputs the regen [`RegenExecutor`] writes), the fencing tokens, the LRU
//! [`MeshCache`], and the latest [`ModelSnapshot`]. Every method here is a plain
//! (async) function the thin `#[tauri::command]` wrappers delegate to, so the app
//! logic is testable without a running webview (plan quality bar).
//!
//! ## Single-writer regen (driver seam)
//!
//! The app layer runs the executor: [`run_regen`](DocumentRuntime::run_regen)
//! compiles the plan from the current timeline, wraps the backend in an
//! [`AdoptingEngine`] (D1 body-id adoption), drives
//! [`RegenExecutor::run`](onecad_core::regen::RegenExecutor::run) against
//! `&mut self.regen`, and — because the caller holds the runtime lock for the whole
//! run — reads the fencing gate from the values captured at plan-build (no other
//! writer can advance the revision mid-run). The
//! [`RegenScheduler`](onecad_core::regen::RegenScheduler) drives this through its
//! [`RegenDriver`](onecad_core::regen::RegenDriver) seam (wired in `crate::run`);
//! debounce/coalesce/preview-priority live in the scheduler, policy-only.
//!
//! **R-WP11 seam.** The runtime holds the backend behind
//! `Arc<dyn `[`GeometryEngine`]`>` + `Arc<dyn `[`MeshProvider`]`>`; the current
//! [`PendingBackend`](crate::worker::PendingBackend) fails every geometry call so
//! the app boots. R-WP11 swaps in the real `WorkerManager` with zero changes here.
//! While a real (slow) worker is in flight the runtime lock is held for the whole
//! regen; making that window truly async (releasing the lock across worker I/O so
//! revision fencing goes live) is R-WP11's refinement.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use onecad_core::document::record::OperationRecord;
use onecad_core::document::Document;
use onecad_core::edit::{CommandOutcome, DocumentSession, EditCommand};
use onecad_core::error::DomainError;
use onecad_core::history::{DependencyGraph, StepState, Timeline};
use onecad_core::ids::{BodyId, DocumentId, DocumentRevision, JobId, WorkerEpoch};
use onecad_core::io::container::{ContainerCaches, ContainerReader, ContainerWriter, SaveMeta};
use onecad_core::io::IoError;
use onecad_core::regen::{
    CancelToken, GeometryEngine, Lod, MeshKey, ModelSnapshot, Outcome, PlanArtifacts, PlanContext,
    PolicyVersions, RegenExecutor, RegenPlanner, RegenRequest, RegenSession, SnapshotPublisher,
    TessellateSpec,
};

use crate::dto::{
    default_label, feature_kind, feature_status, feature_value_text, BodyDto, BodyMeshRef,
    DocStatus, DocumentChange, DocumentProjection, FeatureDto, SketchDto, SketchStatus,
};
use crate::mesh_cache::MeshCache;
use crate::worker::{lod_str, AdoptingEngine, MeshProvider};

/// What one regen produced, for event emission. `outcome` is the executor's
/// terminal; `changed`/`removed` drive the pull-model `document-changed` event.
#[derive(Debug)]
pub struct RegenReport {
    /// The executor terminal (Published / Superseded / EngineFailed / Cancelled / NoOp).
    pub outcome: Outcome,
    /// The document revision the regen was fenced against.
    pub revision: u64,
    /// Bodies present after the regen, with their generation-pinned mesh keys.
    pub changed: Vec<(BodyId, MeshKey)>,
    /// Bodies that were present before but are gone now.
    pub removed: Vec<BodyId>,
}

impl RegenReport {
    /// The `document-changed` payload, or `None` when nothing was published.
    #[must_use]
    pub fn document_change(&self) -> Option<DocumentChange> {
        if !matches!(self.outcome, Outcome::Published(_)) {
            return None;
        }
        Some(DocumentChange {
            revision: self.revision,
            changed_bodies: self
                .changed
                .iter()
                .map(|(body, key)| BodyMeshRef {
                    body_id: body.to_string(),
                    mesh_key: mesh_key_string(*key),
                })
                .collect(),
            removed_bodies: self.removed.iter().map(BodyId::to_string).collect(),
        })
    }
}

/// The per-document runtime (V1 single writer).
pub struct DocumentRuntime {
    session: DocumentSession,
    regen: RegenSession,
    revision: DocumentRevision,
    epoch: WorkerEpoch,
    title: String,
    path: Option<PathBuf>,
    dirty: bool,
    read_only: bool,
    mesh_cache: MeshCache,
    latest_snapshot: Option<Arc<ModelSnapshot>>,
    publisher: SnapshotPublisher,
    engine: Arc<dyn GeometryEngine>,
    meshes: Arc<dyn MeshProvider>,
    occt_fingerprint: String,
    job_seq: u64,
}

impl DocumentRuntime {
    /// A fresh blank document ("Untitled").
    #[must_use]
    pub fn new_blank(engine: Arc<dyn GeometryEngine>, meshes: Arc<dyn MeshProvider>) -> Self {
        let doc = Document::new(DocumentId::new());
        Self::from_document(doc, "Untitled".to_string(), None, false, engine, meshes)
    }

    /// Opens an existing `.onecad` container at `path`.
    ///
    /// # Errors
    /// [`IoError`] on a malformed / hostile / corrupt archive. A low-confidence
    /// migration opens **read-only** (not an error); reflected in [`read_only`].
    ///
    /// [`read_only`]: DocumentRuntime::is_read_only
    pub fn open(
        path: &Path,
        engine: Arc<dyn GeometryEngine>,
        meshes: Arc<dyn MeshProvider>,
    ) -> Result<Self, IoError> {
        let loaded = ContainerReader::open(path)?;
        let read_only = loaded.outcome.read_only;
        let doc = loaded.document().clone();
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Document".to_string());
        Ok(Self::from_document(
            doc,
            title,
            Some(path.to_path_buf()),
            read_only,
            engine,
            meshes,
        ))
    }

    fn from_document(
        doc: Document,
        title: String,
        path: Option<PathBuf>,
        read_only: bool,
        engine: Arc<dyn GeometryEngine>,
        meshes: Arc<dyn MeshProvider>,
    ) -> Self {
        // Seed the regen mirror from the (possibly persisted) geometry outputs so
        // the tree renders saved bodies immediately, before the first regen.
        let regen = RegenSession {
            bodies: doc.bodies.clone(),
            timeline: doc.timeline.clone(),
            repair: doc.repair.clone(),
            elements: doc.elements.clone(),
        };
        Self {
            session: DocumentSession::new(doc),
            regen,
            revision: DocumentRevision(0),
            epoch: WorkerEpoch(1),
            title,
            path,
            dirty: false,
            read_only,
            mesh_cache: MeshCache::new(),
            latest_snapshot: None,
            publisher: SnapshotPublisher::new(),
            engine,
            meshes,
            occt_fingerprint: "pending-r-wp11".to_string(),
            job_seq: 0,
        }
    }

    // ── Accessors ────────────────────────────────────────────────────────────

    /// The document title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The document id (as a string).
    #[must_use]
    pub fn document_id(&self) -> String {
        self.session.document().id.to_string()
    }

    /// The current document revision.
    #[must_use]
    pub fn revision(&self) -> DocumentRevision {
        self.revision
    }

    /// The stored save path, if any.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether the document opened read-only (low-confidence migration).
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Whether there are unsaved changes.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// The scheduler-facing subscription to the latest published snapshot.
    #[must_use]
    pub fn subscribe_snapshots(&self) -> tokio::sync::watch::Receiver<Option<Arc<ModelSnapshot>>> {
        self.publisher.subscribe()
    }

    // ── Edits ────────────────────────────────────────────────────────────────

    /// Applies one [`EditCommand`], syncing the regen mirror and bumping the
    /// revision. Returns the [`CommandOutcome`] (its [`RegenHint`] drives the
    /// scheduler).
    ///
    /// [`RegenHint`]: onecad_core::edit::RegenHint
    ///
    /// # Errors
    /// [`DomainError`] on validation failure; the document is left unchanged.
    pub fn apply(&mut self, cmd: EditCommand) -> Result<CommandOutcome, DomainError> {
        if self.read_only {
            return Err(DomainError::ReadOnly);
        }
        let outcome = self.session.apply(cmd)?;
        self.after_mutation();
        Ok(outcome)
    }

    /// Undoes the newest committed edit. Returns `true` if a step was undone.
    pub fn undo(&mut self) -> bool {
        if self.read_only {
            return false;
        }
        let undone = self.session.undo();
        if undone {
            self.after_mutation();
        }
        undone
    }

    /// Redoes the newest undone edit.
    ///
    /// # Errors
    /// [`DomainError`] if a replayed command fails.
    pub fn redo(&mut self) -> Result<bool, DomainError> {
        if self.read_only {
            return Ok(false);
        }
        let redone = self.session.redo()?;
        if redone {
            self.after_mutation();
        }
        Ok(redone)
    }

    /// After any structural mutation: re-mirror the timeline (all Dirty pending
    /// regen), bump the fencing revision, and mark unsaved.
    fn after_mutation(&mut self) {
        self.sync_regen_timeline();
        self.revision = DocumentRevision(self.revision.0 + 1);
        self.dirty = true;
    }

    /// Rebuilds the regen mirror timeline from the authoritative session timeline
    /// (records + cursor). `from_records` marks every step Dirty; the next regen
    /// recomputes states.
    fn sync_regen_timeline(&mut self) {
        let src = &self.session.document().timeline;
        let mut mirror = Timeline::from_records(src.records().to_vec());
        mirror.set_cursor(src.cursor());
        self.regen.timeline = mirror;
    }

    // ── Regen (the driver body) ──────────────────────────────────────────────

    /// Compiles and drives a regen plan to its terminal (the app-layer executor
    /// run). Enforces D1 body-id adoption via [`AdoptingEngine`]. On a published
    /// snapshot it updates the latest snapshot and returns the changed/removed
    /// bodies for the `document-changed` event.
    ///
    /// The caller holds the single-writer lock for the whole run, so the fencing
    /// gate is the `(revision, epoch)` captured at plan-build.
    pub async fn run_regen(&mut self, request: RegenRequest, cancel: CancelToken) -> RegenReport {
        let ctx = PlanContext {
            policy_versions: PolicyVersions::default(),
            occt_fingerprint: self.occt_fingerprint.clone(),
        };
        let graph = DependencyGraph::new(); // linear timeline: order is authoritative.
        let plan = RegenPlanner::plan(&self.regen.timeline, &graph, &[], request, &ctx);
        if plan.is_empty() {
            return RegenReport {
                outcome: Outcome::NoOp,
                revision: self.revision.0,
                changed: Vec::new(),
                removed: Vec::new(),
            };
        }

        let job = self.next_job_id();
        let plan_rev = self.revision;
        let epoch = self.epoch;
        let artifacts = PlanArtifacts {
            tessellate: Some(TessellateSpec {
                lod: Lod::Coarse,
                include_edges: true,
            }),
        };
        let plan_req =
            plan.into_request(job, plan_rev, epoch, PolicyVersions::default(), artifacts);

        // D1: the worker-minted `created` ids must match a known op in this plan
        // and be unique. Replay-from-0 base is empty, so collisions are in-plan.
        let known_ops: HashSet<Uuid> = plan_req.ops.iter().map(|o| o.record_id.as_uuid()).collect();
        let engine = AdoptingEngine::new(self.engine.clone(), known_ops, HashSet::new());
        let executor = RegenExecutor::new(engine);
        let gate = move || (plan_rev, epoch);

        let prior: Vec<BodyId> = self
            .latest_snapshot
            .as_ref()
            .map(|s| s.bodies.iter().map(|b| b.body).collect())
            .unwrap_or_default();

        let outcome = executor
            .run(plan_req, &mut self.regen, &gate, &cancel, &self.publisher)
            .await;

        let (changed, removed) = if let Outcome::Published(snap) = &outcome {
            self.latest_snapshot = Some(snap.clone());
            self.dirty = true;
            let changed: Vec<(BodyId, MeshKey)> =
                snap.bodies.iter().map(|b| (b.body, b.mesh_key)).collect();
            let current: HashSet<BodyId> = snap.bodies.iter().map(|b| b.body).collect();
            let removed: Vec<BodyId> = prior.into_iter().filter(|b| !current.contains(b)).collect();
            (changed, removed)
        } else {
            (Vec::new(), Vec::new())
        };

        RegenReport {
            outcome,
            revision: self.revision.0,
            changed,
            removed,
        }
    }

    fn next_job_id(&mut self) -> JobId {
        self.job_seq += 1;
        JobId(Uuid::from_u128(u128::from(self.job_seq)))
    }

    // ── Mesh pull ────────────────────────────────────────────────────────────

    /// Fetches a body's MESH1 blob (pull model), caching it. `generation` pins the
    /// snapshot; `None` ⇒ the latest snapshot's generation. `None` on miss (no
    /// document geometry, a stale generation, or a provider failure).
    ///
    /// Bytes are returned behind an `Arc` so the command hands the webview a
    /// zero-copy [`tauri::ipc::Response`].
    pub async fn get_mesh(
        &mut self,
        body: BodyId,
        lod: Lod,
        generation: Option<u64>,
    ) -> Option<Arc<Vec<u8>>> {
        let (gen, snap_id, latest_gen) = {
            let snap = self.latest_snapshot.as_ref()?;
            (
                generation.unwrap_or(snap.generation),
                snap.id,
                snap.generation,
            )
        };
        let key = MeshKey {
            body,
            lod,
            generation: gen,
        };
        if let Some(bytes) = self.mesh_cache.get(&key) {
            return Some(bytes);
        }
        // V1 serves only the current snapshot's generation; a stale one is a miss.
        if gen != latest_gen {
            return None;
        }
        let bytes = self.meshes.fetch_mesh(body, lod, snap_id).await.ok()?;
        let arc = Arc::new(bytes);
        self.mesh_cache.put(key, arc.clone());
        Some(arc)
    }

    // ── Save ─────────────────────────────────────────────────────────────────

    /// Atomically saves the document (+ merged regen geometry outputs) to `path`.
    /// Timestamps come from the caller (the pure core never reads the wall clock).
    ///
    /// # Errors
    /// [`IoError`] on a serialization / filesystem failure; the target is left
    /// untouched on any failure.
    pub fn save(&mut self, path: &Path, meta: SaveMeta) -> Result<(), IoError> {
        let mut doc = self.session.document().clone();
        // Merge regen-derived outputs so a reopen shows the tree before regen.
        doc.bodies = self.regen.bodies.clone();
        doc.elements = self.regen.elements.clone();
        doc.repair = self.regen.repair.clone();
        ContainerWriter::save(path, &doc, &ContainerCaches::none(), &meta)?;
        self.path = Some(path.to_path_buf());
        self.dirty = false;
        Ok(())
    }

    // ── Projection ───────────────────────────────────────────────────────────

    /// Builds the frontend [`DocumentProjection`] from the authoritative document
    /// + the regen mirror (states) + the latest snapshot (body geometry).
    #[must_use]
    pub fn projection(&self) -> DocumentProjection {
        let doc = self.session.document();

        // Bodies: regen geometry outputs, plus any edit-registered bodies the
        // regen has not produced (disjoint in the V1 slice; deduped by id).
        let mut bodies = BTreeMap::new();
        for b in self.regen.bodies.bodies() {
            bodies.insert(
                b.id.to_string(),
                BodyDto {
                    id: b.id.to_string(),
                    name: b.name.clone(),
                    visible: b.visible,
                },
            );
        }
        for b in doc.bodies.bodies() {
            bodies.entry(b.id.to_string()).or_insert_with(|| BodyDto {
                id: b.id.to_string(),
                name: b.name.clone(),
                visible: b.visible,
            });
        }

        // Sketches: dof/status are solver-lane outputs (R-WP11); a placeholder
        // keeps the tree faithful until the solver is wired.
        let mut sketches = BTreeMap::new();
        for (id, sk) in &doc.sketches {
            sketches.insert(
                id.to_string(),
                SketchDto {
                    id: id.to_string(),
                    name: sk.name.clone(),
                    visible: doc.sketch_visible(*id),
                    dof: 0,
                    status: SketchStatus::Ok,
                },
            );
        }

        let features = doc
            .timeline
            .records()
            .iter()
            .enumerate()
            .map(|(i, rec)| self.feature_dto(i, rec))
            .collect();

        DocumentProjection {
            status: DocStatus::Ready,
            revision: self.revision.0,
            title: self.title.clone(),
            dirty: self.dirty,
            bodies,
            sketches,
            features,
        }
    }

    fn feature_dto(&self, index: usize, rec: &OperationRecord) -> FeatureDto {
        let kind = feature_kind(&rec.op);
        let label = if rec.name.is_empty() {
            default_label(kind).to_string()
        } else {
            rec.name.clone()
        };
        let state = self
            .regen
            .timeline
            .state(index)
            .cloned()
            .unwrap_or(StepState::Dirty);
        FeatureDto {
            id: rec.record_id.to_string(),
            kind,
            label,
            value_text: feature_value_text(&rec.op),
            status: feature_status(&state),
        }
    }
}

/// Renders a [`MeshKey`] as the `"<bodyId>:<lod>:<generation>"` string the
/// frontend `document-changed` payload carries (matches the mock's `mockMeshKey`).
#[must_use]
pub fn mesh_key_string(key: MeshKey) -> String {
    format!("{}:{}:{}", key.body, lod_str(key.lod), key.generation)
}

#[cfg(test)]
mod tests;
