//! Tauri command handlers — the webview → Rust API surface.
//!
//! Commands (open/save, apply_command, get_mesh, acquire_element_ids, …) are
//! registered in [`crate::run`]'s `invoke_handler` and land in later WPs.
