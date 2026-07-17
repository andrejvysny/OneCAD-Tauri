//! Frontend-facing API error type.
//!
//! Serialized into failed command results so the webview gets a typed error
//! (`{ kind, message }`). This mirrors the SCHEMA §8 taxonomy at the app boundary:
//! recoverable op failures keep the document editable; protocol/crash are fatal
//! (R-WP11 drives worker restart). `NeedsRepair` is never an error — it is
//! document *state* delivered in the projection.

use serde::Serialize;

use onecad_core::error::DomainError;
use onecad_core::io::IoError;
use onecad_core::regen::EngineError;

/// Error returned from Tauri commands to the webview (`{ kind, message }`).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "message", rename_all = "camelCase")]
pub enum ApiError {
    /// No document is open (the command requires one).
    NoDocument(String),
    /// A command mutated the document invalidly (validation / anti-time-travel).
    InvalidCommand(String),
    /// A recoverable geometry-op failure — the document stays editable.
    OpFailed(String),
    /// A protocol / crash-class failure — R-WP11 restarts the worker.
    Worker(String),
    /// Filesystem / container IO failure (open/save).
    Io(String),
    /// A generic internal failure.
    Internal(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDocument(m) => write!(f, "no document open: {m}"),
            Self::InvalidCommand(m) => write!(f, "invalid command: {m}"),
            Self::OpFailed(m) => write!(f, "operation failed: {m}"),
            Self::Worker(m) => write!(f, "worker error: {m}"),
            Self::Io(m) => write!(f, "io error: {m}"),
            Self::Internal(m) => write!(f, "internal error: {m}"),
        }
    }
}

impl std::error::Error for ApiError {}

impl From<DomainError> for ApiError {
    fn from(e: DomainError) -> Self {
        ApiError::InvalidCommand(e.to_string())
    }
}

impl From<IoError> for ApiError {
    fn from(e: IoError) -> Self {
        ApiError::Io(e.to_string())
    }
}

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::OpFailed { .. } => ApiError::OpFailed(e.to_string()),
            EngineError::Crashed { .. }
            | EngineError::Protocol { .. }
            | EngineError::Timeout { .. } => ApiError::Worker(e.to_string()),
            EngineError::Cancelled => ApiError::OpFailed("cancelled".into()),
        }
    }
}
