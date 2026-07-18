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

use std::collections::BTreeMap;

use serde_json::{json, Value};
use uuid::Uuid;

use onecad_core::document::body::BodyLifecycleEvent;
use onecad_core::document::record::Operation;
use onecad_core::document::refs::{AnchorIntent, ElementKind};
use onecad_core::document::repair::RepairItem;
use onecad_core::ids::{
    BodyId, DocumentRevision, ElementId, EntityId, JobId, SnapshotId, TopoKey, WorkerEpoch,
};
use onecad_core::regen::{
    AcceptResult, AcquireRequest, BodySelector, Diagnostic, ElementMapDelta, ElementMapEntry,
    EngineError, HistoryPrefixHash, OpFailureCode, OpenSessionRequest, PlanPrepared, PlanRequest,
    PlanStepEvent, PlannedOp, RefResolution, ResolveOutcome, ResolveRequest, SessionMode, Severity,
    Signature, StepResult, StepSignatures, StepStatus, StoppedReason, TessellateRequest,
    WorkerElementEvidence, WorkerHead,
};
use onecad_core::sketch::WorldPlane;
use onecad_core::sketch::{Constraint, CurvePosition, Sketch, SketchAttachment, SketchEntity};

use onecad_protocol::messages::{BinSection, ErrorCode, ErrorObject};

use crate::dto::{
    DragSolveDto, PreviewTrianglesDto, SketchRegionDto, SketchSolveStatus, SketchUpsertDto,
};

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
// Solver lane — Sketch → SCHEMA §7.4 wire (the Rust `WireSketch` translator)
// ─────────────────────────────────────────────────────────────────────────────

/// Translates a core [`Sketch`] into the `(plane, entities, constraints)` wire
/// JSON the worker's `WireSketch::translate` consumes (SCHEMA §7.3 entity /
/// constraint shapes, §7.4 solver lane).
///
/// The core model references points **by id** (a [`Line`](SketchEntity::Line)
/// stores its two endpoint ids, an [`Arc`](SketchEntity::Arc)/[`Circle`](SketchEntity::Circle)
/// its center id); this maps 1:1 onto the worker's `p0Ref`/`p1Ref` line form and
/// (for arc/circle) an inlined center coordinate resolved from the center point.
/// [`Ellipse`](SketchEntity::Ellipse) is not translated (the worker's `WireSketch`
/// has no ellipse case — documented V1 limit; ellipses are outside the slice).
#[must_use]
pub fn sketch_wire(sketch: &Sketch) -> (Value, Value, Value) {
    let plane = json!({
        "kind": plane_kind_str(sketch),
        "origin": [sketch.plane.origin.x, sketch.plane.origin.y, sketch.plane.origin.z],
        "xAxis": [sketch.plane.x_axis.x, sketch.plane.x_axis.y, sketch.plane.x_axis.z],
        "yAxis": [sketch.plane.y_axis.x, sketch.plane.y_axis.y, sketch.plane.y_axis.z],
        "normal": [sketch.plane.normal.x, sketch.plane.normal.y, sketch.plane.normal.z],
    });
    let entities: Vec<Value> = sketch
        .entities()
        .iter()
        .filter_map(|e| wire_entity(sketch, e))
        .collect();
    let constraints: Vec<Value> = sketch.constraints().iter().map(wire_constraint).collect();
    (plane, Value::Array(entities), Value::Array(constraints))
}

/// `SketchUpsert.args` (SCHEMA §7.4) for a core [`Sketch`].
#[must_use]
pub fn sketch_upsert_args(sketch: &Sketch) -> Value {
    let (plane, entities, constraints) = sketch_wire(sketch);
    json!({
        "sketchId": sketch.id.to_string(),
        "plane": plane,
        "entities": entities,
        "constraints": constraints,
    })
}

/// `BeginGesture.args` (SCHEMA §7.4). `drag_point` is the point entity being
/// dragged — its wire handle is its uuid (points register under their id).
#[must_use]
pub fn begin_gesture_args(
    sketch_id: &str,
    sketch_revision: u64,
    gesture_id: u64,
    drag_point: EntityId,
    solver_policy_hash: &str,
) -> Value {
    json!({
        "sketchId": sketch_id,
        "sketchRevision": sketch_revision,
        "gestureId": gesture_id,
        "solverPolicyHash": solver_policy_hash,
        "drag": { "pointId": drag_point.to_string() },
        "pointId": drag_point.to_string(),
    })
}

/// `SolveDrag.args` (SCHEMA §7.4) — latest-wins incremental solve.
#[must_use]
pub fn solve_drag_args(gesture_id: u64, seq: u64, drag_point: EntityId, target: [f64; 2]) -> Value {
    json!({
        "gestureId": gesture_id,
        "seq": seq,
        "pointId": drag_point.to_string(),
        "target": [target[0], target[1]],
    })
}

/// `EndGesture.args` (SCHEMA §7.4) — pointer-up final exact solve.
#[must_use]
pub fn end_gesture_args(gesture_id: u64, final_target: Option<[f64; 2]>) -> Value {
    let mut args = json!({ "gestureId": gesture_id });
    if let Some(t) = final_target {
        args["commit"] = json!({ "finalTarget": [t[0], t[1]] });
    }
    args
}

/// `SketchRegions.args` (SCHEMA §7.4).
#[must_use]
pub fn sketch_regions_args(sketch_id: &str) -> Value {
    json!({ "sketchId": sketch_id })
}

/// Parses a `SketchUpsert`/`EndGesture` solve result into a [`SketchUpsertDto`].
/// `EndGesture` also carries a `positions` map (changed points since the gesture
/// began); `SketchUpsert` carries none (identity solve).
///
/// `SketchUpsert` reports the solve `state` (the four PascalCase tokens) directly;
/// `EndGesture` reports a drag `status` (`success`|`partial`|`conflicting`) + `dof`
/// instead, so the solve status is **derived** (`conflicting` ⇒ `Conflicting`, else
/// `dof == 0` ⇒ `FullyConstrained` else `UnderConstrained`).
#[must_use]
pub fn parse_sketch_upsert(sketch_id: &str, result: &Value) -> SketchUpsertDto {
    let dof = parse_dof(result);
    let status = if let Some(state) = result.get("state").and_then(Value::as_str) {
        SketchSolveStatus::parse(state)
    } else {
        match result.get("status").and_then(Value::as_str) {
            Some("conflicting") => SketchSolveStatus::Conflicting,
            _ if dof == 0 => SketchSolveStatus::FullyConstrained,
            _ => SketchSolveStatus::UnderConstrained,
        }
    };
    SketchUpsertDto {
        sketch_id: sketch_id.to_string(),
        sketch_revision: result
            .get("sketchRevision")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        dof,
        status,
        solved_positions: parse_positions(result.get("positions")),
    }
}

/// Parses a `SolveDrag` result into a [`DragSolveDto`]. A stale `seq` may come
/// back `status:"superseded"` (latest-wins) — the caller tolerates it and drops
/// the (empty) positions.
#[must_use]
pub fn parse_solve_drag(result: &Value) -> DragSolveDto {
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("success")
        .to_string();
    DragSolveDto {
        gesture_id: result.get("gestureId").and_then(Value::as_u64).unwrap_or(0),
        seq: result.get("seq").and_then(Value::as_u64).unwrap_or(0),
        superseded: status == "superseded",
        status,
        dof: parse_dof(result),
        conflicting: str_array(result.get("conflicting")),
        positions: parse_positions(result.get("positions")),
        solve_micros: result
            .get("solveMicros")
            .and_then(Value::as_u64)
            .unwrap_or(0),
    }
}

/// Parses a `SketchRegions` result + its response binary tail into region DTOs.
/// `previewTriangles` bins are decoded from the tail (f32 xyz then u32 indices;
/// SCHEMA §7.4) into `(u,v)` positions the frontend fill consumes.
#[must_use]
pub fn parse_sketch_regions(
    result: &Value,
    bin_sections: &[BinSection],
    tail: &[u8],
) -> Vec<SketchRegionDto> {
    let Some(arr) = result.get("regions").and_then(Value::as_array) else {
        return Vec::new();
    };
    arr.iter()
        .map(|r| SketchRegionDto {
            region_id: r
                .get("regionId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            outer_loop: str_array(r.get("outerLoop")),
            holes: r
                .get("holes")
                .and_then(Value::as_array)
                .map(|hs| hs.iter().map(|h| str_array(Some(h))).collect())
                .unwrap_or_default(),
            preview_triangles: parse_preview_triangles(
                r.get("previewTriangles"),
                bin_sections,
                tail,
            ),
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Element identity (SCHEMA §7.5) — AcquireElementIds / ResolveRefs
// ─────────────────────────────────────────────────────────────────────────────

/// `AcquireElementIds.args` (SCHEMA §7.5) — promote TopoKeys to persistent ids.
#[must_use]
pub fn acquire_element_ids_args(req: &AcquireRequest) -> Value {
    let picks: Vec<Value> = req
        .picks
        .iter()
        .map(|p| {
            let mut o = json!({ "topoKey": p.topo_key.as_str() });
            if let Some(anchor) = &p.anchor {
                o["anchor"] = anchor_to_wire(anchor);
            }
            o
        })
        .collect();
    json!({
        "snapshotId": req.snapshot_id.0,
        "bodyId": body_id_wire(req.body),
        "picks": picks,
    })
}

/// Parses an `AcquireElementIds` result into worker evidence (Rust then mints the
/// ids via [`mint_element_ids`](onecad_core::regen::mint_element_ids)). A worker
/// `elementId` (echoed existing binding) rides through as `existing`. `fallback_body`
/// backs a malformed/absent `bodyId`.
#[must_use]
pub fn parse_acquire_evidence(result: &Value, fallback_body: BodyId) -> Vec<WorkerElementEvidence> {
    result
        .get("ids")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|e| WorkerElementEvidence {
                    topo_key: TopoKey::new(e.get("topoKey").and_then(Value::as_str).unwrap_or("")),
                    body: e
                        .get("bodyId")
                        .and_then(Value::as_str)
                        .and_then(|s| parse_body_id(s).ok())
                        .unwrap_or(fallback_body),
                    kind: parse_kind(e.get("kind").and_then(Value::as_str).unwrap_or("face")),
                    anchor: e
                        .get("anchor")
                        .and_then(|a| serde_json::from_value::<AnchorIntent>(a.clone()).ok()),
                    descriptor: e.get("descriptor").cloned(),
                    existing: e
                        .get("elementId")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(ElementId::new),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `ResolveRefs.args` (SCHEMA §7.5) — dry-run ladder for repair dialogs.
#[must_use]
pub fn resolve_refs_args(req: &ResolveRequest) -> Value {
    let refs: Vec<Value> = req
        .refs
        .iter()
        .map(|r| {
            let mut o = serde_json::to_value(&r.element).unwrap_or_else(|_| json!({}));
            if let Some(map) = o.as_object_mut() {
                map.insert("refId".to_string(), json!(r.ref_id));
            }
            o
        })
        .collect();
    json!({ "snapshotId": req.snapshot_id.0, "refs": refs })
}

/// Parses a `ResolveRefs` result into core [`RefResolution`]s. The worker returns
/// a `topoKey` for `autoBind` (Rust mints/binds the real id at bind time); it is
/// carried as the ref's evidence id here (dry-run — nothing is bound). `needsRepair`
/// carries the full [`RepairItem`] evidence.
#[must_use]
pub fn parse_resolve_refs(result: &Value) -> Vec<RefResolution> {
    result
        .get("resolutions")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(parse_one_resolution).collect())
        .unwrap_or_default()
}

fn parse_one_resolution(r: &Value) -> Option<RefResolution> {
    let ref_id = r.get("refId").and_then(Value::as_str)?.to_string();
    let outcome = match r.get("outcome").and_then(Value::as_str)? {
        "autoBind" => {
            // The worker echoes `topoKey` (dry-run — Rust binds the real id later);
            // `elementId` is used when present (an already-bound `unchanged`-like hit).
            let id = r
                .get("elementId")
                .or_else(|| r.get("topoKey"))
                .and_then(Value::as_str)
                .unwrap_or("");
            ResolveOutcome::AutoBind {
                element_id: ElementId::new(id),
                score: r.get("score").and_then(Value::as_f64).unwrap_or(0.0),
                margin: r.get("margin").and_then(Value::as_f64).unwrap_or(0.0),
            }
        }
        "unchanged" => ResolveOutcome::Unchanged,
        "needsRepair" => {
            let mut obj = r.get("needsRepair").cloned().unwrap_or_else(|| json!({}));
            if let Some(map) = obj.as_object_mut() {
                map.entry("stepIndex".to_string()).or_insert(json!(0));
                map.entry("refId".to_string()).or_insert(json!(ref_id));
            }
            ResolveOutcome::NeedsRepair(serde_json::from_value::<RepairItem>(obj).ok()?)
        }
        _ => return None,
    };
    Some(RefResolution { ref_id, outcome })
}

// ─────────────────────────────────────────────────────────────────────────────
// Solver / identity helpers
// ─────────────────────────────────────────────────────────────────────────────

fn plane_kind_str(sketch: &Sketch) -> &'static str {
    match &sketch.attachment {
        SketchAttachment::World { plane } => match plane {
            WorldPlane::XY => "XY",
            WorldPlane::XZ => "XZ",
            WorldPlane::YZ => "YZ",
        },
        // Datum / host-face frames carry a resolved custom basis.
        _ => "custom",
    }
}

/// The `[x, y]` position of a point entity (for inlining arc/circle centers).
fn point_pos(sketch: &Sketch, id: EntityId) -> Option<[f64; 2]> {
    match sketch.get_entity(id)? {
        SketchEntity::Point { at, .. } => Some([at.x, at.y]),
        _ => None,
    }
}

fn wire_entity(sketch: &Sketch, e: &SketchEntity) -> Option<Value> {
    Some(match e {
        SketchEntity::Point {
            id,
            at,
            construction,
            ..
        } => json!({
            "id": id.to_string(), "type": "Point",
            "at": [at.x, at.y], "construction": construction,
        }),
        SketchEntity::Line {
            id,
            start,
            end,
            construction,
        } => json!({
            "id": id.to_string(), "type": "Line",
            "p0Ref": start.to_string(), "p1Ref": end.to_string(), "construction": construction,
        }),
        SketchEntity::Circle {
            id,
            center,
            radius,
            construction,
        } => {
            let c = point_pos(sketch, *center)?;
            json!({
                "id": id.to_string(), "type": "Circle",
                "center": c, "radius": radius, "construction": construction,
            })
        }
        SketchEntity::Arc {
            id,
            center,
            radius,
            start_angle,
            end_angle,
            construction,
        } => {
            let c = point_pos(sketch, *center)?;
            json!({
                "id": id.to_string(), "type": "Arc",
                "center": c, "radius": radius,
                "startAngle": start_angle, "endAngle": end_angle, "construction": construction,
            })
        }
        // No worker `WireSketch` ellipse case — skip (documented V1 limit).
        SketchEntity::Ellipse { .. } => return None,
    })
}

fn wire_constraint(c: &Constraint) -> Value {
    let s = |id: &EntityId| id.to_string();
    match c {
        Constraint::Coincident { point1, point2, .. } => json!({
            "id": cid(c), "type": "Coincident", "entities": [s(point1), s(point2)],
        }),
        Constraint::Horizontal { line, .. } => {
            json!({ "id": cid(c), "type": "Horizontal", "entities": [s(line)] })
        }
        Constraint::Vertical { line, .. } => {
            json!({ "id": cid(c), "type": "Vertical", "entities": [s(line)] })
        }
        Constraint::Fixed { point, .. } => {
            json!({ "id": cid(c), "type": "Fixed", "entities": [s(point)] })
        }
        Constraint::Midpoint { point, line, .. } => {
            json!({ "id": cid(c), "type": "Midpoint", "entities": [s(point), s(line)] })
        }
        Constraint::OnCurve {
            point,
            curve,
            position,
            ..
        } => json!({
            "id": cid(c), "type": "OnCurve",
            "entities": [s(point), s(curve)],
            "positions": ["", curve_position_str(*position)],
        }),
        Constraint::Parallel { line1, line2, .. } => {
            json!({ "id": cid(c), "type": "Parallel", "entities": [s(line1), s(line2)] })
        }
        Constraint::Perpendicular { line1, line2, .. } => {
            json!({ "id": cid(c), "type": "Perpendicular", "entities": [s(line1), s(line2)] })
        }
        Constraint::Tangent {
            entity1, entity2, ..
        } => json!({ "id": cid(c), "type": "Tangent", "entities": [s(entity1), s(entity2)] }),
        Constraint::Concentric {
            entity1, entity2, ..
        } => json!({ "id": cid(c), "type": "Concentric", "entities": [s(entity1), s(entity2)] }),
        Constraint::Equal {
            entity1, entity2, ..
        } => json!({ "id": cid(c), "type": "Equal", "entities": [s(entity1), s(entity2)] }),
        Constraint::Distance {
            entity1,
            entity2,
            value,
            ..
        } => json!({
            "id": cid(c), "type": "Distance",
            "entities": [s(entity1), s(entity2)], "value": value.value,
        }),
        Constraint::HorizontalDistance {
            point1,
            point2,
            value,
            ..
        } => json!({
            "id": cid(c), "type": "HorizontalDistance",
            "entities": [s(point1), s(point2)], "value": value.value,
        }),
        Constraint::VerticalDistance {
            point1,
            point2,
            value,
            ..
        } => json!({
            "id": cid(c), "type": "VerticalDistance",
            "entities": [s(point1), s(point2)], "value": value.value,
        }),
        Constraint::Angle {
            line1,
            line2,
            value,
            ..
        } => json!({
            "id": cid(c), "type": "Angle",
            "entities": [s(line1), s(line2)], "value": value.value,
        }),
        Constraint::Radius { entity, value, .. } => json!({
            "id": cid(c), "type": "Radius", "entities": [s(entity)], "value": value.value,
        }),
        Constraint::Diameter { entity, value, .. } => json!({
            "id": cid(c), "type": "Diameter", "entities": [s(entity)], "value": value.value,
        }),
        Constraint::Symmetric {
            point1,
            point2,
            axis,
            ..
        } => json!({
            "id": cid(c), "type": "Symmetric", "entities": [s(point1), s(point2), s(axis)],
        }),
    }
}

fn cid(c: &Constraint) -> String {
    c.id().to_string()
}

fn curve_position_str(p: CurvePosition) -> &'static str {
    match p {
        CurvePosition::Start => "Start",
        CurvePosition::End => "End",
        CurvePosition::Arbitrary => "Arbitrary",
    }
}

fn anchor_to_wire(anchor: &AnchorIntent) -> Value {
    serde_json::to_value(anchor).unwrap_or_else(|_| json!({}))
}

fn parse_dof(result: &Value) -> u32 {
    result
        .get("dof")
        .and_then(Value::as_i64)
        .map(|d| d.max(0) as u32)
        .unwrap_or(0)
}

/// Parses a solver `positions` map (`{handle: [x, y]}`), keyed by the point
/// entity id (the wire handle for a point).
fn parse_positions(v: Option<&Value>) -> BTreeMap<String, [f64; 2]> {
    let Some(obj) = v.and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    obj.iter()
        .filter_map(|(k, xy)| {
            let a = xy.as_array()?;
            let x = a.first()?.as_f64()?;
            let y = a.get(1)?.as_f64()?;
            Some((k.clone(), [x, y]))
        })
        .collect()
}

/// Decodes one region's `previewTriangles` bin (f32 xyz vertices then u32
/// indices) into `(u,v)` positions + triangle indices.
fn parse_preview_triangles(
    v: Option<&Value>,
    bin_sections: &[BinSection],
    tail: &[u8],
) -> Option<PreviewTrianglesDto> {
    let pt = v?;
    let section_name = pt.get("bin").and_then(Value::as_str)?;
    let vertex_count = pt.get("vertexCount").and_then(Value::as_u64).unwrap_or(0) as usize;
    let triangle_count = pt.get("triangleCount").and_then(Value::as_u64).unwrap_or(0) as usize;
    let section = bin_sections.iter().find(|s| s.name == section_name)?;
    let start = section.off as usize;
    let end = start + section.len as usize;
    let bytes = tail.get(start..end)?;

    let mut positions = Vec::with_capacity(vertex_count * 2);
    for i in 0..vertex_count {
        // xyz f32 per vertex; keep (x, y) — the sketch fill is planar (z == 0).
        let base = i * 12;
        let x = f32::from_le_bytes(bytes.get(base..base + 4)?.try_into().ok()?);
        let y = f32::from_le_bytes(bytes.get(base + 4..base + 8)?.try_into().ok()?);
        positions.push(f64::from(x));
        positions.push(f64::from(y));
    }
    let idx_base = vertex_count * 12;
    let mut indices = Vec::with_capacity(triangle_count * 3);
    for i in 0..(triangle_count * 3) {
        let o = idx_base + i * 4;
        indices.push(u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?));
    }
    Some(PreviewTrianglesDto { positions, indices })
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

#[cfg(test)]
mod solver_wire_tests {
    use super::*;
    use onecad_core::document::variables::Scalar;
    use onecad_core::ids::{ConstraintId, SketchId};
    use onecad_core::math::Vec2;
    use onecad_core::regen::Pick;
    use onecad_core::sketch::{Constraint, Sketch, SketchEntity, WorldPlane};

    fn eid(n: u128) -> EntityId {
        EntityId(Uuid::from_u128(n))
    }
    fn cid(n: u128) -> ConstraintId {
        ConstraintId(Uuid::from_u128(n))
    }

    /// A point-referenced line + a circle (center inlined) + two constraints,
    /// translated to the worker `WireSketch` shapes (SCHEMA §7.3/§7.4).
    #[test]
    fn sketch_wire_maps_topology_to_worker_shapes() {
        let sid = SketchId(Uuid::from_u128(1));
        let (p0, p1, c) = (eid(0x10), eid(0x11), eid(0x12));
        let (line, circle) = (eid(0x20), eid(0x21));
        let mut sk = Sketch::on_world_plane(sid, "S", WorldPlane::XY);
        sk.add_entity(SketchEntity::point(
            p0,
            Vec2::new_unchecked(0.0, 0.0),
            false,
            false,
        ))
        .unwrap();
        sk.add_entity(SketchEntity::point(
            p1,
            Vec2::new_unchecked(40.0, 0.0),
            false,
            false,
        ))
        .unwrap();
        sk.add_entity(SketchEntity::point(
            c,
            Vec2::new_unchecked(10.0, 10.0),
            false,
            false,
        ))
        .unwrap();
        sk.add_entity(SketchEntity::line(line, p0, p1, false))
            .unwrap();
        sk.add_entity(SketchEntity::circle(circle, c, 3.0, false).unwrap())
            .unwrap();
        sk.add_constraint(Constraint::Horizontal { id: cid(1), line })
            .unwrap();
        sk.add_constraint(Constraint::Distance {
            id: cid(2),
            entity1: p0,
            entity2: p1,
            value: Scalar::new(40.0),
        })
        .unwrap();

        let (plane, entities, constraints) = sketch_wire(&sk);
        // Named plane keeps the non-standard XY basis.
        assert_eq!(plane["kind"], "XY");
        assert_eq!(plane["xAxis"], json!([0.0, 1.0, 0.0]));

        let ents = entities.as_array().unwrap();
        // Line references its endpoints by id (p0Ref/p1Ref) — the point-ref form.
        let l = ents
            .iter()
            .find(|e| e["id"] == json!(line.to_string()))
            .unwrap();
        assert_eq!(l["type"], "Line");
        assert_eq!(l["p0Ref"], json!(p0.to_string()));
        assert_eq!(l["p1Ref"], json!(p1.to_string()));
        // Circle inlines its center coordinate.
        let ci = ents
            .iter()
            .find(|e| e["id"] == json!(circle.to_string()))
            .unwrap();
        assert_eq!(ci["type"], "Circle");
        assert_eq!(ci["center"], json!([10.0, 10.0]));
        assert_eq!(ci["radius"], json!(3.0));

        let cons = constraints.as_array().unwrap();
        let h = cons
            .iter()
            .find(|c| c["type"] == json!("Horizontal"))
            .unwrap();
        assert_eq!(h["entities"], json!([line.to_string()]));
        let d = cons
            .iter()
            .find(|c| c["type"] == json!("Distance"))
            .unwrap();
        assert_eq!(d["entities"], json!([p0.to_string(), p1.to_string()]));
        assert_eq!(d["value"], json!(40.0));
    }

    #[test]
    fn solve_drag_parses_superseded_and_positions() {
        let ok = json!({
            "gestureId": 51, "seq": 129, "status": "success", "dof": 1,
            "conflicting": [], "positions": { "e3.start": [42.0, 19.5] }, "solveMicros": 1840
        });
        let d = parse_solve_drag(&ok);
        assert_eq!(d.seq, 129);
        assert!(!d.superseded);
        assert_eq!(d.positions["e3.start"], [42.0, 19.5]);
        assert_eq!(d.dof, 1);

        let stale =
            json!({ "gestureId": 51, "seq": 3, "status": "superseded", "dof": 1, "positions": {} });
        let d = parse_solve_drag(&stale);
        assert!(
            d.superseded,
            "a stale seq resolves superseded (latest-wins)"
        );
        assert!(d.positions.is_empty());
    }

    #[test]
    fn sketch_upsert_parses_state_and_end_gesture_derives_status() {
        // SketchUpsert carries `state`.
        let up = json!({ "sketchId": "sk_1", "sketchRevision": 4, "dof": 2, "state": "UnderConstrained" });
        let d = parse_sketch_upsert("sk_1", &up);
        assert_eq!(d.sketch_revision, 4);
        assert_eq!(d.dof, 2);
        assert_eq!(d.status, SketchSolveStatus::UnderConstrained);
        // EndGesture carries `status` (drag status) + dof; the DTO derives the solve
        // status from dof (0 ⇒ FullyConstrained).
        let end = json!({ "gestureId": 51, "status": "success", "dof": 0,
            "positions": { "00000000-0000-0000-0000-000000000010": [1.0, 2.0] }, "sketchRevision": 5 });
        let d = parse_sketch_upsert("sk_1", &end);
        assert_eq!(d.status, SketchSolveStatus::FullyConstrained);
        assert_eq!(d.sketch_revision, 5);
        assert_eq!(d.solved_positions.len(), 1);
    }

    #[test]
    fn acquire_args_and_evidence_round_trip() {
        let body = BodyId(Uuid::from_u128(0x3));
        let req = AcquireRequest {
            snapshot_id: SnapshotId(5012),
            body,
            picks: vec![Pick {
                topo_key: TopoKey::new("f:22"),
                anchor: None,
            }],
        };
        let args = acquire_element_ids_args(&req);
        assert_eq!(args["snapshotId"], 5012);
        assert_eq!(args["bodyId"], json!(body_id_wire(body)));
        assert_eq!(args["picks"][0]["topoKey"], "f:22");

        // Worker echoes evidence (existing id present ⇒ carried through).
        let result = json!({ "ids": [
            { "topoKey": "f:22", "kind": "face", "bodyId": body_id_wire(body), "elementId": "el_00000000000004a1", "descriptor": {} },
            { "topoKey": "e:3", "kind": "edge", "bodyId": body_id_wire(body), "elementId": "" }
        ]});
        let ev = parse_acquire_evidence(&result, body);
        assert_eq!(ev.len(), 2);
        assert_eq!(
            ev[0].existing.as_ref().unwrap().as_str(),
            "el_00000000000004a1"
        );
        assert_eq!(ev[0].kind, onecad_core::document::refs::ElementKind::Face);
        assert!(ev[1].existing.is_none(), "empty elementId ⇒ Rust mints");
        assert_eq!(ev[1].kind, onecad_core::document::refs::ElementKind::Edge);
    }

    #[test]
    fn resolve_refs_parses_all_three_outcomes() {
        let result = json!({ "resolutions": [
            { "refId": "op_5.input0", "outcome": "autoBind", "topoKey": "f:1", "score": 0.94, "margin": 0.31 },
            { "refId": "op_5.input1", "outcome": "unchanged", "elementId": "el_9", "topoKey": "f:2" },
            { "refId": "op_5.input2", "outcome": "needsRepair",
              "needsRepair": { "refId": "op_5.input2", "ladderFailed": "descriptor", "reason": "ambiguous", "candidates": [] } }
        ]});
        let res = parse_resolve_refs(&result);
        assert_eq!(res.len(), 3);
        assert!(
            matches!(res[0].outcome, ResolveOutcome::AutoBind { score, .. } if (score - 0.94).abs() < 1e-9)
        );
        assert!(matches!(res[1].outcome, ResolveOutcome::Unchanged));
        assert!(matches!(res[2].outcome, ResolveOutcome::NeedsRepair(_)));
    }
}
