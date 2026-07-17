//! The per-document runtime — the app's **single writer**, Tauri-free.
//!
//! V1 = one open document. [`DocumentRuntime`] owns the authoritative
//! [`DocumentSession`] (edits + undo/redo), a mirror [`RegenSession`] (the geometry
//! outputs the regen [`RegenExecutor`] writes), the fencing tokens, the LRU
//! [`MeshCache`], and the latest [`ModelSnapshot`]. Every method here is a plain
//! (async) function the thin `#[tauri::command]` wrappers delegate to, so the app
//! logic is testable without a running webview (plan quality bar).
//!
//! ## Single-writer regen (driver seam) — fencing live (R-WP11)
//!
//! The app layer runs the executor in three phases so a slow worker never blocks
//! edits and revision fencing goes **live**:
//!
//! * **phase 1 (locked)** — [`begin_regen`](DocumentRuntime::begin_regen) compiles
//!   the plan, wraps the backend in an [`AdoptingEngine`] (D1 body-id adoption),
//!   captures the [`FencingCell`] tokens, and **clones** the [`RegenSession`] so the
//!   executor drives on a copy;
//! * **phase 2 (unlocked)** — [`PreparedRegen::drive`] runs
//!   [`RegenExecutor::run`](onecad_core::regen::RegenExecutor::run) over the cloned
//!   scratch with the runtime lock **released**. Its
//!   [`RevisionGate`](onecad_core::regen::RevisionGate) reads the live
//!   [`FencingCell`], so an edit that lands during worker IO advances the revision
//!   and the executor supersedes the stale prepare at accept time;
//! * **phase 3 (locked)** — [`finish_regen`](DocumentRuntime::finish_regen) commits
//!   the driven snapshot into the live session **iff** the tokens are unchanged
//!   (else reports `Superseded`), preserving single-writer for the mutation.
//!
//! [`run_regen`](DocumentRuntime::run_regen) keeps the old inline (lock-held)
//! variant for direct callers/tests. The
//! [`RegenScheduler`](onecad_core::regen::RegenScheduler) drives phase 1→3 through
//! its [`RegenDriver`](onecad_core::regen::RegenDriver) seam (wired in
//! `crate::run`); debounce/coalesce/preview-priority live in the scheduler.
//!
//! The runtime holds the backend behind `Arc<dyn `[`GeometryEngine`]`>` +
//! `Arc<dyn `[`MeshProvider`]`>`; production wires the real `WorkerManager`, with
//! [`PendingBackend`](crate::worker::PendingBackend) the no-worker fallback.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
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
    PlanRequest, PolicyVersions, RegenExecutor, RegenPlanner, RegenRequest, RegenSession,
    SnapshotPublisher, TessellateSpec,
};

use crate::dto::{
    default_label, feature_kind, feature_status, feature_value_text, BodyDto, BodyMeshRef,
    DocStatus, DocumentChange, DocumentProjection, FeatureDto, SketchDto, SketchStatus,
};
use crate::mesh_cache::MeshCache;
use crate::worker::{lod_str, AdoptingEngine, MeshProvider};

/// The `(documentRevision, workerEpoch)` fencing tokens behind an `Arc` so the
/// regen driver's [`RevisionGate`](onecad_core::regen::RevisionGate) can read them
/// **lock-free** while slow worker IO is in flight (R-WP11).
///
/// This is what makes fencing **live**: the live regen path releases the runtime
/// lock across the worker call and drives the executor on a **cloned** scratch
/// session, so an edit that lands during the IO can acquire the lock and
/// [`bump_revision`](FencingCell::bump_revision) here — the executor's gate then
/// observes the change at accept time and supersedes the stale prepare (SCHEMA §7.2
/// fencing). Single-writer for the document is preserved: only the runtime-lock
/// holder ever mutates these tokens; reads are lock-free.
#[derive(Debug)]
pub struct FencingCell {
    revision: AtomicU64,
    epoch: AtomicU64,
}

impl FencingCell {
    fn new(epoch: u64) -> Self {
        Self {
            revision: AtomicU64::new(0),
            epoch: AtomicU64::new(epoch),
        }
    }

    /// The current `(revision, epoch)` — the executor's gate reads this.
    #[must_use]
    pub fn get(&self) -> (DocumentRevision, WorkerEpoch) {
        (
            DocumentRevision(self.revision.load(Ordering::SeqCst)),
            WorkerEpoch(self.epoch.load(Ordering::SeqCst)),
        )
    }

    fn revision(&self) -> DocumentRevision {
        DocumentRevision(self.revision.load(Ordering::SeqCst))
    }

    fn bump_revision(&self) {
        self.revision.fetch_add(1, Ordering::SeqCst);
    }

    fn set_epoch(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::SeqCst);
    }
}

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
    /// The lock-free fencing tokens (revision + worker epoch). See [`FencingCell`].
    fencing: Arc<FencingCell>,
    title: String,
    path: Option<PathBuf>,
    dirty: bool,
    read_only: bool,
    mesh_cache: MeshCache,
    latest_snapshot: Option<Arc<ModelSnapshot>>,
    publisher: Arc<SnapshotPublisher>,
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
            fencing: Arc::new(FencingCell::new(1)),
            title,
            path,
            dirty: false,
            read_only,
            mesh_cache: MeshCache::new(),
            latest_snapshot: None,
            publisher: Arc::new(SnapshotPublisher::new()),
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
        self.fencing.revision()
    }

    /// The lock-free fencing cell (revision + epoch). The live regen driver clones
    /// this `Arc` so its gate observes concurrent edits during worker IO (R-WP11).
    #[must_use]
    pub fn fencing(&self) -> Arc<FencingCell> {
        self.fencing.clone()
    }

    /// A worker (re)start bumped the epoch (SCHEMA §8 restart + replay): adopt the
    /// new epoch so subsequent plans fence against it, and mark the document dirty
    /// so the caller's replay recomputes geometry. Called by the WorkerManager's
    /// restart hook (under the runtime lock).
    pub fn on_worker_restart(&mut self, epoch: WorkerEpoch) {
        self.fencing.set_epoch(epoch.0);
        self.dirty = true;
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
        self.fencing.bump_revision();
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

    /// Compiles and drives a regen plan to its terminal **inline** (holding the
    /// caller's `&mut self`). Kept for direct callers/tests; the live app path uses
    /// the lock-free [`begin_regen`](Self::begin_regen) → drive →
    /// [`finish_regen`](Self::finish_regen) split so a slow worker never blocks
    /// edits and fencing goes live.
    ///
    /// Because this variant holds the runtime lock for the whole run, the fencing
    /// gate cannot change during it (no edit can land) — sound, but fencing is
    /// inert here by construction.
    pub async fn run_regen(&mut self, request: RegenRequest, cancel: CancelToken) -> RegenReport {
        let Some(prepared) = self.begin_regen(request) else {
            return RegenReport {
                outcome: Outcome::NoOp,
                revision: self.fencing.revision().0,
                changed: Vec::new(),
                removed: Vec::new(),
            };
        };
        let driven = prepared.drive(cancel).await;
        self.finish_regen(driven)
    }

    /// Phase 1 (**locked**): compile the plan against the current timeline, capture
    /// the fencing tokens, and **clone** the regen session so the executor can drive
    /// lock-free on the copy. `None` for an empty plan. Enforces D1 body-id
    /// adoption via [`AdoptingEngine`].
    pub fn begin_regen(&mut self, request: RegenRequest) -> Option<PreparedRegen> {
        let ctx = PlanContext {
            policy_versions: PolicyVersions::default(),
            occt_fingerprint: self.occt_fingerprint.clone(),
        };
        let graph = DependencyGraph::new(); // linear timeline: order is authoritative.
        let plan = RegenPlanner::plan(&self.regen.timeline, &graph, &[], request, &ctx);
        if plan.is_empty() {
            return None;
        }
        let job = self.next_job_id();
        let (plan_rev, epoch) = self.fencing.get();
        let artifacts = PlanArtifacts {
            tessellate: Some(TessellateSpec {
                lod: Lod::Coarse,
                include_edges: true,
            }),
        };
        let plan_req =
            plan.into_request(job, plan_rev, epoch, PolicyVersions::default(), artifacts);
        // D1: worker-minted `created` ids must match a known op in this plan and be
        // unique. Replay-from-0 base is empty, so collisions are in-plan.
        let known_ops: HashSet<Uuid> = plan_req.ops.iter().map(|o| o.record_id.as_uuid()).collect();
        let prior: Vec<BodyId> = self
            .latest_snapshot
            .as_ref()
            .map(|s| s.bodies.iter().map(|b| b.body).collect())
            .unwrap_or_default();
        Some(PreparedRegen {
            plan_req,
            engine: AdoptingEngine::new(self.engine.clone(), known_ops, HashSet::new()),
            scratch: self.clone_regen_session(),
            fencing: self.fencing.clone(),
            publisher: self.publisher.clone(),
            expected: (plan_rev, epoch),
            lod: Lod::Coarse,
            prior,
        })
    }

    /// Phase 3 (**locked**): commit a driven regen back into the live session.
    ///
    /// A `Published` snapshot commits **only if** the fencing tokens are unchanged
    /// since [`begin_regen`](Self::begin_regen) — i.e. no edit landed during the
    /// lock-free worker IO. If they advanced, the worker already accepted lock-free
    /// but the document moved on: the snapshot is stale, so it is **not** committed
    /// (the pending edit's regen reconverges) and the outcome is reported as
    /// `Superseded`. This upholds single-writer for the session mutation.
    pub fn finish_regen(&mut self, driven: DrivenRegen) -> RegenReport {
        let DrivenRegen {
            outcome,
            scratch,
            prior,
            expected,
            lod,
        } = driven;
        if let Outcome::Published(snap) = &outcome {
            if self.fencing.get() == expected {
                let (changed, removed) = self.commit_snapshot(scratch, snap, lod, &prior);
                return RegenReport {
                    outcome,
                    revision: self.fencing.revision().0,
                    changed,
                    removed,
                };
            }
            // Window race: worker accepted lock-free but the document advanced.
            return RegenReport {
                outcome: Outcome::Superseded,
                revision: self.fencing.revision().0,
                changed: Vec::new(),
                removed: Vec::new(),
            };
        }
        RegenReport {
            outcome,
            revision: self.fencing.revision().0,
            changed: Vec::new(),
            removed: Vec::new(),
        }
    }

    /// Moves the driven scratch state into the live session and records the
    /// changed/removed bodies for the `document-changed` event.
    fn commit_snapshot(
        &mut self,
        scratch: RegenSession,
        snap: &Arc<ModelSnapshot>,
        lod: Lod,
        prior: &[BodyId],
    ) -> (Vec<(BodyId, MeshKey)>, Vec<BodyId>) {
        let _ = lod;
        self.regen = scratch;
        self.latest_snapshot = Some(snap.clone());
        self.dirty = true;
        let changed: Vec<(BodyId, MeshKey)> =
            snap.bodies.iter().map(|b| (b.body, b.mesh_key)).collect();
        let current: HashSet<BodyId> = snap.bodies.iter().map(|b| b.body).collect();
        let removed: Vec<BodyId> = prior
            .iter()
            .copied()
            .filter(|b| !current.contains(b))
            .collect();
        (changed, removed)
    }

    /// Deep-clones the regen session so the executor drives on a copy (lock-free).
    fn clone_regen_session(&self) -> RegenSession {
        RegenSession {
            bodies: self.regen.bodies.clone(),
            timeline: self.regen.timeline.clone(),
            repair: self.regen.repair.clone(),
            elements: self.regen.elements.clone(),
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
            revision: self.fencing.revision().0,
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

/// A compiled, fenced regen ready to drive **lock-free** (phase 2). Produced by
/// [`DocumentRuntime::begin_regen`] under the lock; [`drive`](PreparedRegen::drive)
/// runs the executor on the cloned scratch with the runtime lock released, so a
/// concurrent edit can advance the fencing tokens and supersede a stale prepare.
pub struct PreparedRegen {
    plan_req: PlanRequest,
    engine: AdoptingEngine,
    scratch: RegenSession,
    fencing: Arc<FencingCell>,
    publisher: Arc<SnapshotPublisher>,
    expected: (
        onecad_core::ids::DocumentRevision,
        onecad_core::ids::WorkerEpoch,
    ),
    lod: Lod,
    prior: Vec<BodyId>,
}

impl PreparedRegen {
    /// Drives the plan to its terminal with the runtime lock **released**. The
    /// executor's [`RevisionGate`](onecad_core::regen::RevisionGate) reads the live
    /// [`FencingCell`], so an edit that lands during worker IO is observed at accept
    /// time (fencing live). Returns the driven result for
    /// [`DocumentRuntime::finish_regen`].
    pub async fn drive(self, cancel: CancelToken) -> DrivenRegen {
        let PreparedRegen {
            plan_req,
            engine,
            mut scratch,
            fencing,
            publisher,
            expected,
            lod,
            prior,
        } = self;
        let gate = move || fencing.get();
        let executor = RegenExecutor::new(engine);
        let outcome = executor
            .run(plan_req, &mut scratch, &gate, &cancel, &publisher)
            .await;
        DrivenRegen {
            outcome,
            scratch,
            prior,
            expected,
            lod,
        }
    }
}

/// The result of driving a [`PreparedRegen`] lock-free (phase 2 → 3 handoff).
pub struct DrivenRegen {
    outcome: Outcome,
    scratch: RegenSession,
    prior: Vec<BodyId>,
    expected: (
        onecad_core::ids::DocumentRevision,
        onecad_core::ids::WorkerEpoch,
    ),
    lod: Lod,
}

/// Renders a [`MeshKey`] as the `"<bodyId>:<lod>:<generation>"` string the
/// frontend `document-changed` payload carries (matches the mock's `mockMeshKey`).
#[must_use]
pub fn mesh_key_string(key: MeshKey) -> String {
    format!("{}:{}:{}", key.body, lod_str(key.lod), key.generation)
}

#[cfg(test)]
mod tests;
