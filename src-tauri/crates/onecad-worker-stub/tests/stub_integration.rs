//! Integration tests that spawn the REAL built stub binary and drive it over
//! OCW1, plus an NDJSON fixture runner that replays `protocol/fixtures/*.ndjson`.
//!
//! The binary path comes from `CARGO_BIN_EXE_onecad-worker-stub` (Cargo sets it
//! for integration tests of the crate that owns the `[[bin]]`).

use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use onecad_protocol::client::ProtocolClient;
use onecad_protocol::framing::{decode_frame, encode_frame, RawFrame};
use onecad_protocol::messages::{ErrorCode, Frame};
use onecad_protocol::ProtocolError;
use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

fn stub_bin() -> &'static str {
    env!("CARGO_BIN_EXE_onecad-worker-stub")
}

fn spawn_stub(envs: &[(&str, &str)]) -> Child {
    let mut cmd = Command::new(stub_bin());
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.spawn().expect("spawn onecad-worker-stub")
}

/// Async OCW1 frame reader over a child's stdout (Vec-buffered; no `bytes` dep).
struct FrameReader<R> {
    inner: R,
    buf: Vec<u8>,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    fn new(inner: R) -> Self {
        FrameReader {
            inner,
            buf: Vec::new(),
        }
    }

    async fn next_frame(&mut self) -> Result<Option<RawFrame>, ProtocolError> {
        loop {
            if let Some((frame, consumed)) = decode_frame(&self.buf)? {
                self.buf.drain(0..consumed);
                return Ok(Some(frame));
            }
            let mut tmp = [0u8; 8192];
            let n = self.inner.read(&mut tmp).await?;
            if n == 0 {
                if self.buf.is_empty() {
                    return Ok(None);
                }
                return Err(ProtocolError::ConnectionLost("eof mid-frame"));
            }
            self.buf.extend_from_slice(&tmp[..n]);
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn drills (task 6c) — via the async ProtocolClient
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hello_round_trip_and_lifecycle() {
    let mut child = spawn_stub(&[]);
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let client = ProtocolClient::connect(stdout, stdin).await.unwrap();

    // Handshake fields (SCHEMA §6).
    let hello = client.hello();
    assert_eq!(hello.protocol_version, 1);
    assert_eq!(hello.worker_version, "stub-0.1");
    assert_eq!(hello.occt.version, "stub");
    assert_eq!(hello.occt.fingerprint.len(), 16); // $hex64
    assert_eq!(hello.quantization_version, 1);
    assert_eq!(hello.solver_policy_version, 1);
    assert!(hello.capabilities.is_empty());

    // OpenSession echoes the fencing tokens (SCHEMA §7.1).
    let resp = client
        .request(
            "OpenSession",
            json!({
                "documentId": "doc_1", "documentRevision": 0, "workerEpoch": 1,
                "tolerancePolicy": { "linear": 1e-7, "angular": 1e-9,
                    "tolerancePolicyHash": "0000000000000000" },
                "mode": "determinism"
            }),
        )
        .await
        .unwrap();
    assert!(resp.ok);
    assert_eq!(resp.worker_epoch, 1);
    assert_eq!(resp.result.unwrap()["sessionOpen"], true);

    // GetWorkerHead reflects the opened session.
    let head = client.request("GetWorkerHead", json!({})).await.unwrap();
    assert!(head.ok);
    let head_result = head.result.unwrap();
    assert_eq!(head_result["workerEpoch"], 1);
    assert_eq!(head_result["hasScratch"], false);

    // Shutdown -> goodbye, clean exit 0.
    let bye = client.request("Shutdown", json!({})).await.unwrap();
    assert!(bye.ok);
    assert_eq!(bye.result.unwrap()["goodbye"], true);

    client.close().await;
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("stub should exit")
        .unwrap();
    assert!(status.success(), "stub should exit 0 after Shutdown");
}

#[tokio::test]
async fn unknown_verb_returns_protocol_error() {
    let mut child = spawn_stub(&[]);
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let client = ProtocolClient::connect(stdout, stdin).await.unwrap();

    let resp = client.request("TotallyBogus", json!({})).await.unwrap();
    assert!(!resp.ok);
    let err = resp.error.unwrap();
    assert_eq!(err.code, ErrorCode::ProtocolError);
    assert!(!err.retriable);

    client.close().await;
}

#[tokio::test]
async fn crash_on_verb_fails_all_pending_with_connection_lost() {
    let mut child = spawn_stub(&[("ONECAD_STUB_CRASH_ON", "Crash")]);
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let client = std::sync::Arc::new(ProtocolClient::connect(stdout, stdin).await.unwrap());

    // Fire a batch of requests that all trigger the crash. When the stub reads
    // the first "Crash" it aborts, closing stdout -> reader EOF -> every pending
    // request resolves to ConnectionLost.
    let mut handles = Vec::new();
    for _ in 0..10 {
        let c = client.clone();
        handles.push(tokio::spawn(
            async move { c.request("Crash", json!({})).await },
        ));
    }
    for h in handles {
        let result = h.await.unwrap();
        assert!(
            matches!(result, Err(ProtocolError::ConnectionLost(_))),
            "expected ConnectionLost, got {result:?}"
        );
    }
    // The child died from SIGABRT.
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("crashed stub should be reaped")
        .unwrap();
    assert!(!status.success());
}

#[tokio::test]
async fn hang_on_verb_times_out_client_side() {
    let mut child = spawn_stub(&[("ONECAD_STUB_HANG_ON", "Hang")]);
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();
    let client = ProtocolClient::connect(stdout, stdin).await.unwrap();

    // The stub sleeps forever on "Hang"; the Rust-side deadline fires (SCHEMA §8
    // timeouts are Rust-enforced).
    let result = client
        .request_timeout("Hang", json!({}), Duration::from_millis(300))
        .await;
    assert!(matches!(result, Err(ProtocolError::Timeout)), "{result:?}");

    client.close().await;
    child.start_kill().ok();
    let _ = child.wait().await;
}

#[tokio::test]
async fn garbage_first_frame_is_bad_magic() {
    let mut child = spawn_stub(&[("ONECAD_STUB_GARBAGE", "1")]);
    let stdout = child.stdout.take().unwrap();
    let stdin = child.stdin.take().unwrap();

    // The very first bytes have a corrupted magic -> connect fails with BadMagic
    // (fatal, no resync; SCHEMA §1/§8).
    match ProtocolClient::connect(stdout, stdin).await {
        Err(ProtocolError::BadMagic { .. }) => {}
        other => panic!("expected BadMagic, got {other:?}"),
    }
    child.start_kill().ok();
    let _ = child.wait().await;
}

// ---------------------------------------------------------------------------
// NDJSON fixture runner (task 7) — replays the canonical fixtures against the
// real stub, proving every frame round-trips through the message types.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fixture_hello_ndjson() {
    run_fixture("hello.ndjson").await;
}

#[tokio::test]
async fn fixture_echo_error_ndjson() {
    run_fixture("echo_error.ndjson").await;
}

fn fixtures_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../protocol/fixtures")
}

async fn run_fixture(name: &str) {
    let path = fixtures_dir().join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));

    let mut child = spawn_stub(&[]);
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = FrameReader::new(child.stdout.take().unwrap());
    let mut caps: HashMap<String, Value> = HashMap::new();

    for (lineno, line) in text.lines().enumerate() {
        let ln = lineno + 1;
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let directive: Value =
            serde_json::from_str(trimmed).unwrap_or_else(|e| panic!("{name}:{ln} directive: {e}"));

        if let Some(send) = directive.get("send") {
            let json = serde_json::to_vec(send).unwrap();
            let framed = encode_frame(&json, &[]).unwrap();
            stdin.write_all(&framed).await.unwrap();
            stdin.flush().await.unwrap();
        } else if let Some(expect) = directive.get("expect") {
            let frame = reader
                .next_frame()
                .await
                .unwrap_or_else(|e| panic!("{name}:{ln} read frame: {e}"))
                .unwrap_or_else(|| panic!("{name}:{ln} expected a frame, got EOF"));
            // Ties fixtures to the message types: every worker frame must parse.
            Frame::from_json_slice(&frame.json)
                .unwrap_or_else(|e| panic!("{name}:{ln} into Frame: {e}"));
            let actual: Value = serde_json::from_slice(&frame.json).unwrap();
            subset_match(expect, &actual, &mut caps)
                .unwrap_or_else(|e| panic!("{name}:{ln} match failed: {e}"));
        } else if directive.get("tolerance").is_some() {
            // The two canonical fixtures use default (exact) tolerance.
        } else {
            panic!("{name}:{ln} unknown directive: {trimmed}");
        }
    }

    // hello.ndjson ends with Shutdown (stub exits 0); echo_error ends with the
    // error resp, then closing stdin drives EOF -> exit 0.
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
        .await
        .expect("stub should exit")
        .unwrap();
    assert!(status.success(), "{name}: stub exited with {status}");
}

/// Subset matcher per `protocol/fixtures/README.md`: every key in the matcher
/// must be present and equal in the actual frame; extra actual keys are ignored.
fn subset_match(
    expected: &Value,
    actual: &Value,
    caps: &mut HashMap<String, Value>,
) -> Result<(), String> {
    match expected {
        Value::String(s) if s.starts_with('$') => match_placeholder(s, actual, caps),
        Value::Object(map) => {
            let a = actual
                .as_object()
                .ok_or_else(|| format!("expected object, got {actual}"))?;
            for (k, v) in map {
                let av = a.get(k).ok_or_else(|| format!("missing key {k:?}"))?;
                subset_match(v, av, caps)?;
            }
            Ok(())
        }
        Value::Array(arr) => {
            let aa = actual
                .as_array()
                .ok_or_else(|| format!("expected array, got {actual}"))?;
            if arr.len() != aa.len() {
                return Err(format!("array len {} != {}", arr.len(), aa.len()));
            }
            for (e, a) in arr.iter().zip(aa) {
                subset_match(e, a, caps)?;
            }
            Ok(())
        }
        Value::Number(_) => {
            // Default tolerance is exact (abs 0).
            if expected.as_f64() == actual.as_f64() {
                Ok(())
            } else {
                Err(format!("number {expected} != {actual}"))
            }
        }
        _ => {
            if expected == actual {
                Ok(())
            } else {
                Err(format!("expected {expected}, got {actual}"))
            }
        }
    }
}

fn match_placeholder(
    token: &str,
    actual: &Value,
    caps: &mut HashMap<String, Value>,
) -> Result<(), String> {
    if token == "$any" {
        return Ok(());
    }
    if token == "$hex64" {
        // A 64-bit (16 char) OR SHA-256 (64 char) lowercase-hex hash — matching the
        // C++ harness's `$hex64` leniency. The canonical hello fixture uses it for
        // both the 16-char occt fingerprint and the 64-char empty-prefix hash.
        return check_hex(actual, &[16, 64]);
    }
    if token == "$hex256" {
        return check_hex(actual, &[64]);
    }
    if let Some(name) = token.strip_prefix("$capture:") {
        caps.insert(name.to_string(), actual.clone());
        return Ok(());
    }
    if let Some(name) = token.strip_prefix("$ref:") {
        return match caps.get(name) {
            Some(v) if v == actual => Ok(()),
            Some(v) => Err(format!("$ref:{name} = {v} != {actual}")),
            None => Err(format!("$ref:{name} not captured")),
        };
    }
    // A literal string that merely starts with '$' (none in the fixtures).
    if Value::String(token.to_string()) == *actual {
        Ok(())
    } else {
        Err(format!("literal {token:?} != {actual}"))
    }
}

fn check_hex(actual: &Value, lens: &[usize]) -> Result<(), String> {
    match actual.as_str() {
        Some(s)
            if lens.contains(&s.len())
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) =>
        {
            Ok(())
        }
        Some(s) => Err(format!(
            "expected lowercase hex of length {lens:?}, got {s:?}"
        )),
        None => Err(format!("expected hex string, got {actual}")),
    }
}
