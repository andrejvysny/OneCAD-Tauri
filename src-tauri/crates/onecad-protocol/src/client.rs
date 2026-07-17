//! Async `ProtocolClient` over the OCW1 stdio transport (feature `client`).
//!
//! Owns a reader task and a writer task over any `AsyncRead + AsyncWrite` pair
//! (the worker's stdout/stdin, or an in-process duplex for tests). It frames
//! requests, correlates each `resp` back to its caller by the Rust-assigned
//! monotonic `id`, and surfaces `progress`/`event`/`chunk` frames on a broadcast
//! channel. On reader EOF or a fatal framing error every pending request fails
//! with [`ProtocolError::ConnectionLost`] (SCHEMA §8 — no resync, caller
//! restarts). See `../../protocol/SCHEMA.md`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::{Buf, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::error::ProtocolError;
use crate::framing::{decode_frame, encode_frame, RawFrame, HEADER_LEN};
use crate::messages::{
    CancelFrame, ChunkFrame, CreditFrame, EventFrame, Frame, HelloResult, Lane, ProgressFrame,
    ReqFrame, RespFrame, PROTOCOL_VERSION,
};

/// A terminal `resp` plus its raw binary tail (SCHEMA §1). The tail is the bytes
/// a `resp` addressed by an inline `bin` section carries (e.g. a small MESH1 blob,
/// SCHEMA §5.2); empty when the frame declared no tail.
type RespWithBin = (RespFrame, Vec<u8>);

/// The correlation table plus a **closed** latch. Once the reader task tears down
/// (EOF / fatal frame — SCHEMA §8, no resync), it drains every waiter with
/// [`ProtocolError::ConnectionLost`] **and** latches `closed`, so a request that
/// races the teardown fails fast instead of registering a oneshot nothing will
/// ever fire (which would hang the caller forever). Registration + closing take
/// the same lock, so there is no lost-wakeup window.
#[derive(Default)]
struct Pending {
    waiters: HashMap<u64, oneshot::Sender<Result<RespWithBin, ProtocolError>>>,
    closed: bool,
}

type PendingMap = Arc<Mutex<Pending>>;

/// A non-terminal frame surfaced to subscribers (SCHEMA §3.3/§3.4/§3.7).
#[derive(Debug, Clone)]
pub enum WorkerEvent {
    /// A `progress` frame.
    Progress(ProgressFrame),
    /// An `event` frame (e.g. `ExecutePlan` `planStep`).
    Event(EventFrame),
    /// A `chunk` frame (manifest or data) with its raw binary tail (SCHEMA §1):
    /// a data frame's tail carries the bytes its `bin` section addresses, which
    /// the bulk assembler concatenates by `byteOffset` (SCHEMA §5.2).
    Chunk(ChunkFrame, Vec<u8>),
    /// A `credit` frame (normally Rust → worker; surfaced for completeness).
    Credit(CreditFrame),
}

/// An in-flight request handle: the assigned correlation `id` and a future for
/// the terminal `resp` (see [`ProtocolClient::start_request`]).
pub struct InflightRequest {
    /// The Rust-assigned monotonic correlation id (SCHEMA §2). Match `event` /
    /// `chunk` frames by this and pass it to [`ProtocolClient::cancel`].
    pub id: u64,
    rx: oneshot::Receiver<Result<RespWithBin, ProtocolError>>,
}

impl InflightRequest {
    /// Awaits the terminal `resp` (its raw binary tail dropped).
    /// [`ProtocolError::ConnectionLost`] if the worker connection dies before the
    /// response arrives (SCHEMA §8 — no resync).
    pub async fn response(self) -> Result<RespFrame, ProtocolError> {
        self.response_with_bin().await.map(|(resp, _)| resp)
    }

    /// Awaits the terminal `resp` **with** its raw binary tail (SCHEMA §1) — for a
    /// verb that inlines a small bulk payload in the `resp` tail (SCHEMA §5.2).
    pub async fn response_with_bin(self) -> Result<RespWithBin, ProtocolError> {
        match self.rx.await {
            Ok(result) => result,
            Err(_) => Err(ProtocolError::ConnectionLost("response channel dropped")),
        }
    }
}

/// Async handle to a running worker connection.
pub struct ProtocolClient {
    next_id: AtomicU64,
    cmd_tx: mpsc::Sender<RawFrame>,
    pending: PendingMap,
    events_tx: broadcast::Sender<WorkerEvent>,
    hello: HelloResult,
    writer_handle: JoinHandle<()>,
    reader_handle: JoinHandle<()>,
}

impl ProtocolClient {
    /// Connect over a reader/writer pair, consuming the worker's unsolicited
    /// `hello` (SCHEMA §6). Returns an error if the first frame is not a valid
    /// `hello` of protocol version 1, or if the transport dies first. A fatal
    /// framing error on the first frame (e.g. bad magic) is surfaced as-is.
    pub async fn connect<R, W>(mut reader: R, writer: W) -> Result<ProtocolClient, ProtocolError>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        // Read the first frame (hello) using the same buffered decode loop the
        // reader task uses, keeping any bytes that arrived after it.
        let mut buf = BytesMut::new();
        let hello_frame = loop {
            if let Some((frame, consumed)) = decode_frame(&buf)? {
                buf.advance(consumed);
                break frame;
            }
            let n = reader.read_buf(&mut buf).await?;
            if n == 0 {
                return Err(ProtocolError::ConnectionLost("closed before hello"));
            }
        };
        let hello = match Frame::from_json_slice(&hello_frame.json)? {
            Frame::Hello(h) => h,
            other => {
                return Err(ProtocolError::Protocol(format!(
                    "expected hello, got {}",
                    frame_kind(&other)
                )))
            }
        };
        if hello.result.protocol_version != PROTOCOL_VERSION {
            return Err(ProtocolError::Protocol(format!(
                "unsupported protocol version {}",
                hello.result.protocol_version
            )));
        }

        let pending: PendingMap = Arc::new(Mutex::new(Pending::default()));
        let (events_tx, _) = broadcast::channel(256);
        let (cmd_tx, cmd_rx) = mpsc::channel::<RawFrame>(64);

        let writer_handle = tokio::spawn(writer_task(writer, cmd_rx));
        let reader_handle =
            tokio::spawn(reader_task(reader, buf, pending.clone(), events_tx.clone()));

        Ok(ProtocolClient {
            next_id: AtomicU64::new(1),
            cmd_tx,
            pending,
            events_tx,
            hello: hello.result,
            writer_handle,
            reader_handle,
        })
    }

    /// The worker's handshake result (SCHEMA §6).
    pub fn hello(&self) -> &HelloResult {
        &self.hello
    }

    /// Subscribe to non-terminal worker frames (progress/event/chunk).
    pub fn subscribe(&self) -> broadcast::Receiver<WorkerEvent> {
        self.events_tx.subscribe()
    }

    /// Send a `req` and await its terminal `resp`, correlated by a fresh
    /// monotonic id. Fails with [`ProtocolError::ConnectionLost`] if the worker
    /// connection dies before a response arrives.
    pub async fn request(
        &self,
        verb: &str,
        args: serde_json::Value,
    ) -> Result<RespFrame, ProtocolError> {
        self.start_request(verb, args, Lane::Control)
            .await?
            .response()
            .await
    }

    /// Send a `req` and return an [`InflightRequest`] carrying the assigned
    /// correlation `id` (for filtering `event`/`chunk` frames on
    /// [`subscribe`](Self::subscribe) and for [`cancel`](Self::cancel)) plus a
    /// future for the terminal `resp`.
    ///
    /// A streaming verb (`ExecutePlan`) needs the id *before* the terminal
    /// arrives so its interleaved `planStep` events can be matched — hence the
    /// split from [`request`](Self::request), which hides the id.
    pub async fn start_request(
        &self,
        verb: &str,
        args: serde_json::Value,
        lane: Lane,
    ) -> Result<InflightRequest, ProtocolError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let frame = Frame::Req(ReqFrame {
            v: PROTOCOL_VERSION,
            id,
            verb: verb.to_string(),
            lane,
            args,
            bin: None,
        });
        let raw = RawFrame::json_only(frame.to_json_vec()?);

        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().unwrap();
            if pending.closed {
                return Err(ProtocolError::ConnectionLost("connection closed"));
            }
            pending.waiters.insert(id, tx);
        }

        if self.cmd_tx.send(raw).await.is_err() {
            self.pending.lock().unwrap().waiters.remove(&id);
            return Err(ProtocolError::ConnectionLost("writer task ended"));
        }
        Ok(InflightRequest { id, rx })
    }

    /// [`request`](Self::request) with a Rust-side deadline (SCHEMA §8 timeouts
    /// are Rust-enforced). On timeout the pending slot is dropped and
    /// [`ProtocolError::Timeout`] is returned.
    pub async fn request_timeout(
        &self,
        verb: &str,
        args: serde_json::Value,
        deadline: Duration,
    ) -> Result<RespFrame, ProtocolError> {
        match tokio::time::timeout(deadline, self.request(verb, args)).await {
            Ok(result) => result,
            Err(_) => Err(ProtocolError::Timeout),
        }
    }

    /// Send a `cancel` for an in-flight request id (SCHEMA §3.5). Fire-and-forget;
    /// the worker still emits a terminal `resp` with `CANCELLED`.
    pub async fn cancel(&self, id: u64) -> Result<(), ProtocolError> {
        let frame = Frame::Cancel(CancelFrame {
            v: PROTOCOL_VERSION,
            id,
        });
        self.send_control(frame).await
    }

    /// Grant bulk-lane byte-budget credit (SCHEMA §3.6/§5.3).
    pub async fn grant_credit(&self, bytes: u64) -> Result<(), ProtocolError> {
        let frame = Frame::Credit(CreditFrame {
            v: PROTOCOL_VERSION,
            lane: Lane::Bulk,
            bytes,
        });
        self.send_control(frame).await
    }

    async fn send_control(&self, frame: Frame) -> Result<(), ProtocolError> {
        let raw = RawFrame::json_only(frame.to_json_vec()?);
        self.cmd_tx
            .send(raw)
            .await
            .map_err(|_| ProtocolError::ConnectionLost("writer task ended"))
    }

    /// Graceful shutdown: stop the writer and reader tasks and drop the
    /// transport. Any still-pending requests will observe `ConnectionLost`.
    pub async fn close(self) {
        drop(self.cmd_tx); // writer task ends when its receiver closes
        self.reader_handle.abort();
        self.writer_handle.abort();
        let _ = self.reader_handle.await;
        let _ = self.writer_handle.await;
    }
}

impl std::fmt::Debug for ProtocolClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProtocolClient")
            .field("worker_version", &self.hello.worker_version)
            .field("next_id", &self.next_id.load(Ordering::Relaxed))
            .field(
                "pending",
                &self.pending.lock().map(|m| m.waiters.len()).unwrap_or(0),
            )
            .finish_non_exhaustive()
    }
}

fn frame_kind(frame: &Frame) -> &'static str {
    match frame {
        Frame::Hello(_) => "hello",
        Frame::Req(_) => "req",
        Frame::Resp(_) => "resp",
        Frame::Progress(_) => "progress",
        Frame::Event(_) => "event",
        Frame::Cancel(_) => "cancel",
        Frame::Credit(_) => "credit",
        Frame::Chunk(_) => "chunk",
    }
}

async fn writer_task<W>(mut writer: W, mut cmd_rx: mpsc::Receiver<RawFrame>)
where
    W: AsyncWrite + Unpin,
{
    while let Some(frame) = cmd_rx.recv().await {
        let bytes = match encode_frame(&frame.json, &frame.bin) {
            Ok(bytes) => bytes,
            Err(err) => {
                eprintln!("onecad-protocol: writer encode error: {err}");
                break;
            }
        };
        if writer.write_all(&bytes).await.is_err() || writer.flush().await.is_err() {
            break; // transport gone; reader will fail pending requests
        }
    }
    let _ = writer.shutdown().await;
}

async fn reader_task<R>(
    mut reader: R,
    mut buf: BytesMut,
    pending: PendingMap,
    events_tx: broadcast::Sender<WorkerEvent>,
) where
    R: AsyncRead + Unpin,
{
    loop {
        // Drain every complete frame currently buffered.
        loop {
            match decode_frame(&buf) {
                Ok(Some((frame, consumed))) => {
                    buf.advance(consumed);
                    dispatch(&frame, &pending, &events_tx);
                }
                Ok(None) => break,
                Err(err) => {
                    // Fatal framing violation (bad magic / over-cap). No resync.
                    eprintln!("onecad-protocol: fatal frame error: {err}");
                    fail_all(&pending);
                    return;
                }
            }
        }
        // Reserve for a known-but-incomplete frame so we read it in one shot.
        if buf.len() >= HEADER_LEN {
            // decode_frame above returned Ok(None) without erroring, so caps are
            // valid; recompute the needed size defensively.
            if let Ok(json_len) = try_len(&buf[4..8]) {
                if let Ok(bin_len) = try_len(&buf[8..12]) {
                    let total = HEADER_LEN + json_len + bin_len;
                    if total > buf.len() {
                        buf.reserve(total - buf.len());
                    }
                }
            }
        }
        match reader.read_buf(&mut buf).await {
            Ok(0) => {
                // EOF. Clean at a boundary or mid-frame: either way, pending
                // requests can never complete -> ConnectionLost.
                fail_all(&pending);
                return;
            }
            Ok(_) => {}
            Err(err) => {
                eprintln!("onecad-protocol: reader io error: {err}");
                fail_all(&pending);
                return;
            }
        }
    }
}

fn try_len(bytes: &[u8]) -> Result<usize, ()> {
    let arr: [u8; 4] = bytes.try_into().map_err(|_| ())?;
    Ok(u32::from_le_bytes(arr) as usize)
}

fn dispatch(frame: &RawFrame, pending: &PendingMap, events_tx: &broadcast::Sender<WorkerEvent>) {
    let parsed = match Frame::from_json_slice(&frame.json) {
        Ok(parsed) => parsed,
        Err(err) => {
            // A well-framed but unparseable envelope is a protocol error; the
            // frame stream is desynchronized. Fail everything (no resync).
            eprintln!("onecad-protocol: malformed envelope: {err}");
            fail_all(pending);
            return;
        }
    };
    match parsed {
        Frame::Resp(resp) => {
            let waiter = pending.lock().unwrap().waiters.remove(&resp.id);
            if let Some(tx) = waiter {
                let _ = tx.send(Ok((resp, frame.bin.clone())));
            } else {
                eprintln!("onecad-protocol: resp for unknown id {}", resp.id);
            }
        }
        Frame::Progress(p) => {
            let _ = events_tx.send(WorkerEvent::Progress(p));
        }
        Frame::Event(e) => {
            let _ = events_tx.send(WorkerEvent::Event(e));
        }
        Frame::Chunk(c) => {
            let _ = events_tx.send(WorkerEvent::Chunk(c, frame.bin.clone()));
        }
        Frame::Credit(c) => {
            let _ = events_tx.send(WorkerEvent::Credit(c));
        }
        Frame::Hello(_) => {
            eprintln!("onecad-protocol: unexpected second hello");
        }
        Frame::Req(_) | Frame::Cancel(_) => {
            eprintln!("onecad-protocol: worker sent a driver-only frame");
        }
    }
}

fn fail_all(pending: &PendingMap) {
    let mut map = pending.lock().unwrap();
    map.closed = true; // latch first: a racing request now fails fast, never hangs.
    for (_, tx) in map.waiters.drain() {
        let _ = tx.send(Err(ProtocolError::ConnectionLost(
            "worker connection closed",
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::write_frame_blocking;
    use crate::messages::{ErrorCode, ErrorObject};
    use serde_json::json;
    use std::io::Cursor;

    /// A minimal in-process mock worker over a duplex: emits a hello, then reads
    /// requests and replies with `resp` frames in a caller-controlled order.
    struct MockServer {
        io: tokio::io::DuplexStream,
        rbuf: BytesMut,
    }

    impl MockServer {
        async fn write_frame(&mut self, frame: &Frame) {
            let bytes = encode_frame(&frame.to_json_vec().unwrap(), &[]).unwrap();
            self.io.write_all(&bytes).await.unwrap();
            self.io.flush().await.unwrap();
        }

        async fn read_req(&mut self) -> ReqFrame {
            loop {
                if let Some((raw, consumed)) = decode_frame(&self.rbuf).unwrap() {
                    self.rbuf.advance(consumed);
                    match Frame::from_json_slice(&raw.json).unwrap() {
                        Frame::Req(req) => return req,
                        other => panic!("expected req, got {}", frame_kind(&other)),
                    }
                }
                let n = self.io.read_buf(&mut self.rbuf).await.unwrap();
                if n == 0 {
                    panic!("mock: client closed while awaiting req");
                }
            }
        }
    }

    fn hello_frame() -> Frame {
        Frame::Hello(crate::messages::HelloFrame {
            v: PROTOCOL_VERSION,
            seq: 0,
            result: HelloResult {
                protocol_version: 1,
                worker_version: "mock-0.1".into(),
                occt: crate::messages::OcctInfo {
                    version: "mock".into(),
                    fingerprint: "0000000000000000".into(),
                },
                quantization_version: 1,
                solver_policy_version: 1,
                capabilities: vec![],
                limits: None,
            },
        })
    }

    fn ok_resp(id: u64) -> Frame {
        Frame::Resp(RespFrame {
            v: PROTOCOL_VERSION,
            id,
            ok: true,
            result: Some(json!({ "echoedId": id })),
            error: None,
            document_revision: 0,
            worker_epoch: 0,
            snapshot_id: 0,
            job_id: None,
            seq: id,
            bin: None,
        })
    }

    #[tokio::test]
    async fn hello_is_consumed_on_connect() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let mut server = MockServer {
            io: server_io,
            rbuf: BytesMut::new(),
        };
        server.write_frame(&hello_frame()).await;

        let (r, w) = tokio::io::split(client_io);
        let client = ProtocolClient::connect(r, w).await.unwrap();
        assert_eq!(client.hello().worker_version, "mock-0.1");
        client.close().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn hundred_concurrent_requests_correlate_out_of_order() {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let mut server = MockServer {
            io: server_io,
            rbuf: BytesMut::new(),
        };
        server.write_frame(&hello_frame()).await;

        // Server: collect all 100 request ids, then reply in REVERSE order to
        // prove correlation is by id, not arrival order.
        let server_task = tokio::spawn(async move {
            let mut ids = Vec::new();
            for _ in 0..100 {
                let req = server.read_req().await;
                ids.push(req.id);
            }
            for &id in ids.iter().rev() {
                server.write_frame(&ok_resp(id)).await;
            }
        });

        let (r, w) = tokio::io::split(client_io);
        let client = Arc::new(ProtocolClient::connect(r, w).await.unwrap());

        let mut handles = Vec::new();
        for _ in 0..100 {
            let c = client.clone();
            handles.push(tokio::spawn(
                async move { c.request("Echo", json!({})).await },
            ));
        }
        for h in handles {
            let resp = h.await.unwrap().unwrap();
            assert!(resp.ok);
            // The echoed id in the result must equal the resp's own id.
            assert_eq!(resp.result.unwrap()["echoedId"], resp.id);
        }
        server_task.await.unwrap();
        Arc::try_unwrap(client).ok().unwrap().close().await;
    }

    #[tokio::test]
    async fn error_resp_is_returned_ok_false() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let mut server = MockServer {
            io: server_io,
            rbuf: BytesMut::new(),
        };
        server.write_frame(&hello_frame()).await;

        let server_task = tokio::spawn(async move {
            let req = server.read_req().await;
            let resp = Frame::Resp(RespFrame {
                v: PROTOCOL_VERSION,
                id: req.id,
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
                seq: 1,
                bin: None,
            });
            server.write_frame(&resp).await;
        });

        let (r, w) = tokio::io::split(client_io);
        let client = ProtocolClient::connect(r, w).await.unwrap();
        let resp = client.request("Bogus", json!({})).await.unwrap();
        assert!(!resp.ok);
        assert_eq!(resp.error.unwrap().code, ErrorCode::ProtocolError);
        server_task.await.unwrap();
        client.close().await;
    }

    #[tokio::test]
    async fn pending_requests_fail_on_reader_eof() {
        let (client_io, server_io) = tokio::io::duplex(4096);
        let mut server = MockServer {
            io: server_io,
            rbuf: BytesMut::new(),
        };
        server.write_frame(&hello_frame()).await;

        let (r, w) = tokio::io::split(client_io);
        let client = Arc::new(ProtocolClient::connect(r, w).await.unwrap());

        // Fire several requests; the server never replies, then drops the io.
        let mut handles = Vec::new();
        for _ in 0..5 {
            let c = client.clone();
            handles.push(tokio::spawn(
                async move { c.request("Never", json!({})).await },
            ));
        }
        // Give the requests a moment to register, then close the server side.
        tokio::task::yield_now().await;
        drop(server); // closes the duplex -> reader sees EOF

        for h in handles {
            let result = h.await.unwrap();
            assert!(matches!(result, Err(ProtocolError::ConnectionLost(_))));
        }
    }

    #[tokio::test]
    async fn request_after_connection_closed_fails_fast_not_hang() {
        // Regression: once the reader tears down (EOF), a NEW request must fail
        // fast rather than register a oneshot nothing will ever fire (which hung
        // the worker manager's post-crash `discard_prepared`). Every request here
        // must resolve within the deadline — a timeout means the latch regressed.
        let (client_io, server_io) = tokio::io::duplex(4096);
        let mut server = MockServer {
            io: server_io,
            rbuf: BytesMut::new(),
        };
        server.write_frame(&hello_frame()).await;
        let (r, w) = tokio::io::split(client_io);
        let client = ProtocolClient::connect(r, w).await.unwrap();
        drop(server); // EOF → reader latches `closed`.

        // Whether the request races the teardown (fired by `fail_all`) or arrives
        // after the latch (rejected at registration), it must resolve ConnectionLost
        // within the deadline — a timeout means the closed-latch regressed.
        match tokio::time::timeout(
            std::time::Duration::from_secs(2),
            client.request("X", json!({})),
        )
        .await
        {
            Ok(Err(ProtocolError::ConnectionLost(_))) => {}
            Ok(other) => panic!("no server should respond: {other:?}"),
            Err(_) => panic!("request hung — closed-latch regression"),
        }
    }

    #[tokio::test]
    async fn connect_fails_on_bad_magic_first_frame() {
        let (client_io, mut server_io) = tokio::io::duplex(4096);
        // Write garbage (bad magic) instead of a hello.
        let mut garbage = Cursor::new(Vec::new());
        // A well-formed length header but wrong magic bytes.
        garbage.get_mut().extend_from_slice(b"XXXX");
        garbage.get_mut().extend_from_slice(&0u32.to_le_bytes());
        garbage.get_mut().extend_from_slice(&0u32.to_le_bytes());
        server_io.write_all(garbage.get_ref()).await.unwrap();
        server_io.flush().await.unwrap();

        let (r, w) = tokio::io::split(client_io);
        match ProtocolClient::connect(r, w).await {
            Err(ProtocolError::BadMagic { .. }) => {}
            other => panic!("expected BadMagic, got {other:?}"),
        }
    }

    #[test]
    fn write_frame_blocking_matches_codec_encoding() {
        // Sanity: the blocking writer the stub uses produces identical bytes.
        let frame = hello_frame();
        let json = frame.to_json_vec().unwrap();
        let mut sink = Vec::new();
        write_frame_blocking(&mut sink, &json, &[]).unwrap();
        assert_eq!(sink, encode_frame(&json, &[]).unwrap());
    }
}
