//! Worker sidecar lifecycle.
//!
//! Uses `tokio::process` (NOT `tauri-plugin-shell`, which lacks AsyncRead /
//! backpressure); `externalBin` is for bundling only. Restart policy: backoff
//! 0.5/1/2s ×3 → Failed banner; a restart marks the document dirty and replays
//! (later from a checkpoint). Fields land in later WPs.

/// Owns the worker child process, the framing client, and the restart policy.
#[derive(Debug, Default)]
pub struct WorkerManager;
