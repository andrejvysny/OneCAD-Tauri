//! STEP-export seam — the app-layer trait the `export_step_file` command drives.
//!
//! The worker's `ExportStep` verb (SCHEMA §7.8) is surfaced on [`WorkerManager`]
//! as an inherent `export_step` method; this module wraps it behind a small
//! object-safe [`StepExporter`] trait so [`AppState`](crate::state::AppState) can
//! hold `Arc<dyn StepExporter>` alongside the geometry backend (the SAME
//! `WorkerManager` Arc; [`PendingBackend`] when no worker resolved). Keeping the
//! trait + impls here means `worker/manager.rs` and `worker/mod.rs` stay untouched.

use async_trait::async_trait;

use onecad_core::ids::BodyId;
use onecad_core::regen::EngineError;

use crate::worker::{PendingBackend, WorkerManager};

/// Exports a set of bodies to a STEP file on disk. `path` is the target file,
/// `bodies` the body ids to write, `schema` the STEP application protocol
/// (e.g. `"AP214IS"`). Object-safe so the app can store it as a trait object.
#[async_trait]
pub trait StepExporter: Send + Sync {
    /// Writes `bodies` to `path` using the STEP `schema`.
    ///
    /// # Errors
    /// [`EngineError`] on a disconnected worker or a worker-side export failure.
    async fn export_step(
        &self,
        path: &str,
        bodies: &[BodyId],
        schema: &str,
    ) -> Result<(), EngineError>;
}

#[async_trait]
impl StepExporter for WorkerManager {
    async fn export_step(
        &self,
        path: &str,
        bodies: &[BodyId],
        schema: &str,
    ) -> Result<(), EngineError> {
        // Inherent `WorkerManager::export_step` (SCHEMA §7.8 passthrough) wins path
        // resolution over this trait method; it returns the bytes written, which the
        // app command does not surface.
        WorkerManager::export_step(self, path, bodies, schema)
            .await
            .map(|_bytes_written| ())
    }
}

#[async_trait]
impl StepExporter for PendingBackend {
    async fn export_step(
        &self,
        _path: &str,
        _bodies: &[BodyId],
        _schema: &str,
    ) -> Result<(), EngineError> {
        Err(EngineError::Protocol {
            message: "worker not started; STEP export unavailable".into(),
        })
    }
}
