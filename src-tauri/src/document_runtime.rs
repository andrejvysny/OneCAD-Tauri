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

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use uuid::Uuid;

use onecad_core::document::element_index::ElementEntry;
use onecad_core::document::record::OperationRecord;
use onecad_core::document::refs::AnchorIntent;
use onecad_core::document::Document;
use onecad_core::edit::{CommandOutcome, DocumentSession, EditCommand, SketchEditOp};
use onecad_core::error::DomainError;
use onecad_core::history::{DependencyGraph, StepState, Timeline};
use onecad_core::ids::{
    BodyId, DocumentId, DocumentRevision, ElementId, EntityId, JobId, SketchId, SnapshotId,
    TopoKey, WorkerEpoch,
};
use onecad_core::io::container::{ContainerCaches, ContainerReader, ContainerWriter, SaveMeta};
use onecad_core::io::IoError;
use onecad_core::math::Vec2;
use onecad_core::regen::{
    mint_element_ids, AcquireRequest, CancelToken, EngineError, GeometryEngine, Lod, MeshKey,
    ModelSnapshot, Outcome, Pick, PlanArtifacts, PlanContext, PlanRequest, PolicyVersions,
    RefResolution, RegenExecutor, RegenPlanner, RegenRequest, RegenSession, ResolveRequest,
    SnapshotPublisher, TessellateSpec,
};
use onecad_core::sketch::Sketch;

use crate::dto::{
    default_label, feature_kind, feature_status, feature_value_text, BodyDto, BodyMeshRef,
    DocStatus, DocumentChange, DocumentProjection, FeatureDto, FinishSketchDto, PromotedElementDto,
    SketchDto, SketchSessionDto, SketchSolveStatus, SketchStatus, SketchUpsertDto,
};
use crate::mesh_cache::MeshCache;
use crate::worker::{lod_str, AdoptingEngine, MeshProvider, SolverEngine};

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
    /// The regen terminal as the `regen-finished` `outcome` token (`published` |
    /// `superseded` | `failed` | `cancelled` | `noop`).
    #[must_use]
    pub fn outcome_str(&self) -> &'static str {
        match self.outcome {
            Outcome::Published(_) => "published",
            Outcome::Superseded => "superseded",
            Outcome::EngineFailed(_) => "failed",
            Outcome::Cancelled => "cancelled",
            Outcome::NoOp => "noop",
        }
    }

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

/// An in-flight sketch drag gesture (SCHEMA §7.4). The `before` sketch is the
/// pre-gesture memento; pointer-up commits **one** [`EditCommand::SketchDragGesture`]
/// so the whole drag is a single undo step (plan "Solver lane in V1").
struct ActiveGesture {
    gesture_id: u64,
    sketch_id: SketchId,
    drag_point: EntityId,
    before: Sketch,
    /// Next `SolveDrag` seq (monotonic; latest-wins).
    next_seq: u64,
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
    solver: Arc<dyn SolverEngine>,
    occt_fingerprint: String,
    job_seq: u64,
    /// Last solver-lane `(dof, status)` per sketch — real projection dof/status
    /// (replaces the `dof:0`/`Ok` placeholders). Empty until a sketch is solved.
    sketch_solve: BTreeMap<SketchId, (u32, SketchSolveStatus)>,
    /// The active drag gesture, if the pointer is down mid-drag.
    active_gesture: Option<ActiveGesture>,
    /// Monotonic gesture id allocator (SCHEMA §7.4 `gestureId`).
    gesture_seq: u64,
    /// Rust-owned promotion cache `(body, topoKey) → ElementId` so re-picking the
    /// same element in a snapshot returns the **same** id (Invariant 1). The worker
    /// only echoes ids it already holds; Rust owns id identity, so this map upholds
    /// the invariant across `AcquireElementIds` calls.
    promoted: HashMap<(BodyId, TopoKey), ElementId>,
}

impl DocumentRuntime {
    /// A fresh blank document ("Untitled").
    #[must_use]
    pub fn new_blank(
        engine: Arc<dyn GeometryEngine>,
        meshes: Arc<dyn MeshProvider>,
        solver: Arc<dyn SolverEngine>,
    ) -> Self {
        let doc = Document::new(DocumentId::new());
        Self::from_document(
            doc,
            "Untitled".to_string(),
            None,
            false,
            engine,
            meshes,
            solver,
        )
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
        solver: Arc<dyn SolverEngine>,
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
            solver,
        ))
    }

    fn from_document(
        doc: Document,
        title: String,
        path: Option<PathBuf>,
        read_only: bool,
        engine: Arc<dyn GeometryEngine>,
        meshes: Arc<dyn MeshProvider>,
        solver: Arc<dyn SolverEngine>,
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
            solver,
            occt_fingerprint: "pending-r-wp11".to_string(),
            job_seq: 0,
            sketch_solve: BTreeMap::new(),
            active_gesture: None,
            gesture_seq: 0,
            promoted: HashMap::new(),
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

        // Sketches: real dof/status come from the last solver-lane solve
        // (`sketch_solve`, updated by enter/upsert/end-gesture, SCHEMA §7.4);
        // an unsolved sketch reads `dof:0`/`Ok` until first solved.
        let mut sketches = BTreeMap::new();
        for (id, sk) in &doc.sketches {
            let (dof, status) = self
                .sketch_solve
                .get(id)
                .map_or((0, SketchStatus::Ok), |(dof, st)| (*dof, st.tree_status()));
            sketches.insert(
                id.to_string(),
                SketchDto {
                    id: id.to_string(),
                    name: sk.name.clone(),
                    visible: doc.sketch_visible(*id),
                    dof,
                    status,
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

    // ── Sketch solver lane (SCHEMA §7.4) ─────────────────────────────────────

    /// Enters sketch mode: syncs the authoritative sketch to the worker solver lane
    /// (`SketchUpsert`) and returns the live session (entities/constraints wire form
    /// plus real dof/status). The sketch must already exist (a new sketch is created
    /// via [`EditCommand::AddSketch`] through [`apply`](Self::apply) first).
    ///
    /// # Errors
    /// [`EngineError`] on an unknown sketch or a worker-side failure.
    pub async fn enter_sketch(
        &mut self,
        sketch_id: SketchId,
    ) -> Result<SketchSessionDto, EngineError> {
        let sketch = self.sketch_or_err(sketch_id, "enterSketch")?;
        let (plane, entities, constraints) = crate::worker::wire::sketch_wire(&sketch);
        let solved = self.solver.sketch_upsert(&sketch).await?;
        self.record_solve(sketch_id, &solved);
        Ok(SketchSessionDto {
            sketch_id: sketch_id.to_string(),
            plane,
            entities,
            constraints,
            dof: solved.dof,
            status: solved.status,
        })
    }

    /// Applies a batch of sketch edits authoritatively (one undoable
    /// [`EditCommand::SketchEdit`]) then re-solves on the worker for live dof/status
    /// (SCHEMA §7.4). A non-drag upsert is an identity solve (no coordinate
    /// write-back — the worker's `SketchUpsert` reports no positions).
    ///
    /// # Errors
    /// [`EngineError`] on a read-only document, an invalid edit, or a worker failure.
    pub async fn sketch_upsert(
        &mut self,
        sketch_id: SketchId,
        ops: Vec<SketchEditOp>,
    ) -> Result<SketchUpsertDto, EngineError> {
        if self.read_only {
            return Err(op_failed("sketchUpsert: read-only document"));
        }
        if !ops.is_empty() {
            self.apply(EditCommand::SketchEdit {
                sketch: sketch_id,
                ops,
            })
            .map_err(|e| op_failed(format!("sketchUpsert edit: {e}")))?;
        }
        let sketch = self.sketch_or_err(sketch_id, "sketchUpsert")?;
        let solved = self.solver.sketch_upsert(&sketch).await?;
        self.record_solve(sketch_id, &solved);
        Ok(solved)
    }

    /// Opens a drag gesture on `drag_point` (SCHEMA §7.4 `BeginGesture`). Snapshots
    /// the pre-gesture sketch (the `before` memento) so pointer-up can commit **one**
    /// undo command for the whole drag.
    ///
    /// # Errors
    /// [`EngineError`] on a read-only document, an unknown sketch, or a worker failure.
    pub async fn begin_gesture(
        &mut self,
        sketch_id: SketchId,
        drag_point: EntityId,
    ) -> Result<crate::dto::BeginGestureDto, EngineError> {
        if self.read_only {
            return Err(op_failed("beginGesture: read-only document"));
        }
        let sketch = self.sketch_or_err(sketch_id, "beginGesture")?;
        // Ensure the worker holds the current sketch (its BeginGesture reads it).
        let solved = self.solver.sketch_upsert(&sketch).await?;
        self.record_solve(sketch_id, &solved);
        let gesture_id = self.next_gesture_id();
        let ready = self
            .solver
            .begin_gesture(
                &sketch_id.to_string(),
                solved.sketch_revision,
                gesture_id,
                drag_point,
                "",
            )
            .await?;
        self.active_gesture = Some(ActiveGesture {
            gesture_id,
            sketch_id,
            drag_point,
            before: sketch,
            next_seq: 1,
        });
        Ok(ready)
    }

    /// One incremental drag solve (SCHEMA §7.4 `SolveDrag`). Fired latest-wins: the
    /// caller sends the newest `target` without awaiting each serially. The returned
    /// positions are a **preview** (not committed) — only [`end_gesture`](Self::end_gesture)
    /// mutates the document.
    ///
    /// # Errors
    /// [`EngineError`] when no gesture is active or the worker fails.
    pub async fn solve_drag(
        &mut self,
        target: [f64; 2],
    ) -> Result<crate::dto::DragSolveDto, EngineError> {
        let (gesture_id, drag_point, seq) = {
            let g = self
                .active_gesture
                .as_mut()
                .ok_or_else(|| op_failed("solveDrag: no active gesture"))?;
            let seq = g.next_seq;
            g.next_seq += 1;
            (g.gesture_id, g.drag_point, seq)
        };
        self.solver
            .solve_drag(gesture_id, seq, drag_point, target)
            .await
    }

    /// Pointer-up final exact solve (SCHEMA §7.4 `EndGesture`): applies the solved
    /// positions to the `before` memento and commits **one** [`EditCommand::SketchDragGesture`]
    /// (single undo step for the whole drag). Returns the final dof/status/positions.
    ///
    /// # Errors
    /// [`EngineError`] when no gesture is active, the commit is invalid, or the
    /// worker fails.
    pub async fn end_gesture(
        &mut self,
        final_target: Option<[f64; 2]>,
    ) -> Result<SketchUpsertDto, EngineError> {
        let gesture = self
            .active_gesture
            .take()
            .ok_or_else(|| op_failed("endGesture: no active gesture"))?;
        let solved = self
            .solver
            .end_gesture(
                &gesture.sketch_id.to_string(),
                gesture.gesture_id,
                final_target,
            )
            .await?;
        let mut after = gesture.before.clone();
        after.apply_solved_positions(&typed_positions(&solved.solved_positions));
        self.apply(EditCommand::SketchDragGesture {
            sketch: gesture.sketch_id,
            before: gesture.before,
            after,
        })
        .map_err(|e| op_failed(format!("endGesture commit: {e}")))?;
        self.record_solve(gesture.sketch_id, &solved);
        Ok(solved)
    }

    /// Exits sketch mode / cancels an in-flight gesture without committing (SCHEMA
    /// §7.4 — discard scratch). The document is unchanged.
    ///
    /// # Errors
    /// Never fails hard; a best-effort worker `EndGesture` (no commit) is ignored.
    pub async fn cancel_sketch(&mut self, _sketch_id: SketchId) -> Result<(), EngineError> {
        if let Some(g) = self.active_gesture.take() {
            // Best-effort: end the worker gesture so it does not leak (no commit).
            let _ = self
                .solver
                .end_gesture(&g.sketch_id.to_string(), g.gesture_id, None)
                .await;
        }
        Ok(())
    }

    /// Computes the closed profile regions for a sketch (SCHEMA §7.4 `SketchRegions`)
    /// — the extrude/revolve profile source. Syncs the sketch first so the regions
    /// reflect the latest geometry. Regions are a rebuildable cache, so they are
    /// returned but not persisted (the worker re-derives the same normative
    /// `regionId` during regen).
    ///
    /// # Errors
    /// [`EngineError`] on an unknown sketch or a worker failure.
    pub async fn finish_sketch(
        &mut self,
        sketch_id: SketchId,
    ) -> Result<FinishSketchDto, EngineError> {
        let sketch = self.sketch_or_err(sketch_id, "finishSketch")?;
        let solved = self.solver.sketch_upsert(&sketch).await?;
        self.record_solve(sketch_id, &solved);
        let regions = self.solver.sketch_regions(&sketch_id.to_string()).await?;
        Ok(FinishSketchDto { regions })
    }

    // ── Element identity (SCHEMA §7.5) ───────────────────────────────────────

    /// Promotes snapshot-scoped TopoKey picks to persistent, globally-unique
    /// `ElementId`s (SCHEMA §7.5 `AcquireElementIds`): the worker returns the
    /// resolved `topoKey → (kind, descriptor, anchor)` evidence and **Rust mints /
    /// owns the ids** ([`mint_element_ids`]). The promotion cache upholds Invariant 1
    /// (re-picking the same `(body, topoKey)` returns the same id) and the binding is
    /// recorded in the document element partition index.
    ///
    /// # Errors
    /// [`EngineError`] on a worker failure.
    pub async fn promote_selection(
        &mut self,
        snapshot: SnapshotId,
        body: BodyId,
        picks: Vec<(TopoKey, Option<AnchorIntent>)>,
    ) -> Result<Vec<PromotedElementDto>, EngineError> {
        let req = AcquireRequest {
            snapshot_id: snapshot,
            body,
            picks: picks
                .into_iter()
                .map(|(topo_key, anchor)| Pick { topo_key, anchor })
                .collect(),
        };
        let mut evidence = self.engine.acquire_element_ids(req).await?;
        // Rust owns id identity: seed `existing` from the promotion cache so a
        // re-pick of the same (body, topoKey) reuses the id (Invariant 1).
        for e in &mut evidence {
            if e.existing.is_none() {
                if let Some(id) = self.promoted.get(&(e.body, e.topo_key.clone())) {
                    e.existing = Some(id.clone());
                }
            }
        }
        let minted = mint_element_ids(evidence);
        let mut out = Vec::with_capacity(minted.len());
        for (id, ev) in minted {
            self.promoted
                .insert((ev.body, ev.topo_key.clone()), id.clone());
            // Record the partition binding into the (regen-mirror) element index.
            self.regen
                .elements
                .insert(id.clone(), ElementEntry::new(ev.body, ev.kind));
            out.push(PromotedElementDto {
                topo_key: ev.topo_key.as_str().to_string(),
                element_id: id.as_str().to_string(),
                kind: kind_str(ev.kind).to_string(),
                body_id: crate::worker::wire::body_id_wire(ev.body),
            });
        }
        Ok(out)
    }

    /// Dry-run ladder resolution for repair dialogs (SCHEMA §7.5 `ResolveRefs`) —
    /// binds nothing. Thin passthrough to the engine.
    ///
    /// # Errors
    /// [`EngineError`] on a worker failure.
    pub async fn resolve_refs(
        &self,
        req: ResolveRequest,
    ) -> Result<Vec<RefResolution>, EngineError> {
        self.engine.resolve_refs(req).await
    }

    // ── Sketch-flow helpers ──────────────────────────────────────────────────

    fn sketch_or_err(&self, id: SketchId, verb: &str) -> Result<Sketch, EngineError> {
        self.session
            .document()
            .sketch(id)
            .cloned()
            .ok_or_else(|| op_failed(format!("{verb}: unknown sketch {id}")))
    }

    fn record_solve(&mut self, sketch: SketchId, solved: &SketchUpsertDto) {
        self.sketch_solve
            .insert(sketch, (solved.dof, solved.status));
    }

    fn next_gesture_id(&mut self) -> u64 {
        self.gesture_seq += 1;
        self.gesture_seq
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

/// A sketch-flow domain issue as a recoverable [`EngineError`] (the session stays
/// editable) — surfaced to the command as [`ApiError::OpFailed`](crate::error::ApiError).
fn op_failed(message: impl Into<String>) -> onecad_core::regen::EngineError {
    onecad_core::regen::EngineError::OpFailed {
        code: onecad_core::regen::OpFailureCode::OpFailed,
        recoverable: true,
        message: message.into(),
    }
}

/// Converts a solver `positions` map (point-entity-id string → `[x, y]`) into the
/// typed `(EntityId, Vec2)` pairs [`Sketch::apply_solved_positions`] consumes.
/// Non-uuid keys / non-finite coords are skipped.
fn typed_positions(positions: &BTreeMap<String, [f64; 2]>) -> Vec<(EntityId, Vec2)> {
    positions
        .iter()
        .filter_map(|(k, xy)| {
            let id = EntityId::from_str(k).ok()?;
            let v = Vec2::new(xy[0], xy[1])?; // rejects non-finite
            Some((id, v))
        })
        .collect()
}

/// The wire kind string for an element (SCHEMA §7.5).
fn kind_str(kind: onecad_core::document::refs::ElementKind) -> &'static str {
    use onecad_core::document::refs::ElementKind;
    match kind {
        ElementKind::Face => "face",
        ElementKind::Edge => "edge",
        ElementKind::Vertex => "vertex",
    }
}

#[cfg(test)]
mod tests;
