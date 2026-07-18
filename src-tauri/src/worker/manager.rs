//! [`WorkerManager`] — the C++ OCCT sidecar's lifecycle owner (R-WP11).
//!
//! Spawns the worker via `tokio::process` (NOT `tauri-plugin-shell` — the plan
//! mandates real `AsyncRead`/backpressure), reads the unsolicited `hello`
//! (SCHEMA §6), and **supervises** it:
//!
//! * **liveness** — ping (`GetWorkerHead`) every 5 s; 2 missed → `SIGKILL`
//!   (SCHEMA §8 hung-worker rule);
//! * **restart** — on exit/crash/kill, bump the [`WorkerEpoch`], invoke the
//!   restart hook (**mark dirty + replay** via the regen path), and reconnect with
//!   backoff `0.5 / 1 / 2 s ×3`. A failed *start* OR a **rapid death** (a worker
//!   that dies within `healthy_threshold` of becoming Ready — a connect-then-die
//!   flap) both count toward one strike budget (`max_rapid_deaths`); exhausting it
//!   ⇒ [`WorkerState::Failed`] (F2). A death after a healthy period resets it;
//! * **poison / circuit breaker** — a plan that crashes the worker on the same
//!   `(historyPrefixHash, crashing-op, fingerprint)` key `poison_threshold` times
//!   opens that plan-key's circuit: it then **fails fast without dispatch** (so it
//!   stops killing the worker) but the worker stays alive/restarting so **other
//!   plans still run** (F3). The circuit never sets [`WorkerState::Failed`] — only
//!   the F2 flap budget does.
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

use onecad_core::ids::{
    BodyId, DocumentId, DocumentRevision, EntityId, JobId, SnapshotId, WorkerEpoch,
};
use onecad_core::regen::{
    AcceptResult, AcquireRequest, BodySelector, CheckpointArtifacts, EngineError, Fencing,
    GeometryEngine, Lod, OpenSessionRequest, PlanEvent, PlanRequest, RefResolution, ResolveRequest,
    RestoreRequest, RestoreResult, SessionMode, TessellateRequest, TessellateResult,
    WorkerElementEvidence, WorkerHead,
};
use onecad_core::sketch::Sketch;

use crate::dto::{BeginGestureDto, DragSolveDto, SketchRegionDto, SketchUpsertDto};

use super::{wire, MeshProvider, SolverEngine};

/// Lifecycle state surfaced to the app (drives the worker-status banner).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerState {
    /// Spawning / connecting the first time.
    Starting,
    /// Connected, handshook, session open — geometry calls flow.
    Ready,
    /// Died; reconnecting under backoff.
    Restarting,
    /// The flap budget was exhausted (too many failed starts / rapid deaths) — no
    /// worker. A poison circuit does NOT reach this state (F3).
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
    /// A plan-key's crash circuit tripped (poison): that plan now fails fast without
    /// dispatch, but the worker stays alive so other plans still run (F3).
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
    /// Backoff delays between failed *start* attempts.
    pub backoff: Vec<Duration>,
    /// Strike budget for the unified flap counter (failed *starts* + **rapid
    /// deaths**); exceeding it ⇒ [`WorkerState::Failed`] (F2). Defaults to
    /// `backoff.len()` so the start-failure cadence is unchanged.
    pub max_rapid_deaths: u32,
    /// A worker that dies within this window of reaching [`WorkerState::Ready`] is a
    /// **flap** (counts toward `max_rapid_deaths`); a death after living at least
    /// this long resets the flap counter (F2 — a connect-then-die loop can no longer
    /// restart forever).
    pub healthy_threshold: Duration,
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
            max_rapid_deaths: 3,
            healthy_threshold: Duration::from_secs(5),
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

    /// Clears the crash count + open flag for every one of a plan's candidate op
    /// keys (a successful prepare heals the plan — F3).
    fn record_success(&self, keys: &[String]) {
        let mut p = self.poison.lock().unwrap();
        for key in keys {
            p.counts.remove(key);
            p.open.remove(key);
        }
    }

    /// Whether ANY of a plan's candidate op keys has an open circuit — the fail-fast
    /// gate (the crashing op is one of them, so a poisoned plan is caught up front).
    fn plan_circuit_open(&self, keys: &[String]) -> bool {
        let p = self.poison.lock().unwrap();
        keys.iter().any(|k| p.open.contains(k))
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

/// The supervision loop: (re)connect, run until death, restart with backoff. A
/// unified `flap_strikes` counter spans failed *starts* and **rapid deaths**
/// (connect-then-die within `healthy_threshold`); exhausting `max_rapid_deaths` ⇒
/// Failed (F2). A poisoned plan-key never stops the supervisor (F3) — the worker
/// keeps restarting so other plans run.
async fn supervise(shared: Arc<Shared>) {
    let mut flap_strikes = 0u32;
    // F6: the restart hook (mark-dirty + enqueue replay) fires on the post-restart
    // READY transition, not at death — so the replay it enqueues dispatches over a
    // live connection instead of racing `conn == None`. `Some(epoch)` between a
    // death and the next Ready.
    let mut pending_restart_epoch: Option<u64> = None;
    loop {
        match spawn_and_connect(&shared).await {
            Ok((child, client)) => {
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
                // Fire the deferred restart hook now that the worker is Ready + the
                // connection is live (F6).
                if let Some(epoch) = pending_restart_epoch.take() {
                    fire_restart_hook(&shared, WorkerEpoch(epoch));
                }
                let ready_at = tokio::time::Instant::now();

                let death = run_until_death(&shared, &client, child).await;
                let alive = ready_at.elapsed();

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
                // Defer the restart hook to the NEXT Ready (F6): a replay enqueued now
                // would race `conn == None`. If the flap budget is exhausted below we
                // never reach that Ready — correct, there is no worker to replay on.
                pending_restart_epoch = Some(new_epoch);

                // F2: a death within `healthy_threshold` of becoming Ready is a flap
                // (counts toward the strike budget); a longer-lived worker resets it.
                if alive < shared.config.healthy_threshold {
                    flap_strikes += 1;
                    if flap_strikes > shared.config.max_rapid_deaths {
                        shared.set_state(WorkerState::Failed);
                        shared.emit(WorkerLifecycle::Failed {
                            reason: format!(
                                "rapid-death budget exhausted after {flap_strikes} flaps ({reason})"
                            ),
                        });
                        return;
                    }
                    // Back off like a start failure so a connect-die loop can't spin.
                    let idx = (flap_strikes as usize - 1)
                        .min(shared.config.backoff.len().saturating_sub(1));
                    let delay = shared
                        .config
                        .backoff
                        .get(idx)
                        .copied()
                        .unwrap_or_else(|| first_delay(&shared));
                    tokio::time::sleep(delay).await;
                } else {
                    flap_strikes = 0;
                    // Restart cadence: reuse the first backoff delay for runtime deaths.
                    tokio::time::sleep(first_delay(&shared)).await;
                }
            }
            Err(reason) => {
                flap_strikes += 1;
                shared.set_state(WorkerState::Restarting);
                shared.emit(WorkerLifecycle::Restarting {
                    epoch: shared.epoch.load(Ordering::SeqCst),
                    reason: format!("start failed: {reason}"),
                });
                if flap_strikes > shared.config.max_rapid_deaths {
                    shared.set_state(WorkerState::Failed);
                    shared.emit(WorkerLifecycle::Failed {
                        reason: format!("backoff exhausted after {flap_strikes} tries: {reason}"),
                    });
                    return;
                }
                let idx = (flap_strikes as usize - 1).min(shared.config.backoff.len() - 1);
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
        req: AcquireRequest,
    ) -> Result<Vec<WorkerElementEvidence>, EngineError> {
        // SCHEMA §7.5: the worker returns resolved `topoKey → (kind, descriptor,
        // anchor)` evidence + any already-held id; **Rust mints/owns the ids**.
        let client = self.client_or_err()?;
        let fallback = req.body;
        let resp = client
            .request("AcquireElementIds", wire::acquire_element_ids_args(&req))
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_acquire_evidence(&r, fallback))
    }

    async fn resolve_refs(&self, req: ResolveRequest) -> Result<Vec<RefResolution>, EngineError> {
        // SCHEMA §7.5 dry-run ladder for repair dialogs (binds nothing).
        let client = self.client_or_err()?;
        let resp = client
            .request("ResolveRefs", wire::resolve_refs_args(&req))
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_resolve_refs(&r))
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

// ─────────────────────────────────────────────────────────────────────────────
// SolverEngine (sketch solver lane, SCHEMA §7.4)
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl SolverEngine for WorkerManager {
    async fn sketch_upsert(&self, sketch: &Sketch) -> Result<SketchUpsertDto, EngineError> {
        let client = self.client_or_err()?;
        let resp = client
            .request("SketchUpsert", wire::sketch_upsert_args(sketch))
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_sketch_upsert(&sketch.id.to_string(), &r))
    }

    async fn begin_gesture(
        &self,
        sketch_id: &str,
        sketch_revision: u64,
        gesture_id: u64,
        drag_point: EntityId,
        solver_policy_hash: &str,
    ) -> Result<BeginGestureDto, EngineError> {
        let client = self.client_or_err()?;
        let args = wire::begin_gesture_args(
            sketch_id,
            sketch_revision,
            gesture_id,
            drag_point,
            solver_policy_hash,
        );
        let resp = client
            .request("BeginGesture", args)
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| BeginGestureDto {
            gesture_id: r
                .get("gestureId")
                .and_then(Value::as_u64)
                .unwrap_or(gesture_id),
            ready: r.get("ready").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    async fn solve_drag(
        &self,
        gesture_id: u64,
        seq: u64,
        drag_point: EntityId,
        target: [f64; 2],
    ) -> Result<DragSolveDto, EngineError> {
        // Fired latest-wins: the client sends the newest seq without serial awaits;
        // a stale seq may resolve `superseded` (SCHEMA §7.4) — parsed, not an error.
        let client = self.client_or_err()?;
        let resp = client
            .request(
                "SolveDrag",
                wire::solve_drag_args(gesture_id, seq, drag_point, target),
            )
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_solve_drag(&r))
    }

    async fn end_gesture(
        &self,
        sketch_id: &str,
        gesture_id: u64,
        final_target: Option<[f64; 2]>,
    ) -> Result<SketchUpsertDto, EngineError> {
        let client = self.client_or_err()?;
        let resp = client
            .request(
                "EndGesture",
                wire::end_gesture_args(gesture_id, final_target),
            )
            .await
            .map_err(protocol_err)?;
        ok_result(resp).map(|r| wire::parse_sketch_upsert(sketch_id, &r))
    }

    async fn sketch_regions(&self, sketch_id: &str) -> Result<Vec<SketchRegionDto>, EngineError> {
        // Uses the resp binary tail (previewTriangles bins, SCHEMA §7.4 inline §5.2).
        let client = self.client_or_err()?;
        let inflight = client
            .start_request(
                "SketchRegions",
                wire::sketch_regions_args(sketch_id),
                Lane::Control,
            )
            .await
            .map_err(protocol_err)?;
        let (resp, tail) = inflight.response_with_bin().await.map_err(protocol_err)?;
        let sections = resp.bin.clone().unwrap_or_default();
        let result = ok_result(resp)?;
        Ok(wire::parse_sketch_regions(&result, &sections, &tail))
    }
}

/// Drives one `ExecutePlan`: send it, forward each `planStep` `event` as
/// [`PlanEvent::Step`], then the terminal `PlanPrepared`/error/crash. Records
/// crashes against the **crashing op's** poison key and fast-fails an open circuit
/// (SCHEMA §8 / F3).
async fn stream_plan(shared: Arc<Shared>, request: PlanRequest, tx: mpsc::Sender<PlanEvent>) {
    let job = request.job_id;
    // One candidate poison key per op (`base | opRecordId | fingerprint`); the
    // crashing op is one of them, so a poisoned plan is caught up front (F3).
    let op_keys = plan_op_keys(&request, &shared.fingerprint());

    if shared.plan_circuit_open(&op_keys) {
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

    // Count executed steps so a transport loss can be attributed to the crashing op
    // (= the op at `steps_received`, clamped) rather than the plan's last op (F3).
    let mut steps_received = 0usize;
    let terminal = loop {
        tokio::select! {
            biased;
            ev = events.recv() => match ev {
                Ok(WorkerEvent::Event(e)) if e.id == id && e.event == "planStep" => {
                    let step = e.step_index.map_or(0, |s| s as usize);
                    match wire::parse_plan_step(&e.payload, step) {
                        Ok(s) => {
                            steps_received += 1;
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
    let event = finish_plan(&shared, &op_keys, steps_received, job, terminal);
    let _ = tx.send(event).await;
}

/// The stream terminal: a wire `resp` (or transport error) vs a local parse error.
enum StreamEnd {
    Resp(Result<onecad_protocol::messages::RespFrame, ProtocolError>),
    Local(EngineError),
}

/// Maps a stream terminal to the final [`PlanEvent`], updating poison state.
/// `op_keys` is the plan's per-op candidate keys (execution order); `steps_received`
/// is how many `planStep`s arrived, so a crash is attributed to the crashing op.
fn finish_plan(
    shared: &Shared,
    op_keys: &[String],
    steps_received: usize,
    job: JobId,
    terminal: StreamEnd,
) -> PlanEvent {
    match terminal {
        StreamEnd::Local(err) => PlanEvent::Failed(err),
        StreamEnd::Resp(Ok(resp)) if resp.ok => {
            let result = resp.result.unwrap_or(Value::Null);
            match wire::parse_plan_prepared(job, &result) {
                Ok(p) => {
                    shared.record_success(op_keys);
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
            // Transport died mid-plan ⇒ crash. Key the poison entry on the CRASHING
            // op (the op at `steps_received`, clamped to the last), not the plan's
            // last op (F3). Opening the circuit FAILS FAST that plan-key next time,
            // but does NOT kill the worker (the supervisor keeps it alive so other
            // plans still run) — only the F2 flap budget reaches Failed.
            if let Some(key) = op_keys.get(steps_received.min(op_keys.len().saturating_sub(1))) {
                if shared.record_crash(key) {
                    shared.emit(WorkerLifecycle::CircuitOpen { key: key.clone() });
                }
            }
            PlanEvent::Failed(EngineError::Crashed {
                message: format!("worker crashed mid-plan: {proto}"),
            })
        }
    }
}

/// A plan's candidate poison keys — one per op, `historyPrefixHash | opRecordId |
/// fingerprint` in execution order (SCHEMA §8 keys on `(history hash, op,
/// fingerprint)`; the crashing op is one of these — F3).
fn plan_op_keys(req: &PlanRequest, fingerprint: &str) -> Vec<String> {
    req.ops
        .iter()
        .map(|o| {
            format!(
                "{}|{}|{}",
                req.expected_base_hash.as_str(),
                o.record_id,
                fingerprint
            )
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// MeshProvider (Tessellate + MESH1 bulk assembly, SCHEMA §5.2 / §7.6)
// ─────────────────────────────────────────────────────────────────────────────

/// A finalized bulk payload: `(bytes, manifest totalBytes, manifest sha256)`.
type MeshPayload = (Vec<u8>, Option<u64>, Option<String>);

/// A bulk MESH1 stream being reassembled from chunk frames. Records the manifest's
/// integrity fields + every received byte segment so [`finalize`](Self::finalize)
/// can verify the payload tiles `[0, totalBytes)` EXACTLY — a gap or overlap is a
/// PROTOCOL_ERROR, never zero-fill-and-hope (F5).
#[derive(Default)]
struct StreamAcc {
    buf: Vec<u8>,
    total_bytes: Option<u64>,
    sha256: Option<String>,
    /// Received `(start, end)` byte ranges, in arrival order.
    segments: Vec<(u64, u64)>,
}

impl StreamAcc {
    fn apply(&mut self, c: &onecad_protocol::messages::ChunkFrame, bin: &[u8]) {
        match c.kind {
            onecad_protocol::messages::ChunkKind::Manifest => {
                self.total_bytes = c.total_bytes;
                self.sha256 = c.sha256.clone();
            }
            onecad_protocol::messages::ChunkKind::Data => {
                let off = c.byte_offset.unwrap_or(0);
                let end = off + bin.len() as u64;
                let (start_us, end_us) = (off as usize, end as usize);
                if self.buf.len() < end_us {
                    self.buf.resize(end_us, 0);
                }
                self.buf[start_us..end_us].copy_from_slice(bin);
                self.segments.push((off, end));
            }
        }
    }

    /// Verifies the received segments tile `[0, totalBytes)` with no gap and no
    /// overlap, then returns `(payload, manifest totalBytes, manifest sha256)`.
    fn finalize(mut self) -> Result<MeshPayload, EngineError> {
        let mut segs = self.segments.clone();
        segs.sort_by_key(|&(start, _)| start);
        let mut cursor = 0u64;
        for &(start, end) in &segs {
            match start.cmp(&cursor) {
                std::cmp::Ordering::Greater => {
                    return Err(protocol("MESH1 stream has a gap between chunks"));
                }
                std::cmp::Ordering::Less => {
                    return Err(protocol("MESH1 stream has overlapping chunks"));
                }
                std::cmp::Ordering::Equal => cursor = end,
            }
        }
        if let Some(t) = self.total_bytes {
            if cursor != t {
                return Err(protocol("MESH1 stream chunks do not cover [0, totalBytes)"));
            }
            self.buf.truncate(t as usize);
        }
        Ok((self.buf, self.total_bytes, self.sha256))
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
/// section in the resp tail) or a bulk stream (`streamId`). A streamed payload's
/// chunks must tile `[0, totalBytes)` exactly (F5 gap-detection). The integrity
/// fields (`totalBytes`/`sha256`) are cross-checked between the manifest and the
/// resp handle when both are present (mismatch = error); when the resp handle omits
/// them the manifest's are used (a worker omitting resp fields never skips
/// verification); when BOTH are absent only the MESH1 header is validated. Verifies
/// size + SHA-256 and validates the header (Invariant 5 forward-verbatim).
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

    let resp_total = mesh.get("totalBytes").and_then(Value::as_u64);
    let resp_sha = mesh
        .get("sha256")
        .and_then(Value::as_str)
        .map(str::to_string);

    // (blob, manifest totalBytes, manifest sha256). Inline handles carry no manifest.
    let (blob, manifest_total, manifest_sha) =
        if let Some(name) = mesh.get("bin").and_then(Value::as_str) {
            let section = resp
                .bin
                .as_ref()
                .and_then(|secs| secs.iter().find(|s| s.name == name))
                .ok_or_else(|| protocol("inline mesh bin section missing"))?;
            let start = section.off as usize;
            let end = start + section.len as usize;
            let bytes = tail
                .get(start..end)
                .ok_or_else(|| protocol("inline mesh bin section out of range"))?
                .to_vec();
            (bytes, None, None)
        } else if let Some(stream_id) = mesh.get("streamId").and_then(Value::as_u64) {
            let acc = streams
                .remove(&stream_id)
                .ok_or_else(|| protocol("mesh stream frames never arrived"))?;
            acc.finalize()? // gap/overlap detection (F5)
        } else {
            return Err(protocol("mesh handle has neither inline bin nor streamId"));
        };

    let total = reconcile_field(manifest_total, resp_total, "totalBytes")?;
    let sha = reconcile_field(manifest_sha, resp_sha, "sha256")?;
    if total.is_none() && sha.is_none() {
        eprintln!(
            "worker: mesh handle carried no totalBytes/sha256 (manifest or resp) — \
             validating MESH1 header only"
        );
    }
    verify_mesh(&blob, total, sha.as_deref())?;
    Ok(blob)
}

/// Reconciles a manifest-level and a resp-level copy of an integrity field: both
/// present and unequal ⇒ error; otherwise the present value (manifest preferred),
/// or `None` when neither carries it (F5).
fn reconcile_field<T: PartialEq + std::fmt::Display>(
    manifest: Option<T>,
    resp: Option<T>,
    field: &str,
) -> Result<Option<T>, EngineError> {
    match (manifest, resp) {
        (Some(m), Some(r)) => {
            if m != r {
                return Err(EngineError::Protocol {
                    message: format!("mesh {field} mismatch: manifest {m} != resp {r}"),
                });
            }
            Ok(Some(m))
        }
        (Some(m), None) => Ok(Some(m)),
        (None, r) => Ok(r),
    }
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
