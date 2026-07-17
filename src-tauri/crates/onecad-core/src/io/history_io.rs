//! Read/write of the operation timeline projection (`timeline/ops.jsonl`) and the
//! `opsHash` cache-freshness token.
//!
//! `ops.jsonl` is a **derived, human-readable projection** of the authoritative
//! `document.json` timeline records — one canonical record JSON per line. It is
//! not a second source of truth: on load it is cross-validated against
//! `document.json` and, on any divergence, `document.json` wins and a `Warning`
//! diagnostic is emitted ([`cross_validate`]). See [`super`] module docs for the
//! rationale.

use serde_json::Value;

use crate::document::record::OperationRecord;

use super::{sha256_hex, Diagnostic, IoError, IoResult};

/// Computes the `opsHash`: SHA-256 (lowercase hex) of the **canonical**
/// timeline-records JSON.
///
/// Canonicalization routes the records through [`serde_json::Value`], whose object
/// maps are `BTreeMap`s (sorted keys) — so the hash is independent of field
/// emission order and stable across runs/platforms. This is the cache-freshness
/// token stored in the manifest: any change to the records (an edit, a migration)
/// changes the hash, marking every geometry/mesh/checkpoint cache stale
/// (Invariant 7 — a stale cache degrades performance, never correctness).
#[must_use]
pub fn ops_hash(records: &[OperationRecord]) -> String {
    let canonical = canonical_records_json(records);
    sha256_hex(canonical.as_bytes())
}

/// The canonical (sorted-key) JSON string of the records array.
fn canonical_records_json(records: &[OperationRecord]) -> String {
    // `to_value` cannot fail for these types; fall back to `[]` defensively rather
    // than panicking (this feeds a hash, never trusted geometry).
    let value = serde_json::to_value(records).unwrap_or(Value::Array(Vec::new()));
    serde_json::to_string(&value).unwrap_or_default()
}

/// Serializes records to `ops.jsonl` bytes: one canonical (sorted-key, compact)
/// record JSON per line, each line `\n`-terminated.
///
/// # Errors
/// [`IoError::Corrupt`] if a record fails to serialize (should not happen for a
/// valid in-memory record).
pub fn serialize_ops_jsonl(records: &[OperationRecord]) -> IoResult<Vec<u8>> {
    let mut out = Vec::new();
    for rec in records {
        let value = serde_json::to_value(rec)
            .map_err(|e| IoError::Corrupt(format!("ops.jsonl serialize: {e}")))?;
        let line = serde_json::to_string(&value)
            .map_err(|e| IoError::Corrupt(format!("ops.jsonl serialize: {e}")))?;
        out.extend_from_slice(line.as_bytes());
        out.push(b'\n');
    }
    Ok(out)
}

/// Parses `ops.jsonl` bytes into one [`serde_json::Value`] per non-blank line.
///
/// Kept at `Value` granularity (not typed `OperationRecord`) so a future record
/// shape never fails the projection read — cross-validation compares structurally.
///
/// # Errors
/// [`IoError::Corrupt`] if any non-blank line is not valid JSON.
pub fn parse_ops_jsonl(bytes: &[u8]) -> IoResult<Vec<Value>> {
    let text =
        std::str::from_utf8(bytes).map_err(|e| IoError::Corrupt(format!("ops.jsonl utf8: {e}")))?;
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|e| IoError::Corrupt(format!("ops.jsonl line {}: {e}", i + 1)))?;
        out.push(value);
    }
    Ok(out)
}

/// Cross-validates the derived `ops.jsonl` lines against the authoritative
/// `document.json` records. Returns a `Warning` [`Diagnostic`] on any divergence
/// (count or content), `None` when they agree.
///
/// Comparison is structural (`Value` equality, order-independent for object keys).
/// The caller always keeps `document.json`'s records — this only surfaces that the
/// readable projection was out of date / tampered.
#[must_use]
pub fn cross_validate(records: &[OperationRecord], ops_lines: &[Value]) -> Option<Diagnostic> {
    if records.len() != ops_lines.len() {
        return Some(Diagnostic::warning(
            "ops-jsonl-divergence",
            format!(
                "timeline/ops.jsonl has {} record(s) but document.json has {}; \
                 using document.json (authoritative)",
                ops_lines.len(),
                records.len()
            ),
        ));
    }
    for (i, (rec, line)) in records.iter().zip(ops_lines).enumerate() {
        let canonical = serde_json::to_value(rec).unwrap_or(Value::Null);
        if &canonical != line {
            return Some(Diagnostic::warning(
                "ops-jsonl-divergence",
                format!(
                    "timeline/ops.jsonl record {i} differs from document.json; \
                     using document.json (authoritative)"
                ),
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::record::{
        BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation,
    };
    use crate::document::variables::Scalar;
    use crate::ids::RecordId;
    use uuid::Uuid;

    fn extrude(seed: u128, distance: f64) -> OperationRecord {
        let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
            profile: None,
            distance: Scalar::new(distance),
            draft_angle_deg: Scalar::new(0.0),
            mode: ExtrudeMode::Blind,
            boolean_mode: BooleanMode::NewBody,
            target_body: None,
            target_face: None,
            two_directions: false,
            mode2: ExtrudeMode::Blind,
            distance2: Scalar::new(0.0),
            target_face2: None,
            extra: Default::default(),
        }));
        OperationRecord::new(RecordId(Uuid::from_u128(seed)), 0, "Extrude", op)
    }

    #[test]
    fn ops_hash_is_deterministic_and_content_sensitive() {
        let a = [extrude(1, 10.0), extrude(2, 5.0)];
        let b = [extrude(1, 10.0), extrude(2, 5.0)];
        assert_eq!(ops_hash(&a), ops_hash(&b));
        let c = [extrude(1, 10.0), extrude(2, 6.0)]; // distance changed
        assert_ne!(ops_hash(&a), ops_hash(&c));
        assert_eq!(ops_hash(&[]).len(), 64); // sha-256 hex
    }

    #[test]
    fn jsonl_round_trips_and_cross_validates() {
        let recs = vec![extrude(1, 10.0), extrude(2, 5.0)];
        let bytes = serialize_ops_jsonl(&recs).unwrap();
        assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 2);
        let lines = parse_ops_jsonl(&bytes).unwrap();
        assert_eq!(lines.len(), 2);
        assert!(cross_validate(&recs, &lines).is_none());
    }

    #[test]
    fn cross_validate_flags_divergence() {
        let recs = vec![extrude(1, 10.0), extrude(2, 5.0)];
        let lines = parse_ops_jsonl(&serialize_ops_jsonl(&recs).unwrap()).unwrap();
        // Drop a line → count mismatch.
        let diag = cross_validate(&recs, &lines[..1]).unwrap();
        assert_eq!(diag.code, "ops-jsonl-divergence");
        // Content mismatch.
        let tampered =
            parse_ops_jsonl(&serialize_ops_jsonl(&[extrude(1, 10.0), extrude(2, 99.0)]).unwrap())
                .unwrap();
        assert!(cross_validate(&recs, &tampered).is_some());
    }

    #[test]
    fn parse_skips_blank_lines_and_errors_on_garbage() {
        let ok = parse_ops_jsonl(b"{\"a\":1}\n\n{\"b\":2}\n").unwrap();
        assert_eq!(ok.len(), 2);
        assert!(matches!(
            parse_ops_jsonl(b"{not json}\n"),
            Err(IoError::Corrupt(_))
        ));
    }
}
