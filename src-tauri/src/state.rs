//! Shared application state, managed as `tauri::State<AppState>`.
//!
//! V1 = a single open document behind an async [`tokio::sync::Mutex`] — the
//! single-writer lock the [`DocumentRuntime`] and the regen driver share (plan
//! "DocumentSession single writer"). Commands and the scheduler's
//! [`RegenDriver`](onecad_core::regen::RegenDriver) both lock this one runtime, so
//! writes serialize deterministically.

use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;

use onecad_core::regen::{GeometryEngine, SchedulerHandle};

use crate::document_runtime::DocumentRuntime;
use crate::worker::{MeshProvider, PendingBackend};

/// The geometry backend split into its two facets (the executor drives the
/// [`GeometryEngine`]; the mesh cache pulls bytes from the [`MeshProvider`]).
pub type BackendPair = (Arc<dyn GeometryEngine>, Arc<dyn MeshProvider>);

/// Builds a fresh backend for a newly opened document. R-WP11 swaps this factory
/// for one that spawns the real `WorkerManager`; nothing else here changes.
pub type BackendFactory = Arc<dyn Fn() -> BackendPair + Send + Sync>;

/// Root application state handed to every command.
pub struct AppState {
    /// The single open document (V1), or `None` when nothing is open. The regen
    /// driver and every command lock this one runtime (single writer).
    pub runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    /// The regen scheduler control surface, set once in `crate::run`'s setup.
    pub scheduler: OnceLock<SchedulerHandle>,
    backend_factory: BackendFactory,
}

impl AppState {
    /// Builds state over a backend factory (tests inject a scripted backend;
    /// production uses [`PendingBackend`] until R-WP11 wires the real worker).
    #[must_use]
    pub fn new(backend_factory: BackendFactory) -> Self {
        Self {
            runtime: Arc::new(Mutex::new(None)),
            scheduler: OnceLock::new(),
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
        // Production boot: the placeholder backend fails every geometry call so the
        // webview still loads; R-WP11 replaces the factory with the real worker.
        Self::new(Arc::new(|| {
            let backend = Arc::new(PendingBackend);
            let engine: Arc<dyn GeometryEngine> = backend.clone();
            let meshes: Arc<dyn MeshProvider> = backend;
            (engine, meshes)
        }))
    }
}
