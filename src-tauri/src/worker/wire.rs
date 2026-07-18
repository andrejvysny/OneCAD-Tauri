//! Wire translation between the core [`GeometryEngine`] domain types and the
//! OCW1 SCHEMA JSON — the seam the [`WorkerManager`](super::manager::WorkerManager)
//! speaks over [`ProtocolClient`](onecad_protocol::client::ProtocolClient).
//!
//! Pure functions (no async, no IO). They map:
//!
//! * a [`PlanRequest`] → `ExecutePlan.args` (SCHEMA §7.2), each op serialized to
//!   the `{opType, opId, inputs, params, determinism}` wire shape (§7.3);
//! * a streamed `planStep` `event` payload → [`PlanStepEvent`] — the key boundary
//!   is `bodyEvents`/`elementMapDelta` `bodyId` strings **`body_<opId>` →
//!   `BodyId(opId uuid)`** (R-WP10 inherited flag; adoption re-checks
//!   `body.as_uuid() ∈ known_ops` in [`validate_created`](super::validate_created));
//! * a terminal `PlanPrepared` result → [`PlanPrepared`];
//! * lifecycle/accept/head/tessellate args + results.
//!
//! `NeedsRepair` payloads are parsed as **state** (SCHEMA §8/§9) into the step's
//! `needs_repair`, never mapped to an [`EngineError`]. `scoringVersion` rides
//! through verbatim (the `RepairItem` already carries the optional field).

use serde_json::{json, Value};
use uuid::Uuid;

use onecad_core::document::body::BodyLifecycleEvent;
use onecad_core::document::record::Operation;
use onecad_core::document::refs::ElementKind;
use onecad_core::document::repair::RepairItem;
use onecad_core::ids::{
    BodyId, DocumentRevision, ElementId, JobId, SnapshotId, TopoKey, WorkerEpoch,
};
use onecad_core::regen::{
    AcceptResult, BodySelector, Diagnostic, ElementMapDelta, ElementMapEntry, EngineError,
    HistoryPrefixHash, OpFailureCode, OpenSessionRequest, PlanPrepared, PlanRequest, PlanStepEvent,
    PlannedOp, SessionMode, Severity, Signature, StepResult, StepSignatures, StepStatus,
    StoppedReason, TessellateRequest, WorkerHead,
};

use onecad_protocol::messages::{ErrorCode, ErrorObject};

use super::lod_str;

// ─────────────────────────────────────────────────────────────────────────────
// BodyId ↔ wire (`body_<opId>`)
// ─────────────────────────────────────────────────────────────────────────────

/// The wire form of a [`BodyId`]: `body_<uuid>` (SCHEMA §2 — a NewBody id is
/// `body_<opId>`, the `opId` being the Rust-minted record-id uuid).
#[must_use]
pub fn body_id_wire(body: BodyId) -> String {
    format!("body_{}", body.0)
}

/// Parses a worker `body_<opId>` string back to a core [`BodyId`] (R-WP10 flag):
/// strip the `body_` prefix and parse the remainder as the op uuid. Split children
/// (`body_<opId>:<k>`) are deferred (W-WP6) and rejected here.
///
/// # Errors
/// A human reason on a missing prefix, a split-child form, or a non-uuid opId.
pub fn parse_body_id(s: &str) -> Result<BodyId, String> {
    let op = s
        .strip_prefix("body_")
        .ok_or_else(|| format!("bodyId {s:?} missing 'body_' prefix (D1)"))?;
    if op.contains(':') {
        return Err(format!("split-child bodyId {s:?} deferred to W-WP6"));
    }
    Uuid::parse_str(op)
        .map(BodyId)
        .map_err(|e| format!("bodyId {s:?} opId is not a uuid: {e}"))
}

/// The wire `jobId` (SCHEMA §2 `u64`) for a core [`JobId`].
///
/// **Collision-safety invariant:** a `JobId` is minted from a strictly-monotonic
/// per-document `u64` counter via `Uuid::from_u128(u128::from(counter))` (see
/// `DocumentRuntime::next_job_id`), so the uuid's full 128-bit value equals the
/// counter and always fits in the low 64 bits. Truncating to `u64` here is
/// therefore lossless and collision-free per connection — two distinct jobs never
/// map to the same wire id. The `debug_assert` pins that invariant at the
/// truncation site: a `JobId` with any high bits set would be a mis-minted id.
#[must_use]
pub fn job_id_wire(job: JobId) -> u64 {
    debug_assert_eq!(
        job.0.as_u128() >> 64,
        0,
        "JobId must be minted from a monotonic u64 counter (no high bits) so the wire \
         truncation is collision-free"
    );
    job.0.as_u128() as u64
}

// ─────────────────────────────────────────────────────────────────────────────
// ExecutePlan args (SCHEMA §7.2 / §7.3)
// ─────────────────────────────────────────────────────────────────────────────

/// Builds the `ExecutePlan.args` for a fenced [`PlanRequest`] (SCHEMA §7.2).
#[must_use]
pub fn execute_plan_args(req: &PlanRequest) -> Value {
    let ops: Vec<Value> = req.ops.iter().map(wire_op).collect();
    let prefix: Vec<Value> = req
        .prefix_hashes
        .iter()
        .map(|h| Value::String(h.as_str().to_string()))
        .collect();
    let mut args = json!({
        "jobId": job_id_wire(req.job_id),
        "documentRevision": req.document_revision.0,
        "workerEpoch": req.worker_epoch.0,
        "expectedBaseHash": req.expected_base_hash.as_str(),
        "prefixHashes": prefix,
        "policyVersions": {
            "quantizationVersion": req.policy_versions.quantization,
            "solverPolicyVersion": req.policy_versions.solver_policy,
            "descriptorVersion": req.policy_versions.descriptor,
            "resolverVersion": req.policy_versions.resolver,
            "signatureVersion": req.policy_versions.signature,
        },
        "targetStep": req.target_step,
        "ops": ops,
    });
    if let Some(cp) = &req.base_checkpoint {
        args["baseCheckpoint"] =
            json!({ "stepIndex": cp.step_index, "checkpointId": cp.checkpoint_id.as_str() });
    }
    if let Some(t) = &req.artifacts.tessellate {
        args["artifacts"] =
            json!({ "tessellate": { "lod": lod_str(t.lod), "includeEdges": t.include_edges } });
    }
    args
}

/// One op in `ExecutePlan.ops` (SCHEMA §7.3): `{opType, opId, inputs, params,
/// determinism}`. `opType`/`params` come from the typed [`Operation`] (same split
/// the planner hashes over); `inputs`/`determinism` serialize their core structs.
fn wire_op(op: &PlannedOp) -> Value {
    let op_val = serde_json::to_value(&op.operation).unwrap_or(Value::Null);
    let (op_type, params) = match &op.operation {
        Operation::Known(_) => (
            op_val.get("opType").cloned().unwrap_or(Value::Null),
            op_val.get("params").cloned().unwrap_or(Value::Null),
        ),
        Operation::Opaque(_) => {
            let mut obj = op_val.as_object().cloned().unwrap_or_default();
            let t = obj.remove("opType").unwrap_or(Value::Null);
            (t, Value::Object(obj))
        }
    };
    json!({
        "opType": op_type,
        "opId": op.record_id.to_string(),
        "inputs": serde_json::to_value(&op.inputs).unwrap_or(Value::Null),
        "params": params,
        "determinism": serde_json::to_value(&op.determinism).unwrap_or(Value::Null),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// planStep event → PlanStepEvent (SCHEMA §7.2)
// ─────────────────────────────────────────────────────────────────────────────

/// Parses one `planStep` event payload into a core [`PlanStepEvent`].
///
/// # Errors
/// A human reason on a malformed `bodyId` / `elementMapDelta` / `needsRepair`
/// payload (surfaced by the caller as `PROTOCOL_ERROR`).
pub fn parse_plan_step(payload: &Value, fallback_step: usize) -> Result<PlanStepEvent, String> {
    let step_index = payload
        .get("stepIndex")
        .and_then(Value::as_u64)
        .map_or(fallback_step, |s| s as usize);
    Ok(PlanStepEvent {
        step_index,
        body_events: parse_body_events(payload.get("bodyEvents"))?,
        element_map_delta: parse_element_delta(payload.get("elementMapDelta"))?,
        needs_repair: parse_needs_repair(payload.get("needsRepair"), step_index)?,
        signatures: parse_signatures(payload.get("signatures")),
        diagnostics: parse_diagnostics(payload.get("diagnostics")),
    })
}

fn parse_body_events(v: Option<&Value>) -> Result<Vec<BodyLifecycleEvent>, String> {
    let arr = v.and_then(Value::as_array).cloned().unwrap_or_default();
    arr.iter().map(parse_body_event).collect()
}

fn parse_body_event(ev: &Value) -> Result<BodyLifecycleEvent, String> {
    let kind = ev.get("kind").and_then(Value::as_str).unwrap_or("");
    let body = || body_field(ev, "bodyId");
    match kind {
        "created" => Ok(BodyLifecycleEvent::Created { body: body()? }),
        "modified" => Ok(BodyLifecycleEvent::Modified { body: body()? }),
        "deleted" => Ok(BodyLifecycleEvent::Deleted { body: body()? }),
        "split" => Ok(BodyLifecycleEvent::Split {
            parent: body_field(ev, "parent")?,
            children: body_array(ev.get("children"))?,
        }),
        "merged" => Ok(BodyLifecycleEvent::Merged {
            inputs: body_array(ev.get("inputs"))?,
            winner: body_field(ev, "winner")?,
        }),
        other => Err(format!("unknown bodyEvent kind {other:?}")),
    }
}

fn body_field(ev: &Value, key: &str) -> Result<BodyId, String> {
    parse_body_id(ev.get(key).and_then(Value::as_str).unwrap_or(""))
}

fn body_array(v: Option<&Value>) -> Result<Vec<BodyId>, String> {
    str_array(v).iter().map(|s| parse_body_id(s)).collect()
}

fn parse_element_delta(v: Option<&Value>) -> Result<ElementMapDelta, String> {
    let get = |k: &str| v.and_then(|d| d.get(k));
    Ok(ElementMapDelta {
        added: parse_entries(get("added"))?,
        relabeled: parse_entries(get("relabeled"))?,
        removed: str_array(get("removed"))
            .into_iter()
            .map(ElementId::new)
            .collect(),
    })
}

fn parse_entries(v: Option<&Value>) -> Result<Vec<ElementMapEntry>, String> {
    let arr = v.and_then(Value::as_array).cloned().unwrap_or_default();
    arr.iter()
        .map(|e| {
            Ok(ElementMapEntry {
                element_id: ElementId::new(
                    e.get("elementId").and_then(Value::as_str).unwrap_or(""),
                ),
                topo_key: TopoKey::new(e.get("topoKey").and_then(Value::as_str).unwrap_or("")),
                kind: parse_kind(e.get("kind").and_then(Value::as_str).unwrap_or("face")),
                body: body_field(e, "bodyId")?,
            })
        })
        .collect()
}

/// Parses `needsRepair[]` **state** (SCHEMA §9), injecting the step index each
/// item omits (it is implicit from the enclosing `planStep`). `scoringVersion`
/// rides through as the `RepairItem`'s optional field.
fn parse_needs_repair(v: Option<&Value>, step: usize) -> Result<Vec<RepairItem>, String> {
    let arr = v.and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let mut obj = item;
        if let Some(map) = obj.as_object_mut() {
            map.entry("stepIndex".to_string()).or_insert(json!(step));
        }
        out.push(serde_json::from_value(obj).map_err(|e| format!("needsRepair parse: {e}"))?);
    }
    Ok(out)
}

fn parse_signatures(v: Option<&Value>) -> StepSignatures {
    let sig = |k: &str| {
        Signature::new(
            v.and_then(|s| s.get(k))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        )
    };
    StepSignatures {
        geometry: sig("geometry"),
        body_lifecycle: sig("bodyLifecycle"),
        referenced_binding: sig("referencedBinding"),
    }
}

fn parse_diagnostics(v: Option<&Value>) -> Vec<Diagnostic> {
    v.and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|d| Diagnostic {
                    severity: match d.get("severity").and_then(Value::as_str) {
                        Some("error") => Severity::Error,
                        Some("info") => Severity::Info,
                        _ => Severity::Warning,
                    },
                    code: d
                        .get("code")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    message: d
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

// ─────────────────────────────────────────────────────────────────────────────
// PlanPrepared (SCHEMA §7.2)
// ─────────────────────────────────────────────────────────────────────────────

/// Parses a terminal `PlanPrepared` result. `job` is the [`JobId`] Rust sent (the
/// executor checks the prepare is for *this* job), not re-parsed from the wire.
///
/// # Errors
/// A human reason on a missing `preparedSnapshotId` or a malformed `bodyIds`.
pub fn parse_plan_prepared(job: JobId, result: &Value) -> Result<PlanPrepared, String> {
    let prepared_snapshot_id = SnapshotId(
        result
            .get("preparedSnapshotId")
            .and_then(Value::as_u64)
            .ok_or("PlanPrepared missing preparedSnapshotId")?,
    );
    let last_valid_step = match result.get("lastValidStep") {
        Some(Value::Number(n)) => n.as_u64().map(|v| v as usize),
        _ => None,
    };
    let stopped_reason = match result.get("stoppedReason").and_then(Value::as_str) {
        Some("opFailed") => StoppedReason::OpFailed,
        Some("needsRepair") => StoppedReason::NeedsRepair,
        _ => StoppedReason::Completed,
    };
    Ok(PlanPrepared {
        job_id: job,
        prepared_snapshot_id,
        last_valid_step,
        stopped_reason,
        per_step: parse_per_step(result.get("perStepResults"))?,
        history_prefix_hash: HistoryPrefixHash::new(
            result
                .get("historyPrefixHash")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
    })
}

fn parse_per_step(v: Option<&Value>) -> Result<Vec<StepResult>, String> {
    let arr = v.and_then(Value::as_array).cloned().unwrap_or_default();
    arr.iter()
        .map(|r| {
            Ok(StepResult {
                step_index: r.get("stepIndex").and_then(Value::as_u64).unwrap_or(0) as usize,
                status: match r.get("status").and_then(Value::as_str) {
                    Some("needsRepair") => StepStatus::NeedsRepair,
                    Some("opFailed") => StepStatus::OpFailed,
                    _ => StepStatus::Ok,
                },
                body_ids: body_array(r.get("bodyIds"))?,
            })
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle / accept / head / tessellate
// ─────────────────────────────────────────────────────────────────────────────

/// `OpenSession.args` (SCHEMA §7.1).
#[must_use]
pub fn open_session_args(req: &OpenSessionRequest) -> Value {
    json!({
        "documentId": req.document_id.to_string(),
        "documentRevision": req.document_revision.0,
        "workerEpoch": req.worker_epoch.0,
        "mode": match req.mode { SessionMode::Fast => "fast", SessionMode::Determinism => "determinism" },
    })
}

/// Parses an `OpenSession` result head (SCHEMA §7.1); `epoch` is the epoch Rust
/// opened with.
#[must_use]
pub fn parse_open_session(result: &Value, epoch: WorkerEpoch) -> WorkerHead {
    let head = result.get("workerHead");
    WorkerHead {
        document_revision: DocumentRevision(u64_at(head, "documentRevision")),
        worker_epoch: epoch,
        snapshot_id: SnapshotId(u64_at(head, "snapshotId")),
        history_prefix_hash: HistoryPrefixHash::empty(),
        has_scratch: false,
    }
}

/// Parses a `GetWorkerHead` result (SCHEMA §7.1).
#[must_use]
pub fn parse_worker_head(result: &Value) -> WorkerHead {
    WorkerHead {
        document_revision: DocumentRevision(
            result
                .get("documentRevision")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
        worker_epoch: WorkerEpoch(
            result
                .get("workerEpoch")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
        snapshot_id: SnapshotId(
            result
                .get("snapshotId")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
        history_prefix_hash: HistoryPrefixHash::new(
            result
                .get("historyPrefixHash")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ),
        has_scratch: result
            .get("hasScratch")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// Parses an `AcceptPrepared` result (SCHEMA §7.2).
#[must_use]
pub fn parse_accept(result: &Value) -> AcceptResult {
    AcceptResult {
        snapshot_id: SnapshotId(
            result
                .get("snapshotId")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
        document_revision: DocumentRevision(
            result
                .get("documentRevision")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ),
    }
}

/// `Tessellate.args` (SCHEMA §7.6).
#[must_use]
pub fn tessellate_args(req: &TessellateRequest) -> Value {
    let bodies = match &req.bodies {
        BodySelector::All => json!("all"),
        BodySelector::Ids(ids) => json!(ids.iter().map(|b| body_id_wire(*b)).collect::<Vec<_>>()),
    };
    json!({ "bodyIds": bodies, "lod": lod_str(req.lod), "includeEdges": req.include_edges })
}

/// `ExportStep.args` (SCHEMA §7.8).
#[must_use]
pub fn export_step_args(path: &str, bodies: &[BodyId], schema: &str) -> Value {
    json!({
        "path": path,
        "bodyIds": bodies.iter().map(|b| body_id_wire(*b)).collect::<Vec<_>>(),
        "schema": schema,
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Error mapping (SCHEMA §8) — NeedsRepair is NEVER here.
// ─────────────────────────────────────────────────────────────────────────────

/// Maps a wire [`ErrorObject`] to the core [`EngineError`] taxonomy (SCHEMA §8).
#[must_use]
pub fn map_error(err: &ErrorObject) -> EngineError {
    let op = |code| EngineError::OpFailed {
        code,
        recoverable: true,
        message: err.message.clone(),
    };
    match err.code {
        ErrorCode::OpFailed => op(OpFailureCode::OpFailed),
        ErrorCode::RefUnresolved => op(OpFailureCode::RefUnresolved),
        ErrorCode::GeometryInvalid => op(OpFailureCode::GeometryInvalid),
        ErrorCode::Unsupported => op(OpFailureCode::Unsupported),
        ErrorCode::Cancelled => EngineError::Cancelled,
        ErrorCode::ProtocolError => EngineError::Protocol {
            message: err.message.clone(),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Small helpers
// ─────────────────────────────────────────────────────────────────────────────

fn parse_kind(s: &str) -> ElementKind {
    match s {
        "edge" => ElementKind::Edge,
        "vertex" => ElementKind::Vertex,
        _ => ElementKind::Face,
    }
}

fn str_array(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn u64_at(v: Option<&Value>, key: &str) -> u64 {
    v.and_then(|o| o.get(key))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_id_round_trips_through_wire() {
        let b = BodyId(Uuid::from_u128(0x4a1));
        let wire = body_id_wire(b);
        assert!(wire.starts_with("body_"));
        assert_eq!(parse_body_id(&wire).unwrap(), b);
    }

    #[test]
    fn job_id_wire_is_lossless_for_counter_minted_ids() {
        // JobId collision-safety invariant: counter-minted ids (u128::from(u64))
        // truncate to u64 losslessly, so distinct counters ⇒ distinct wire ids.
        for counter in [0u64, 1, 2, 88, u32::MAX as u64, u64::MAX] {
            let job = JobId(Uuid::from_u128(u128::from(counter)));
            assert_eq!(job_id_wire(job), counter, "wire id must equal the counter");
        }
        // A large counter and its successor never collide on the wire.
        let a = JobId(Uuid::from_u128(u128::from(u64::MAX - 1)));
        let b = JobId(Uuid::from_u128(u128::from(u64::MAX)));
        assert_ne!(job_id_wire(a), job_id_wire(b));
    }

    #[test]
    fn parse_body_id_rejects_bad_shapes() {
        assert!(parse_body_id("body_not-a-uuid").is_err());
        assert!(parse_body_id("7").is_err(), "missing prefix");
        assert!(
            parse_body_id("body_00000000-0000-0000-0000-000000000001:0").is_err(),
            "split child deferred"
        );
    }

    #[test]
    fn plan_step_parses_created_body_and_delta() {
        let op = Uuid::from_u128(0x10);
        let payload = json!({
            "stepIndex": 3,
            "bodyEvents": [ { "kind": "created", "bodyId": format!("body_{op}") } ],
            "elementMapDelta": {
                "added": [ { "elementId": "el_1", "topoKey": "f:2", "kind": "face", "bodyId": format!("body_{op}") } ],
                "removed": ["el_9"], "relabeled": []
            },
            "needsRepair": [],
            "signatures": { "geometry": "aa", "bodyLifecycle": "bb", "referencedBinding": "cc" },
            "diagnostics": [ { "severity": "warning", "code": "X", "message": "m" } ]
        });
        let step = parse_plan_step(&payload, 0).unwrap();
        assert_eq!(step.step_index, 3);
        assert!(
            matches!(step.body_events[0], BodyLifecycleEvent::Created { body } if body == BodyId(op))
        );
        assert_eq!(step.element_map_delta.added[0].body, BodyId(op));
        assert_eq!(step.element_map_delta.removed[0], ElementId::new("el_9"));
        assert_eq!(step.signatures.geometry.as_str(), "aa");
        assert_eq!(step.diagnostics.len(), 1);
    }

    #[test]
    fn plan_step_rejects_unadoptable_body_id() {
        let payload = json!({
            "stepIndex": 0,
            "bodyEvents": [ { "kind": "created", "bodyId": "body_bogus" } ]
        });
        assert!(parse_plan_step(&payload, 0).is_err());
    }

    #[test]
    fn needs_repair_injects_step_index_and_keeps_scoring_version() {
        let payload = json!({
            "stepIndex": 5,
            "needsRepair": [ {
                "refId": "op_5.input0", "ladderFailed": "descriptor", "reason": "ambiguous",
                "scoringVersion": 1, "candidates": []
            } ]
        });
        let step = parse_plan_step(&payload, 0).unwrap();
        assert_eq!(step.needs_repair.len(), 1);
        assert_eq!(step.needs_repair[0].step_index, 5);
        assert_eq!(step.needs_repair[0].scoring_version, Some(1));
    }

    #[test]
    fn plan_prepared_parses_terminal() {
        let job = JobId(Uuid::from_u128(7));
        let op = Uuid::from_u128(0x10);
        let result = json!({
            "planPrepared": true, "preparedSnapshotId": 5013, "lastValidStep": 6,
            "stoppedReason": "completed",
            "perStepResults": [ { "stepIndex": 6, "status": "ok", "bodyIds": [ format!("body_{op}") ] } ],
            "historyPrefixHash": "9c4d"
        });
        let p = parse_plan_prepared(job, &result).unwrap();
        assert_eq!(p.job_id, job);
        assert_eq!(p.prepared_snapshot_id, SnapshotId(5013));
        assert_eq!(p.last_valid_step, Some(6));
        assert_eq!(p.stopped_reason, StoppedReason::Completed);
        assert_eq!(p.per_step[0].body_ids[0], BodyId(op));
        assert_eq!(p.history_prefix_hash.as_str(), "9c4d");
    }

    #[test]
    fn plan_prepared_base_only_last_valid_null() {
        let job = JobId(Uuid::from_u128(1));
        let result = json!({
            "preparedSnapshotId": 1, "lastValidStep": null, "stoppedReason": "needsRepair",
            "perStepResults": [], "historyPrefixHash": "e3b0"
        });
        let p = parse_plan_prepared(job, &result).unwrap();
        assert_eq!(p.last_valid_step, None);
        assert_eq!(p.stopped_reason, StoppedReason::NeedsRepair);
    }

    #[test]
    fn error_mapping_keeps_needs_repair_out() {
        let e = map_error(&ErrorObject {
            code: ErrorCode::ProtocolError,
            message: "boom".into(),
            detail: None,
            retriable: false,
        });
        assert!(matches!(e, EngineError::Protocol { .. }));
        let e = map_error(&ErrorObject {
            code: ErrorCode::OpFailed,
            message: "x".into(),
            detail: None,
            retriable: false,
        });
        assert!(matches!(
            e,
            EngineError::OpFailed {
                recoverable: true,
                ..
            }
        ));
    }
}
