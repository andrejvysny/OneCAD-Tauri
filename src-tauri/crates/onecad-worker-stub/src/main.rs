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
//!
//! Logs go to stderr only; stdout carries frames exclusively (SCHEMA §1).

use std::io::Write;

use onecad_protocol::framing::{read_frame_blocking, write_frame_blocking, MAGIC_BYTES};
use onecad_protocol::messages::{
    CloseSessionResult, ErrorCode, ErrorObject, Frame, HelloFrame, HelloLimits, HelloResult,
    OcctInfo, OpenSessionArgs, OpenSessionResult, ReqFrame, RespFrame, ShutdownResult, Stamp,
    WorkerHeadBrief, WorkerHeadResult, PROTOCOL_VERSION,
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
}

impl StubState {
    fn new() -> Self {
        StubState {
            seq: 0,
            session_open: false,
            document_revision: 0,
            worker_epoch: 0,
            snapshot_id: 0,
        }
    }

    /// Allocate the next output `seq`.
    fn next_seq(&mut self) -> u64 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    fn stamp(&mut self) -> Stamp {
        Stamp {
            document_revision: self.document_revision,
            worker_epoch: self.worker_epoch,
            snapshot_id: self.snapshot_id,
            job_id: None,
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
                    has_scratch: false,
                },
            )
        }
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
