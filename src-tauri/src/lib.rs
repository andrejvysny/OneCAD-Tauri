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
pub mod mesh_cache;
pub mod state;
pub mod worker;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tokio::sync::Mutex;

use onecad_core::regen::{Outcome, RegenDirective, RegenScheduler};

use crate::document_runtime::DocumentRuntime;
use crate::state::AppState;

/// The boxed future the regen driver hands the scheduler (the driver closure is a
/// distinct type per call, so it is type-erased for the scheduler's `select!`).
type BoxFut = Pin<Box<dyn Future<Output = Outcome> + Send>>;

/// Builds the app-layer regen driver (plan "app layer runs the executor"): for one
/// [`RegenDirective`] it locks the single-writer runtime, drives the executor via
/// [`DocumentRuntime::run_regen`], and emits the post-regen `document-changed` +
/// `projection-updated` events. Debounce/coalesce/preview-priority stay in the
/// [`RegenScheduler`] (policy only); this is the executor side of the seam.
fn make_regen_driver(
    runtime: Arc<Mutex<Option<DocumentRuntime>>>,
    app: AppHandle,
) -> impl Fn(RegenDirective) -> BoxFut + Send + Sync + 'static {
    move |directive: RegenDirective| {
        let runtime = runtime.clone();
        let app = app.clone();
        Box::pin(async move {
            let (report, projection) = {
                let mut guard = runtime.lock().await;
                let Some(rt) = guard.as_mut() else {
                    return Outcome::NoOp; // document closed while the job was queued.
                };
                let report = rt.run_regen(directive.request, directive.cancel).await;
                let projection = rt.projection();
                (report, projection)
            };
            api::emit_regen_events(&app, &report, &projection);
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
            let driver = make_regen_driver(state.runtime.clone(), app.handle().clone());
            let (scheduler, handle) = RegenScheduler::new(driver);
            tauri::async_runtime::spawn(scheduler.run());
            let _ = state.scheduler.set(handle);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api::new_document,
            api::open_document,
            api::import_step,
            api::save_document,
            api::close_document,
            api::apply_edit_command,
            api::undo,
            api::redo,
            api::get_projection,
            api::get_mesh,
            api::list_recents,
            api::open_file_dialog,
            api::save_file_dialog,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
