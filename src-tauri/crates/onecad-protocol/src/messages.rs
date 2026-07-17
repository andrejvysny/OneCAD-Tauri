//! JSON control envelopes exchanged over OCW1.
//!
//! One [`Frame`] models every envelope shape in `../../protocol/SCHEMA.md` §3,
//! internally tagged by the `t` field (`hello`/`req`/`resp`/`progress`/`event`/
//! `cancel`/`credit`/`chunk`). All object keys are camelCase; `u64` ids ride as
//! JSON numbers (safe — no JavaScript on this path). 64-bit hashes are hex
//! strings. Non-finite floats: serde_json REJECTS `NaN`/`Infinity` tokens on
//! read (SCHEMA §4), but on WRITE it silently emits `null` — so producers must
//! call [`ensure_finite`] at payload-construction time to uphold "producers MUST
//! NOT emit them".
//!
//! Only the verbs the tests exercise (Hello, Shutdown, OpenSession/CloseSession,
//! GetWorkerHead) get typed arg/result structs; every other verb rides the
//! generic [`Frame::Req`] escape hatch (`verb: String` + `args: Value`). Typed op
//! payloads land with R-WP7.
//!
//! NOTE (SCHEMA vs task wording): the frame-type discriminator on the wire is the
//! `t` field per SCHEMA §3 and the fixtures; the crate models it exactly.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::ProtocolError;

/// Wire protocol version (SCHEMA §3: every envelope carries `v: 1`).
pub const PROTOCOL_VERSION: u32 = 1;

fn default_version() -> u32 {
    PROTOCOL_VERSION
}

/// Reject `NaN`/`±Infinity` before a float enters a payload.
///
/// `serde_json` already errors on non-finite floats at serialize time; this is
/// the explicit guard SCHEMA §4 calls for during payload construction (so a
/// non-finite value is caught at the call site, not deep inside serialization).
pub fn ensure_finite(x: f64) -> Result<f64, ProtocolError> {
    if x.is_finite() {
        Ok(x)
    } else {
        Err(ProtocolError::NonFinite)
    }
}

/// Logical transport lane (SCHEMA §5.1). Omitted on the wire ⇒ `Control`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lane {
    /// Latency-sensitive control frames; never blocked by flow control.
    #[default]
    Control,
    /// Bulk chunk streams (MESH1 / BREP); subject to byte-budget credit.
    Bulk,
}

impl Lane {
    fn is_control(&self) -> bool {
        matches!(self, Lane::Control)
    }
}

/// Named section inside a frame's binary tail (SCHEMA §1). `off`/`len` are byte
/// offsets relative to the start of the tail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BinSection {
    /// UTF-8 section name, unique within the frame.
    pub name: String,
    /// Byte offset from the start of the binary tail.
    pub off: u32,
    /// Section length in bytes.
    pub len: u32,
}

/// Error taxonomy codes (SCHEMA §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// Recoverable op failure; scratch only, session intact.
    OpFailed,
    /// A hard reference resolve failure (distinct from NeedsRepair state).
    RefUnresolved,
    /// Invalid geometry produced.
    GeometryInvalid,
    /// Known verb, unsupported op/param.
    Unsupported,
    /// Cooperative cancellation (terminal frame is never dropped).
    Cancelled,
    /// Protocol violation. Framing sub-case tears down; well-framed-illegal
    /// sub-case (unknown verb, stale fencing, bad args) replies with this in a
    /// terminal `resp`.
    ProtocolError,
}

/// The `error` object of a failed `resp` (SCHEMA §8).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorObject {
    /// Machine-readable code.
    pub code: ErrorCode,
    /// Human-readable message.
    pub message: String,
    /// Optional structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
    /// Whether the caller may retry as-is.
    pub retriable: bool,
}

// ---------------------------------------------------------------------------
// Frame enum
// ---------------------------------------------------------------------------

/// Every OCW1 JSON envelope, internally tagged by `t` (SCHEMA §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "camelCase")]
pub enum Frame {
    /// Unsolicited worker handshake (SCHEMA §6).
    Hello(HelloFrame),
    /// Rust → worker request.
    Req(ReqFrame),
    /// Worker → Rust terminal response (exactly one per request id).
    Resp(RespFrame),
    /// Worker → Rust non-terminal progress.
    Progress(ProgressFrame),
    /// Worker → Rust non-terminal structured event.
    Event(EventFrame),
    /// Rust → worker cancellation.
    Cancel(CancelFrame),
    /// Rust → worker bulk-lane credit grant.
    Credit(CreditFrame),
    /// Worker → Rust bulk stream frame (manifest or data).
    Chunk(ChunkFrame),
}

impl Frame {
    /// Serialize to JSON envelope bytes. Rejects `NaN`/`Inf` (serde_json default).
    pub fn to_json_vec(&self) -> Result<Vec<u8>, ProtocolError> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Parse a JSON envelope.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Frame, ProtocolError> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// `hello` frame (SCHEMA §6). Carries only `seq` (no full stamp).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    pub seq: u64,
    pub result: HelloResult,
}

/// `req` frame (SCHEMA §3.1). Verb-specific body rides `args` (escape hatch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReqFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    pub id: u64,
    pub verb: String,
    #[serde(default, skip_serializing_if = "Lane::is_control")]
    pub lane: Lane,
    #[serde(default)]
    pub args: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Vec<BinSection>>,
}

/// `resp` frame (SCHEMA §3.2). Terminal; `result` iff `ok`, `error` iff `!ok`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RespFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    pub id: u64,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
    pub document_revision: u64,
    pub worker_epoch: u64,
    pub snapshot_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<u64>,
    pub seq: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Vec<BinSection>>,
}

/// `progress` frame (SCHEMA §3.3). Informational; never required for correctness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    pub id: u64,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub document_revision: u64,
    pub worker_epoch: u64,
    pub snapshot_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<u64>,
    pub seq: u64,
}

/// `event` frame (SCHEMA §3.4). Correlation-scoped structured domain events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    pub id: u64,
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<u64>,
    #[serde(default)]
    pub payload: Value,
    pub document_revision: u64,
    pub worker_epoch: u64,
    pub snapshot_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<u64>,
    pub seq: u64,
}

/// `cancel` frame (SCHEMA §3.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    pub id: u64,
}

/// `credit` frame (SCHEMA §3.6) — bulk-lane byte-budget grant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    #[serde(default)]
    pub lane: Lane,
    pub bytes: u64,
}

/// Discriminator for a [`ChunkFrame`] (SCHEMA §3.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChunkKind {
    /// First frame of a stream: `count`/`totalBytes`/`sha256`/`meta`.
    Manifest,
    /// A data frame carrying a slice of the payload in its binary tail.
    Data,
}

/// `chunk` frame (SCHEMA §3.7). A flat union of the manifest and data shapes;
/// assembly logic lands in a later WP — this crate only decodes/surfaces them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkFrame {
    #[serde(default = "default_version")]
    pub v: u32,
    pub id: u64,
    pub stream_id: u64,
    pub kind: ChunkKind,
    // -- manifest fields --
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
    // -- data fields --
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub byte_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bin: Option<Vec<BinSection>>,
    // -- stamp --
    pub document_revision: u64,
    pub worker_epoch: u64,
    pub snapshot_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<u64>,
    pub seq: u64,
}

/// The worker → Rust frame stamp (SCHEMA §3): fencing + ordering tokens shared by
/// every worker-originated frame except `hello` (which carries only `seq`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stamp {
    pub document_revision: u64,
    pub worker_epoch: u64,
    pub snapshot_id: u64,
    pub job_id: Option<u64>,
    pub seq: u64,
}

// ---------------------------------------------------------------------------
// Handshake result (SCHEMA §6)
// ---------------------------------------------------------------------------

/// `hello.result` (SCHEMA §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloResult {
    pub protocol_version: u32,
    pub worker_version: String,
    pub occt: OcctInfo,
    pub quantization_version: u32,
    pub solver_policy_version: u32,
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<HelloLimits>,
}

/// OCCT identity in the handshake (SCHEMA §6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OcctInfo {
    pub version: String,
    /// 64-bit fingerprint hex; governs BREP/checkpoint cache compatibility.
    pub fingerprint: String,
}

/// Negotiated transport limits (SCHEMA §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloLimits {
    pub chunk_size: u64,
    pub initial_bulk_credit: u64,
}

// ---------------------------------------------------------------------------
// Verb payloads the tests exercise (SCHEMA §7.1)
// ---------------------------------------------------------------------------

/// `OpenSession` args (SCHEMA §7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionArgs {
    pub document_id: String,
    pub document_revision: u64,
    pub worker_epoch: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerance_policy: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// `OpenSession` result (SCHEMA §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionResult {
    pub session_open: bool,
    pub worker_head: WorkerHeadBrief,
}

/// The brief `workerHead` inside an `OpenSession` result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHeadBrief {
    pub document_revision: u64,
    pub snapshot_id: u64,
}

/// `CloseSession` result (SCHEMA §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloseSessionResult {
    pub session_closed: bool,
}

/// `Shutdown` result (SCHEMA §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShutdownResult {
    pub goodbye: bool,
}

/// `GetWorkerHead` result (SCHEMA §7.1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkerHeadResult {
    pub document_revision: u64,
    pub worker_epoch: u64,
    pub snapshot_id: u64,
    /// 64-bit hex history-prefix hash.
    pub history_prefix_hash: String,
    pub has_scratch: bool,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn error_code_screaming_snake_case() {
        assert_eq!(
            serde_json::to_value(ErrorCode::ProtocolError).unwrap(),
            json!("PROTOCOL_ERROR")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::OpFailed).unwrap(),
            json!("OP_FAILED")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::RefUnresolved).unwrap(),
            json!("REF_UNRESOLVED")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::GeometryInvalid).unwrap(),
            json!("GEOMETRY_INVALID")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::Unsupported).unwrap(),
            json!("UNSUPPORTED")
        );
        assert_eq!(
            serde_json::to_value(ErrorCode::Cancelled).unwrap(),
            json!("CANCELLED")
        );
    }

    #[test]
    fn req_serializes_with_t_tag_and_omits_control_lane() {
        let f = Frame::Req(ReqFrame {
            v: PROTOCOL_VERSION,
            id: 42,
            verb: "OpenSession".into(),
            lane: Lane::Control,
            args: json!({ "documentId": "doc_1" }),
            bin: None,
        });
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["t"], "req");
        assert_eq!(v["v"], 1);
        assert_eq!(v["id"], 42);
        assert_eq!(v["verb"], "OpenSession");
        assert!(v.get("lane").is_none(), "control lane must be omitted");
        assert!(v.get("bin").is_none());
    }

    #[test]
    fn bulk_lane_is_emitted() {
        let f = Frame::Credit(CreditFrame {
            v: PROTOCOL_VERSION,
            lane: Lane::Bulk,
            bytes: 4194304,
        });
        let v = serde_json::to_value(&f).unwrap();
        assert_eq!(v["t"], "credit");
        assert_eq!(v["lane"], "bulk");
        assert_eq!(v["bytes"], 4194304);
    }

    #[test]
    fn resp_round_trip_ok_and_err() {
        let ok = Frame::Resp(RespFrame {
            v: PROTOCOL_VERSION,
            id: 1,
            ok: true,
            result: Some(json!({ "sessionOpen": true })),
            error: None,
            document_revision: 0,
            worker_epoch: 1,
            snapshot_id: 0,
            job_id: None,
            seq: 5,
            bin: None,
        });
        let bytes = ok.to_json_vec().unwrap();
        assert_eq!(Frame::from_json_slice(&bytes).unwrap(), ok);

        let err = Frame::Resp(RespFrame {
            v: PROTOCOL_VERSION,
            id: 2,
            ok: false,
            result: None,
            error: Some(ErrorObject {
                code: ErrorCode::ProtocolError,
                message: "unknown verb".into(),
                detail: None,
                retriable: false,
            }),
            document_revision: 0,
            worker_epoch: 0,
            snapshot_id: 0,
            job_id: None,
            seq: 6,
            bin: None,
        });
        let bytes = err.to_json_vec().unwrap();
        let back = Frame::from_json_slice(&bytes).unwrap();
        match back {
            Frame::Resp(r) => {
                assert!(!r.ok);
                assert_eq!(r.error.unwrap().code, ErrorCode::ProtocolError);
            }
            other => panic!("expected resp, got {other:?}"),
        }
    }

    #[test]
    fn chunk_manifest_and_data_decode() {
        let manifest = json!({
            "v": 1, "t": "chunk", "id": 42, "streamId": 700, "kind": "manifest",
            "purpose": "mesh", "count": 8, "totalBytes": 4194304,
            "sha256": "aa", "meta": { "bodyId": "body_3" },
            "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012, "jobId": 88, "seq": 906
        });
        match Frame::from_json_slice(manifest.to_string().as_bytes()).unwrap() {
            Frame::Chunk(c) => {
                assert_eq!(c.kind, ChunkKind::Manifest);
                assert_eq!(c.count, Some(8));
                assert_eq!(c.job_id, Some(88));
            }
            other => panic!("expected chunk, got {other:?}"),
        }

        let data = json!({
            "v": 1, "t": "chunk", "id": 42, "streamId": 700, "kind": "data",
            "index": 0, "byteOffset": 0,
            "bin": [ { "name": "chunk", "off": 0, "len": 524288 } ],
            "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012, "jobId": 88, "seq": 907
        });
        match Frame::from_json_slice(data.to_string().as_bytes()).unwrap() {
            Frame::Chunk(c) => {
                assert_eq!(c.kind, ChunkKind::Data);
                assert_eq!(c.index, Some(0));
                assert_eq!(c.bin.unwrap()[0].len, 524288);
            }
            other => panic!("expected chunk, got {other:?}"),
        }
    }

    #[test]
    fn hello_result_round_trip() {
        let hello = HelloResult {
            protocol_version: 1,
            worker_version: "0.1.0".into(),
            occt: OcctInfo {
                version: "7.9.3".into(),
                fingerprint: "9a1c33f0e7b24d10".into(),
            },
            quantization_version: 1,
            solver_policy_version: 1,
            capabilities: vec!["op.extrude".into()],
            limits: Some(HelloLimits {
                chunk_size: 1048576,
                initial_bulk_credit: 8388608,
            }),
        };
        let v = serde_json::to_value(&hello).unwrap();
        assert_eq!(v["protocolVersion"], 1);
        assert_eq!(v["occt"]["fingerprint"], "9a1c33f0e7b24d10");
        assert_eq!(v["limits"]["chunkSize"], 1048576);
        let back: HelloResult = serde_json::from_value(v).unwrap();
        assert_eq!(back, hello);
    }

    #[test]
    fn ensure_finite_guards_non_finite() {
        assert_eq!(ensure_finite(3.5).unwrap(), 3.5);
        assert!(matches!(
            ensure_finite(f64::NAN),
            Err(ProtocolError::NonFinite)
        ));
        assert!(matches!(
            ensure_finite(f64::INFINITY),
            Err(ProtocolError::NonFinite)
        ));
    }

    #[test]
    fn serde_json_nulls_non_finite_on_serialize() {
        // NOTE (SCHEMA §4 nuance): serde_json does NOT error when serializing a
        // non-finite float — it emits `null`. Enforcement of "producers MUST NOT
        // emit NaN/Inf" therefore lives in `ensure_finite`, which callers use at
        // payload-construction time. This test pins that observed behavior so the
        // gap is explicit.
        let f = Frame::Progress(ProgressFrame {
            v: PROTOCOL_VERSION,
            id: 1,
            phase: "x".into(),
            fraction: Some(f64::NAN),
            message: None,
            document_revision: 0,
            worker_epoch: 0,
            snapshot_id: 0,
            job_id: None,
            seq: 1,
        });
        let value: Value = serde_json::from_slice(&f.to_json_vec().unwrap()).unwrap();
        assert!(value["fraction"].is_null(), "serde_json nulls NaN on write");
    }

    #[test]
    fn reader_rejects_non_finite_tokens() {
        // Enforcement on READ is native to serde_json: `NaN`/`Infinity` literals
        // are rejected (SCHEMA §4). A `resp` whose float is `NaN` fails to parse.
        let bad = br#"{"t":"progress","v":1,"id":1,"phase":"x","fraction":NaN,
            "documentRevision":0,"workerEpoch":0,"snapshotId":0,"seq":1}"#;
        assert!(Frame::from_json_slice(bad).is_err());
        let bad_inf = br#"{"t":"progress","v":1,"id":1,"phase":"x","fraction":Infinity,
            "documentRevision":0,"workerEpoch":0,"snapshotId":0,"seq":1}"#;
        assert!(Frame::from_json_slice(bad_inf).is_err());
    }

    /// Task 7: the NDJSON fixtures parse into these message types. `send`
    /// directives carry concrete frames (parsed strictly into `Frame`); `expect`
    /// directives are matchers with `$any`/`$hex64` placeholders, so we assert
    /// only that their `t` maps to a known variant.
    #[test]
    fn ndjson_fixtures_parse_into_message_types() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../protocol/fixtures");
        for name in ["hello.ndjson", "echo_error.ndjson"] {
            let path = format!("{dir}/{name}");
            let text =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
            let mut sends = 0usize;
            let mut expects = 0usize;
            for (lineno, line) in text.lines().enumerate() {
                let trimmed = line.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let directive: Value = serde_json::from_str(trimmed)
                    .unwrap_or_else(|e| panic!("{name}:{} parse directive: {e}", lineno + 1));
                if let Some(frame) = directive.get("send") {
                    // Concrete driver frame: must parse strictly into Frame.
                    let parsed: Frame = serde_json::from_value(frame.clone())
                        .unwrap_or_else(|e| panic!("{name}:{} send into Frame: {e}", lineno + 1));
                    // The two fixtures only send `req` frames.
                    assert!(matches!(parsed, Frame::Req(_)), "{name}:{}", lineno + 1);
                    sends += 1;
                } else if let Some(matcher) = directive.get("expect") {
                    // Matcher: assert the discriminator names a known variant.
                    let t = matcher["t"].as_str().unwrap_or("");
                    assert!(
                        matches!(
                            t,
                            "hello" | "resp" | "progress" | "event" | "cancel" | "credit" | "chunk"
                        ),
                        "{name}:{} unknown frame type {t:?}",
                        lineno + 1
                    );
                    expects += 1;
                } else {
                    panic!("{name}:{} unknown directive: {trimmed}", lineno + 1);
                }
            }
            assert!(sends > 0 && expects > 0, "{name}: empty fixture");
        }
    }

    #[test]
    fn open_session_args_and_result_round_trip() {
        let args = json!({
            "documentId": "doc_1", "documentRevision": 0, "workerEpoch": 1,
            "tolerancePolicy": { "linear": 1e-7 }, "mode": "determinism"
        });
        let parsed: OpenSessionArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.document_id, "doc_1");
        assert_eq!(parsed.worker_epoch, 1);

        let result = OpenSessionResult {
            session_open: true,
            worker_head: WorkerHeadBrief {
                document_revision: 0,
                snapshot_id: 0,
            },
        };
        let v = serde_json::to_value(&result).unwrap();
        assert_eq!(v["sessionOpen"], true);
        assert_eq!(v["workerHead"]["documentRevision"], 0);
    }
}
