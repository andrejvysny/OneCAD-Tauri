//! OneCAD IPC protocol crate.
//!
//! The source of truth for the wire contract is `../../protocol/SCHEMA.md`
//! (control JSON) and `../../protocol/mesh_format.md` (MESH1 bulk). This crate
//! provides the OCW1 framing codec, the JSON message enums, MESH1 header
//! validation, and (behind the `client` feature) the async [`client::ProtocolClient`].
//!
//! Layers:
//! - [`framing`] — OCW1 codec: pure `encode_frame`/`decode_frame`, blocking
//!   helpers, and the `tokio_util` `OcwCodec` (feature `client`).
//! - [`messages`] — the [`messages::Frame`] envelope enum + verb payloads.
//! - [`mesh`] — MESH1 header/section-table validation (`validate_mesh_blob`).
//! - [`client`] — the async `ProtocolClient` (feature `client`).

pub mod error;
pub mod framing;
pub mod mesh;
pub mod messages;

#[cfg(feature = "client")]
pub mod client;

pub use error::ProtocolError;
