//! OneCAD pure application core.
//!
//! Owns the authoritative document, linear timeline (+ derived dependency graph),
//! OperationRecord v2 schema (= file format), edit commands / undo, regen
//! planning + scheduling, ElementId identity / repair semantics, and v2 container
//! IO. All OCCT geometry is delegated to the C++ worker over `onecad-protocol`.
//!
//! This crate is deliberately free of any `tauri`/UI dependency (see Cargo.toml).
//! Everything here is a WP0 skeleton: documented types with no logic yet.

pub mod document;
pub mod edit;
pub mod error;
pub mod history;
pub mod ids;
pub mod io;
pub mod math;
pub mod regen;
pub mod selection;
pub mod sketch;
