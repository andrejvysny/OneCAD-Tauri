//! OneCAD Tauri application shell (crate `onecad`, lib `onecad_lib`).
//!
//! Thin host around [`onecad_core`]: owns the webview, the worker manager, and
//! the command/event surface. All filesystem/dialog IO happens in Rust (the
//! webview has zero shell/fs capabilities); see `capabilities/default.json`.

pub mod api;
pub mod autosave;
pub mod error;
pub mod events;
pub mod state;
pub mod worker;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Logs go to stderr; stdout is reserved for worker OCW1 frames downstream.
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::default())
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
