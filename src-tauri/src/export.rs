//! Geometry-export seam — the app-layer trait the `export_*_file` commands drive.
//!
//! The worker's `ExportStep` / `ExportStl` / `ExportObj` verbs (SCHEMA §7.8) are
//! surfaced on [`WorkerManager`] as inherent `export_*` methods; this module wraps
//! them behind a small object-safe [`GeometryExporter`] trait so
//! [`AppState`](crate::state::AppState) can hold `Arc<dyn GeometryExporter>`
//! alongside the geometry backend (the SAME `WorkerManager` Arc;
//! [`PendingBackend`] when no worker resolved). Keeping the trait + impls here means
//! `worker/manager.rs` and `worker/mod.rs` stay untouched.

use async_trait::async_trait;

use onecad_core::ids::BodyId;
use onecad_core::regen::EngineError;

use crate::worker::{PendingBackend, WorkerManager};

/// Exports bodies to a mesh/BREP file on disk. `path` is the target file, `bodies`
/// the body ids to write. Object-safe so the app can store it as a trait object.
///
/// One trait for all three formats (M5a widened the M4 STEP-only seam): a document
/// has exactly one worker, so a single `Arc<dyn GeometryExporter>` routes every
/// export to it.
#[async_trait]
pub trait GeometryExporter: Send + Sync {
    /// Writes `bodies` to `path` as STEP using the `schema` (e.g. `"AP214IS"`).
    ///
    /// # Errors
    /// [`EngineError`] on a disconnected worker or a worker-side export failure.
    async fn export_step(
        &self,
        path: &str,
        bodies: &[BodyId],
        schema: &str,
    ) -> Result<(), EngineError>;

    /// Writes `bodies` to `path` as STL (binary when `binary`, else ASCII), meshed
    /// at `lod` (SCHEMA §7.8).
    ///
    /// # Errors
    /// [`EngineError`] on a disconnected worker or a worker-side export failure.
    async fn export_stl(
        &self,
        path: &str,
        bodies: &[BodyId],
        binary: bool,
        lod: &str,
    ) -> Result<(), EngineError>;

    /// Writes `bodies` to `path` as ASCII OBJ, meshed at `lod` (SCHEMA §7.8).
    ///
    /// # Errors
    /// [`EngineError`] on a disconnected worker or a worker-side export failure.
    async fn export_obj(&self, path: &str, bodies: &[BodyId], lod: &str)
        -> Result<(), EngineError>;
}

#[async_trait]
impl GeometryExporter for WorkerManager {
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

    async fn export_stl(
        &self,
        path: &str,
        bodies: &[BodyId],
        binary: bool,
        lod: &str,
    ) -> Result<(), EngineError> {
        WorkerManager::export_stl(self, path, bodies, binary, lod)
            .await
            .map(|_written| ())
    }

    async fn export_obj(
        &self,
        path: &str,
        bodies: &[BodyId],
        lod: &str,
    ) -> Result<(), EngineError> {
        WorkerManager::export_obj(self, path, bodies, lod)
            .await
            .map(|_bytes_written| ())
    }
}

#[async_trait]
impl GeometryExporter for PendingBackend {
    async fn export_step(
        &self,
        _path: &str,
        _bodies: &[BodyId],
        _schema: &str,
    ) -> Result<(), EngineError> {
        Err(not_ready("STEP"))
    }

    async fn export_stl(
        &self,
        _path: &str,
        _bodies: &[BodyId],
        _binary: bool,
        _lod: &str,
    ) -> Result<(), EngineError> {
        Err(not_ready("STL"))
    }

    async fn export_obj(
        &self,
        _path: &str,
        _bodies: &[BodyId],
        _lod: &str,
    ) -> Result<(), EngineError> {
        Err(not_ready("OBJ"))
    }
}

fn not_ready(kind: &str) -> EngineError {
    EngineError::Protocol {
        message: format!("worker not started; {kind} export unavailable"),
    }
}
