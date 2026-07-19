//! Shared application state, managed as `tauri::State<AppState>`.
//!
//! V1 = a single open document behind an async [`tokio::sync::Mutex`] — the
//! single-writer lock the [`DocumentRuntime`] and the regen driver share (plan
//! "DocumentSession single writer"). Commands and the scheduler's
//! [`RegenDriver`](onecad_core::regen::RegenDriver) both lock this one runtime, so
//! writes serialize deterministically.
//!
//! ## Backend factory (R-WP11)
//!
//! [`AppState::default`] builds a factory that spawns a real
//! [`WorkerManager`]-backed backend when the worker binary resolves
//! ([`resolve_worker_path`]), wiring its **restart hook** to bump the document's
//! epoch + enqueue a replay (SCHEMA §8 crash → restart + replay). When no binary
//! is present it falls back to [`PendingBackend`] so the app still boots. The
//! factory also produces the [`GeometryExporter`] (the same `WorkerManager` Arc) that
//! `export_step_file` drives, and spawns a **worker-status forwarder** that relays
//! [`WorkerLifecycle`] transitions to the webview as `worker-status` events.

use std::sync::{Arc, Mutex as StdMutex, OnceLock, RwLock};

use tauri::{AppHandle, Emitter};
use tokio::sync::{watch, Mutex};

use onecad_core::io::recovery::RecoveryOffer;
use onecad_core::regen::{GeometryEngine, RegenRequest, SchedulerHandle};

use crate::document_runtime::DocumentRuntime;
use crate::dto::WorkerStatusDto;
use crate::events;
use crate::export::GeometryExporter;
use crate::worker::{
    resolve_worker_path, MeshProvider, PendingBackend, SolverEngine, SupervisorConfig,
    WorkerLifecycle, WorkerManager, WorkerState,
};

/// The geometry backend split into its three facets (the executor drives the
/// [`GeometryEngine`]; the mesh cache pulls bytes from the [`MeshProvider`]; the
/// sketch flow drives the [`SolverEngine`] lane, SCHEMA §7.4).
pub type BackendPair = (
    Arc<dyn GeometryEngine>,
    Arc<dyn MeshProvider>,
    Arc<dyn SolverEngine>,
);

/// A backend bundle: the [`BackendPair`] facets plus the [`GeometryExporter`] for the
/// same worker (`export_step_file` drives it). Same `WorkerManager` Arc throughout.
pub type BackendBundle = (
    Arc<dyn GeometryEngine>,
    Arc<dyn MeshProvider>,
    Arc<dyn SolverEngine>,
    Arc<dyn GeometryExporter>,
);

/// Builds a fresh backend bundle for a newly opened document.
pub type BackendFactory = Arc<dyn Fn() -> BackendBundle + Send + Sync>;

/// A shared, set-once regen scheduler handle (the setup wires it; the factory's
/// restart hook reads it to enqueue a replay).
pub type SharedScheduler = Arc<OnceLock<SchedulerHandle>>;

/// A shared, set-once [`AppHandle`] — the setup fills it so the factory's
/// worker-status forwarder can emit events (it is built before the handle exists).
pub type SharedAppHandle = Arc<OnceLock<AppHandle>>;

/// Root application state handed to every command.
pub struct AppState {
    /// The single open document (V1), or `None` when nothing is open. The regen
    /// driver and every command lock this one runtime (single writer).
    pub runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    /// The regen scheduler control surface, set once in `crate::run`'s setup.
    pub scheduler: SharedScheduler,
    /// The app handle, set once in `crate::run`'s setup (the worker-status
    /// forwarder + any late event emitter read it).
    pub app: SharedAppHandle,
    /// A monotonic "document mutated" tick the autosave driver debounces on. Every
    /// document-mutating command bumps it via [`note_mutation`](Self::note_mutation);
    /// the driver subscribes in `crate::run`'s setup (the autosave signal seam).
    pub autosave_tick: Arc<watch::Sender<u64>>,
    /// The crash-recovery offer surfaced at startup by
    /// [`check_recovery`](crate::api::check_recovery), consumed by
    /// [`recover_document`](crate::api::recover_document). `None` until scanned / after
    /// a decision (V1 single-document ⇒ at most one offer).
    pub pending_recovery: StdMutex<Option<RecoveryOffer>>,
    /// The current document's STEP exporter (the same `WorkerManager` Arc the
    /// backend uses, or [`PendingBackend`] when no worker). Swapped by
    /// [`make_backend`](AppState::make_backend) on every new/open.
    exporter: RwLock<Arc<dyn GeometryExporter>>,
    backend_factory: BackendFactory,
}

impl AppState {
    /// Builds state over an explicit backend factory (tests inject a scripted
    /// backend). Production uses [`AppState::default`].
    #[must_use]
    pub fn new(backend_factory: BackendFactory) -> Self {
        Self {
            runtime: Arc::new(Mutex::new(None)),
            scheduler: Arc::new(OnceLock::new()),
            app: Arc::new(OnceLock::new()),
            autosave_tick: Arc::new(watch::channel(0u64).0),
            pending_recovery: StdMutex::new(None),
            exporter: RwLock::new(Arc::new(PendingBackend)),
            backend_factory,
        }
    }

    /// Signals the autosave driver that the open document changed (every
    /// document-mutating command calls this — the autosave debounce seam).
    pub fn note_mutation(&self) {
        self.autosave_tick.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// A fresh backend pair for a new/open document. Also swaps in the matching
    /// [`GeometryExporter`] so a later `export_step_file` routes to this document's
    /// worker.
    #[must_use]
    pub fn make_backend(&self) -> BackendPair {
        let (engine, meshes, solver, exporter) = (self.backend_factory)();
        if let Ok(mut slot) = self.exporter.write() {
            *slot = exporter;
        }
        (engine, meshes, solver)
    }

    /// The current document's STEP exporter (see [`make_backend`](Self::make_backend)).
    #[must_use]
    pub fn exporter(&self) -> Arc<dyn GeometryExporter> {
        self.exporter.read().unwrap().clone()
    }
}

impl Default for AppState {
    fn default() -> Self {
        let runtime = Arc::new(Mutex::new(None));
        let scheduler: SharedScheduler = Arc::new(OnceLock::new());
        let app: SharedAppHandle = Arc::new(OnceLock::new());
        let backend_factory = real_worker_factory(runtime.clone(), scheduler.clone(), app.clone());
        Self {
            runtime,
            scheduler,
            app,
            autosave_tick: Arc::new(watch::channel(0u64).0),
            pending_recovery: StdMutex::new(None),
            exporter: RwLock::new(Arc::new(PendingBackend)),
            backend_factory,
        }
    }
}

/// The production factory: spawn a [`WorkerManager`] over the resolved binary and
/// wire its restart hook to mark the document dirty + replay; else
/// [`PendingBackend`]. The hook captures the shared runtime + scheduler so a
/// worker restart re-drives geometry against the freshly-bumped epoch. Each real
/// worker also gets a [`WorkerLifecycle`] forwarder emitting `worker-status`.
fn real_worker_factory(
    runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    scheduler: SharedScheduler,
    app: SharedAppHandle,
) -> BackendFactory {
    Arc::new(move || match resolve_worker_path() {
        Some(path) => {
            let wm = WorkerManager::spawn(SupervisorConfig::production(path));
            let rt = runtime.clone();
            let sch = scheduler.clone();
            wm.set_restart_hook(Arc::new(move |epoch| {
                let rt = rt.clone();
                let sch = sch.clone();
                tokio::spawn(async move {
                    {
                        let mut guard = rt.lock().await;
                        if let Some(doc) = guard.as_mut() {
                            doc.on_worker_restart(epoch);
                        }
                    }
                    if let Some(handle) = sch.get() {
                        handle.request(RegenRequest::ToEnd { from: 0 });
                    }
                });
            }));
            spawn_status_forwarder(&wm, app.clone());
            let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
            let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
            let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
            let exporter: Arc<dyn GeometryExporter> = Arc::new(wm);
            (engine, meshes, solver, exporter)
        }
        None => {
            let backend = Arc::new(PendingBackend);
            let engine: Arc<dyn GeometryEngine> = backend.clone();
            let meshes: Arc<dyn MeshProvider> = backend.clone();
            let solver: Arc<dyn SolverEngine> = backend.clone();
            let exporter: Arc<dyn GeometryExporter> = backend;
            (engine, meshes, solver, exporter)
        }
    })
}

/// Subscribes to a worker's [`WorkerLifecycle`] broadcast and relays each
/// transition to the webview as a `worker-status` event, plus one immediate emit
/// of the current state at subscription time (so a late-loading webview still
/// learns the state without a separate fetch).
fn spawn_status_forwarder(wm: &WorkerManager, app: SharedAppHandle) {
    let mut rx = wm.subscribe();
    let initial = status_from_state(wm.state(), wm.epoch().0);
    tokio::spawn(async move {
        if let Some(handle) = app.get() {
            let _ = handle.emit(events::WORKER_STATUS, &initial);
        }
        while let Ok(ev) = rx.recv().await {
            if let Some(dto) = status_from_lifecycle(&ev) {
                if let Some(handle) = app.get() {
                    let _ = handle.emit(events::WORKER_STATUS, &dto);
                }
            }
        }
    });
}

/// Maps a current [`WorkerState`] to a `worker-status` payload (initial emit).
fn status_from_state(state: WorkerState, epoch: u64) -> WorkerStatusDto {
    let state = match state {
        WorkerState::Starting => "starting",
        WorkerState::Ready => "ready",
        WorkerState::Restarting => "restarting",
        WorkerState::Failed => "failed",
    };
    WorkerStatusDto {
        state: state.into(),
        epoch,
    }
}

/// Maps a [`WorkerLifecycle`] transition to a `worker-status` payload. `CircuitOpen`
/// is a per-plan poison event (not a worker-level state), so it is not forwarded.
fn status_from_lifecycle(ev: &WorkerLifecycle) -> Option<WorkerStatusDto> {
    match ev {
        WorkerLifecycle::Ready { epoch, .. } => Some(WorkerStatusDto {
            state: "ready".into(),
            epoch: *epoch,
        }),
        WorkerLifecycle::Restarting { epoch, .. } => Some(WorkerStatusDto {
            state: "restarting".into(),
            epoch: *epoch,
        }),
        WorkerLifecycle::Failed { .. } => Some(WorkerStatusDto {
            state: "failed".into(),
            epoch: 0,
        }),
        WorkerLifecycle::CircuitOpen { .. } => None,
    }
}
