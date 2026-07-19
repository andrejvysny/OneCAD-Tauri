//! OneCAD Tauri application shell (crate `onecad`, lib `onecad_lib`).
//!
//! Thin host around [`onecad_core`]: owns the webview, the geometry-backend seam,
//! and the command/event surface. All filesystem/dialog IO happens in Rust (the
//! webview has zero shell/fs capabilities; capabilities stay `core:default`).
//!
//! Layers:
//! * [`document_runtime`] — the per-document single writer (all domain logic).
//! * [`state`] — [`AppState`](state::AppState): the managed runtime + scheduler
//!   handle + backend factory.
//! * [`api`] — thin `#[tauri::command]` wrappers that delegate to the runtime.
//! * [`worker`] — the [`GeometryEngine`](onecad_core::regen::GeometryEngine) +
//!   [`MeshProvider`](worker::MeshProvider) seam R-WP11 plugs the real sidecar
//!   into, plus the D1 [`AdoptingEngine`](worker::AdoptingEngine).
//! * [`dto`]/[`events`] — the camelCase projection DTOs + event channel names.

pub mod api;
pub mod autosave;
pub mod document_runtime;
pub mod dto;
pub mod error;
pub mod events;
pub mod export;
pub mod mesh_cache;
pub mod recents;
pub mod state;
pub mod worker;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{watch, Mutex};

use onecad_core::regen::{Outcome, RegenDirective, RegenScheduler};

use crate::document_runtime::DocumentRuntime;
use crate::state::AppState;

/// The boxed future the regen driver hands the scheduler (the driver closure is a
/// distinct type per call, so it is type-erased for the scheduler's `select!`).
type BoxFut = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// Builds the app-layer regen driver (plan "app layer runs the executor").
///
/// **Fencing goes live (R-WP11):** the driver holds the single-writer lock only
/// for the two short phases that mutate the document — phase 1
/// ([`begin_regen`](DocumentRuntime::begin_regen): compile the plan + clone a
/// scratch session) and phase 3 ([`finish_regen`](DocumentRuntime::finish_regen):
/// commit or supersede). The slow worker IO (phase 2,
/// [`PreparedRegen::drive`](crate::document_runtime::PreparedRegen::drive)) runs
/// with the lock **released**, so an edit can land during it, advance the fencing
/// tokens, and supersede the stale prepare via the executor's revision gate.
/// Debounce/coalesce/preview-priority stay in the [`RegenScheduler`] (policy only).
fn make_regen_driver(
    runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    app: AppHandle,
    autosave_tick: Arc<watch::Sender<u64>>,
) -> impl Fn(RegenDirective) -> BoxFut + Send + Sync + 'static {
    move |directive: RegenDirective| {
        let runtime = runtime.clone();
        let app = app.clone();
        let autosave_tick = autosave_tick.clone();
        Box::pin(async move {
            // Phase 1 (locked): compile the plan + clone the scratch session.
            let prepared = {
                let mut guard = runtime.lock().await;
                let Some(rt) = guard.as_mut() else {
                    return Outcome::NoOp; // document closed while the job was queued.
                };
                rt.begin_regen(directive.request)
            };
            let Some(prepared) = prepared else {
                return Outcome::NoOp; // empty plan.
            };
            // Phase 2 (UNLOCKED): drive the worker; concurrent edits may supersede.
            let driven = prepared.drive(directive.cancel).await;
            // Phase 3 (locked): commit iff still current, then emit events.
            let (report, projection) = {
                let mut guard = runtime.lock().await;
                let Some(rt) = guard.as_mut() else {
                    return Outcome::NoOp; // document closed during the worker IO.
                };
                let report = rt.finish_regen(driven);
                let projection = rt.projection();
                (report, projection)
            };
            api::emit_regen_events(&app, &report, &projection);
            // A published regen produced new geometry outputs worth autosaving
            // (the debounce coalesces the edit-tick + this publish-tick).
            if report.published() {
                autosave_tick.send_modify(|v| *v = v.wrapping_add(1));
            }
            report.outcome
        })
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Logs go to stderr; stdout is reserved for worker OCW1 frames downstream.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
            // Spawn the single regen scheduler over the shared runtime + app handle.
            let state = app.state::<AppState>();
            // Publish the app handle so the backend factory's worker-status
            // forwarder can emit events (the factory is built before this exists).
            let _ = state.app.set(app.handle().clone());
            let driver = make_regen_driver(
                state.runtime.clone(),
                app.handle().clone(),
                state.autosave_tick.clone(),
            );
            let (scheduler, handle) = RegenScheduler::new(driver);
            tauri::async_runtime::spawn(scheduler.run());
            let _ = state.scheduler.set(handle);
            // Spawn the debounced autosave driver over the shared runtime + app-data
            // root. Each autosave emits `events::AUTOSAVE {path, atMs}`. A headless /
            // pathless environment (no app-data dir) simply skips autosave.
            if let Some(app_data) = autosave::autosave_root(app.handle()) {
                let runtime = state.runtime.clone();
                let tick = state.autosave_tick.subscribe();
                let emitter = app.handle().clone();
                tauri::async_runtime::spawn(autosave::run(
                    runtime,
                    app_data,
                    tick,
                    autosave::AUTOSAVE_DEBOUNCE,
                    move |ev| {
                        let _ = emitter.emit(events::AUTOSAVE, &ev);
                    },
                ));
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api::new_document,
            api::open_document,
            api::import_step,
            api::save_document,
            api::export_step_file,
            api::export_stl_file,
            api::export_obj_file,
            api::close_document,
            api::check_recovery,
            api::recover_document,
            api::apply_edit_command,
            api::undo,
            api::redo,
            api::get_projection,
            api::get_mesh,
            api::enter_sketch,
            api::sketch_upsert,
            api::begin_gesture,
            api::solve_drag,
            api::end_gesture,
            api::cancel_sketch,
            api::finish_sketch,
            api::promote_selection,
            api::resolve_refs,
            api::list_recents,
            api::open_file_dialog,
            api::save_file_dialog,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
