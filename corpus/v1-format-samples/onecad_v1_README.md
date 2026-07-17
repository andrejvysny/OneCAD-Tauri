# v1 Compatibility Fixtures

This folder stores hand-authored legacy `history` payloads used by compatibility prototypes.

- `history_ops_legacy_basic.jsonl`: Minimal legacy operation log without `meta` objects.
- `history_state_legacy_basic.json`: Legacy state file with cursor + suppression fields.

These fixtures intentionally use only v1 fields to verify additive read compatibility.
