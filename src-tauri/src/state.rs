//! Shared application state, managed as `tauri::State<AppState>`.

/// Root application state handed to every command. Fields (DocumentSession,
/// WorkerManager, mesh cache) land in later WPs; kept as a braced struct so the
/// `AppState::default()` construction site survives those additions.
#[derive(Debug, Default)]
pub struct AppState {}
