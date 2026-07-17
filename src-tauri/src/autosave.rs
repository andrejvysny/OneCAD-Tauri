//! Autosave + crash-recovery marker driver (app-side).
//!
//! Schedules periodic autosave (~2 min) and writes a pid crash marker so the
//! next launch can offer recovery. Wraps [`onecad_core::io::recovery`]. Fields
//! land in a later WP.

/// Drives periodic autosave and the crash-recovery marker. Stub for now.
#[derive(Debug, Default)]
pub struct Autosave;
