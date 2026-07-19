//! Autosave + crash-recovery driver (app-side).
//!
//! Wraps [`onecad_core::io::recovery`] (the layout + marker lifecycle) with the
//! app's scheduling, filesystem writes and event emission. The core module owns
//! **where** autosaves live (`<app_data>/autosave/<documentId>.onecad`) and the
//! `SessionMarker` (pid crash marker); this module owns **when** and **how** they
//! are written.
//!
//! ## The driver
//!
//! [`run`] is a debounced loop the app spawns once at startup. It subscribes to a
//! `watch` "mutation tick" (bumped by every document-mutating command via
//! [`AppState::note_mutation`](crate::state::AppState::note_mutation)): after the
//! first change it waits [`AUTOSAVE_DEBOUNCE`] of quiet, then writes an autosave
//! for the currently-open document and emits [`events::AUTOSAVE`](crate::events::AUTOSAVE).
//! When **no document is open** the write is skipped (zero autosave activity).
//!
//! ## Recovery lifecycle
//!
//! * a clean [`save_document`](crate::api::save_document) /
//!   [`close_document`](crate::api::close_document) calls [`clear_recovery_state`]
//!   (remove the marker + delete the stale autosave);
//! * the next launch scans for **stale** markers (a marker whose `pid` is no longer
//!   alive, per [`pid_alive`]) with a surviving autosave, and offers recovery
//!   ([`check_recovery`](crate::api::check_recovery)).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Manager};
use tokio::sync::{watch, Mutex};

use onecad_core::ids::DocumentId;
use onecad_core::io::container::SaveMeta;
use onecad_core::io::recovery::{
    autosave_dir, autosave_path, remove_marker, write_marker, SessionMarker,
};

use crate::document_runtime::DocumentRuntime;

/// Quiet window after the last document mutation before an autosave fires
/// (V1/V2 lifecycle). Constant by design — a debounce, not a fixed cadence: a busy
/// editor never autosaves mid-burst, and an idle-but-dirty document autosaves once,
/// ~30 s after the last edit.
pub const AUTOSAVE_DEBOUNCE: Duration = Duration::from_secs(30);

/// One completed autosave — the payload of the [`events::AUTOSAVE`](crate::events::AUTOSAVE)
/// event (`{path, atMs}`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutosaveEvent {
    /// The autosave container that was written (absolute path).
    pub path: String,
    /// Wall-clock time of the write, in Unix-epoch milliseconds.
    pub at_ms: u64,
}

/// The app-data root the autosave layout lives under
/// (`<app_data>/autosave/...`), or `None` in a headless / permission-denied
/// environment (autosave then degrades to a no-op).
#[must_use]
pub fn autosave_root(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_data_dir().ok()
}

/// Whether `pid` names a live process — the liveness predicate
/// [`scan_stale_markers`](onecad_core::io::recovery::scan_stale_markers) injects.
///
/// On Unix, `kill(pid, 0)`: `0` ⇒ alive; `EPERM` ⇒ alive (exists, not ours);
/// `ESRCH` ⇒ dead. Elsewhere the conservative answer is **alive** — it merely
/// defers recovery to a later launch, never a false "safe to discard".
#[must_use]
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        if ret == 0 {
            return true;
        }
        matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::EPERM)
        )
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

/// Writes an autosave container + refreshes the session crash marker for the
/// currently-open document. Returns the [`AutosaveEvent`], or `None` when **no
/// document is open** (the zero-activity guard) or the write failed (logged).
///
/// The document's live save path and dirty flag are untouched (this is a recovery
/// snapshot, not a real save — see [`DocumentRuntime::write_autosave`]).
pub async fn autosave_current(
    runtime: &Mutex<Option<DocumentRuntime>>,
    app_data: &Path,
) -> Option<AutosaveEvent> {
    let guard = runtime.lock().await;
    let rt = guard.as_ref()?; // no document open ⇒ zero autosave activity.
    let doc_id = rt.document_uuid();
    let opened = rt.path().map(Path::to_path_buf);

    // The marker writer creates the dir, but the container writer needs it first.
    if let Err(e) = std::fs::create_dir_all(autosave_dir(app_data)) {
        tracing::warn!("autosave: create dir failed: {e}");
        return None;
    }
    let path = autosave_path(app_data, doc_id);
    let now = now_rfc3339();
    let meta = SaveMeta {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        occt_fingerprint: None,
        created: now.clone(),
        modified: now.clone(),
    };
    if let Err(e) = rt.write_autosave(&path, meta) {
        tracing::warn!("autosave: write container failed: {e}");
        return None;
    }
    let marker = SessionMarker {
        document_id: doc_id,
        pid: std::process::id(),
        opened_path: opened,
        last_autosave: now,
    };
    if let Err(e) = write_marker(app_data, &marker) {
        tracing::warn!("autosave: write marker failed: {e}");
    }
    Some(AutosaveEvent {
        path: path.to_string_lossy().into_owned(),
        at_ms: now_ms(),
    })
}

/// Clears a document's crash marker + stale autosave (a clean save/close, or a
/// recovery decision). Best-effort: a missing marker/file is not an error.
pub fn clear_recovery_state(app_data: &Path, document_id: DocumentId) {
    if let Err(e) = remove_marker(app_data, document_id) {
        tracing::warn!("clear recovery: remove marker failed: {e}");
    }
    let autosave = autosave_path(app_data, document_id);
    match std::fs::remove_file(&autosave) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("clear recovery: remove autosave {autosave:?}: {e}"),
    }
}

/// The debounced autosave loop (spawned once at startup). Blocks on `tick` until a
/// mutation lands, waits `debounce` of quiet (resetting on every further mutation),
/// autosaves the open document, and hands the [`AutosaveEvent`] to `emit`. Returns
/// when every `tick` sender is dropped (app shutdown).
pub async fn run<F>(
    runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    app_data: PathBuf,
    mut tick: watch::Receiver<u64>,
    debounce: Duration,
    emit: F,
) where
    F: Fn(AutosaveEvent),
{
    loop {
        // Wait for the first mutation since the last cycle (the freshly-subscribed
        // receiver treats its initial value as seen ⇒ an idle app never autosaves).
        if tick.changed().await.is_err() {
            return; // all senders dropped.
        }
        // Debounce: fire only after `debounce` of quiet; a newer mutation restarts it.
        loop {
            tokio::select! {
                () = tokio::time::sleep(debounce) => break,
                r = tick.changed() => {
                    if r.is_err() {
                        return;
                    }
                }
            }
        }
        if let Some(ev) = autosave_current(&runtime, &app_data).await {
            emit(ev);
        }
    }
}

/// Current wall-clock time in Unix-epoch milliseconds.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Current UTC time as an RFC-3339 string (shares the calendar-free helper the
/// save path uses).
fn now_rfc3339() -> String {
    crate::api::rfc3339_from_secs(now_ms() / 1000)
}
