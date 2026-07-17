//! Unit tests for [`DocumentRuntime`] driven by a local scripted backend.
//!
//! The `FakeBackend` implements both [`GeometryEngine`] and
//! [`MeshProvider`](crate::worker::MeshProvider) with no OCCT: each op creates a
//! deterministic body (`BodyId(opId.uuid)` — the D1 `body_<opId>` rule in the
//! core's UUID space) unless overridden, echoes the plan's opaque history-prefix
//! token the executor verifies, and serves canned MESH1 bytes per body.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use uuid::Uuid;

use onecad_core::document::body::BodyLifecycleEvent;
use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord,
};
use onecad_core::document::variables::Scalar;
use onecad_core::edit::EditCommand;
use onecad_core::ids::{
    BodyId, DocumentId, DocumentRevision, JobId, RecordId, SnapshotId, WorkerEpoch,
};
use onecad_core::regen::{
    AcceptResult, AcquireRequest, CheckpointArtifacts, ElementMapDelta, EngineError, Fencing,
    GeometryEngine, HistoryPrefixHash, Lod, OpFailureCode, OpenSessionRequest, Outcome, PlanEvent,
    PlanPrepared, PlanRequest, PlanStepEvent, RefResolution, RegenRequest, ResolveRequest,
    RestoreRequest, RestoreResult, Signature, StepResult, StepSignatures, StepStatus,
    StoppedReason, TessellateRequest, TessellateResult, WorkerElementEvidence, WorkerHead,
};

use super::*;
use crate::worker::MeshProvider;

// ─────────────────────────────────────────────────────────────────────────────
// Scripted backend
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Default)]
struct FakeState {
    prepared: HashMap<JobId, SnapshotId>,
    snapshot_counter: u64,
}

struct FakeBackend {
    /// Per-step created-body overrides; a step without an entry creates one body
    /// `BodyId(opId.uuid)` (the deterministic D1 id).
    body_overrides: HashMap<usize, Vec<BodyId>>,
    state: Mutex<FakeState>,
}

impl FakeBackend {
    fn new() -> Self {
        Self {
            body_overrides: HashMap::new(),
            state: Mutex::new(FakeState::default()),
        }
    }

    fn with_overrides(overrides: HashMap<usize, Vec<BodyId>>) -> Self {
        Self {
            body_overrides: overrides,
            state: Mutex::new(FakeState::default()),
        }
    }

    fn bodies_for(&self, step: usize, record: RecordId) -> Vec<BodyId> {
        self.body_overrides
            .get(&step)
            .cloned()
            .unwrap_or_else(|| vec![BodyId(record.as_uuid())])
    }
}

fn sigs(step: usize) -> StepSignatures {
    StepSignatures {
        geometry: Signature::new(format!("g{step}")),
        body_lifecycle: Signature::new(format!("b{step}")),
        referenced_binding: Signature::new(format!("r{step}")),
    }
}

/// The opaque history-prefix token a well-behaved worker echoes (mirrors the
/// executor's expectation, so verification passes by construction).
fn echo_hash(request: &PlanRequest, last_valid: Option<usize>) -> HistoryPrefixHash {
    match last_valid {
        Some(step) => request
            .ops
            .iter()
            .position(|o| o.step_index == step)
            .and_then(|j| request.prefix_hashes.get(j).cloned())
            .unwrap_or_else(|| request.expected_base_hash.clone()),
        None => request.expected_base_hash.clone(),
    }
}

#[async_trait]
impl GeometryEngine for FakeBackend {
    async fn execute_plan(&self, request: PlanRequest) -> mpsc::Receiver<PlanEvent> {
        let (events, ()) = {
            let mut st = self.state.lock().unwrap();
            st.snapshot_counter += 1;
            let snapshot_id = SnapshotId(5000 + st.snapshot_counter);
            let job = request.job_id;

            let mut events = Vec::new();
            let mut per_step: Vec<StepResult> = Vec::new();
            let mut last_valid: Option<usize> = None;
            for op in &request.ops {
                let step = op.step_index;
                let body_ids = self.bodies_for(step, op.record_id);
                let body_events: Vec<BodyLifecycleEvent> = body_ids
                    .iter()
                    .map(|b| BodyLifecycleEvent::Created { body: *b })
                    .collect();
                events.push(PlanEvent::Step(PlanStepEvent {
                    step_index: step,
                    body_events,
                    element_map_delta: ElementMapDelta::default(),
                    needs_repair: vec![],
                    signatures: sigs(step),
                    diagnostics: vec![],
                }));
                per_step.push(StepResult {
                    step_index: step,
                    status: StepStatus::Ok,
                    body_ids,
                });
                last_valid = Some(step);
            }
            st.prepared.insert(job, snapshot_id);
            events.push(PlanEvent::Prepared(PlanPrepared {
                job_id: job,
                prepared_snapshot_id: snapshot_id,
                last_valid_step: last_valid,
                stopped_reason: StoppedReason::Completed,
                per_step,
                history_prefix_hash: echo_hash(&request, last_valid),
            }));
            (events, ())
        };

        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for ev in events {
                if tx.send(ev).await.is_err() {
                    return;
                }
            }
        });
        rx
    }

    async fn accept_prepared(
        &self,
        job_id: JobId,
        fencing: Fencing,
    ) -> Result<AcceptResult, EngineError> {
        let mut st = self.state.lock().unwrap();
        let snapshot_id = st.prepared.remove(&job_id).unwrap_or(SnapshotId(0));
        Ok(AcceptResult {
            snapshot_id,
            document_revision: DocumentRevision(fencing.document_revision.0 + 1),
        })
    }

    async fn discard_prepared(&self, job_id: JobId) -> Result<(), EngineError> {
        self.state.lock().unwrap().prepared.remove(&job_id);
        Ok(())
    }

    async fn open_session(&self, req: OpenSessionRequest) -> Result<WorkerHead, EngineError> {
        Ok(WorkerHead {
            document_revision: req.document_revision,
            worker_epoch: req.worker_epoch,
            snapshot_id: SnapshotId(0),
            history_prefix_hash: HistoryPrefixHash::empty(),
            has_scratch: false,
        })
    }
    async fn close_session(&self, _d: DocumentId, _e: WorkerEpoch) -> Result<(), EngineError> {
        Ok(())
    }
    async fn reset(&self, _d: DocumentId, e: WorkerEpoch) -> Result<WorkerEpoch, EngineError> {
        Ok(WorkerEpoch(e.0 + 1))
    }
    async fn get_worker_head(&self) -> Result<WorkerHead, EngineError> {
        Ok(WorkerHead {
            document_revision: DocumentRevision(0),
            worker_epoch: WorkerEpoch(1),
            snapshot_id: SnapshotId(0),
            history_prefix_hash: HistoryPrefixHash::empty(),
            has_scratch: false,
        })
    }
    async fn tessellate(&self, _r: TessellateRequest) -> Result<TessellateResult, EngineError> {
        Ok(TessellateResult { meshes: vec![] })
    }
    async fn save_checkpoint(&self, _s: usize) -> Result<CheckpointArtifacts, EngineError> {
        Err(EngineError::OpFailed {
            code: OpFailureCode::Unsupported,
            recoverable: true,
            message: "fake".into(),
        })
    }
    async fn restore_checkpoint(&self, _r: RestoreRequest) -> Result<RestoreResult, EngineError> {
        Err(EngineError::Protocol {
            message: "fake has no checkpoints".into(),
        })
    }
    async fn acquire_element_ids(
        &self,
        _r: AcquireRequest,
    ) -> Result<Vec<WorkerElementEvidence>, EngineError> {
        Ok(vec![])
    }
    async fn resolve_refs(&self, _r: ResolveRequest) -> Result<Vec<RefResolution>, EngineError> {
        Ok(vec![])
    }
    async fn cancel(&self, _j: JobId) -> Result<(), EngineError> {
        Ok(())
    }
    async fn ping(&self) -> Result<(), EngineError> {
        Ok(())
    }
}

#[async_trait]
impl MeshProvider for FakeBackend {
    async fn fetch_mesh(
        &self,
        body: BodyId,
        lod: Lod,
        _snapshot: SnapshotId,
    ) -> Result<Vec<u8>, EngineError> {
        Ok(format!("MESH1:{body}:{}", crate::worker::lod_str(lod)).into_bytes())
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixtures
// ─────────────────────────────────────────────────────────────────────────────

fn extrude_record(seed: u128, distance: f64) -> OperationRecord {
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: None,
        distance: Scalar::new(distance),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: BooleanMode::NewBody,
        target_body: None,
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    }));
    OperationRecord::new(RecordId(Uuid::from_u128(seed)), 0, "Extrude", op)
}

fn add_extrude(seed: u128, distance: f64) -> EditCommand {
    EditCommand::AddOperation {
        record: extrude_record(seed, distance),
        at_cursor: true,
    }
}

fn runtime_with(backend: Arc<FakeBackend>) -> DocumentRuntime {
    let engine: Arc<dyn GeometryEngine> = backend.clone();
    let meshes: Arc<dyn MeshProvider> = backend;
    DocumentRuntime::new_blank(engine, meshes)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn apply_then_regen_publishes_body_and_marks_feature_ok() {
    let mut rt = runtime_with(Arc::new(FakeBackend::new()));
    let out = rt.apply(add_extrude(0x10, 25.0)).unwrap();
    assert!(matches!(out.regen, onecad_core::edit::RegenHint::ToEnd));

    let report = rt
        .run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await;
    assert!(
        matches!(report.outcome, Outcome::Published(_)),
        "{:?}",
        report.outcome
    );

    let proj = rt.projection();
    let body = BodyId(Uuid::from_u128(0x10)).to_string();
    assert!(proj.bodies.contains_key(&body), "regen body in projection");
    assert_eq!(proj.features.len(), 1);
    assert_eq!(proj.features[0].value_text, "25.0 mm");
    assert_eq!(proj.features[0].status, crate::dto::FeatureStatus::Ok);
    assert!(proj.dirty);
}

#[tokio::test]
async fn undo_redo_round_trips_the_timeline() {
    let mut rt = runtime_with(Arc::new(FakeBackend::new()));
    rt.apply(add_extrude(0x10, 10.0)).unwrap();
    rt.apply(add_extrude(0x11, 20.0)).unwrap();
    assert_eq!(rt.projection().features.len(), 2);

    assert!(rt.undo(), "undo removes the second op");
    assert_eq!(rt.projection().features.len(), 1);

    assert!(rt.redo().unwrap(), "redo re-applies it");
    assert_eq!(rt.projection().features.len(), 2);

    // The redo re-executed the forward command → revision advanced past the apply.
    assert!(rt.revision().0 >= 4);
}

#[tokio::test]
async fn d1_adoption_rejects_malformed_body_id() {
    // Op 0x10 mints a body whose id is NOT derived from any known opId.
    let mut overrides = HashMap::new();
    overrides.insert(0usize, vec![BodyId(Uuid::from_u128(0xBAD))]);
    let mut rt = runtime_with(Arc::new(FakeBackend::with_overrides(overrides)));
    rt.apply(add_extrude(0x10, 10.0)).unwrap();

    let report = rt
        .run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await;
    match report.outcome {
        Outcome::EngineFailed(EngineError::Protocol { .. }) => {}
        other => panic!("malformed body must reject the plan, got {other:?}"),
    }
    // Nothing published: no body, no document-changed payload.
    assert!(rt.projection().bodies.is_empty());
    assert!(report.document_change().is_none());
}

#[tokio::test]
async fn d1_adoption_rejects_colliding_body_id() {
    // Two ops (0x10, 0x11); the second re-mints op-0's body id → collision.
    let mut overrides = HashMap::new();
    overrides.insert(1usize, vec![BodyId(Uuid::from_u128(0x10))]);
    let mut rt = runtime_with(Arc::new(FakeBackend::with_overrides(overrides)));
    rt.apply(add_extrude(0x10, 10.0)).unwrap();
    rt.apply(add_extrude(0x11, 20.0)).unwrap();

    let report = rt
        .run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await;
    assert!(
        matches!(
            report.outcome,
            Outcome::EngineFailed(EngineError::Protocol { .. })
        ),
        "collision must reject the plan, got {:?}",
        report.outcome
    );
    assert!(rt.projection().bodies.is_empty());
}

#[tokio::test]
async fn mesh_cache_miss_then_hit_returns_identical_bytes() {
    let backend = Arc::new(FakeBackend::new());
    let mut rt = runtime_with(backend);
    rt.apply(add_extrude(0x10, 10.0)).unwrap();
    rt.run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await;

    let body = BodyId(Uuid::from_u128(0x10));
    let first = rt
        .get_mesh(body, Lod::Coarse, None)
        .await
        .expect("miss → fetch");
    let expected = b"MESH1:00000000-0000-0000-0000-000000000010:coarse".to_vec();
    assert_eq!(*first, expected, "provider bytes served verbatim");

    let second = rt
        .get_mesh(body, Lod::Coarse, None)
        .await
        .expect("cache hit");
    assert!(
        Arc::ptr_eq(&first, &second),
        "hit returns the same cached Arc"
    );
}

#[tokio::test]
async fn regen_report_builds_document_change_payload() {
    let mut rt = runtime_with(Arc::new(FakeBackend::new()));
    rt.apply(add_extrude(0x10, 10.0)).unwrap();
    let report = rt
        .run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await;

    let change = report
        .document_change()
        .expect("published → change payload");
    assert_eq!(change.changed_bodies.len(), 1);
    let ref_ = &change.changed_bodies[0];
    assert_eq!(ref_.body_id, "00000000-0000-0000-0000-000000000010");
    // meshKey = "<bodyId>:<lod>:<generation>" (matches the mock's mockMeshKey).
    assert!(
        ref_.mesh_key
            .starts_with(&format!("{}:coarse:", ref_.body_id)),
        "{}",
        ref_.mesh_key
    );
    assert!(change.removed_bodies.is_empty());
}

#[tokio::test]
async fn get_mesh_without_geometry_is_a_miss() {
    let mut rt = runtime_with(Arc::new(FakeBackend::new()));
    // No regen yet → no snapshot → get_mesh returns None (not a panic).
    let got = rt
        .get_mesh(BodyId(Uuid::from_u128(0x10)), Lod::Coarse, None)
        .await;
    assert!(got.is_none());
}

#[tokio::test]
async fn save_then_reopen_round_trips_the_document() {
    use onecad_core::io::container::SaveMeta;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("model.onecad");

    let mut rt = runtime_with(Arc::new(FakeBackend::new()));
    rt.apply(add_extrude(0x10, 25.0)).unwrap();
    rt.run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await;

    let meta = SaveMeta {
        app_version: "0.1.0-test".into(),
        occt_fingerprint: None,
        created: "2026-07-17T00:00:00Z".into(),
        modified: "2026-07-17T00:00:00Z".into(),
    };
    rt.save(&path, meta).unwrap();
    assert!(!rt.is_dirty(), "save clears the dirty flag");

    // Reopen with a fresh backend: the timeline (feature) + merged geometry body
    // survive the round-trip; a reopened document starts clean.
    let backend = Arc::new(FakeBackend::new());
    let engine: Arc<dyn GeometryEngine> = backend.clone();
    let meshes: Arc<dyn MeshProvider> = backend;
    let reopened = DocumentRuntime::open(&path, engine, meshes).unwrap();
    let proj = reopened.projection();
    assert_eq!(proj.features.len(), 1);
    assert_eq!(proj.features[0].value_text, "25.0 mm");
    assert!(
        proj.bodies
            .contains_key(&BodyId(Uuid::from_u128(0x10)).to_string()),
        "merged regen body persisted"
    );
    assert!(!reopened.is_dirty());
}
