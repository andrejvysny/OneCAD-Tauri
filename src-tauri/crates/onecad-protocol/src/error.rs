//! Shared error type for the OCW1 protocol crate.
//!
//! `ProtocolError` spans framing (magic/caps/EOF), JSON envelope handling, the
//! finite-float guard, and the async client transport. Framing violations are
//! fatal per `../../protocol/SCHEMA.md` §8 (no resync — the caller restarts the
//! worker); `BadMagic`/`TooLarge`/`ConnectionLost` are the load-bearing variants
//! the codec surfaces.

use thiserror::Error;

/// Errors produced anywhere on the OCW1 path.
#[derive(Debug, Error)]
pub enum ProtocolError {
    /// The 4 frame-magic bytes were not `OCW1`. Fatal per SCHEMA §1/§8: the
    /// reader tears down without resync. The magic is compared as BYTES, never as
    /// an endian-decoded integer.
    #[error("bad frame magic: expected {expected:02x?}, got {got:02x?}")]
    BadMagic {
        /// The normative expected bytes (`OCW1`).
        expected: [u8; 4],
        /// The bytes actually seen at the frame head.
        got: [u8; 4],
    },

    /// A declared frame section length exceeded its cap (`jsonLen` ≤ 16 MiB,
    /// `binLen` ≤ 1 GiB). Fatal `PROTOCOL_ERROR` (SCHEMA §1).
    #[error("frame {what} length {len} exceeds cap {cap}")]
    TooLarge {
        /// Which section (`"json"` or `"bin"`).
        what: &'static str,
        /// The declared length.
        len: u32,
        /// The cap it violated.
        cap: u32,
    },

    /// The stream ended (or errored) part-way through a frame, or the peer closed
    /// the connection. Pending requests fail with this.
    #[error("connection lost: {0}")]
    ConnectionLost(&'static str),

    /// A `NaN`/`±Infinity` float was rejected. Producers MUST NOT emit them
    /// (SCHEMA §4). `serde_json` also rejects them on serialize; this variant is
    /// for the explicit finite-check helper used during payload construction.
    #[error("non-finite float rejected (NaN/Inf are not allowed on the wire)")]
    NonFinite,

    /// A well-framed but protocol-illegal condition detected in-band (e.g. the
    /// first frame was not `hello`, or an unexpected protocol version).
    #[error("protocol violation: {0}")]
    Protocol(String),

    /// A Rust-side per-request deadline elapsed (SCHEMA §8 timeouts are
    /// Rust-enforced, never worker-enforced).
    #[error("request deadline exceeded")]
    Timeout,

    /// JSON (de)serialization of an envelope failed. A malformed envelope is a
    /// fatal `PROTOCOL_ERROR` (SCHEMA §8).
    #[error("json (de)serialization error: {0}")]
    Json(#[from] serde_json::Error),

    /// Underlying transport IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

impl ProtocolError {
    /// Construct a [`ProtocolError::BadMagic`] with the normative expected bytes.
    pub(crate) fn bad_magic(got: [u8; 4]) -> Self {
        ProtocolError::BadMagic {
            expected: crate::framing::MAGIC_BYTES,
            got,
        }
    }
}
