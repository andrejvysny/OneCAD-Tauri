//! Frontend-facing API error type.
//!
//! Serialized into failed command results so the webview gets a typed error.
//! The full recoverable/cancelled/protocol/crash taxonomy lands in a later WP.

use serde::Serialize;

/// Error returned from Tauri commands to the webview.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "message")]
pub enum ApiError {
    /// Generic internal failure. Replaced by a typed taxonomy in a later WP.
    Internal(String),
}
