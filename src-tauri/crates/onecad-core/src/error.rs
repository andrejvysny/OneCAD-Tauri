//! Core error taxonomy.
//!
//! Note: `NeedsRepair` is a *state* (see [`crate::document::repair`]), never an
//! `Err`. Errors here are genuine failures (IO, decode, protocol, domain
//! invariant violations) only.

use thiserror::Error;

use crate::ids::RecordId;

/// Domain-level failures raised by document / timeline / edit operations
/// (migration plan, "Rust core specifics"). These are hard failures, distinct
/// from `NeedsRepair` state.
#[derive(Debug, Error)]
pub enum DomainError {
    /// A referenced timeline record does not exist.
    #[error("record not found: {0}")]
    RecordNotFound(RecordId),

    /// An operation input reference could not be resolved / is malformed.
    #[error("invalid reference: {0}")]
    InvalidReference(String),

    /// A dependency cycle was detected in the timeline graph.
    #[error("dependency cycle: {0}")]
    Cycle(String),

    /// A timeline / cursor invariant was violated (e.g. anti-time-travel).
    #[error("timeline violation: {0}")]
    Timeline(String),

    /// A record or parameter failed validation.
    #[error("validation failed: {0}")]
    Validation(String),

    /// A mutation was attempted on a read-only document (e.g. low-confidence
    /// migration forced read-only).
    #[error("document is read-only")]
    ReadOnly,
}

/// Errors surfaced by the core IO / container layer.
#[derive(Debug, Error)]
pub enum CoreError {
    /// Container / filesystem IO failure.
    #[error("io error: {0}")]
    Io(String),

    /// A document or record failed to decode / validate.
    #[error("invalid document: {0}")]
    InvalidDocument(String),

    /// A migration could not be applied with sufficient confidence.
    #[error("migration failed: {0}")]
    Migration(String),

    /// A domain invariant was violated.
    #[error(transparent)]
    Domain(#[from] DomainError),
}

/// Convenience result alias for the core IO layer.
pub type Result<T> = std::result::Result<T, CoreError>;
