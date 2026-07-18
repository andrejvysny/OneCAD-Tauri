//! `onecad-worker-stub` — a real fake sidecar speaking OCW1 over stdio.
//!
//! It implements the handshake and the lifecycle verbs the protocol tests need
//! (Hello unsolicited, Shutdown, OpenSession/CloseSession, GetWorkerHead) and
//! replies to any unknown verb with a well-framed `PROTOCOL_ERROR` terminal
//! `resp` (SCHEMA §8 well-framed-illegal sub-case). It is intentionally
//! synchronous (blocking stdin/stdout) — the wire bytes are identical to a real
//! worker's, and blocking IO keeps the chaos hooks trivially deterministic.
//!
//! Chaos hooks (env vars), for the client's crash/hang/garbage drills:
//! - `ONECAD_STUB_CRASH_ON=<verb>` — `abort()` when that verb arrives.
//! - `ONECAD_STUB_HANG_ON=<verb>`  — sleep forever when that verb arrives.
//! - `ONECAD_STUB_GARBAGE=1`       — emit one invalid-magic frame at startup
//!   (drives the client's `BadMagic` path), then continue.
//! - `ONECAD_STUB_CRASH_COUNTDOWN=<file>` — the R-WP11 convergence drill: the file
//!   holds a decimal counter; each `ExecutePlan` reads it, and while `> 0`
//!   decrements it (persisting across restarts) and `abort()`s **mid-plan** (after
//!   emitting the first `planStep`), so a fresh worker crashes N times then
//!   succeeds — the document must always converge to the last-valid snapshot.
//! - `ONECAD_STUB_CHUNKED_MESH=1` — `Tessellate` streams its MESH1 blob as a bulk
//!   chunk manifest + data frames (SCHEMA §5.2) instead of inlining it, exercising
//!   the client's chunk-assembly + credit path.
//! - `ONECAD_STUB_CHUNKED_MESH_GAP=<mode>` — with `ONECAD_STUB_CHUNKED_MESH=1`,
//!   corrupts the tiling of the streamed chunks (`gap` = leave a hole; `overlap` =
//!   overlap two chunks) so the client's StreamAcc gap-detection fires (F5).
//! - `ONECAD_STUB_EXIT_AFTER_HELLO=1` — exit 0 immediately after the unsolicited
//!   `hello` (connect-then-die), driving the supervisor's rapid-death restart cap
//!   (F2). No session/plan is ever served.
//! - `ONECAD_STUB_CRASH_ON_OP=<substr>` — `abort()` mid-plan when an op's `opId`
//!   contains `<substr>` (the F3 poison test: crash one specific plan's op so its
//!   crashing-op key poisons while a different plan still runs).
//!
//! Beyond the lifecycle verbs, the stub speaks a minimal `ExecutePlan`
//! (one `planStep` per op minting `body_<opId>`, terminal `PlanPrepared` echoing
//! the plan's opaque history-prefix token), `AcceptPrepared`/`DiscardPrepared`,
//! `ResetSession`, and `Tessellate` (a header-only valid MESH1 blob) — enough to
//! drive the real regen/mesh path end-to-end without OCCT.
//!
//! Fencing (D4/D5): the stub mirrors the real worker — `ExecutePlan` fences on
//! `workerEpoch` + `expectedBaseHash` ONLY (same PROTOCOL_ERROR shapes), never on
//! `documentRevision`; the head ADOPTS the plan's `documentRevision` + echoed
//! `historyPrefixHash` at `AcceptPrepared`. Per D5 a from-0 plan (no `baseCheckpoint`
//! AND `expectedBaseHash` == the empty anchor) is ALWAYS base-valid — the head-hash
//! comparison is skipped so sequential replay-from-0 regens keep preparing after the
//! head token advances (the epoch fence still applies). The one divergence from the
//! real worker (which requires `OpenSession`): when a plan arrives before any
//! `OpenSession` (the chaos drills), the stub adopts that first plan's epoch + base
//! hash as the fencing baseline and fences every plan thereafter.
//!
//! Logs go to stderr only; stdout carries frames exclusively (SCHEMA §1).

use std::collections::BTreeMap;
use std::io::Write;

use serde_json::{json, Value};

use onecad_protocol::framing::{read_frame_blocking, write_frame_blocking, MAGIC_BYTES};
use onecad_protocol::messages::{
    BinSection, ChunkFrame, ChunkKind, CloseSessionResult, ErrorCode, ErrorObject, EventFrame,
    Frame, HelloFrame, HelloLimits, HelloResult, OcctInfo, OpenSessionArgs, OpenSessionResult,
    ReqFrame, RespFrame, ShutdownResult, Stamp, WorkerHeadBrief, WorkerHeadResult,
    PROTOCOL_VERSION,
};
use onecad_protocol::ProtocolError;

/// The SHA-256 of zero bytes — the empty-prefix `historyPrefixHash` anchor (the
/// base of a replay-from-0 plan). Mirrors the real worker's `kEmptyPrefixHash` and
/// onecad-core `HistoryPrefixHash::empty()`.
const EMPTY_PREFIX_HASH: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

/// One prepared-but-not-published scratch job (D4: carries the head token it would
/// adopt on accept + the plan's advisory documentRevision).
struct Prepared {
    job_id: u64,
    prepared_snapshot_id: u64,
    /// The `historyPrefixHash` this job would adopt as the head on accept.
    history_prefix_hash: String,
    /// The plan's Rust-owned documentRevision, ADOPTED as the head on accept (D4).
    plan_document_revision: u64,
}

/// One stub sketch on the solver lane (SCHEMA §7.4): point positions + a deterministic
/// dof/state derived from `2·points − constraints`.
#[derive(Default, Clone)]
struct StubSketch {
    revision: u64,
    /// Point wire id → `[x, y]`.
    points: BTreeMap<String, [f64; 2]>,
    point_count: usize,
    constraint_count: usize,
}

impl StubSketch {
    fn dof(&self) -> i64 {
        (2 * self.point_count as i64 - self.constraint_count as i64).max(0)
    }
    fn state(&self) -> &'static str {
        if self.dof() == 0 {
            "FullyConstrained"
        } else {
            "UnderConstrained"
        }
    }
}

/// One in-flight drag gesture (SCHEMA §7.4). `max_seq` drives latest-wins: a `seq`
/// not newer than `max_seq` resolves `superseded`.
struct StubGesture {
    sketch_id: String,
    drag_point: String,
    max_seq: u64,
    baseline: BTreeMap<String, [f64; 2]>,
}

/// Mutable stub state across the request loop.
struct StubState {
    /// Worker output sequence number (SCHEMA §2: monotonic across every frame).
    seq: u64,
    session_open: bool,
    document_revision: u64,
    worker_epoch: u64,
    snapshot_id: u64,
    /// The head `historyPrefixHash` (fencing token; adopted from a plan's echo on
    /// accept). Mirrors the real worker's session head.
    history_prefix_hash: String,
    /// Whether the fencing baseline (epoch + head hash) has been established — set
    /// by OpenSession, or lazily by the first ExecutePlan when no session was
    /// opened (the chaos drills drive ExecutePlan directly, without OpenSession).
    fencing_baseline_set: bool,
    /// Prepared-but-not-published scratch jobs (D4-aware).
    prepared: Vec<Prepared>,
    /// Monotonic bulk-stream id allocator (SCHEMA §2 `streamId`).
    stream_id: u64,
    /// Solver-lane sketches by id (SCHEMA §7.4).
    sketches: BTreeMap<String, StubSketch>,
    /// In-flight drag gestures by gestureId (SCHEMA §7.4).
    gestures: BTreeMap<u64, StubGesture>,
}

impl StubState {
    fn new() -> Self {
        StubState {
            seq: 0,
            session_open: false,
            document_revision: 0,
            worker_epoch: 0,
            snapshot_id: 0,
            history_prefix_hash: EMPTY_PREFIX_HASH.to_string(),
            fencing_baseline_set: false,
            prepared: Vec::new(),
            stream_id: 700,
            sketches: BTreeMap::new(),
            gestures: BTreeMap::new(),
        }
    }

    /// Allocate the next output `seq`.
    fn next_seq(&mut self) -> u64 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    fn stamp(&mut self) -> Stamp {
        self.stamp_job(None)
    }

    fn stamp_job(&mut self, job_id: Option<u64>) -> Stamp {
        Stamp {
            document_revision: self.document_revision,
            worker_epoch: self.worker_epoch,
            snapshot_id: self.snapshot_id,
            job_id,
            seq: self.next_seq(),
        }
    }
}

fn main() {
    let code = run();
    std::process::exit(code);
}

fn run() -> i32 {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut state = StubState::new();

    // Chaos: emit one invalid-magic frame before anything else.
    if env_flag("ONECAD_STUB_GARBAGE") {
        if let Err(err) = emit_garbage(&mut writer) {
            eprintln!("stub: failed to emit garbage: {err}");
            return 1;
        }
    }

    // Unsolicited hello (SCHEMA §6): seq 0.
    let hello_seq = state.next_seq();
    debug_assert_eq!(hello_seq, 0);
    if let Err(err) = write_hello(&mut writer, hello_seq) {
        eprintln!("stub: failed to write hello: {err}");
        return 1;
    }

    // Chaos: connect-then-die immediately after the hello (F2 rapid-death cap).
    if env_flag("ONECAD_STUB_EXIT_AFTER_HELLO") {
        eprintln!("stub: EXIT_AFTER_HELLO -> exit 0 right after hello");
        return 0;
    }

    loop {
        let raw = match read_frame_blocking(&mut reader) {
            Ok(Some(raw)) => raw,
            Ok(None) => {
                eprintln!("stub: stdin closed; exiting");
                return 0;
            }
            Err(ProtocolError::ConnectionLost(reason)) => {
                eprintln!("stub: connection lost ({reason}); exiting");
                return 0;
            }
            Err(err) => {
                eprintln!("stub: fatal frame error: {err}");
                return 1;
            }
        };

        let frame = match Frame::from_json_slice(&raw.json) {
            Ok(frame) => frame,
            Err(err) => {
                // Malformed envelope: framing sub-case of PROTOCOL_ERROR, no id
                // to reply against -> tear down (SCHEMA §8).
                eprintln!("stub: malformed envelope: {err}");
                return 1;
            }
        };

        match frame {
            Frame::Req(req) => {
                if let Some(code) = handle_req(&mut writer, &mut state, req) {
                    return code;
                }
            }
            Frame::Cancel(c) => {
                // Synchronous stub: the target request already completed, so a
                // cancel is a no-op (SCHEMA §3.5).
                eprintln!("stub: cancel for id {} (no-op)", c.id);
            }
            Frame::Credit(_) => {
                // The stub emits no bulk streams; credit is a no-op.
            }
            other => {
                eprintln!("stub: ignoring unexpected driver frame: {other:?}");
            }
        }
    }
}

/// Handle one request. Returns `Some(exit_code)` if the process should exit.
fn handle_req<W: Write>(writer: &mut W, state: &mut StubState, req: ReqFrame) -> Option<i32> {
    // Chaos hooks fire BEFORE any reply (SCHEMA-agnostic test hooks).
    if env_matches("ONECAD_STUB_CRASH_ON", &req.verb) {
        eprintln!("stub: CRASH_ON {} -> abort()", req.verb);
        std::process::abort();
    }
    if env_matches("ONECAD_STUB_HANG_ON", &req.verb) {
        eprintln!("stub: HANG_ON {} -> sleeping forever", req.verb);
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    let result: Result<(), ProtocolError> = match req.verb.as_str() {
        "Shutdown" => {
            let stamp = state.stamp();
            let r = write_resp_ok(writer, req.id, stamp, &ShutdownResult { goodbye: true });
            if let Err(err) = r {
                eprintln!("stub: failed to write Shutdown resp: {err}");
            }
            // Graceful stop (SCHEMA §7.1): flush, reply, exit 0.
            return Some(0);
        }
        "OpenSession" => handle_open_session(writer, state, &req),
        "CloseSession" => {
            state.session_open = false;
            let stamp = state.stamp();
            write_resp_ok(
                writer,
                req.id,
                stamp,
                &CloseSessionResult {
                    session_closed: true,
                },
            )
        }
        "GetWorkerHead" => {
            let stamp = state.stamp();
            write_resp_ok(
                writer,
                req.id,
                stamp,
                &WorkerHeadResult {
                    document_revision: state.document_revision,
                    worker_epoch: state.worker_epoch,
                    snapshot_id: state.snapshot_id,
                    history_prefix_hash: state.history_prefix_hash.clone(),
                    has_scratch: !state.prepared.is_empty(),
                },
            )
        }
        "ResetSession" => {
            state.worker_epoch += 1;
            state.prepared.clear();
            let stamp = state.stamp();
            write_resp_value(
                writer,
                req.id,
                stamp,
                json!({ "reset": true, "workerEpoch": state.worker_epoch }),
                &[],
                &[],
            )
        }
        "ExecutePlan" => handle_execute_plan(writer, state, &req),
        "AcceptPrepared" => handle_accept_prepared(writer, state, &req),
        "DiscardPrepared" => handle_discard_prepared(writer, state, &req),
        "Tessellate" => handle_tessellate(writer, state, &req),
        // --- solver lane (SCHEMA §7.4) ---
        "SketchUpsert" => handle_sketch_upsert(writer, state, &req),
        "BeginGesture" => handle_begin_gesture(writer, state, &req),
        "SolveDrag" => handle_solve_drag(writer, state, &req),
        "EndGesture" => handle_end_gesture(writer, state, &req),
        "SketchRegions" => handle_sketch_regions(writer, state, &req),
        // --- element identity (SCHEMA §7.5) ---
        "AcquireElementIds" => handle_acquire_element_ids(writer, state, &req),
        "ResolveRefs" => handle_resolve_refs(writer, state, &req),
        unknown => {
            // Well-framed but protocol-illegal: terminal error resp (SCHEMA §8).
            let stamp = state.stamp();
            write_resp_err(
                writer,
                req.id,
                stamp,
                ErrorObject {
                    code: ErrorCode::ProtocolError,
                    message: format!("unknown verb: {unknown}"),
                    detail: None,
                    retriable: false,
                },
            )
        }
    };

    if let Err(err) = result {
        eprintln!("stub: write error for verb {}: {err}", req.verb);
        // A broken stdout means the driver is gone; exit cleanly.
        return Some(0);
    }
    None
}

fn handle_open_session<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    match serde_json::from_value::<OpenSessionArgs>(req.args.clone()) {
        Ok(args) => {
            state.session_open = true;
            state.document_revision = args.document_revision;
            state.worker_epoch = args.worker_epoch;
            state.snapshot_id = 0;
            // Fresh document ⇒ empty-prefix head hash; the fencing baseline is now set
            // (SCHEMA §7.1 / D4).
            state.history_prefix_hash = EMPTY_PREFIX_HASH.to_string();
            state.fencing_baseline_set = true;
            let stamp = state.stamp();
            write_resp_ok(
                writer,
                req.id,
                stamp,
                &OpenSessionResult {
                    session_open: true,
                    worker_head: WorkerHeadBrief {
                        document_revision: state.document_revision,
                        snapshot_id: state.snapshot_id,
                    },
                },
            )
        }
        Err(err) => {
            // Malformed args for a known verb: well-framed-illegal PROTOCOL_ERROR.
            let stamp = state.stamp();
            write_resp_err(
                writer,
                req.id,
                stamp,
                ErrorObject {
                    code: ErrorCode::ProtocolError,
                    message: format!("invalid OpenSession args: {err}"),
                    detail: None,
                    retriable: false,
                },
            )
        }
    }
}

/// Minimal `ExecutePlan` (SCHEMA §7.2): one `planStep` per op minting a
/// deterministic `body_<opId>`, then a terminal `PlanPrepared` echoing the plan's
/// opaque last-executed history-prefix token (the executor verifies that echo).
///
/// The convergence-drill hook `ONECAD_STUB_CRASH_COUNTDOWN` `abort()`s **mid-plan**
/// (after the first `planStep`) while its persisted counter is `> 0`.
fn handle_execute_plan<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let args = &req.args;
    let job_id = args.get("jobId").and_then(Value::as_u64);
    let ops = args
        .get("ops")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let prefix_hashes = args
        .get("prefixHashes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let expected_base = args
        .get("expectedBaseHash")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let plan_revision = args
        .get("documentRevision")
        .and_then(Value::as_u64)
        .unwrap_or(state.document_revision);
    let plan_epoch = args
        .get("workerEpoch")
        .and_then(Value::as_u64)
        .unwrap_or(state.worker_epoch);

    // D4 fencing — mirror the real worker: workerEpoch + expectedBaseHash ONLY,
    // never documentRevision (an advisory Rust-owned edit counter). Lazily adopt the
    // baseline from the first plan when no OpenSession established it (chaos drills).
    if !state.fencing_baseline_set {
        state.worker_epoch = plan_epoch;
        state.history_prefix_hash = expected_base.clone();
        state.fencing_baseline_set = true;
    }
    if plan_epoch != state.worker_epoch {
        let stamp = state.stamp_job(job_id);
        return write_resp_err(
            writer,
            req.id,
            stamp,
            ErrorObject {
                code: ErrorCode::ProtocolError,
                message: "ExecutePlan: workerEpoch fencing mismatch".into(),
                detail: Some(json!({ "headEpoch": state.worker_epoch, "planEpoch": plan_epoch })),
                retriable: false,
            },
        );
    }
    // D5: a from-0 plan — no `baseCheckpoint` AND expectedBaseHash == the empty anchor
    // — is ALWAYS base-valid: SKIP the head-hash comparison so a full-replay regen
    // still prepares after the head token advanced past the empty anchor (the
    // RegenPlanner always replays from 0; after the first accept the head is nonzero,
    // and the strict fence would reject every subsequent regen). workerEpoch fencing
    // (above) is unchanged; on accept the head is replaced wholesale. Incremental
    // plans (nonzero expectedBaseHash) keep the strict head-hash fence. Mirrors the
    // real worker's Session::fence_and_clone.
    let from_zero = expected_base == EMPTY_PREFIX_HASH && args.get("baseCheckpoint").is_none();
    if !from_zero && expected_base != state.history_prefix_hash {
        let stamp = state.stamp_job(job_id);
        return write_resp_err(
            writer,
            req.id,
            stamp,
            ErrorObject {
                code: ErrorCode::ProtocolError,
                message: "ExecutePlan: expectedBaseHash mismatch".into(),
                detail: Some(
                    json!({ "expected": expected_base, "actual": state.history_prefix_hash }),
                ),
                retriable: false,
            },
        );
    }

    let mut per_step: Vec<Value> = Vec::new();
    let mut last_valid: Option<u64> = None;
    for (i, op) in ops.iter().enumerate() {
        let step_index = op
            .get("stepIndex")
            .and_then(Value::as_u64)
            .unwrap_or(i as u64);
        let op_id = op
            .get("opId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        // F3 poison test: crash (transport loss) when a specific op executes, so the
        // crash is attributed to THAT op's poison key (not the plan's last op). The
        // crash is before the planStep, so `steps_received` points at this op.
        if crash_on_op_matches(&op_id) {
            eprintln!("stub: CRASH_ON_OP {op_id} -> abort() mid-plan");
            std::process::abort();
        }
        let body_id = format!("body_{op_id}");
        let payload = json!({
            "stepIndex": step_index,
            "bodyEvents": [ { "kind": "created", "bodyId": body_id } ],
            "elementMapDelta": { "added": [], "removed": [], "relabeled": [] },
            "needsRepair": [],
            "signatures": {
                "geometry": format!("g{step_index}"),
                "bodyLifecycle": format!("b{step_index}"),
                "referencedBinding": format!("r{step_index}"),
            },
            "diagnostics": [],
        });
        let stamp = state.stamp_job(job_id);
        write_event(writer, req.id, "planStep", step_index, payload, stamp)?;
        // Mid-plan convergence-drill crash: after the first step is on the wire.
        if i == 0 {
            crash_countdown();
        }
        per_step.push(json!({ "stepIndex": step_index, "status": "ok", "bodyIds": [body_id] }));
        last_valid = Some(step_index);
    }

    let prepared_snapshot = state.snapshot_id + 1;
    // The head token this job would adopt on accept: prefixHashes[last] (or the base
    // hash for a base-only prepare) — the same opaque token the real worker echoes.
    let echo = prefix_hashes
        .last()
        .and_then(Value::as_str)
        .unwrap_or(&expected_base)
        .to_string();
    if let Some(j) = job_id {
        state.prepared.push(Prepared {
            job_id: j,
            prepared_snapshot_id: prepared_snapshot,
            history_prefix_hash: echo.clone(),
            plan_document_revision: plan_revision,
        });
    }
    let result = json!({
        "planPrepared": true,
        "preparedSnapshotId": prepared_snapshot,
        "lastValidStep": last_valid,
        "stoppedReason": "completed",
        "perStepResults": per_step,
        "historyPrefixHash": echo,
    });
    let stamp = state.stamp_job(job_id);
    write_resp_value(writer, req.id, stamp, result, &[], &[])
}

/// `AcceptPrepared` (SCHEMA §7.2 / D4): publish the scratch snapshot; ADOPT the
/// plan's advisory `documentRevision` + echoed `historyPrefixHash` as the head.
fn handle_accept_prepared<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let job_id = req.args.get("jobId").and_then(Value::as_u64);
    let prepared = job_id.and_then(|j| {
        let pos = state.prepared.iter().position(|p| p.job_id == j)?;
        Some(state.prepared.remove(pos))
    });
    match prepared {
        Some(p) => {
            state.snapshot_id = p.prepared_snapshot_id;
            // D4: adopt the plan's revision (not a worker-owned +1) + the head token.
            state.document_revision = p.plan_document_revision;
            state.history_prefix_hash = p.history_prefix_hash;
            let stamp = state.stamp_job(job_id);
            write_resp_value(
                writer,
                req.id,
                stamp,
                json!({ "accepted": true, "snapshotId": p.prepared_snapshot_id, "documentRevision": state.document_revision }),
                &[],
                &[],
            )
        }
        None => {
            let stamp = state.stamp_job(job_id);
            write_resp_err(
                writer,
                req.id,
                stamp,
                ErrorObject {
                    code: ErrorCode::ProtocolError,
                    message: "AcceptPrepared for unknown/absent job".into(),
                    detail: None,
                    retriable: false,
                },
            )
        }
    }
}

/// `DiscardPrepared` (SCHEMA §7.2): drop the scratch job; session unchanged.
fn handle_discard_prepared<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    if let Some(j) = req.args.get("jobId").and_then(Value::as_u64) {
        state.prepared.retain(|p| p.job_id != j);
    }
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({ "discarded": true }),
        &[],
        &[],
    )
}

// ── Solver lane (SCHEMA §7.4) — echo-style deterministic solve ───────────────

fn read_xy(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

/// `SketchUpsert` (SCHEMA §7.4): store the point positions + report a deterministic
/// `dof = max(0, 2·points − constraints)` and derived state.
fn handle_sketch_upsert<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let args = &req.args;
    let sketch_id = args
        .get("sketchId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let entities = args
        .get("entities")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let constraint_count = args
        .get("constraints")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    let mut points = BTreeMap::new();
    let mut point_count = 0;
    for e in &entities {
        if e.get("type").and_then(Value::as_str) == Some("Point") {
            point_count += 1;
            if let (Some(id), Some(xy)) = (
                e.get("id").and_then(Value::as_str),
                e.get("at").and_then(read_xy),
            ) {
                points.insert(id.to_string(), xy);
            }
        }
    }
    let prev_rev = state.sketches.get(&sketch_id).map_or(0, |s| s.revision);
    let sk = StubSketch {
        revision: prev_rev + 1,
        points,
        point_count,
        constraint_count,
    };
    let (dof, st, rev) = (sk.dof(), sk.state(), sk.revision);
    state.sketches.insert(sketch_id.clone(), sk);
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({ "upserted": true, "sketchId": sketch_id, "sketchRevision": rev, "dof": dof, "state": st }),
        &[],
        &[],
    )
}

/// `BeginGesture` (SCHEMA §7.4): snapshot the point baseline for change reporting.
fn handle_begin_gesture<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let args = &req.args;
    let gesture_id = args.get("gestureId").and_then(Value::as_u64).unwrap_or(0);
    let sketch_id = args
        .get("sketchId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let drag_point = args
        .get("drag")
        .and_then(|d| d.get("pointId"))
        .and_then(Value::as_str)
        .or_else(|| args.get("pointId").and_then(Value::as_str))
        .unwrap_or("")
        .to_string();
    let baseline = state
        .sketches
        .get(&sketch_id)
        .map(|s| s.points.clone())
        .unwrap_or_default();
    state.gestures.insert(
        gesture_id,
        StubGesture {
            sketch_id,
            drag_point,
            max_seq: 0,
            baseline,
        },
    );
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({ "gestureId": gesture_id, "ready": true }),
        &[],
        &[],
    )
}

/// `SolveDrag` (SCHEMA §7.4): apply the drag delta to the dragged point. A `seq`
/// not newer than the gesture's `max_seq` resolves **superseded** (latest-wins).
fn handle_solve_drag<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let args = &req.args;
    let gesture_id = args.get("gestureId").and_then(Value::as_u64).unwrap_or(0);
    let seq = args.get("seq").and_then(Value::as_u64).unwrap_or(0);
    let target = args.get("target").and_then(read_xy).unwrap_or([0.0, 0.0]);

    let (sketch_id, drag_point, stale) = match state.gestures.get_mut(&gesture_id) {
        Some(g) => {
            let stale = g.max_seq > 0 && seq <= g.max_seq;
            if !stale {
                g.max_seq = seq;
            }
            (g.sketch_id.clone(), g.drag_point.clone(), stale)
        }
        None => {
            let stamp = state.stamp();
            return write_resp_err(
                writer,
                req.id,
                stamp,
                ErrorObject {
                    code: ErrorCode::RefUnresolved,
                    message: "SolveDrag: unknown or ended gesture".into(),
                    detail: None,
                    retriable: false,
                },
            );
        }
    };

    let mut positions = serde_json::Map::new();
    if !stale {
        if let Some(sk) = state.sketches.get_mut(&sketch_id) {
            sk.points.insert(drag_point.clone(), target);
        }
        positions.insert(drag_point, json!([target[0], target[1]]));
    }
    let dof = state.sketches.get(&sketch_id).map_or(0, StubSketch::dof);
    let status = if stale { "superseded" } else { "success" };
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({
            "gestureId": gesture_id, "seq": seq, "status": status, "dof": dof,
            "conflicting": [], "positions": Value::Object(positions), "solveMicros": 42,
        }),
        &[],
        &[],
    )
}

/// `EndGesture` (SCHEMA §7.4): apply the final target, bump the sketch revision,
/// and report the points changed since the gesture began.
fn handle_end_gesture<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let args = &req.args;
    let gesture_id = args.get("gestureId").and_then(Value::as_u64).unwrap_or(0);
    let Some(g) = state.gestures.remove(&gesture_id) else {
        let stamp = state.stamp();
        return write_resp_err(
            writer,
            req.id,
            stamp,
            ErrorObject {
                code: ErrorCode::RefUnresolved,
                message: "EndGesture: unknown or ended gesture".into(),
                detail: None,
                retriable: false,
            },
        );
    };
    if let Some(ft) = args
        .get("commit")
        .and_then(|c| c.get("finalTarget"))
        .and_then(read_xy)
    {
        if let Some(sk) = state.sketches.get_mut(&g.sketch_id) {
            sk.points.insert(g.drag_point.clone(), ft);
        }
    }
    let (rev, dof, positions) = match state.sketches.get_mut(&g.sketch_id) {
        Some(sk) => {
            sk.revision += 1;
            let mut pos = serde_json::Map::new();
            for (k, v) in &sk.points {
                let changed = g
                    .baseline
                    .get(k)
                    .is_none_or(|b| (b[0] - v[0]).abs() > 1e-9 || (b[1] - v[1]).abs() > 1e-9);
                if changed {
                    pos.insert(k.clone(), json!([v[0], v[1]]));
                }
            }
            (sk.revision, sk.dof(), Value::Object(pos))
        }
        None => (0, 0, json!({})),
    };
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({ "gestureId": gesture_id, "status": "success", "dof": dof, "positions": positions, "sketchRevision": rev }),
        &[],
        &[],
    )
}

/// `SketchRegions` (SCHEMA §7.4): the stub has no loop detector, so it returns an
/// empty region set (the flow is exercised; real regions need the C++ worker).
fn handle_sketch_regions<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let sketch_id = req
        .args
        .get("sketchId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let rev = state.sketches.get(&sketch_id).map_or(0, |s| s.revision);
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({ "sketchId": sketch_id, "sketchRevision": rev, "regions": [] }),
        &[],
        &[],
    )
}

// ── Element identity (SCHEMA §7.5) — echo-style evidence ─────────────────────

/// `AcquireElementIds` (SCHEMA §7.5): echo one evidence entry per pick with an
/// empty `elementId` (Rust mints the id) + a stub descriptor.
fn handle_acquire_element_ids<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let args = &req.args;
    let body_id = args
        .get("bodyId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut ids = Vec::new();
    if let Some(picks) = args.get("picks").and_then(Value::as_array) {
        for p in picks {
            let topo = p.get("topoKey").and_then(Value::as_str).unwrap_or("");
            let kind = match topo.chars().next() {
                Some('e') => "edge",
                Some('v') => "vertex",
                _ => "face",
            };
            let mut entry = json!({
                "topoKey": topo, "kind": kind, "bodyId": body_id,
                "elementId": "", "descriptor": { "stub": true },
            });
            if let Some(a) = p.get("anchor") {
                entry["anchor"] = a.clone();
            }
            ids.push(entry);
        }
    }
    let stamp = state.stamp();
    write_resp_value(writer, req.id, stamp, json!({ "ids": ids }), &[], &[])
}

/// `ResolveRefs` (SCHEMA §7.5): a deterministic dry run — an already-bound ref is
/// `unchanged`, anything else `autoBind`s with a canned high score.
fn handle_resolve_refs<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let mut resolutions = Vec::new();
    if let Some(refs) = req.args.get("refs").and_then(Value::as_array) {
        for r in refs {
            let ref_id = r.get("refId").and_then(Value::as_str).unwrap_or("");
            let existing = r
                .get("primary")
                .and_then(|p| p.get("elementId"))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            resolutions.push(match existing {
                Some(eid) => json!({ "refId": ref_id, "outcome": "unchanged", "elementId": eid, "topoKey": "f:0" }),
                None => json!({ "refId": ref_id, "outcome": "autoBind", "topoKey": "f:0", "score": 0.95, "margin": 0.5 }),
            });
        }
    }
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({ "resolutions": resolutions }),
        &[],
        &[],
    )
}

/// `Tessellate` (SCHEMA §7.6): a header-only valid MESH1 blob per requested body,
/// inline in the resp tail — or streamed as a bulk chunk manifest + data frames
/// when `ONECAD_STUB_CHUNKED_MESH=1` (SCHEMA §5.2), exercising the chunk path.
fn handle_tessellate<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let lod = req
        .args
        .get("lod")
        .and_then(Value::as_str)
        .unwrap_or("coarse");
    let body_ids: Vec<String> = match req.args.get("bodyIds") {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        // "all" (or unspecified) has no body registry in the stub → one synthetic.
        _ => vec!["body_all".to_string()],
    };

    let chunked = env_flag("ONECAD_STUB_CHUNKED_MESH");
    let mut meshes: Vec<Value> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    let mut bin_sections: Vec<BinSection> = Vec::new();
    for body in &body_ids {
        let blob = mesh1_blob(lod);
        let sha = sha256_hex(&blob);
        if chunked {
            let stream_id = state.stream_id;
            state.stream_id += 1;
            stream_mesh(writer, state, req.id, stream_id, body, lod, &blob, &sha)?;
            meshes.push(json!({
                "bodyId": body, "streamId": stream_id, "format": "MESH1",
                "totalBytes": blob.len(), "sha256": sha, "snapshotId": state.snapshot_id,
            }));
        } else {
            let name = format!("mesh:{body}");
            let off = tail.len() as u32;
            tail.extend_from_slice(&blob);
            bin_sections.push(BinSection {
                name: name.clone(),
                off,
                len: blob.len() as u32,
            });
            meshes.push(json!({
                "bodyId": body, "bin": name, "format": "MESH1",
                "totalBytes": blob.len(), "sha256": sha, "snapshotId": state.snapshot_id,
            }));
        }
    }
    let stamp = state.stamp();
    write_resp_value(
        writer,
        req.id,
        stamp,
        json!({ "meshes": meshes }),
        &bin_sections,
        &tail,
    )
}

/// Stream one MESH1 blob as a manifest chunk + `count` data chunks (SCHEMA §5.2).
#[allow(clippy::too_many_arguments)]
fn stream_mesh<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    id: u64,
    stream_id: u64,
    body: &str,
    lod: &str,
    blob: &[u8],
    sha: &str,
) -> Result<(), ProtocolError> {
    // Split into 2 data frames to prove multi-frame assembly by byteOffset. Each
    // frame is `(byteOffset, slice)`. The F5 gap hook perturbs the tiling so the
    // client's StreamAcc gap-detection must fire (a hole or an overlap in
    // [0, totalBytes)); `mode` ∈ "gap" (hole) | "overlap".
    let mid = blob.len().div_ceil(2);
    let gap_mode = std::env::var("ONECAD_STUB_CHUNKED_MESH_GAP").ok();
    let frames: Vec<(u64, &[u8])> = if mid == 0 || mid >= blob.len() {
        vec![(0, blob)]
    } else {
        match gap_mode.as_deref() {
            // Hole: the 2nd chunk starts 4 bytes past `mid`, leaving [mid, mid+4)
            // uncovered.
            Some("gap") => vec![(0, &blob[..mid]), (mid as u64 + 4, &blob[mid..])],
            // Overlap: the 2nd chunk starts 4 bytes before `mid`, re-covering
            // [mid-4, mid).
            Some("overlap") => vec![(0, &blob[..mid]), (mid as u64 - 4, &blob[mid - 4..])],
            _ => vec![(0, &blob[..mid]), (mid as u64, &blob[mid..])],
        }
    };
    let manifest = ChunkFrame {
        v: PROTOCOL_VERSION,
        id,
        stream_id,
        kind: ChunkKind::Manifest,
        purpose: Some("mesh".into()),
        count: Some(frames.len() as u32),
        total_bytes: Some(blob.len() as u64),
        sha256: Some(sha.to_string()),
        meta: Some(json!({ "bodyId": body, "lod": lod, "format": "MESH1" })),
        index: None,
        byte_offset: None,
        bin: None,
        document_revision: state.document_revision,
        worker_epoch: state.worker_epoch,
        snapshot_id: state.snapshot_id,
        job_id: None,
        seq: state.next_seq(),
    };
    write_frame(writer, &Frame::Chunk(manifest))?;
    for (index, (offset, part)) in frames.iter().enumerate() {
        let data = ChunkFrame {
            v: PROTOCOL_VERSION,
            id,
            stream_id,
            kind: ChunkKind::Data,
            purpose: None,
            count: None,
            total_bytes: None,
            sha256: None,
            meta: None,
            index: Some(index as u32),
            byte_offset: Some(*offset),
            bin: Some(vec![BinSection {
                name: "chunk".into(),
                off: 0,
                len: part.len() as u32,
            }]),
            document_revision: state.document_revision,
            worker_epoch: state.worker_epoch,
            snapshot_id: state.snapshot_id,
            job_id: None,
            seq: state.next_seq(),
        };
        let json = Frame::Chunk(data).to_json_vec()?;
        write_frame_blocking(writer, &json, part)?;
    }
    Ok(())
}

/// A minimal but valid MESH1 blob: the 64-byte header alone (`sectionCount = 0`)
/// passes `validate_mesh_blob`. Enough for the smoke path's "MESH1 validates".
fn mesh1_blob(lod: &str) -> Vec<u8> {
    let mut b = vec![0u8; 64];
    b[0x00..0x04].copy_from_slice(&0x4D45_5348u32.to_le_bytes()); // "MESH" magic (LE)
    b[0x04..0x06].copy_from_slice(&1u16.to_le_bytes()); // version
    let lod_v: u16 = match lod {
        "medium" => 1,
        "fine" => 2,
        _ => 0,
    };
    b[0x1C..0x1E].copy_from_slice(&lod_v.to_le_bytes());
    // flags/counts/sectionCount/bbox/reserved all zero.
    b
}

/// Lowercase-hex SHA-256 (SCHEMA §2 hash form).
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// The `ONECAD_STUB_CRASH_COUNTDOWN` mid-plan crash: if the env names a counter
/// file whose value is `> 0`, decrement it (persisting across restarts) and
/// `abort()`. Absent env / zero counter ⇒ no-op (the plan completes).
fn crash_countdown() {
    let Ok(path) = std::env::var("ONECAD_STUB_CRASH_COUNTDOWN") else {
        return;
    };
    let n: i64 = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0);
    if n > 0 {
        let _ = std::fs::write(&path, (n - 1).to_string());
        eprintln!("stub: CRASH_COUNTDOWN {n} -> abort() mid-plan");
        std::process::abort();
    }
}

/// Write one non-terminal `event` frame (SCHEMA §3.4).
fn write_event<W: Write>(
    writer: &mut W,
    id: u64,
    event: &str,
    step_index: u64,
    payload: Value,
    stamp: Stamp,
) -> Result<(), ProtocolError> {
    let frame = Frame::Event(EventFrame {
        v: PROTOCOL_VERSION,
        id,
        event: event.to_string(),
        step_index: Some(step_index),
        payload,
        document_revision: stamp.document_revision,
        worker_epoch: stamp.worker_epoch,
        snapshot_id: stamp.snapshot_id,
        job_id: stamp.job_id,
        seq: stamp.seq,
    });
    write_frame(writer, &frame)
}

/// Write a success `resp` with an arbitrary JSON result + optional binary tail.
fn write_resp_value<W: Write>(
    writer: &mut W,
    id: u64,
    stamp: Stamp,
    result: Value,
    bin_sections: &[BinSection],
    bin: &[u8],
) -> Result<(), ProtocolError> {
    let frame = Frame::Resp(RespFrame {
        v: PROTOCOL_VERSION,
        id,
        ok: true,
        result: Some(result),
        error: None,
        document_revision: stamp.document_revision,
        worker_epoch: stamp.worker_epoch,
        snapshot_id: stamp.snapshot_id,
        job_id: stamp.job_id,
        seq: stamp.seq,
        bin: if bin_sections.is_empty() {
            None
        } else {
            Some(bin_sections.to_vec())
        },
    });
    let json = frame.to_json_vec()?;
    write_frame_blocking(writer, &json, bin)
}

fn write_hello<W: Write>(writer: &mut W, seq: u64) -> Result<(), ProtocolError> {
    let hello = Frame::Hello(HelloFrame {
        v: PROTOCOL_VERSION,
        seq,
        result: HelloResult {
            protocol_version: 1,
            worker_version: "stub-0.1".into(),
            occt: OcctInfo {
                version: "stub".into(),
                // SCHEMA §2 + hello.ndjson require a 64-bit hex fingerprint
                // ($hex64). The task's literal "stub" would fail that matcher, so
                // the stub emits a valid all-zero hex fingerprint instead.
                fingerprint: "0000000000000000".into(),
            },
            quantization_version: 1,
            solver_policy_version: 1,
            capabilities: vec![],
            // SCHEMA §6 + the hello fixture require a `limits` object.
            limits: Some(HelloLimits {
                chunk_size: 1_048_576,
                initial_bulk_credit: 8_388_608,
            }),
        },
    });
    write_frame(writer, &hello)
}

fn write_resp_ok<W: Write, T: serde::Serialize>(
    writer: &mut W,
    id: u64,
    stamp: Stamp,
    result: &T,
) -> Result<(), ProtocolError> {
    let value = serde_json::to_value(result)?;
    let frame = Frame::Resp(RespFrame {
        v: PROTOCOL_VERSION,
        id,
        ok: true,
        result: Some(value),
        error: None,
        document_revision: stamp.document_revision,
        worker_epoch: stamp.worker_epoch,
        snapshot_id: stamp.snapshot_id,
        job_id: stamp.job_id,
        seq: stamp.seq,
        bin: None,
    });
    write_frame(writer, &frame)
}

fn write_resp_err<W: Write>(
    writer: &mut W,
    id: u64,
    stamp: Stamp,
    error: ErrorObject,
) -> Result<(), ProtocolError> {
    let frame = Frame::Resp(RespFrame {
        v: PROTOCOL_VERSION,
        id,
        ok: false,
        result: None,
        error: Some(error),
        document_revision: stamp.document_revision,
        worker_epoch: stamp.worker_epoch,
        snapshot_id: stamp.snapshot_id,
        job_id: stamp.job_id,
        seq: stamp.seq,
        bin: None,
    });
    write_frame(writer, &frame)
}

fn write_frame<W: Write>(writer: &mut W, frame: &Frame) -> Result<(), ProtocolError> {
    let json = frame.to_json_vec()?;
    write_frame_blocking(writer, &json, &[])
}

/// Emit one frame whose magic is NOT `OCW1`, to drive the client's `BadMagic`.
fn emit_garbage<W: Write>(writer: &mut W) -> std::io::Result<()> {
    // A plausible-looking header with a corrupted magic + zero lengths, so the
    // reader stops at the magic comparison (SCHEMA §1: bytes are authoritative).
    let mut bad = Vec::new();
    let mut magic = MAGIC_BYTES;
    magic[0] ^= 0xFF; // definitely not 'O'
    bad.extend_from_slice(&magic);
    bad.extend_from_slice(&0u32.to_le_bytes()); // jsonLen (well under MAX)
    bad.extend_from_slice(&0u32.to_le_bytes()); // binLen
    writer.write_all(&bad)?;
    writer.flush()
}

/// Read an environment flag as truthy (`1`).
fn env_flag(key: &str) -> bool {
    std::env::var(key).map(|v| v == "1").unwrap_or(false)
}

/// Whether env var `key` equals `verb`.
fn env_matches(key: &str, verb: &str) -> bool {
    std::env::var(key).map(|v| v == verb).unwrap_or(false)
}

/// Whether `ONECAD_STUB_CRASH_ON_OP` names a (non-empty) substring of `op_id` — the
/// F3 hook that crashes the worker only on a specific plan's op.
fn crash_on_op_matches(op_id: &str) -> bool {
    std::env::var("ONECAD_STUB_CRASH_ON_OP")
        .ok()
        .filter(|v| !v.is_empty())
        .is_some_and(|v| op_id.contains(&v))
}
