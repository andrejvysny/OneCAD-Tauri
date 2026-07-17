//! Read/write of `document.json` — the authoritative document body.
//!
//! Thin glue over [`Document`]'s own serde ([`crate::document`]): serialize to
//! pretty JSON on write; on read, parse to a [`serde_json::Value`] **first** so the
//! migration chain ([`super::migrate`]) can transform older payloads *before*
//! typed deserialization, then deserialize the (possibly migrated) value into a
//! [`Document`]. All parse/decode failures surface as [`IoError::Corrupt`] — the
//! authoritative payload is either well-formed or the container is unusable.

use serde_json::Value;

use crate::document::Document;

use super::{IoError, IoResult};

/// Serializes a [`Document`] to the pretty-printed `document.json` bytes.
///
/// # Errors
/// [`IoError::Corrupt`] if serialization fails (should not happen for a valid
/// in-memory document; surfaced rather than panicking).
pub fn serialize_document(document: &Document) -> IoResult<Vec<u8>> {
    serde_json::to_vec_pretty(document)
        .map_err(|e| IoError::Corrupt(format!("document.json serialize: {e}")))
}

/// Parses raw `document.json` bytes into a [`serde_json::Value`].
///
/// Nesting depth is bounded by `serde_json`'s default recursion limit (128); a
/// deeper document returns an `Err` (mapped to [`IoError::Corrupt`]) rather than
/// overflowing the stack.
///
/// # Errors
/// [`IoError::Corrupt`] if the bytes are not valid JSON.
pub fn parse_value(bytes: &[u8]) -> IoResult<Value> {
    serde_json::from_slice(bytes).map_err(|e| IoError::Corrupt(format!("document.json parse: {e}")))
}

/// Deserializes a (possibly migrated) JSON value into a typed [`Document`].
///
/// # Errors
/// [`IoError::Corrupt`] if the value does not match the document schema.
pub fn document_from_value(value: Value) -> IoResult<Document> {
    serde_json::from_value(value)
        .map_err(|e| IoError::Corrupt(format!("document.json decode: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::DocumentId;
    use uuid::Uuid;

    #[test]
    fn round_trips_through_value() {
        let doc = Document::new(DocumentId(Uuid::from_u128(42)));
        let bytes = serialize_document(&doc).unwrap();
        let value = parse_value(&bytes).unwrap();
        let back = document_from_value(value).unwrap();
        assert_eq!(back.id, doc.id);
        assert_eq!(back.schema_version, doc.schema_version);
    }

    #[test]
    fn garbage_bytes_are_corrupt_not_panic() {
        assert!(matches!(
            parse_value(b"\xff\x00not json"),
            Err(IoError::Corrupt(_))
        ));
    }

    #[test]
    fn deeply_nested_json_errors_not_overflow() {
        // ~2000 levels of nesting — well past serde_json's default limit (128).
        let mut s = String::new();
        for _ in 0..2000 {
            s.push('[');
        }
        for _ in 0..2000 {
            s.push(']');
        }
        assert!(matches!(
            parse_value(s.as_bytes()),
            Err(IoError::Corrupt(_))
        ));
    }
}
