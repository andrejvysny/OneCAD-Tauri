//! Tauri command handlers — the webview → Rust API surface.
//!
//! Commands are **thin**: they lock the single-writer runtime, delegate to a
//! [`DocumentRuntime`] method (all the domain logic — testable without a webview),
//! emit the projection/document events, and return a DTO. The command set mirrors
//! the frontend `CadClient` seam (`src/ipc/client.ts`); F-WP8 swaps its mock for a
//! `tauriClient` that calls these. No webview capability is widened — Rust does all
//! filesystem/dialog IO (capabilities stay `core:default`).

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, State};

use onecad_core::edit::EditCommand;
use onecad_core::ids::BodyId;
use onecad_core::io::container::SaveMeta;
use onecad_core::regen::RegenRequest;

use crate::document_runtime::{DocumentRuntime, RegenReport};
use crate::dto::{DocumentProjection, DocumentSnapshotDto, RecentProjectDto};
use crate::error::ApiError;
use crate::events;
use crate::state::AppState;
use crate::worker::lod_from_str;

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Creates a blank document and opens it (`CadClient.newDocument`).
#[tauri::command]
pub async fn new_document(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DocumentSnapshotDto, ApiError> {
    let (engine, meshes) = state.make_backend();
    let (snapshot, projection) = {
        let mut guard = state.runtime.lock().await;
        *guard = Some(DocumentRuntime::new_blank(engine, meshes));
        let rt = guard.as_ref().unwrap();
        (snapshot_of(rt), rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    Ok(snapshot)
}

/// Opens an existing `.onecad` project (`CadClient.openDocument`).
#[tauri::command]
pub async fn open_document(
    state: State<'_, AppState>,
    app: AppHandle,
    path: String,
) -> Result<DocumentSnapshotDto, ApiError> {
    let (engine, meshes) = state.make_backend();
    let rt = DocumentRuntime::open(Path::new(&path), engine, meshes)?;
    let (snapshot, projection) = {
        let mut guard = state.runtime.lock().await;
        *guard = Some(rt);
        let rt = guard.as_ref().unwrap();
        (snapshot_of(rt), rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    // Rebuild geometry from the loaded (all-Dirty) timeline.
    if let Some(sched) = state.scheduler.get() {
        sched.request(RegenRequest::ToEnd { from: 0 });
    }
    Ok(snapshot)
}

/// Imports a STEP file into a new document. The `ImportStep` worker verb lands
/// with R-WP11 / W-WP6; until then this reports the worker is not ready.
#[tauri::command]
pub async fn import_step(
    _state: State<'_, AppState>,
    _path: String,
) -> Result<DocumentSnapshotDto, ApiError> {
    Err(ApiError::Worker(
        "STEP import lands with the worker (R-WP11 / W-WP6)".into(),
    ))
}

/// Saves the open document (`CadClient` save). `path` `None` reuses the last save
/// path; an unsaved document with no path is an error.
#[tauri::command]
pub async fn save_document(
    state: State<'_, AppState>,
    path: Option<String>,
) -> Result<(), ApiError> {
    let mut guard = state.runtime.lock().await;
    let rt = guard
        .as_mut()
        .ok_or_else(|| ApiError::NoDocument("save".into()))?;
    let target: PathBuf = match path {
        Some(p) => PathBuf::from(p),
        None => rt
            .path()
            .map(Path::to_path_buf)
            .ok_or_else(|| ApiError::Io("no save path; provide one".into()))?,
    };
    rt.save(&target, save_meta())?;
    Ok(())
}

/// Closes the open document, dropping its runtime + caches.
#[tauri::command]
pub async fn close_document(state: State<'_, AppState>, app: AppHandle) -> Result<(), ApiError> {
    *state.runtime.lock().await = None;
    let _ = app.emit(events::PROJECTION_UPDATED, &DocumentProjection::empty());
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Edits + queries
// ─────────────────────────────────────────────────────────────────────────────

/// Applies one [`EditCommand`] and enqueues the resulting regen. Returns the
/// (pre-regen) projection; post-regen geometry arrives via `document-changed` +
/// `projection-updated` events (projection stores are written only by events).
#[tauri::command]
pub async fn apply_edit_command(
    state: State<'_, AppState>,
    app: AppHandle,
    command: EditCommand,
) -> Result<DocumentProjection, ApiError> {
    let (outcome, projection) = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("apply".into()))?;
        let outcome = rt.apply(command)?;
        (outcome, rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    if let Some(sched) = state.scheduler.get() {
        sched.handle(&outcome);
    }
    Ok(projection)
}

/// Undoes the last committed edit (`CadClient.undo`).
#[tauri::command]
pub async fn undo(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DocumentProjection, ApiError> {
    let (changed, projection) = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("undo".into()))?;
        (rt.undo(), rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    if changed {
        if let Some(sched) = state.scheduler.get() {
            sched.request(RegenRequest::ToEnd { from: 0 });
        }
    }
    Ok(projection)
}

/// Redoes the last undone edit (`CadClient.redo`).
#[tauri::command]
pub async fn redo(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DocumentProjection, ApiError> {
    let (changed, projection) = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("redo".into()))?;
        (rt.redo()?, rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    if changed {
        if let Some(sched) = state.scheduler.get() {
            sched.request(RegenRequest::ToEnd { from: 0 });
        }
    }
    Ok(projection)
}

/// The current document projection (empty when nothing is open).
#[tauri::command]
pub async fn get_projection(state: State<'_, AppState>) -> Result<DocumentProjection, ApiError> {
    let guard = state.runtime.lock().await;
    Ok(guard
        .as_ref()
        .map_or_else(DocumentProjection::empty, DocumentRuntime::projection))
}

/// Fetches a body's MESH1 blob as a zero-copy `ArrayBuffer` (pull model).
/// `generation` `None` ⇒ the latest snapshot. A miss yields an empty response.
#[tauri::command]
pub async fn get_mesh(
    state: State<'_, AppState>,
    body_id: String,
    lod: String,
    generation: Option<u64>,
) -> Result<tauri::ipc::Response, ApiError> {
    let body = BodyId::from_str(&body_id)
        .map_err(|e| ApiError::InvalidCommand(format!("bad bodyId {body_id:?}: {e}")))?;
    let lod = lod_from_str(&lod);
    let bytes = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("getMesh".into()))?;
        rt.get_mesh(body, lod, generation).await
    };
    // MESH1 travels verbatim; a miss is an empty buffer (frontend keeps its mesh).
    let data = bytes.map(|a| a.as_ref().clone()).unwrap_or_default();
    Ok(tauri::ipc::Response::new(data))
}

// ─────────────────────────────────────────────────────────────────────────────
// Start screen + native dialogs (Rust-side; webview has zero fs/dialog cap)
// ─────────────────────────────────────────────────────────────────────────────

/// Recent projects for the start screen. A persisted recents store is a later WP;
/// V1 returns none (the frontend seeds its own until then).
#[tauri::command]
pub async fn list_recents(_state: State<'_, AppState>) -> Result<Vec<RecentProjectDto>, ApiError> {
    Ok(Vec::new())
}

/// Shows a native open dialog (Rust owns the dialog; `tauri-plugin-dialog` Rust
/// API). Resolves to the chosen path or `None` if cancelled.
#[tauri::command]
pub async fn open_file_dialog(app: AppHandle) -> Result<Option<String>, ApiError> {
    Ok(pick_file(app, false).await)
}

/// Shows a native save dialog. Resolves to the chosen path or `None`.
#[tauri::command]
pub async fn save_file_dialog(app: AppHandle) -> Result<Option<String>, ApiError> {
    Ok(pick_file(app, true).await)
}

async fn pick_file(app: AppHandle, save: bool) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let dialog = app.dialog().file().add_filter("OneCAD", &["onecad"]);
    let cb = move |file: Option<tauri_plugin_dialog::FilePath>| {
        let _ = tx.send(file);
    };
    if save {
        dialog.save_file(cb);
    } else {
        dialog.pick_file(cb);
    }
    rx.await
        .ok()
        .flatten()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared helpers (also used by the regen driver in `crate::run`)
// ─────────────────────────────────────────────────────────────────────────────

/// Emits the post-regen events: `document-changed` (pull-model body refs) when a
/// snapshot published, and the refreshed `projection-updated`.
pub fn emit_regen_events(app: &AppHandle, report: &RegenReport, projection: &DocumentProjection) {
    if let Some(change) = report.document_change() {
        let _ = app.emit(events::DOCUMENT_CHANGED, change);
    }
    let _ = app.emit(events::PROJECTION_UPDATED, projection);
}

fn snapshot_of(rt: &DocumentRuntime) -> DocumentSnapshotDto {
    DocumentSnapshotDto {
        document_id: rt.document_id(),
        title: rt.title().to_string(),
    }
}

/// Provenance metadata for a save. The pure core never reads the wall clock, so
/// the app supplies the timestamps here.
fn save_meta() -> SaveMeta {
    let now = now_rfc3339();
    SaveMeta {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        occt_fingerprint: None,
        created: now.clone(),
        modified: now,
    }
}

/// The current UTC time as an RFC-3339 string (`YYYY-MM-DDThh:mm:ssZ`), computed
/// from the Unix clock without a calendar dependency (Howard Hinnant's civil-date
/// algorithm).
fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mon, d) = civil_from_days(days);
    format!("{y:04}-{mon:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}Z")
}

/// `(year, month, day)` from a Unix day count (days since 1970-01-01).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_epoch_and_a_known_date() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-07-17 is day 20651 since the Unix epoch.
        assert_eq!(civil_from_days(20_651), (2026, 7, 17));
    }

    #[test]
    fn now_rfc3339_is_well_formed() {
        let s = now_rfc3339();
        assert_eq!(s.len(), 20, "YYYY-MM-DDThh:mm:ssZ");
        assert!(s.ends_with('Z') && s.contains('T'));
    }
}
