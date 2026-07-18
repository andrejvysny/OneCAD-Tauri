//! The geometry-backend seam the app layer drives regen against.
//!
//! R-WP10 owns **no worker lifecycle** (that is R-WP11). It codes against the
//! [`GeometryEngine`] trait (core-owned, transport-agnostic) plus a small
//! [`MeshProvider`] seam for the bulk MESH1 bytes the core trait does not carry.
//! The two together are a [`Backend`]:
//!
//! * `FakeEngine` / `onecad-worker-stub` implement [`Backend`] in tests;
//! * R-WP11's `WorkerManager` (`tokio::process` + `ProtocolClient`) implements it
//!   over the real C++ sidecar and slots into [`AppState`](crate::state::AppState)
//!   with zero changes here (that is the seam).
//!
//! [`AdoptingEngine`] wraps any [`GeometryEngine`] to enforce **D1** (the
//! approved cross-track decision): NewBody `BodyId`s are worker-minted
//! deterministic `body_<opId>`; Rust adopts them from `planStep` `bodyEvents` and
//! rejects a prepared plan on malformation / collision (see [`validate_created`]).
//!
//! [`AppState`]: crate::state::AppState

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use uuid::Uuid;

use onecad_core::document::body::BodyLifecycleEvent;
use onecad_core::ids::{BodyId, DocumentId, JobId, SnapshotId, WorkerEpoch};
use onecad_core::regen::{
    AcceptResult, AcquireRequest, CheckpointArtifacts, EngineError, Fencing, GeometryEngine, Lod,
    OpenSessionRequest, PlanEvent, PlanRequest, RefResolution, ResolveRequest, RestoreRequest,
    RestoreResult, TessellateRequest, TessellateResult, WorkerElementEvidence, WorkerHead,
};

pub mod manager;
pub mod wire;

pub use manager::{RestartHook, SupervisorConfig, WorkerLifecycle, WorkerManager, WorkerState};

/// The default dev-tree worker binary, relative to `src-tauri/`.
pub const DEV_WORKER_PATH: &str = "../worker/build/onecad-worker";

/// The `ONECAD_WORKER_PATH` override env var (highest precedence).
pub const WORKER_PATH_ENV: &str = "ONECAD_WORKER_PATH";

/// Resolves the worker binary path (SCHEMA-agnostic packaging seam):
/// `ONECAD_WORKER_PATH` override → the dev fallback `../worker/build/onecad-worker`
/// (a Tauri `externalBin`/resource path is resolved by the caller when bundled).
/// Returns `None` when no candidate exists on disk, so the app keeps the
/// [`PendingBackend`] fallback rather than spawning a missing binary.
#[must_use]
pub fn resolve_worker_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var(WORKER_PATH_ENV) {
        let path = PathBuf::from(p);
        return path.exists().then_some(path);
    }
    let dev = PathBuf::from(DEV_WORKER_PATH);
    dev.exists().then_some(dev)
}

// ─────────────────────────────────────────────────────────────────────────────
// Mesh bytes seam
// ─────────────────────────────────────────────────────────────────────────────

/// Supplies the raw MESH1 bytes for a body/LOD in a published snapshot.
///
/// The core [`GeometryEngine::tessellate`] returns only mesh *handles* (identity
/// + integrity); the bytes stream on the bulk lane and are assembled by the
/// transport (R-WP11's `WorkerManager`). This seam lets the app-layer mesh cache
/// pull the assembled blob without the core trait carrying bulk payloads.
#[async_trait]
pub trait MeshProvider: Send + Sync {
    /// Fetches the MESH1 blob for `body` at `lod` in `snapshot`.
    async fn fetch_mesh(
        &self,
        body: BodyId,
        lod: Lod,
        snapshot: SnapshotId,
    ) -> Result<Vec<u8>, EngineError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// Solver-lane seam (SCHEMA §7.4) — separate from GeometryEngine
// ─────────────────────────────────────────────────────────────────────────────

/// The sketch **solver lane** (SCHEMA §7.4) — a dedicated seam distinct from the
/// OCCT-lane [`GeometryEngine`] because the worker runs PlaneGCS on a separate
/// thread/actor: drags must **never queue behind** an `ExecutePlan` (plan "Solver
/// lane in V1"). The transport ([`ProtocolClient`](onecad_protocol::client::ProtocolClient))
/// already multiplexes concurrent in-flight requests, so a `SolveDrag` frame goes
/// out and resolves while a plan is mid-flight.
///
/// **Latest-wins** is a client-side contract: the caller fires the newest
/// `SolveDrag` (monotonic `seq`) without awaiting each serially and tolerates a
/// `superseded`/`CANCELLED` terminal for a stale `seq` (SCHEMA §7.4) — it simply
/// drops that response's positions.
#[async_trait]
pub trait SolverEngine: Send + Sync {
    /// `SketchUpsert` (SCHEMA §7.4) — sync the authoritative sketch + report dof/state.
    async fn sketch_upsert(
        &self,
        sketch: &onecad_core::sketch::Sketch,
    ) -> Result<crate::dto::SketchUpsertDto, EngineError>;

    /// `BeginGesture` (SCHEMA §7.4) — open a drag gesture on a point.
    async fn begin_gesture(
        &self,
        sketch_id: &str,
        sketch_revision: u64,
        gesture_id: u64,
        drag_point: onecad_core::ids::EntityId,
        solver_policy_hash: &str,
    ) -> Result<crate::dto::BeginGestureDto, EngineError>;

    /// `SolveDrag` (SCHEMA §7.4) — one latest-wins incremental solve.
    async fn solve_drag(
        &self,
        gesture_id: u64,
        seq: u64,
        drag_point: onecad_core::ids::EntityId,
        target: [f64; 2],
    ) -> Result<crate::dto::DragSolveDto, EngineError>;

    /// `EndGesture` (SCHEMA §7.4) — pointer-up final exact solve; carries the
    /// changed positions the caller applies as one undo command.
    async fn end_gesture(
        &self,
        sketch_id: &str,
        gesture_id: u64,
        final_target: Option<[f64; 2]>,
    ) -> Result<crate::dto::SketchUpsertDto, EngineError>;

    /// `SketchRegions` (SCHEMA §7.4) — closed profile regions for extrude/preview.
    async fn sketch_regions(
        &self,
        sketch_id: &str,
    ) -> Result<Vec<crate::dto::SketchRegionDto>, EngineError>;
}

/// The full geometry backend: a [`GeometryEngine`] plus its [`MeshProvider`].
/// Blanket-implemented, so any type that is both is a `Backend`.
pub trait Backend: GeometryEngine + MeshProvider {}
impl<T: GeometryEngine + MeshProvider> Backend for T {}

/// The wire string for a [`Lod`] (`"coarse"`/`"medium"`/`"fine"`; SCHEMA §7.6).
#[must_use]
pub fn lod_str(lod: Lod) -> &'static str {
    match lod {
        Lod::Coarse => "coarse",
        Lod::Medium => "medium",
        Lod::Fine => "fine",
    }
}

/// Parses a wire LOD string; unknown ⇒ `Coarse` (the safe default tier).
#[must_use]
pub fn lod_from_str(s: &str) -> Lod {
    match s {
        "medium" => Lod::Medium,
        "fine" => Lod::Fine,
        _ => Lod::Coarse,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// D1 — worker-minted BodyId adoption
// ─────────────────────────────────────────────────────────────────────────────

/// Validates one step's `created` body events against the D1 adoption rule.
///
/// A NewBody id is worker-minted deterministic `body_<opId>` (`opId` is
/// Rust-minted, so replay is stable). Adoption accepts a `created` body iff:
///
/// * its id is the deterministic id of a **known op in the plan** — in the
///   core's UUID `BodyId` space that is `body.as_uuid() ∈ known_ops` (the
///   `body_<opId>` string form maps to `BodyId(opId.uuid)` at the wire boundary,
///   R-WP11); and
/// * it is **unique** — neither an already-present session body (`existing`) nor
///   a duplicate of an earlier `created` id in this plan (`seen`).
///
/// A malformed or colliding id returns `Err(message)`; the caller rejects the
/// whole prepared plan (never silently adopts). Split children (`body_<opId>:<k>`)
/// are deferred to W-WP6 — only `Created` events are validated here.
///
/// # Errors
/// A human-readable reason on malformation/collision (surfaced as `PROTOCOL_ERROR`).
pub fn validate_created(
    events: &[BodyLifecycleEvent],
    known_ops: &HashSet<Uuid>,
    existing: &HashSet<BodyId>,
    seen: &mut HashSet<BodyId>,
) -> Result<(), String> {
    for ev in events {
        let BodyLifecycleEvent::Created { body } = ev else {
            continue;
        };
        if !known_ops.contains(&body.as_uuid()) {
            return Err(format!(
                "worker-minted NewBody id {body} does not match any known opId (D1 malformation)"
            ));
        }
        if existing.contains(body) || !seen.insert(*body) {
            return Err(format!(
                "worker-minted NewBody id {body} collides with an existing/duplicate body (D1)"
            ));
        }
    }
    Ok(())
}

/// Wraps a [`GeometryEngine`] to enforce D1 body-id adoption on the `execute_plan`
/// stream. Every other verb delegates unchanged to the inner engine.
///
/// On a malformed / colliding `created` id the wrapper converts the terminal
/// `PlanPrepared` into a `PlanEvent::Failed(PROTOCOL_ERROR)`, so the executor
/// **discards** the scratch job (rejecting the prepared plan) rather than
/// publishing worker-minted ids Rust cannot adopt.
pub struct AdoptingEngine {
    inner: Arc<dyn GeometryEngine>,
    known_ops: HashSet<Uuid>,
    existing: HashSet<BodyId>,
}

impl AdoptingEngine {
    /// Wraps `inner`, validating `created` ids against the plan's `known_ops`
    /// (op record-id UUIDs) and the scratch base's `existing` bodies.
    #[must_use]
    pub fn new(
        inner: Arc<dyn GeometryEngine>,
        known_ops: HashSet<Uuid>,
        existing: HashSet<BodyId>,
    ) -> Self {
        Self {
            inner,
            known_ops,
            existing,
        }
    }
}

#[async_trait]
impl GeometryEngine for AdoptingEngine {
    async fn execute_plan(&self, request: PlanRequest) -> mpsc::Receiver<PlanEvent> {
        let mut inner_rx = self.inner.execute_plan(request).await;
        let (tx, rx) = mpsc::channel(256);
        let known = self.known_ops.clone();
        let existing = self.existing.clone();
        tokio::spawn(async move {
            let mut seen: HashSet<BodyId> = HashSet::new();
            let mut violation: Option<String> = None;
            while let Some(ev) = inner_rx.recv().await {
                // Validate `created` ids on each step until a violation is latched.
                if violation.is_none() {
                    if let PlanEvent::Step(step) = &ev {
                        if let Err(msg) =
                            validate_created(&step.body_events, &known, &existing, &mut seen)
                        {
                            violation = Some(msg);
                        }
                    }
                }
                // Reject a prepared plan that violated adoption: the executor
                // discards the scratch instead of publishing un-adoptable ids.
                if matches!(ev, PlanEvent::Prepared(_)) {
                    if let Some(msg) = violation.take() {
                        let _ = tx
                            .send(PlanEvent::Failed(EngineError::Protocol { message: msg }))
                            .await;
                        return;
                    }
                }
                if tx.send(ev).await.is_err() {
                    return;
                }
            }
        });
        rx
    }

    async fn open_session(&self, req: OpenSessionRequest) -> Result<WorkerHead, EngineError> {
        self.inner.open_session(req).await
    }
    async fn close_session(
        &self,
        document_id: DocumentId,
        worker_epoch: WorkerEpoch,
    ) -> Result<(), EngineError> {
        self.inner.close_session(document_id, worker_epoch).await
    }
    async fn reset(
        &self,
        document_id: DocumentId,
        worker_epoch: WorkerEpoch,
    ) -> Result<WorkerEpoch, EngineError> {
        self.inner.reset(document_id, worker_epoch).await
    }
    async fn accept_prepared(
        &self,
        job_id: JobId,
        fencing: Fencing,
    ) -> Result<AcceptResult, EngineError> {
        self.inner.accept_prepared(job_id, fencing).await
    }
    async fn discard_prepared(&self, job_id: JobId) -> Result<(), EngineError> {
        self.inner.discard_prepared(job_id).await
    }
    async fn get_worker_head(&self) -> Result<WorkerHead, EngineError> {
        self.inner.get_worker_head().await
    }
    async fn tessellate(&self, req: TessellateRequest) -> Result<TessellateResult, EngineError> {
        self.inner.tessellate(req).await
    }
    async fn save_checkpoint(&self, step_index: usize) -> Result<CheckpointArtifacts, EngineError> {
        self.inner.save_checkpoint(step_index).await
    }
    async fn restore_checkpoint(&self, req: RestoreRequest) -> Result<RestoreResult, EngineError> {
        self.inner.restore_checkpoint(req).await
    }
    async fn acquire_element_ids(
        &self,
        req: AcquireRequest,
    ) -> Result<Vec<WorkerElementEvidence>, EngineError> {
        self.inner.acquire_element_ids(req).await
    }
    async fn resolve_refs(&self, req: ResolveRequest) -> Result<Vec<RefResolution>, EngineError> {
        self.inner.resolve_refs(req).await
    }
    async fn cancel(&self, job_id: JobId) -> Result<(), EngineError> {
        self.inner.cancel(job_id).await
    }
    async fn ping(&self) -> Result<(), EngineError> {
        self.inner.ping().await
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Placeholder backend (production boot before R-WP11 wires the real worker)
// ─────────────────────────────────────────────────────────────────────────────

/// A [`Backend`] that fails every geometry call, so the app boots and the webview
/// loads before R-WP11 spawns the real worker. Every regen surfaces a
/// `PROTOCOL_ERROR` (recoverable — the session stays editable).
#[derive(Debug, Default)]
pub struct PendingBackend;

impl PendingBackend {
    fn not_ready() -> EngineError {
        EngineError::Protocol {
            message: "worker not started (R-WP11 wires the real sidecar)".into(),
        }
    }
}

#[async_trait]
impl GeometryEngine for PendingBackend {
    async fn execute_plan(&self, _request: PlanRequest) -> mpsc::Receiver<PlanEvent> {
        let (tx, rx) = mpsc::channel(1);
        let _ = tx.send(PlanEvent::Failed(Self::not_ready())).await;
        rx
    }
    async fn open_session(&self, _req: OpenSessionRequest) -> Result<WorkerHead, EngineError> {
        Err(Self::not_ready())
    }
    async fn close_session(&self, _d: DocumentId, _e: WorkerEpoch) -> Result<(), EngineError> {
        Ok(())
    }
    async fn reset(&self, _d: DocumentId, e: WorkerEpoch) -> Result<WorkerEpoch, EngineError> {
        Ok(WorkerEpoch(e.0 + 1))
    }
    async fn accept_prepared(&self, _j: JobId, _f: Fencing) -> Result<AcceptResult, EngineError> {
        Err(Self::not_ready())
    }
    async fn discard_prepared(&self, _j: JobId) -> Result<(), EngineError> {
        Ok(())
    }
    async fn get_worker_head(&self) -> Result<WorkerHead, EngineError> {
        Err(Self::not_ready())
    }
    async fn tessellate(&self, _r: TessellateRequest) -> Result<TessellateResult, EngineError> {
        Err(Self::not_ready())
    }
    async fn save_checkpoint(&self, _s: usize) -> Result<CheckpointArtifacts, EngineError> {
        Err(Self::not_ready())
    }
    async fn restore_checkpoint(&self, _r: RestoreRequest) -> Result<RestoreResult, EngineError> {
        Err(Self::not_ready())
    }
    async fn acquire_element_ids(
        &self,
        _r: AcquireRequest,
    ) -> Result<Vec<WorkerElementEvidence>, EngineError> {
        Err(Self::not_ready())
    }
    async fn resolve_refs(&self, _r: ResolveRequest) -> Result<Vec<RefResolution>, EngineError> {
        Err(Self::not_ready())
    }
    async fn cancel(&self, _j: JobId) -> Result<(), EngineError> {
        Ok(())
    }
    async fn ping(&self) -> Result<(), EngineError> {
        Err(Self::not_ready())
    }
}

#[async_trait]
impl MeshProvider for PendingBackend {
    async fn fetch_mesh(
        &self,
        _body: BodyId,
        _lod: Lod,
        _snapshot: SnapshotId,
    ) -> Result<Vec<u8>, EngineError> {
        Err(Self::not_ready())
    }
}

#[async_trait]
impl SolverEngine for PendingBackend {
    async fn sketch_upsert(
        &self,
        _sketch: &onecad_core::sketch::Sketch,
    ) -> Result<crate::dto::SketchUpsertDto, EngineError> {
        Err(Self::not_ready())
    }
    async fn begin_gesture(
        &self,
        _sketch_id: &str,
        _sketch_revision: u64,
        _gesture_id: u64,
        _drag_point: onecad_core::ids::EntityId,
        _solver_policy_hash: &str,
    ) -> Result<crate::dto::BeginGestureDto, EngineError> {
        Err(Self::not_ready())
    }
    async fn solve_drag(
        &self,
        _gesture_id: u64,
        _seq: u64,
        _drag_point: onecad_core::ids::EntityId,
        _target: [f64; 2],
    ) -> Result<crate::dto::DragSolveDto, EngineError> {
        Err(Self::not_ready())
    }
    async fn end_gesture(
        &self,
        _sketch_id: &str,
        _gesture_id: u64,
        _final_target: Option<[f64; 2]>,
    ) -> Result<crate::dto::SketchUpsertDto, EngineError> {
        Err(Self::not_ready())
    }
    async fn sketch_regions(
        &self,
        _sketch_id: &str,
    ) -> Result<Vec<crate::dto::SketchRegionDto>, EngineError> {
        Err(Self::not_ready())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(n: u128) -> BodyId {
        BodyId(Uuid::from_u128(n))
    }

    #[test]
    fn adoption_accepts_deterministic_new_body_id() {
        let op = Uuid::from_u128(0x10);
        let known: HashSet<Uuid> = [op].into_iter().collect();
        let mut seen = HashSet::new();
        let ev = BodyLifecycleEvent::Created {
            body: BodyId(op), // body.uuid == opId (deterministic body_<opId>)
        };
        assert!(validate_created(&[ev], &known, &HashSet::new(), &mut seen).is_ok());
    }

    #[test]
    fn adoption_rejects_unknown_op_id() {
        let known: HashSet<Uuid> = [Uuid::from_u128(0x10)].into_iter().collect();
        let mut seen = HashSet::new();
        let ev = BodyLifecycleEvent::Created { body: body(0xBAD) };
        let err = validate_created(&[ev], &known, &HashSet::new(), &mut seen).unwrap_err();
        assert!(err.contains("malformation"), "{err}");
    }

    #[test]
    fn adoption_rejects_duplicate_and_existing_collision() {
        let op = Uuid::from_u128(0x10);
        let known: HashSet<Uuid> = [op].into_iter().collect();
        // Duplicate within one plan.
        let mut seen = HashSet::new();
        let dup = vec![
            BodyLifecycleEvent::Created { body: BodyId(op) },
            BodyLifecycleEvent::Created { body: BodyId(op) },
        ];
        assert!(validate_created(&dup, &known, &HashSet::new(), &mut seen)
            .unwrap_err()
            .contains("collides"));
        // Collision with an existing session body.
        let mut seen2 = HashSet::new();
        let existing: HashSet<BodyId> = [BodyId(op)].into_iter().collect();
        let ev = BodyLifecycleEvent::Created { body: BodyId(op) };
        assert!(validate_created(&[ev], &known, &existing, &mut seen2)
            .unwrap_err()
            .contains("collides"));
    }
}
