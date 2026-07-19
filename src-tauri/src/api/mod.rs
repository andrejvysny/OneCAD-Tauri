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

use serde::Deserialize;
use tauri::{AppHandle, Emitter, State};

use onecad_core::document::refs::{AnchorIntent, ElementRef};
use onecad_core::edit::{EditCommand, SketchEditOp};
use onecad_core::ids::{BodyId, EntityId, SketchId, SnapshotId, TopoKey};
use onecad_core::io::container::SaveMeta;
use onecad_core::regen::{RegenRequest, ResolveRef, ResolveRequest};

use crate::document_runtime::{DocumentRuntime, RegenReport};
use crate::dto::{
    BeginGestureDto, DocumentProjection, DocumentSnapshotDto, DragSolveDto, FinishSketchDto,
    PromotedElementDto, RecentProjectDto, ResolveRefDto, SketchSessionDto, SketchUpsertDto,
};
use crate::error::ApiError;
use crate::events;
use crate::recents;
use crate::state::AppState;
use crate::worker::{lod_from_str, wire};

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle
// ─────────────────────────────────────────────────────────────────────────────

/// Creates a blank document and opens it (`CadClient.newDocument`).
#[tauri::command]
pub async fn new_document(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<DocumentSnapshotDto, ApiError> {
    let (engine, meshes, solver) = state.make_backend();
    let (snapshot, projection) = {
        let mut guard = state.runtime.lock().await;
        *guard = Some(DocumentRuntime::new_blank(engine, meshes, solver));
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
    let (engine, meshes, solver) = state.make_backend();
    let rt = DocumentRuntime::open(Path::new(&path), engine, meshes, solver)?;
    let (snapshot, projection) = {
        let mut guard = state.runtime.lock().await;
        *guard = Some(rt);
        let rt = guard.as_ref().unwrap();
        (snapshot_of(rt), rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    recents::record(&app, Path::new(&path));
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
/// path; an unsaved document with no path is an error (the frontend's Save action
/// then falls back to Save As). Records the saved path in the recents store.
#[tauri::command]
pub async fn save_document(
    state: State<'_, AppState>,
    app: AppHandle,
    path: Option<String>,
) -> Result<(), ApiError> {
    let target: PathBuf = {
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
        target
    };
    recents::record(&app, &target);
    Ok(())
}

/// Exports every body at head to a STEP file (`CadClient.exportStep`). `path`
/// `None` shows a native save dialog (`.step` filter); a cancel resolves to `None`.
/// Schema is AP214 (`"AP214IS"`); returns the written path. Rust owns the dialog
/// and the worker `ExportStep` verb (the webview has zero fs capability).
#[tauri::command]
pub async fn export_step_file(
    state: State<'_, AppState>,
    app: AppHandle,
    path: Option<String>,
) -> Result<Option<String>, ApiError> {
    let target = match path {
        Some(p) => p,
        None => match pick_step_save(app).await {
            Some(p) => p,
            None => return Ok(None), // dialog cancelled
        },
    };
    let bodies: Vec<BodyId> = {
        let guard = state.runtime.lock().await;
        let rt = guard
            .as_ref()
            .ok_or_else(|| ApiError::NoDocument("exportStep".into()))?;
        rt.head_body_ids()
    };
    let exporter = state.exporter();
    exporter.export_step(&target, &bodies, "AP214IS").await?;
    Ok(Some(target))
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
// Sketch solver lane (SCHEMA §7.4) — mirrors the frontend `localSolver` seam
// ─────────────────────────────────────────────────────────────────────────────

/// Enters sketch mode: syncs the sketch to the worker solver lane and returns the
/// live session + real dof/status (`CadClient.enterSketch`; the F-WP9 swap target).
#[tauri::command]
pub async fn enter_sketch(
    state: State<'_, AppState>,
    app: AppHandle,
    sketch_id: String,
) -> Result<SketchSessionDto, ApiError> {
    let id = parse_sketch_id(&sketch_id)?;
    let (session, projection) = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("enterSketch".into()))?;
        let session = rt.enter_sketch(id).await?;
        (session, rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    Ok(session)
}

/// Applies sketch edits (add/move/delete entities+constraints) then re-solves for
/// live dof/status (`CadClient.sketchUpsert`).
#[tauri::command]
pub async fn sketch_upsert(
    state: State<'_, AppState>,
    app: AppHandle,
    sketch_id: String,
    ops: Vec<SketchEditOp>,
) -> Result<SketchUpsertDto, ApiError> {
    let id = parse_sketch_id(&sketch_id)?;
    let (result, projection) = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("sketchUpsert".into()))?;
        let result = rt.sketch_upsert(id, ops).await?;
        (result, rt.projection())
    };
    let _ = app.emit(events::SKETCH_SOLVED, &result);
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    Ok(result)
}

/// Opens a drag gesture on a point (`BeginGesture`; SCHEMA §7.4).
#[tauri::command]
pub async fn begin_gesture(
    state: State<'_, AppState>,
    sketch_id: String,
    drag_point: String,
) -> Result<BeginGestureDto, ApiError> {
    let id = parse_sketch_id(&sketch_id)?;
    let point = EntityId::from_str(&drag_point)
        .map_err(|e| ApiError::InvalidCommand(format!("bad dragPoint {drag_point:?}: {e}")))?;
    let mut guard = state.runtime.lock().await;
    let rt = guard
        .as_mut()
        .ok_or_else(|| ApiError::NoDocument("beginGesture".into()))?;
    Ok(rt.begin_gesture(id, point).await?)
}

/// One latest-wins incremental drag solve (`SolveDrag`; preview only).
#[tauri::command]
pub async fn solve_drag(
    state: State<'_, AppState>,
    target: [f64; 2],
) -> Result<DragSolveDto, ApiError> {
    let mut guard = state.runtime.lock().await;
    let rt = guard
        .as_mut()
        .ok_or_else(|| ApiError::NoDocument("solveDrag".into()))?;
    Ok(rt.solve_drag(target).await?)
}

/// Pointer-up: final exact solve committed as ONE undo command (`EndGesture`).
#[tauri::command]
pub async fn end_gesture(
    state: State<'_, AppState>,
    app: AppHandle,
    final_target: Option<[f64; 2]>,
) -> Result<SketchUpsertDto, ApiError> {
    let (result, projection) = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("endGesture".into()))?;
        let result = rt.end_gesture(final_target).await?;
        (result, rt.projection())
    };
    let _ = app.emit(events::SKETCH_SOLVED, &result);
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    Ok(result)
}

/// Exits sketch mode / cancels an in-flight gesture without committing.
#[tauri::command]
pub async fn cancel_sketch(state: State<'_, AppState>, sketch_id: String) -> Result<(), ApiError> {
    let id = parse_sketch_id(&sketch_id)?;
    let mut guard = state.runtime.lock().await;
    let rt = guard
        .as_mut()
        .ok_or_else(|| ApiError::NoDocument("cancelSketch".into()))?;
    rt.cancel_sketch(id).await?;
    Ok(())
}

/// Computes the closed profile regions for extrude/revolve selection + preview fill
/// (`finishSketch` → `SketchRegions`).
#[tauri::command]
pub async fn finish_sketch(
    state: State<'_, AppState>,
    sketch_id: String,
) -> Result<FinishSketchDto, ApiError> {
    let id = parse_sketch_id(&sketch_id)?;
    let mut guard = state.runtime.lock().await;
    let rt = guard
        .as_mut()
        .ok_or_else(|| ApiError::NoDocument("finishSketch".into()))?;
    Ok(rt.finish_sketch(id).await?)
}

// ─────────────────────────────────────────────────────────────────────────────
// Element identity (SCHEMA §7.5) — pick → promote
// ─────────────────────────────────────────────────────────────────────────────

/// One pick to promote (`{topoKey, anchor?}`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickInput {
    pub topo_key: String,
    #[serde(default)]
    pub anchor: Option<AnchorIntent>,
}

/// One ref to dry-run-resolve (`{refId, primary?, intent?, anchor?}`).
#[derive(Debug, Deserialize)]
pub struct ResolveRefInput {
    #[serde(rename = "refId")]
    pub ref_id: String,
    #[serde(flatten)]
    pub element: ElementRef,
}

/// Promotes snapshot-scoped TopoKey picks to persistent, Rust-minted `ElementId`s
/// (`AcquireElementIds`; SCHEMA §7.5) — the pick→promote surface for M2.
#[tauri::command]
pub async fn promote_selection(
    state: State<'_, AppState>,
    app: AppHandle,
    snapshot_id: u64,
    body_id: String,
    picks: Vec<PickInput>,
) -> Result<Vec<PromotedElementDto>, ApiError> {
    let body = wire::parse_body_id(&body_id).map_err(ApiError::InvalidCommand)?;
    let picks: Vec<(TopoKey, Option<AnchorIntent>)> = picks
        .into_iter()
        .map(|p| (TopoKey::new(p.topo_key), p.anchor))
        .collect();
    let (ids, projection) = {
        let mut guard = state.runtime.lock().await;
        let rt = guard
            .as_mut()
            .ok_or_else(|| ApiError::NoDocument("promoteSelection".into()))?;
        let ids = rt
            .promote_selection(SnapshotId(snapshot_id), body, picks)
            .await?;
        (ids, rt.projection())
    };
    let _ = app.emit(events::PROJECTION_UPDATED, &projection);
    Ok(ids)
}

/// Dry-run ladder resolution for repair dialogs (`ResolveRefs`; SCHEMA §7.5) —
/// binds nothing.
#[tauri::command]
pub async fn resolve_refs(
    state: State<'_, AppState>,
    snapshot_id: u64,
    refs: Vec<ResolveRefInput>,
) -> Result<Vec<ResolveRefDto>, ApiError> {
    let req = ResolveRequest {
        snapshot_id: SnapshotId(snapshot_id),
        refs: refs
            .into_iter()
            .map(|r| ResolveRef {
                ref_id: r.ref_id,
                element: r.element,
            })
            .collect(),
    };
    let resolutions = {
        let guard = state.runtime.lock().await;
        let rt = guard
            .as_ref()
            .ok_or_else(|| ApiError::NoDocument("resolveRefs".into()))?;
        rt.resolve_refs(req).await?
    };
    Ok(resolutions
        .into_iter()
        .map(ResolveRefDto::from_resolution)
        .collect())
}

fn parse_sketch_id(s: &str) -> Result<SketchId, ApiError> {
    SketchId::from_str(s).map_err(|e| ApiError::InvalidCommand(format!("bad sketchId {s:?}: {e}")))
}

// ─────────────────────────────────────────────────────────────────────────────
// Start screen + native dialogs (Rust-side; webview has zero fs/dialog cap)
// ─────────────────────────────────────────────────────────────────────────────

/// Recent projects for the start screen, read from the persisted recents store at
/// `<app_config_dir>/recents.json` (a missing file ⇒ empty). Written on every
/// successful open/save by [`recents::record`].
#[tauri::command]
pub async fn list_recents(app: AppHandle) -> Result<Vec<RecentProjectDto>, ApiError> {
    Ok(recents::list(&app))
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

/// Shows a native STEP save dialog (`.step`/`.stp` filter). Resolves to the chosen
/// path or `None` on cancel. Mirrors [`pick_file`] but with the STEP filter.
async fn pick_step_save(app: AppHandle) -> Option<String> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("STEP", &["step", "stp"])
        .save_file(move |file: Option<tauri_plugin_dialog::FilePath>| {
            let _ = tx.send(file);
        });
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
/// snapshot published, the refreshed `projection-updated`, `regen-finished`
/// (`{revision, outcome}`) at the end of **every** regen so the frontend
/// correlation resolves promptly without the 8 s fallback (F-WP8 flag 3), and —
/// on a **published** regen — `needs-repair` (`{revision, items}`) so the repair
/// banner appears (items non-empty) or is dropped (items empty ⇒ repairs cleared;
/// M4a). A superseded/failed/no-op regen leaves the live repair state unchanged, so
/// no `needs-repair` is emitted for those.
pub fn emit_regen_events(app: &AppHandle, report: &RegenReport, projection: &DocumentProjection) {
    if let Some(change) = report.document_change() {
        let _ = app.emit(events::DOCUMENT_CHANGED, change);
    }
    let _ = app.emit(events::PROJECTION_UPDATED, projection);
    let _ = app.emit(
        events::REGEN_FINISHED,
        crate::dto::RegenFinished {
            revision: report.revision,
            outcome: report.outcome_str().to_string(),
        },
    );
    if report.published() {
        let _ = app.emit(
            events::NEEDS_REPAIR,
            crate::dto::NeedsRepairEvent {
                revision: report.revision,
                items: report.needs_repair.clone(),
            },
        );
    }
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

/// The current UTC time as an RFC-3339 string (`YYYY-MM-DDThh:mm:ssZ`).
fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    rfc3339_from_secs(secs)
}

/// An RFC-3339 string (`YYYY-MM-DDThh:mm:ssZ`) for `secs` since the Unix epoch,
/// computed without a calendar dependency (Howard Hinnant's civil-date algorithm).
/// Shared with the recents store (last-opened timestamps).
pub(crate) fn rfc3339_from_secs(secs: u64) -> String {
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
