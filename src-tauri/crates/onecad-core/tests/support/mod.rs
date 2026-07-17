//! Shared support for the regen golden fixtures (R-WP7): the scripted
//! [`FakeEngine`] test double + deterministic record/plan builders.
//!
//! The `FakeEngine` implements [`GeometryEngine`] with no OCCT: each timeline
//! step is answered from a queued [`StepScript`] keyed by step index
//! (`Ok`/`NeedsRepair`/`Fail`/`Crash`/`Hang`), it records a full call log (plans
//! received incl. `expected_base_hash`, accepts, discards, cancels) for
//! assertions, and it mints **deterministic** snapshot ids. It lives here (not in
//! the library) so it never ships in production builds, matching this crate's
//! `tests/common` pattern; it is shared by the `regen_executor` and
//! `regen_planner` integration tests via `mod support;`.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{mpsc, Notify};
use uuid::Uuid;

use onecad_core::document::body::{BodyLifecycleEvent, BodyRegistry};
use onecad_core::document::element_index::ElementIndex;
use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord,
};
use onecad_core::document::repair::RepairItem;
use onecad_core::document::variables::Scalar;
use onecad_core::history::Timeline;
use onecad_core::ids::{
    BodyId, DocumentId, DocumentRevision, ElementId, JobId, RecordId, SnapshotId, WorkerEpoch,
};
use onecad_core::regen::{
    AcceptResult, AcquireRequest, CheckpointArtifacts, Diagnostic, ElementMapDelta, EngineError,
    Fencing, GeometryEngine, HistoryPrefixHash, OpFailureCode, OpenSessionRequest, PlanArtifacts,
    PlanContext, PlanEvent, PlanPrepared, PlanRequest, PlanStepEvent, PolicyVersions,
    RefResolution, RegenPlan, RegenPlanner, RegenRequest, ResolveRequest, RestoreRequest,
    RestoreResult, Severity, Signature, StepResult, StepSignatures, StepStatus, StoppedReason,
    TessellateRequest, TessellateResult, WorkerElementEvidence, WorkerHead,
};

// ─────────────────────────────────────────────────────────────────────────────
// Deterministic builders
// ─────────────────────────────────────────────────────────────────────────────

/// A deterministic record id from a small seed.
pub fn rid(n: u128) -> RecordId {
    RecordId(Uuid::from_u128(n))
}

/// The default body a plain `Ok` step produces (derived from the op's record id
/// so it is deterministic and unique per op).
pub fn default_body_of(record: RecordId) -> BodyId {
    BodyId(record.as_uuid())
}

/// A minimal blind-extrude record.
pub fn extrude_record(seed: u128, distance: f64) -> OperationRecord {
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
    OperationRecord::new(rid(seed), 0, "Extrude", op)
}

/// A timeline of `n` blind-extrude records (seeds `0x10..`), all applied.
pub fn timeline_of(n: usize) -> Timeline {
    let mut tl = Timeline::new();
    for i in 0..n {
        tl.insert_at_cursor(extrude_record(0x10 + i as u128, 5.0 + i as f64));
    }
    tl
}

/// The default plan context (all policy axes at 1, fingerprint `"fp"`).
pub fn default_ctx() -> PlanContext {
    PlanContext {
        policy_versions: PolicyVersions::default(),
        occt_fingerprint: "fp".into(),
    }
}

/// Compiles a plan and converts it to a worker request with fixed fencing tokens.
pub fn plan_request(
    timeline: &Timeline,
    request: RegenRequest,
    job: JobId,
    revision: DocumentRevision,
    epoch: WorkerEpoch,
) -> PlanRequest {
    let plan: RegenPlan = RegenPlanner::plan(
        timeline,
        &onecad_core::history::DependencyGraph::new(),
        &[],
        request,
        &default_ctx(),
    );
    plan.into_request(
        job,
        revision,
        epoch,
        PolicyVersions::default(),
        PlanArtifacts::default(),
    )
}

/// A deterministic three-signature set for `step`.
pub fn sigs(step: usize) -> StepSignatures {
    StepSignatures {
        geometry: Signature::new(format!("g{step}")),
        body_lifecycle: Signature::new(format!("b{step}")),
        referenced_binding: Signature::new(format!("r{step}")),
    }
}

/// A `Created` body-lifecycle event.
pub fn created(body: BodyId) -> BodyLifecycleEvent {
    BodyLifecycleEvent::Created { body }
}

// ─────────────────────────────────────────────────────────────────────────────
// Scripted engine
// ─────────────────────────────────────────────────────────────────────────────

/// What the [`FakeEngine`] does for one plan step.
#[derive(Debug, Clone)]
pub enum StepScript {
    /// Success: emit the given body events + element-map delta + signatures.
    Ok {
        body_events: Vec<BodyLifecycleEvent>,
        deltas: ElementMapDelta,
        signatures: StepSignatures,
    },
    /// NeedsRepair STATE: emit a step event with these repair items (+ any body
    /// events) and stop; the plan still prepares `m−1`.
    NeedsRepair {
        items: Vec<RepairItem>,
        body_events: Vec<BodyLifecycleEvent>,
    },
    /// Recoverable op failure: emit an error diagnostic and stop at this step
    /// (still a successful `PlanPrepared`, `stoppedReason = opFailed`).
    Fail { code: OpFailureCode },
    /// Hard crash: emit steps so far then a terminal `Failed(Crashed)` (no
    /// prepare).
    Crash,
    /// Emit steps so far then **hang** — hold the sender open (never send a
    /// terminal) until `cancel(job)` drops it. Exercises the executor's
    /// `select!`-on-cancel wakeup.
    Hang,
}

/// The recorded engine call log (for assertions).
#[derive(Debug, Default, Clone)]
pub struct CallLog {
    pub plans: Vec<PlanRequest>,
    pub accepts: Vec<(JobId, Fencing)>,
    pub discards: Vec<JobId>,
    pub cancels: Vec<JobId>,
}

/// How the fake answers `restore_checkpoint` (review F3 / F12 fixtures).
#[derive(Clone)]
pub struct RestoreConfig {
    pub restored: bool,
    pub drift: bool,
    pub base_registry: BodyRegistry,
    pub base_elements: ElementIndex,
}

impl Default for RestoreConfig {
    fn default() -> Self {
        Self {
            restored: true,
            drift: false,
            base_registry: BodyRegistry::new(),
            base_elements: ElementIndex::new(),
        }
    }
}

struct FakeState {
    scripts: BTreeMap<usize, StepScript>,
    log: CallLog,
    snapshot_counter: u64,
    prepared: HashMap<JobId, SnapshotId>,
    hang_notify: HashMap<JobId, Arc<Notify>>,
    worker_epoch: WorkerEpoch,
    /// F10: echo a deliberately wrong `historyPrefixHash` in `PlanPrepared`.
    force_bad_hash: bool,
    /// F2: make `accept_prepared` fail with this error (fencing rejection).
    reject_accept: Option<EngineError>,
    /// F3/F12: how `restore_checkpoint` answers.
    restore: RestoreConfig,
    /// Count of `restore_checkpoint` calls (for retry assertions).
    restore_calls: usize,
}

/// A scripted, OCCT-free [`GeometryEngine`].
pub struct FakeEngine {
    inner: Arc<Mutex<FakeState>>,
}

impl FakeEngine {
    /// A fake with per-step scripts; unlisted steps default to a plain `Ok` that
    /// creates one body (`default_body_of(record)`).
    #[must_use]
    pub fn new(scripts: BTreeMap<usize, StepScript>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(FakeState {
                scripts,
                log: CallLog::default(),
                snapshot_counter: 0,
                prepared: HashMap::new(),
                hang_notify: HashMap::new(),
                worker_epoch: WorkerEpoch(1),
                force_bad_hash: false,
                reject_accept: None,
                restore: RestoreConfig::default(),
                restore_calls: 0,
            })),
        }
    }

    /// A fake where every step succeeds (creating one body each).
    #[must_use]
    pub fn all_ok() -> Self {
        Self::new(BTreeMap::new())
    }

    /// F10: echo a wrong `historyPrefixHash` so the executor's opaque-echo check
    /// fails (`PROTOCOL_ERROR` surfaced as `EngineFailed`).
    #[must_use]
    pub fn with_forced_bad_hash(self) -> Self {
        self.inner.lock().unwrap().force_bad_hash = true;
        self
    }

    /// F2: reject `accept_prepared` with `err` (a fencing rejection).
    #[must_use]
    pub fn with_accept_rejection(self, err: EngineError) -> Self {
        self.inner.lock().unwrap().reject_accept = Some(err);
        self
    }

    /// F3/F12: configure `restore_checkpoint`'s answer (base state + drift flag).
    #[must_use]
    pub fn with_restore(self, config: RestoreConfig) -> Self {
        self.inner.lock().unwrap().restore = config;
        self
    }

    /// A snapshot of the call log.
    #[must_use]
    pub fn log(&self) -> CallLog {
        self.inner.lock().unwrap().log.clone()
    }

    /// How many times `restore_checkpoint` was called (F12 retry assertions).
    #[must_use]
    pub fn restore_calls(&self) -> usize {
        self.inner.lock().unwrap().restore_calls
    }
}

impl FakeState {
    /// The `Ok` script for a step with no explicit entry: create one body from
    /// the op's record id, no element-map change, deterministic signatures.
    fn default_ok(record: RecordId, step: usize) -> StepScript {
        StepScript::Ok {
            body_events: vec![created(default_body_of(record))],
            deltas: ElementMapDelta::default(),
            signatures: sigs(step),
        }
    }
}

#[async_trait]
impl GeometryEngine for FakeEngine {
    async fn execute_plan(&self, request: PlanRequest) -> mpsc::Receiver<PlanEvent> {
        // Build the whole event list while holding the lock (no await under lock),
        // then await-send it from a spawned task (F15).
        let (events, hang_notify): (Vec<PlanEvent>, Option<Arc<Notify>>) = {
            let mut st = self.inner.lock().unwrap();
            st.log.plans.push(request.clone());
            let job = request.job_id;
            st.snapshot_counter += 1;
            let snapshot_id = SnapshotId(5000 + st.snapshot_counter);

            let mut events = Vec::new();
            let mut per_step: Vec<StepResult> = Vec::new();
            let mut last_valid: Option<usize> = None;
            let mut stopped = StoppedReason::Completed;
            let mut send_prepared = true;
            let mut hang = false;

            for op in &request.ops {
                let step = op.step_index;
                let script = st
                    .scripts
                    .get(&step)
                    .cloned()
                    .unwrap_or_else(|| FakeState::default_ok(op.record_id, step));
                match script {
                    StepScript::Ok {
                        body_events,
                        deltas,
                        signatures,
                    } => {
                        let bodies = body_ids(&body_events);
                        events.push(PlanEvent::Step(PlanStepEvent {
                            step_index: step,
                            body_events,
                            element_map_delta: deltas,
                            needs_repair: vec![],
                            signatures,
                            diagnostics: vec![],
                        }));
                        per_step.push(StepResult {
                            step_index: step,
                            status: StepStatus::Ok,
                            body_ids: bodies,
                        });
                        last_valid = Some(step);
                    }
                    StepScript::NeedsRepair { items, body_events } => {
                        events.push(PlanEvent::Step(PlanStepEvent {
                            step_index: step,
                            body_events,
                            element_map_delta: ElementMapDelta::default(),
                            needs_repair: items,
                            signatures: sigs(step),
                            diagnostics: vec![],
                        }));
                        per_step.push(StepResult {
                            step_index: step,
                            status: StepStatus::NeedsRepair,
                            body_ids: vec![],
                        });
                        stopped = StoppedReason::NeedsRepair;
                        break;
                    }
                    StepScript::Fail { code } => {
                        events.push(PlanEvent::Step(PlanStepEvent {
                            step_index: step,
                            body_events: vec![],
                            element_map_delta: ElementMapDelta::default(),
                            needs_repair: vec![],
                            signatures: sigs(step),
                            diagnostics: vec![Diagnostic {
                                severity: Severity::Error,
                                code: format!("{code:?}"),
                                message: format!("scripted op failure at step {step}"),
                            }],
                        }));
                        per_step.push(StepResult {
                            step_index: step,
                            status: StepStatus::OpFailed,
                            body_ids: vec![],
                        });
                        stopped = StoppedReason::OpFailed;
                        break;
                    }
                    StepScript::Crash => {
                        events.push(PlanEvent::Failed(EngineError::Crashed {
                            message: format!("scripted crash at step {step}"),
                        }));
                        send_prepared = false;
                        break;
                    }
                    StepScript::Hang => {
                        hang = true;
                        break;
                    }
                }
            }

            let hang_notify = if hang {
                let notify = Arc::new(Notify::new());
                st.hang_notify.insert(job, notify.clone());
                Some(notify)
            } else {
                if send_prepared {
                    st.prepared.insert(job, snapshot_id);
                    // Echo the OPAQUE token Rust minted for the last executed op
                    // (or the base hash) — the executor verifies this. F10 forces a
                    // wrong echo to exercise the PROTOCOL_ERROR path.
                    let history_prefix_hash = if st.force_bad_hash {
                        HistoryPrefixHash::new("badhash_forced_by_fake")
                    } else {
                        echo_hash(&request, last_valid)
                    };
                    events.push(PlanEvent::Prepared(PlanPrepared {
                        job_id: job,
                        prepared_snapshot_id: snapshot_id,
                        last_valid_step: last_valid,
                        stopped_reason: stopped,
                        per_step,
                        history_prefix_hash,
                    }));
                }
                None
            };
            (events, hang_notify)
        };

        // F15: await-send on a bounded channel (never a lossy try_send), spawned so
        // events stream while the executor consumes. The terminal is never dropped.
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(async move {
            for ev in events {
                if tx.send(ev).await.is_err() {
                    return; // receiver dropped — stop.
                }
            }
            // Hang: hold the sender open until cancel(job) fires; then it drops here
            // and the stream closes. Non-hang: tx drops now, closing the stream.
            if let Some(notify) = hang_notify {
                notify.notified().await;
            }
        });
        rx
    }

    async fn accept_prepared(
        &self,
        job_id: JobId,
        fencing: Fencing,
    ) -> Result<AcceptResult, EngineError> {
        let mut st = self.inner.lock().unwrap();
        st.log.accepts.push((job_id, fencing));
        if let Some(err) = st.reject_accept.clone() {
            return Err(err); // F2: scripted fencing rejection.
        }
        let snapshot_id = st.prepared.remove(&job_id).unwrap_or(SnapshotId(0));
        Ok(AcceptResult {
            snapshot_id,
            document_revision: DocumentRevision(fencing.document_revision.0 + 1),
        })
    }

    async fn discard_prepared(&self, job_id: JobId) -> Result<(), EngineError> {
        let mut st = self.inner.lock().unwrap();
        st.log.discards.push(job_id);
        st.prepared.remove(&job_id);
        Ok(())
    }

    async fn cancel(&self, job_id: JobId) -> Result<(), EngineError> {
        let notify = {
            let mut st = self.inner.lock().unwrap();
            st.log.cancels.push(job_id);
            st.hang_notify.remove(&job_id)
        };
        if let Some(n) = notify {
            n.notify_one(); // release the hung sender → the stream closes.
        }
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

    async fn close_session(
        &self,
        _document_id: DocumentId,
        _worker_epoch: WorkerEpoch,
    ) -> Result<(), EngineError> {
        Ok(())
    }

    async fn reset(
        &self,
        _document_id: DocumentId,
        _worker_epoch: WorkerEpoch,
    ) -> Result<WorkerEpoch, EngineError> {
        let mut st = self.inner.lock().unwrap();
        st.worker_epoch = WorkerEpoch(st.worker_epoch.0 + 1);
        Ok(st.worker_epoch)
    }

    async fn get_worker_head(&self) -> Result<WorkerHead, EngineError> {
        Ok(WorkerHead {
            document_revision: DocumentRevision(0),
            worker_epoch: self.inner.lock().unwrap().worker_epoch,
            snapshot_id: SnapshotId(0),
            history_prefix_hash: HistoryPrefixHash::empty(),
            has_scratch: false,
        })
    }

    async fn tessellate(&self, _req: TessellateRequest) -> Result<TessellateResult, EngineError> {
        Ok(TessellateResult { meshes: vec![] })
    }

    async fn save_checkpoint(
        &self,
        _step_index: usize,
    ) -> Result<CheckpointArtifacts, EngineError> {
        Err(EngineError::OpFailed {
            code: OpFailureCode::Unsupported,
            recoverable: true,
            message: "fake engine does not save checkpoints".into(),
        })
    }

    async fn restore_checkpoint(&self, req: RestoreRequest) -> Result<RestoreResult, EngineError> {
        let mut st = self.inner.lock().unwrap();
        st.restore_calls += 1;
        let cfg = st.restore.clone();
        Ok(RestoreResult {
            restored: cfg.restored,
            snapshot_id: SnapshotId(0),
            drift_detected: cfg.drift,
            drift_detail: None,
            checkpoint_step: req.checkpoint.step_index,
            base_registry: cfg.base_registry,
            base_elements: cfg.base_elements,
        })
    }

    async fn acquire_element_ids(
        &self,
        _req: AcquireRequest,
    ) -> Result<Vec<WorkerElementEvidence>, EngineError> {
        Ok(vec![])
    }

    async fn resolve_refs(&self, _req: ResolveRequest) -> Result<Vec<RefResolution>, EngineError> {
        Ok(vec![])
    }

    async fn ping(&self) -> Result<(), EngineError> {
        Ok(())
    }
}

/// The body ids created/modified by a set of lifecycle events (for the per-step
/// summary).
fn body_ids(events: &[BodyLifecycleEvent]) -> Vec<BodyId> {
    let mut ids = Vec::new();
    for e in events {
        for b in e.bodies() {
            if !ids.contains(&b) {
                ids.push(b);
            }
        }
    }
    ids
}

/// A fixed element id for repair-fixture candidates.
pub fn elem(id: &str) -> ElementId {
    ElementId::new(id)
}

/// The opaque `historyPrefixHash` a well-behaved worker echoes for a prepare whose
/// last valid step is `last_valid`: the plan's `prefix_hashes[j]` for the executed
/// op at that step, or `expected_base_hash` for a base-only prepare. Mirrors the
/// executor's own expectation, so the echo verification passes by construction.
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
