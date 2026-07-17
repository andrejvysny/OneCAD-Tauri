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
//! is present it falls back to [`PendingBackend`] so the app still boots.

use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use onecad_core::regen::{GeometryEngine, RegenRequest, SchedulerHandle};

use crate::document_runtime::DocumentRuntime;
use crate::worker::{
    resolve_worker_path, MeshProvider, PendingBackend, SupervisorConfig, WorkerManager,
};

/// The geometry backend split into its two facets (the executor drives the
/// [`GeometryEngine`]; the mesh cache pulls bytes from the [`MeshProvider`]).
pub type BackendPair = (Arc<dyn GeometryEngine>, Arc<dyn MeshProvider>);

/// Builds a fresh backend for a newly opened document.
pub type BackendFactory = Arc<dyn Fn() -> BackendPair + Send + Sync>;

/// A shared, set-once regen scheduler handle (the setup wires it; the factory's
/// restart hook reads it to enqueue a replay).
pub type SharedScheduler = Arc<OnceLock<SchedulerHandle>>;

/// Root application state handed to every command.
pub struct AppState {
    /// The single open document (V1), or `None` when nothing is open. The regen
    /// driver and every command lock this one runtime (single writer).
    pub runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    /// The regen scheduler control surface, set once in `crate::run`'s setup.
    pub scheduler: SharedScheduler,
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
            backend_factory,
        }
    }

    /// A fresh backend pair for a new/open document.
    #[must_use]
    pub fn make_backend(&self) -> BackendPair {
        (self.backend_factory)()
    }
}

impl Default for AppState {
    fn default() -> Self {
        let runtime = Arc::new(Mutex::new(None));
        let scheduler: SharedScheduler = Arc::new(OnceLock::new());
        let backend_factory = real_worker_factory(runtime.clone(), scheduler.clone());
        Self {
            runtime,
            scheduler,
            backend_factory,
        }
    }
}

/// The production factory: spawn a [`WorkerManager`] over the resolved binary and
/// wire its restart hook to mark the document dirty + replay; else
/// [`PendingBackend`]. The hook captures the shared runtime + scheduler so a
/// worker restart re-drives geometry against the freshly-bumped epoch.
fn real_worker_factory(
    runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    scheduler: SharedScheduler,
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
            let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
            let meshes: Arc<dyn MeshProvider> = Arc::new(wm);
            (engine, meshes)
        }
        None => {
            let backend = Arc::new(PendingBackend);
            let engine: Arc<dyn GeometryEngine> = backend.clone();
            let meshes: Arc<dyn MeshProvider> = backend;
            (engine, meshes)
        }
    })
}
