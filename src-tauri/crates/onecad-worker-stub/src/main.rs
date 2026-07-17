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
//!
//! Beyond the lifecycle verbs, the stub speaks a minimal `ExecutePlan`
//! (one `planStep` per op minting `body_<opId>`, terminal `PlanPrepared` echoing
//! the plan's opaque history-prefix token), `AcceptPrepared`/`DiscardPrepared`,
//! `ResetSession`, and `Tessellate` (a header-only valid MESH1 blob) — enough to
//! drive the real regen/mesh path end-to-end without OCCT.
//!
//! Logs go to stderr only; stdout carries frames exclusively (SCHEMA §1).

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

/// Mutable stub state across the request loop.
struct StubState {
    /// Worker output sequence number (SCHEMA §2: monotonic across every frame).
    seq: u64,
    session_open: bool,
    document_revision: u64,
    worker_epoch: u64,
    snapshot_id: u64,
    /// Prepared-but-not-published scratch jobs: `(jobId, preparedSnapshotId)`.
    prepared: Vec<(u64, u64)>,
    /// Monotonic bulk-stream id allocator (SCHEMA §2 `streamId`).
    stream_id: u64,
}

impl StubState {
    fn new() -> Self {
        StubState {
            seq: 0,
            session_open: false,
            document_revision: 0,
            worker_epoch: 0,
            snapshot_id: 0,
            prepared: Vec::new(),
            stream_id: 700,
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
                    history_prefix_hash: "0000000000000000".into(),
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
    state.document_revision = args
        .get("documentRevision")
        .and_then(Value::as_u64)
        .unwrap_or(state.document_revision);
    state.worker_epoch = args
        .get("workerEpoch")
        .and_then(Value::as_u64)
        .unwrap_or(state.worker_epoch);

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
    if let Some(j) = job_id {
        state.prepared.push((j, prepared_snapshot));
    }
    let echo = prefix_hashes
        .last()
        .and_then(Value::as_str)
        .unwrap_or(&expected_base)
        .to_string();
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

/// `AcceptPrepared` (SCHEMA §7.2): publish the scratch snapshot, bump the revision.
fn handle_accept_prepared<W: Write>(
    writer: &mut W,
    state: &mut StubState,
    req: &ReqFrame,
) -> Result<(), ProtocolError> {
    let job_id = req.args.get("jobId").and_then(Value::as_u64);
    let snapshot = job_id.and_then(|j| {
        let pos = state.prepared.iter().position(|(pj, _)| *pj == j)?;
        Some(state.prepared.remove(pos).1)
    });
    match snapshot {
        Some(snap) => {
            state.snapshot_id = snap;
            state.document_revision += 1;
            let stamp = state.stamp_job(job_id);
            write_resp_value(
                writer,
                req.id,
                stamp,
                json!({ "accepted": true, "snapshotId": snap, "documentRevision": state.document_revision }),
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
        state.prepared.retain(|(pj, _)| *pj != j);
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
    // Split into 2 data frames to prove multi-frame assembly by byteOffset.
    let mid = blob.len().div_ceil(2);
    let parts: Vec<&[u8]> = if mid == 0 || mid >= blob.len() {
        vec![blob]
    } else {
        vec![&blob[..mid], &blob[mid..]]
    };
    let manifest = ChunkFrame {
        v: PROTOCOL_VERSION,
        id,
        stream_id,
        kind: ChunkKind::Manifest,
        purpose: Some("mesh".into()),
        count: Some(parts.len() as u32),
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
    let mut offset = 0u64;
    for (index, part) in parts.iter().enumerate() {
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
            byte_offset: Some(offset),
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
        offset += part.len() as u64;
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
