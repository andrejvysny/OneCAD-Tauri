//! [`WorkerManager`] — the C++ OCCT sidecar's lifecycle owner (R-WP11).
//!
//! Spawns the worker via `tokio::process` (NOT `tauri-plugin-shell` — the plan
//! mandates real `AsyncRead`/backpressure), reads the unsolicited `hello`
//! (SCHEMA §6), and **supervises** it:
//!
//! * **liveness** — ping (`GetWorkerHead`) every 5 s; 2 missed → `SIGKILL`
//!   (SCHEMA §8 hung-worker rule);
//! * **restart** — on exit/crash/kill, bump the [`WorkerEpoch`], invoke the
//!   restart hook (**mark dirty + replay** via the regen path), and reconnect
//!   with backoff `0.5 / 1 / 2 s ×3` → [`WorkerState::Failed`];
//! * **poison / circuit breaker** — a plan that crashes the worker on the same
//!   `(historyPrefixHash, op, fingerprint)` key `poison_threshold` times stops
//!   being retried and surfaces [`WorkerState::Failed`] (plan crash circuit
//!   breaker), so a repeatedly-crashing plan converges instead of flapping.
//!
//! It implements [`GeometryEngine`] + [`MeshProvider`] by translating core types
//! to the OCW1 wire via [`wire`](super::wire) over the current
//! [`ProtocolClient`], swapping the client on every restart.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc};

use onecad_protocol::client::{ProtocolClient, WorkerEvent};
use onecad_protocol::messages::{HelloResult, Lane};
use onecad_protocol::ProtocolError;

use onecad_core::ids::{BodyId, DocumentId, DocumentRevision, JobId, SnapshotId, WorkerEpoch};
use onecad_core::regen::{
    AcceptResult, AcquireRequest, BodySelector, CheckpointArtifacts, EngineError, Fencing,
    GeometryEngine, Lod, OpenSessionRequest, PlanEvent, PlanRequest, RefResolution, ResolveRequest,
    RestoreRequest, RestoreResult, SessionMode, TessellateRequest, TessellateResult,
    WorkerElementEvidence, WorkerHead,
};

use super::{wire, MeshProvider};

/// Lifecycle state surfaced to the app (drives the worker-status banner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// Spawning / connecting the first time.
    Starting,
    /// Connected, handshook, session open — geometry calls flow.
    Ready,
    /// Died; reconnecting under backoff.
    Restarting,
    /// Backoff exhausted or a poison circuit tripped — no worker.
    Failed,
}

/// A lifecycle transition broadcast to the app (worker-status event; R-WP11).
#[derive(Debug, Clone)]
pub enum WorkerLifecycle {
    /// Connected + handshook; carries the new epoch and OCCT fingerprint.
    Ready { epoch: u64, fingerprint: String },
    /// A restart began (crash / exit / ping timeout).
    Restarting { epoch: u64, reason: String },
    /// Terminal: backoff exhausted (cannot start).
    Failed { reason: String },
    /// A plan's crash circuit tripped (poison) — that plan is abandoned.
    CircuitOpen { key: String },
}

/// The restart hook: called with the freshly-bumped epoch after every restart, so
/// the app can **mark the document dirty and enqueue a replay** (SCHEMA §8 crash →
/// restart + replay). Default is a no-op (tests / headless).
pub type RestartHook = Arc<dyn Fn(WorkerEpoch) + Send + Sync>;

/// Tunable supervision policy (production defaults; tests inject fast values).
#[derive(Debug, Clone)]
pub struct SupervisorConfig {
    pub binary: PathBuf,
    /// Extra environment for the spawn (chaos-hook drills set these).
    pub envs: Vec<(String, String)>,
    pub ping_interval: Duration,
    pub ping_timeout: Duration,
    pub max_missed_pings: u32,
    /// Backoff delays between failed *start* attempts; length = strikes → Failed.
    pub backoff: Vec<Duration>,
    /// Consecutive same-key crashes before the plan's circuit opens (poison).
    pub poison_threshold: u32,
    /// Auto-`OpenSession` after connect (production true; smoke test opens itself).
    pub auto_open_session: bool,
}

impl SupervisorConfig {
    /// Production supervision policy (SCHEMA §8: ping 5 s ×2, backoff 0.5/1/2 s).
    #[must_use]
    pub fn production(binary: PathBuf) -> Self {
        Self {
            binary,
            envs: Vec::new(),
            ping_interval: Duration::from_secs(5),
            ping_timeout: Duration::from_secs(5),
            max_missed_pings: 2,
            backoff: vec![
                Duration::from_millis(500),
                Duration::from_secs(1),
                Duration::from_secs(2),
            ],
            poison_threshold: 3,
            auto_open_session: true,
        }
    }
}

/// Shared supervisor + connection state (behind the `WorkerManager`).
struct Shared {
    config: SupervisorConfig,
    /// The current connection; `None` while (re)starting or Failed.
    conn: RwLock<Option<Arc<ProtocolClient>>>,
    epoch: AtomicU64,
    state: Mutex<WorkerState>,
    hello: Mutex<Option<HelloResult>>,
    /// `jobId → in-flight request id`, for cancel propagation (SCHEMA §3.5).
    inflight: Mutex<HashMap<JobId, u64>>,
    poison: Mutex<Poison>,
    lifecycle: broadcast::Sender<WorkerLifecycle>,
    restart_hook: RwLock<Option<RestartHook>>,
}

/// Crash-circuit state (poison detection).
#[derive(Default)]
struct Poison {
    counts: HashMap<String, u32>,
    open: HashSet<String>,
}

impl Shared {
    fn set_state(&self, s: WorkerState) {
        *self.state.lock().unwrap() = s;
    }

    fn client(&self) -> Option<Arc<ProtocolClient>> {
        self.conn.read().unwrap().clone()
    }

    fn fingerprint(&self) -> String {
        self.hello
            .lock()
            .unwrap()
            .as_ref()
            .map(|h| h.occt.fingerprint.clone())
            .unwrap_or_default()
    }

    fn emit(&self, ev: WorkerLifecycle) {
        let _ = self.lifecycle.send(ev);
    }

    /// Records a same-key crash; returns `true` if the circuit just opened.
    fn record_crash(&self, key: &str) -> bool {
        let mut p = self.poison.lock().unwrap();
        if p.open.contains(key) {
            return false;
        }
        let c = p.counts.entry(key.to_string()).or_insert(0);
        *c += 1;
        if *c >= self.config.poison_threshold {
            p.open.insert(key.to_string());
            true
        } else {
            false
        }
    }

    fn record_success(&self, key: &str) {
        let mut p = self.poison.lock().unwrap();
        p.counts.remove(key);
        p.open.remove(key);
    }

    fn is_circuit_open(&self, key: &str) -> bool {
        self.poison.lock().unwrap().open.contains(key)
    }

    fn any_circuit_open(&self) -> bool {
        !self.poison.lock().unwrap().open.is_empty()
    }
}

/// The worker sidecar lifecycle owner (see the module docs).
#[derive(Clone)]
pub struct WorkerManager {
    shared: Arc<Shared>,
}

impl WorkerManager {
    /// Spawns the supervisor for `config` (returns immediately; the connection is
    /// established asynchronously). Call [`wait_ready`](Self::wait_ready) to await
    /// the first successful handshake.
    #[must_use]
    pub fn spawn(config: SupervisorConfig) -> Self {
        let (lifecycle, _) = broadcast::channel(64);
        let shared = Arc::new(Shared {
            config,
            conn: RwLock::new(None),
            epoch: AtomicU64::new(1),
            state: Mutex::new(WorkerState::Starting),
            hello: Mutex::new(None),
            inflight: Mutex::new(HashMap::new()),
            poison: Mutex::new(Poison::default()),
            lifecycle,
            restart_hook: RwLock::new(None),
        });
        tokio::spawn(supervise(shared.clone()));
        Self { shared }
    }

    /// The current lifecycle state.
    #[must_use]
    pub fn state(&self) -> WorkerState {
        *self.shared.state.lock().unwrap()
    }

    /// The current worker epoch (bumped on every restart).
    #[must_use]
    pub fn epoch(&self) -> WorkerEpoch {
        WorkerEpoch(self.shared.epoch.load(Ordering::SeqCst))
    }

    /// The worker's handshake result once connected (fingerprint policy surface).
    #[must_use]
    pub fn hello(&self) -> Option<HelloResult> {
        self.shared.hello.lock().unwrap().clone()
    }

    /// Subscribe to lifecycle transitions (for the worker-status banner).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<WorkerLifecycle> {
        self.shared.lifecycle.subscribe()
    }

    /// Sets the restart hook (mark-dirty + replay). Replaces any prior hook.
    pub fn set_restart_hook(&self, hook: RestartHook) {
        *self.shared.restart_hook.write().unwrap() = Some(hook);
    }

    /// Awaits [`WorkerState::Ready`] up to `timeout`; `false` on `Failed`/timeout.
    pub async fn wait_ready(&self, timeout: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match self.state() {
                WorkerState::Ready => return true,
                WorkerState::Failed => return false,
                _ => {}
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// `ExportStep` verb passthrough (SCHEMA §7.8) — surfaced here so an app
    /// command can drive STEP export later. Returns bytes written.
    ///
    /// # Errors
    /// [`EngineError`] on a disconnected worker or a worker-side failure.
    pub async fn export_step(
        &self,
        path: &str,
        bodies: &[BodyId],
        schema: &str,
    ) -> Result<u64, EngineError> {
        let client = self.client_or_err()?;
        let args = wire::export_step_args(path, bodies, schema);
        let resp = client
            .request("ExportStep", args)
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| r.get("bytes").and_then(Value::as_u64).unwrap_or(0))
    }

    /// Graceful `Shutdown` (SCHEMA §7.1): ask the worker to flush + exit 0.
    /// Best-effort — a disconnected worker is already gone.
    pub async fn shutdown(&self) {
        if let Some(client) = self.shared.client() {
            let _ = client.request("Shutdown", json!({})).await;
        }
    }

    fn client_or_err(&self) -> Result<Arc<ProtocolClient>, EngineError> {
        self.shared.client().ok_or_else(not_connected)
    }
}

impl std::fmt::Debug for WorkerManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerManager")
            .field("state", &self.state())
            .field("epoch", &self.epoch().0)
            .finish_non_exhaustive()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Supervisor
// ─────────────────────────────────────────────────────────────────────────────

/// Why the running worker stopped.
enum Death {
    /// Exited on its own (crash / clean).
    Exited,
    /// Killed after `max_missed_pings` (hung).
    PingTimeout,
}

/// The supervision loop: (re)connect, run until death, restart with backoff /
/// poison-aware stop.
async fn supervise(shared: Arc<Shared>) {
    let mut consecutive_start_failures = 0u32;
    loop {
        match spawn_and_connect(&shared).await {
            Ok((child, client)) => {
                consecutive_start_failures = 0;
                *shared.conn.write().unwrap() = Some(client.clone());
                *shared.hello.lock().unwrap() = Some(client.hello().clone());
                if shared.config.auto_open_session {
                    let _ = auto_open_session(&shared, &client).await;
                }
                shared.set_state(WorkerState::Ready);
                shared.emit(WorkerLifecycle::Ready {
                    epoch: shared.epoch.load(Ordering::SeqCst),
                    fingerprint: client.hello().occt.fingerprint.clone(),
                });

                let death = run_until_death(&shared, &client, child).await;

                // Torn down: drop the connection and bump the epoch (fencing).
                *shared.conn.write().unwrap() = None;
                let new_epoch = shared.epoch.fetch_add(1, Ordering::SeqCst) + 1;
                let reason = match death {
                    Death::Exited => "worker exited/crashed",
                    Death::PingTimeout => "worker hung (ping timeout) → SIGKILL",
                };
                shared.set_state(WorkerState::Restarting);
                shared.emit(WorkerLifecycle::Restarting {
                    epoch: new_epoch,
                    reason: reason.into(),
                });
                fire_restart_hook(&shared, WorkerEpoch(new_epoch));

                // Poison: a tripped plan means we stop chasing a crash loop.
                if shared.any_circuit_open() {
                    shared.set_state(WorkerState::Failed);
                    shared.emit(WorkerLifecycle::Failed {
                        reason: "crash circuit breaker open (poison)".into(),
                    });
                    return;
                }
                // Restart cadence: reuse the first backoff delay for runtime deaths.
                tokio::time::sleep(first_delay(&shared)).await;
            }
            Err(reason) => {
                consecutive_start_failures += 1;
                shared.set_state(WorkerState::Restarting);
                shared.emit(WorkerLifecycle::Restarting {
                    epoch: shared.epoch.load(Ordering::SeqCst),
                    reason: format!("start failed: {reason}"),
                });
                let strikes = shared.config.backoff.len() as u32;
                if consecutive_start_failures > strikes {
                    shared.set_state(WorkerState::Failed);
                    shared.emit(WorkerLifecycle::Failed {
                        reason: format!(
                            "backoff exhausted after {consecutive_start_failures} tries: {reason}"
                        ),
                    });
                    return;
                }
                let idx =
                    (consecutive_start_failures as usize - 1).min(shared.config.backoff.len() - 1);
                tokio::time::sleep(shared.config.backoff[idx]).await;
            }
        }
    }
}

fn first_delay(shared: &Shared) -> Duration {
    shared
        .config
        .backoff
        .first()
        .copied()
        .unwrap_or(Duration::from_millis(500))
}

fn fire_restart_hook(shared: &Shared, epoch: WorkerEpoch) {
    let hook = shared.restart_hook.read().unwrap().clone();
    if let Some(hook) = hook {
        hook(epoch);
    }
}

/// Spawns the child and completes the OCW1 handshake. `Err(reason)` on spawn or
/// handshake failure (bad magic, EOF before hello) — a restart trigger.
async fn spawn_and_connect(
    shared: &Shared,
) -> Result<(tokio::process::Child, Arc<ProtocolClient>), String> {
    let mut cmd = tokio::process::Command::new(&shared.config.binary);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    for (k, v) in &shared.config.envs {
        cmd.env(k, v);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {:?}: {e}", shared.config.binary))?;
    let stdout = child.stdout.take().ok_or("child stdout missing")?;
    let stdin = child.stdin.take().ok_or("child stdin missing")?;
    let client = ProtocolClient::connect(stdout, stdin)
        .await
        .map_err(|e| format!("handshake: {e}"))?;
    Ok((child, Arc::new(client)))
}

async fn auto_open_session(shared: &Shared, client: &ProtocolClient) -> Result<(), ProtocolError> {
    let epoch = shared.epoch.load(Ordering::SeqCst);
    let args = wire::open_session_args(&OpenSessionRequest {
        document_id: DocumentId::from_uuid(uuid::Uuid::nil()),
        document_revision: DocumentRevision(0),
        worker_epoch: WorkerEpoch(epoch),
        mode: SessionMode::Determinism,
    });
    let _ = client.request("OpenSession", args).await?;
    Ok(())
}

/// Runs while the worker lives: ping on an interval, watch for exit; `SIGKILL` on
/// `max_missed_pings` (SCHEMA §8 hung worker).
async fn run_until_death(
    shared: &Shared,
    client: &ProtocolClient,
    mut child: tokio::process::Child,
) -> Death {
    let mut missed = 0u32;
    let mut ticker = tokio::time::interval(shared.config.ping_interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ticker.tick().await; // consume the immediate first tick.
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let ping = client.request_timeout("GetWorkerHead", json!({}), shared.config.ping_timeout);
                match ping.await {
                    Ok(resp) if resp.ok => missed = 0,
                    _ => {
                        missed += 1;
                        if missed >= shared.config.max_missed_pings {
                            let _ = child.start_kill();
                            let _ = child.wait().await;
                            return Death::PingTimeout;
                        }
                    }
                }
            }
            _ = child.wait() => return Death::Exited,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// GeometryEngine (wire translation)
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl GeometryEngine for WorkerManager {
    async fn execute_plan(&self, request: PlanRequest) -> mpsc::Receiver<PlanEvent> {
        let (tx, rx) = mpsc::channel(256);
        tokio::spawn(stream_plan(self.shared.clone(), request, tx));
        rx
    }

    async fn open_session(&self, req: OpenSessionRequest) -> Result<WorkerHead, EngineError> {
        let client = self.client_or_err()?;
        let epoch = req.worker_epoch;
        let resp = client
            .request("OpenSession", wire::open_session_args(&req))
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_open_session(&r, epoch))
    }

    async fn close_session(
        &self,
        document_id: DocumentId,
        worker_epoch: WorkerEpoch,
    ) -> Result<(), EngineError> {
        let client = self.client_or_err()?;
        let args = json!({ "documentId": document_id.to_string(), "workerEpoch": worker_epoch.0 });
        let resp = client
            .request("CloseSession", args)
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|_| ())
    }

    async fn reset(
        &self,
        document_id: DocumentId,
        worker_epoch: WorkerEpoch,
    ) -> Result<WorkerEpoch, EngineError> {
        let client = self.client_or_err()?;
        let args = json!({ "documentId": document_id.to_string(), "workerEpoch": worker_epoch.0 });
        let resp = client
            .request("ResetSession", args)
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| {
            WorkerEpoch(
                r.get("workerEpoch")
                    .and_then(Value::as_u64)
                    .unwrap_or(worker_epoch.0 + 1),
            )
        })
    }

    async fn accept_prepared(
        &self,
        job_id: JobId,
        fencing: Fencing,
    ) -> Result<AcceptResult, EngineError> {
        let client = self.client_or_err()?;
        let args = json!({
            "jobId": wire::job_id_wire(job_id),
            "documentRevision": fencing.document_revision.0,
            "workerEpoch": fencing.worker_epoch.0,
        });
        let resp = client
            .request("AcceptPrepared", args)
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_accept(&r))
    }

    async fn discard_prepared(&self, job_id: JobId) -> Result<(), EngineError> {
        let client = self.client_or_err()?;
        let args = json!({ "jobId": wire::job_id_wire(job_id) });
        let resp = client
            .request("DiscardPrepared", args)
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|_| ())
    }

    async fn get_worker_head(&self) -> Result<WorkerHead, EngineError> {
        let client = self.client_or_err()?;
        let resp = client
            .request("GetWorkerHead", json!({}))
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_worker_head(&r))
    }

    async fn tessellate(&self, req: TessellateRequest) -> Result<TessellateResult, EngineError> {
        // The wire tessellate result carries mesh *handles*; bytes are pulled via
        // `MeshProvider::fetch_mesh`. The handle set is not folded by the executor,
        // so a single-body request is enough for the current mesh-cache path.
        let _ = req;
        Ok(TessellateResult { meshes: vec![] })
    }

    async fn save_checkpoint(
        &self,
        _step_index: usize,
    ) -> Result<CheckpointArtifacts, EngineError> {
        Err(unsupported("SaveCheckpoint not wired in V1"))
    }

    async fn restore_checkpoint(&self, _req: RestoreRequest) -> Result<RestoreResult, EngineError> {
        Err(EngineError::Protocol {
            message: "RestoreCheckpoint not wired in V1 (plans replay from 0)".into(),
        })
    }

    async fn acquire_element_ids(
        &self,
        _req: AcquireRequest,
    ) -> Result<Vec<WorkerElementEvidence>, EngineError> {
        Err(unsupported("AcquireElementIds not wired in V1"))
    }

    async fn resolve_refs(&self, _req: ResolveRequest) -> Result<Vec<RefResolution>, EngineError> {
        Ok(vec![])
    }

    async fn cancel(&self, job_id: JobId) -> Result<(), EngineError> {
        let id = self.shared.inflight.lock().unwrap().get(&job_id).copied();
        if let (Some(id), Some(client)) = (id, self.shared.client()) {
            let _ = client.cancel(id).await;
        }
        Ok(())
    }

    async fn ping(&self) -> Result<(), EngineError> {
        self.get_worker_head().await.map(|_| ())
    }
}

/// Drives one `ExecutePlan`: send it, forward each `planStep` `event` as
/// [`PlanEvent::Step`], then the terminal `PlanPrepared`/error/crash. Records
/// crashes against the poison key and fast-fails an open circuit (SCHEMA §8).
async fn stream_plan(shared: Arc<Shared>, request: PlanRequest, tx: mpsc::Sender<PlanEvent>) {
    let job = request.job_id;
    let key = poison_key(&request, &shared.fingerprint());

    if shared.is_circuit_open(&key) {
        let _ = tx
            .send(PlanEvent::Failed(EngineError::Crashed {
                message: "crash circuit breaker open (poison) — plan abandoned".into(),
            }))
            .await;
        return;
    }
    let Some(client) = shared.client() else {
        let _ = tx
            .send(PlanEvent::Failed(EngineError::Crashed {
                message: "worker not connected".into(),
            }))
            .await;
        return;
    };

    let mut events = client.subscribe();
    let inflight = match client
        .start_request(
            "ExecutePlan",
            wire::execute_plan_args(&request),
            Lane::Control,
        )
        .await
    {
        Ok(i) => i,
        Err(e) => {
            let _ = tx.send(PlanEvent::Failed(protocol_err(e))).await;
            return;
        }
    };
    let id = inflight.id;
    shared.inflight.lock().unwrap().insert(job, id);
    let mut resp_fut = Box::pin(inflight.response());

    let terminal = loop {
        tokio::select! {
            biased;
            ev = events.recv() => match ev {
                Ok(WorkerEvent::Event(e)) if e.id == id && e.event == "planStep" => {
                    let step = e.step_index.map_or(0, |s| s as usize);
                    match wire::parse_plan_step(&e.payload, step) {
                        Ok(s) => {
                            if tx.send(PlanEvent::Step(s)).await.is_err() {
                                shared.inflight.lock().unwrap().remove(&job);
                                return;
                            }
                        }
                        Err(msg) => break StreamEnd::Local(EngineError::Protocol { message: msg }),
                    }
                }
                Ok(_) => {}
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    break StreamEnd::Local(EngineError::Protocol {
                        message: "planStep event stream lagged (dropped frame)".into(),
                    });
                }
                Err(broadcast::error::RecvError::Closed) => {} // resp future resolves ConnectionLost.
            },
            resp = &mut resp_fut => break StreamEnd::Resp(resp),
        }
    };

    shared.inflight.lock().unwrap().remove(&job);
    let event = finish_plan(&shared, &key, job, terminal);
    let _ = tx.send(event).await;
}

/// The stream terminal: a wire `resp` (or transport error) vs a local parse error.
enum StreamEnd {
    Resp(Result<onecad_protocol::messages::RespFrame, ProtocolError>),
    Local(EngineError),
}

/// Maps a stream terminal to the final [`PlanEvent`], updating poison state.
fn finish_plan(shared: &Shared, key: &str, job: JobId, terminal: StreamEnd) -> PlanEvent {
    match terminal {
        StreamEnd::Local(err) => PlanEvent::Failed(err),
        StreamEnd::Resp(Ok(resp)) if resp.ok => {
            let result = resp.result.unwrap_or(Value::Null);
            match wire::parse_plan_prepared(job, &result) {
                Ok(p) => {
                    shared.record_success(key);
                    PlanEvent::Prepared(p)
                }
                Err(msg) => PlanEvent::Failed(EngineError::Protocol { message: msg }),
            }
        }
        StreamEnd::Resp(Ok(resp)) => {
            // Well-framed worker error (recoverable op failure / protocol) — NOT a
            // crash, so the poison counter is untouched.
            let err = resp.error.as_ref().map_or_else(
                || EngineError::Protocol {
                    message: "resp ok=false without error".into(),
                },
                wire::map_error,
            );
            PlanEvent::Failed(err)
        }
        StreamEnd::Resp(Err(proto)) => {
            // Transport died mid-plan ⇒ crash. Feed the circuit breaker.
            if shared.record_crash(key) {
                shared.set_state(WorkerState::Failed);
                shared.emit(WorkerLifecycle::CircuitOpen {
                    key: key.to_string(),
                });
            }
            PlanEvent::Failed(EngineError::Crashed {
                message: format!("worker crashed mid-plan: {proto}"),
            })
        }
    }
}

/// The poison key: `historyPrefixHash | last-op | fingerprint` (SCHEMA §8 crash
/// circuit breaker keys on the same `(history hash, op, fingerprint)`).
fn poison_key(req: &PlanRequest, fingerprint: &str) -> String {
    let last_op = req
        .ops
        .last()
        .map(|o| o.record_id.to_string())
        .unwrap_or_default();
    format!(
        "{}|{}|{}",
        req.expected_base_hash.as_str(),
        last_op,
        fingerprint
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// MeshProvider (Tessellate + MESH1 bulk assembly, SCHEMA §5.2 / §7.6)
// ─────────────────────────────────────────────────────────────────────────────

/// A bulk MESH1 stream being reassembled from chunk frames.
#[derive(Default)]
struct StreamAcc {
    buf: Vec<u8>,
    total_bytes: Option<u64>,
    sha256: Option<String>,
}

impl StreamAcc {
    fn apply(&mut self, c: &onecad_protocol::messages::ChunkFrame, bin: &[u8]) {
        match c.kind {
            onecad_protocol::messages::ChunkKind::Manifest => {
                self.total_bytes = c.total_bytes;
                self.sha256 = c.sha256.clone();
            }
            onecad_protocol::messages::ChunkKind::Data => {
                let off = c.byte_offset.unwrap_or(0) as usize;
                let end = off + bin.len();
                if self.buf.len() < end {
                    self.buf.resize(end, 0);
                }
                self.buf[off..end].copy_from_slice(bin);
            }
        }
    }
}

#[async_trait]
impl MeshProvider for WorkerManager {
    async fn fetch_mesh(
        &self,
        body: BodyId,
        lod: Lod,
        _snapshot: SnapshotId,
    ) -> Result<Vec<u8>, EngineError> {
        let client = self.client_or_err()?;
        let req = TessellateRequest {
            bodies: BodySelector::Ids(vec![body]),
            lod,
            include_edges: true,
        };
        let mut events = client.subscribe();
        let inflight = client
            .start_request("Tessellate", wire::tessellate_args(&req), Lane::Control)
            .await
            .map_err(protocol_err)?;
        let id = inflight.id;
        let mut resp_fut = Box::pin(inflight.response_with_bin());
        let mut streams: HashMap<u64, StreamAcc> = HashMap::new();

        let terminal = loop {
            tokio::select! {
                biased;
                ev = events.recv() => match ev {
                    Ok(WorkerEvent::Chunk(c, bin)) if c.id == id => {
                        let is_data = matches!(c.kind, onecad_protocol::messages::ChunkKind::Data);
                        streams.entry(c.stream_id).or_default().apply(&c, &bin);
                        if is_data && !bin.is_empty() {
                            let _ = client.grant_credit(bin.len() as u64).await; // replenish (SCHEMA §5.3)
                        }
                    }
                    Ok(_) => {}
                    Err(broadcast::error::RecvError::Lagged(_)) =>
                        return Err(EngineError::Protocol { message: "mesh chunk stream lagged".into() }),
                    Err(broadcast::error::RecvError::Closed) => {}
                },
                resp = &mut resp_fut => break resp,
            }
        };

        let (resp, tail) = terminal.map_err(protocol_err)?;
        // Drain any chunk frames buffered ahead of the terminal.
        while let Ok(WorkerEvent::Chunk(c, bin)) = events.try_recv() {
            if c.id == id {
                streams.entry(c.stream_id).or_default().apply(&c, &bin);
            }
        }
        let result = ok_result(resp.clone())?;
        assemble_mesh(&result, &resp, &tail, &mut streams)
    }
}

/// Extracts + verifies one MESH1 blob from a `Tessellate` result: inline (`bin`
/// section in the resp tail) or a bulk stream (`streamId`). Verifies size +
/// SHA-256 and validates the MESH1 header (Invariant 5 forward-verbatim).
fn assemble_mesh(
    result: &Value,
    resp: &onecad_protocol::messages::RespFrame,
    tail: &[u8],
    streams: &mut HashMap<u64, StreamAcc>,
) -> Result<Vec<u8>, EngineError> {
    let mesh = result
        .get("meshes")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| protocol("Tessellate result carried no meshes"))?;

    let blob = if let Some(name) = mesh.get("bin").and_then(Value::as_str) {
        let section = resp
            .bin
            .as_ref()
            .and_then(|secs| secs.iter().find(|s| s.name == name))
            .ok_or_else(|| protocol("inline mesh bin section missing"))?;
        let start = section.off as usize;
        let end = start + section.len as usize;
        tail.get(start..end)
            .ok_or_else(|| protocol("inline mesh bin section out of range"))?
            .to_vec()
    } else if let Some(stream_id) = mesh.get("streamId").and_then(Value::as_u64) {
        let acc = streams
            .remove(&stream_id)
            .ok_or_else(|| protocol("mesh stream frames never arrived"))?;
        acc.buf
    } else {
        return Err(protocol("mesh handle has neither inline bin nor streamId"));
    };

    verify_mesh(
        &blob,
        mesh.get("totalBytes").and_then(Value::as_u64),
        mesh.get("sha256").and_then(Value::as_str),
    )?;
    Ok(blob)
}

fn verify_mesh(blob: &[u8], total: Option<u64>, sha: Option<&str>) -> Result<(), EngineError> {
    if let Some(t) = total {
        if blob.len() as u64 != t {
            return Err(protocol("MESH1 assembled length != totalBytes"));
        }
    }
    if let Some(want) = sha {
        if sha256_hex(blob) != want {
            return Err(protocol("MESH1 SHA-256 mismatch (corrupt stream)"));
        }
    }
    onecad_protocol::mesh::validate_mesh_blob(blob).map_err(|e| EngineError::Protocol {
        message: format!("MESH1 header invalid: {e}"),
    })?;
    Ok(())
}

fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(data);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Small helpers
// ─────────────────────────────────────────────────────────────────────────────

fn not_connected() -> EngineError {
    EngineError::Crashed {
        message: "worker not connected (restarting or failed)".into(),
    }
}

fn unsupported(msg: &str) -> EngineError {
    EngineError::OpFailed {
        code: onecad_core::regen::OpFailureCode::Unsupported,
        recoverable: true,
        message: msg.into(),
    }
}

fn protocol(msg: &str) -> EngineError {
    EngineError::Protocol {
        message: msg.into(),
    }
}

/// Maps a transport [`ProtocolError`] to the engine taxonomy: a lost connection is
/// a crash (restart + replay); anything else is a protocol violation (SCHEMA §8).
fn protocol_err(e: ProtocolError) -> EngineError {
    match e {
        ProtocolError::ConnectionLost(_) | ProtocolError::Timeout => EngineError::Crashed {
            message: format!("worker connection lost: {e}"),
        },
        other => EngineError::Protocol {
            message: other.to_string(),
        },
    }
}

/// Unwraps a success `resp` into its `result` object, or maps its error terminal.
fn ok_result(resp: onecad_protocol::messages::RespFrame) -> Result<Value, EngineError> {
    if resp.ok {
        Ok(resp.result.unwrap_or(Value::Null))
    } else {
        Err(resp
            .error
            .as_ref()
            .map_or_else(|| protocol("resp ok=false without error"), wire::map_error))
    }
}
