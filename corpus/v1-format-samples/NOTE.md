# v1 format samples

Copied verbatim from `OneCAD-CPP/tests/fixtures/onecad_v1/` at frozen commit
`b4ddcccc48134531f3ff80f11ddf9f42ad5a967e`.

## Files

| file | copied from | what it is |
|------|-------------|------------|
| `onecad_v1_README.md` | `tests/fixtures/onecad_v1/README.md` | original fixture README (renamed to avoid clashing with corpus README) |
| `history_ops_legacy_basic.jsonl` | same name | minimal legacy operation log — one op per line, NO `meta` objects (v1-only fields) |
| `history_state_legacy_basic.json` | same name | legacy timeline state: `cursor.appliedOpCount`, `cursor.lastAppliedOpId`, `suppressedOps` |

These are **hand-authored legacy payloads** used by `OneCAD-CPP`'s additive
read-compatibility prototypes (`proto_history_io_compat`,
`proto_document_roundtrip_compat`). They intentionally use only v1 fields to verify
forward-additive reads.

## Format version facts (for the later `.onecad` v1.1 importer)

Verified in `OneCAD-CPP/src/io/` at the frozen commit:

- **Container manifest** `FORMAT_VERSION = "1.1.0"` — `src/io/ManifestIO.h:24`.
- **Document schema** `SCHEMA_VERSION = "1.0.0"` — `src/io/ManifestIO.h:25`; also
  written as `schemaVersion: "1.0.0"` by `DocumentIO.cpp:123`, `ElementMapIO.cpp:247`,
  `SketchIO.cpp:113`.
- **Migration chain** registered `1.0.0 -> 1.1.0` — `src/io/MigrationRegistry.cpp:11`
  (H3 shipped: "wire MigrationRegistry + FORMAT_VERSION 1.1.0", `TODO.md:147`).
- **ElementMap serialization** header line is `ElementMap v1` — `ElementMap.h:1083`
  (`write()`) / `:1125` (`read()` rejects anything else). Text format: count, then
  per-entry `id, kind, opId, sources[], descriptor(14 fields), end`.
- **Container** = ZIP (`.onecad`) or directory package (`.onecadpkg`), uncompressed,
  JSON + OCCT BRep binary. Sections: `manifest.json`, `document.json`,
  `history/ops.jsonl`, `history/state.json`, `elementmap`, `sketches/` — per
  `OneCAD-CPP/CLAUDE.md` and `docs/FILE_FORMAT.md`.

**Legacy op field shape** (from `history_ops_legacy_basic.jsonl`): each op is
`{opId, type, inputs:{sketch|body:{…}}, params:{…}, resultBodyIds:[…]}`. Note the
legacy `inputs` wrapper (`inputs.sketch.{sketchId,regionId}`, `inputs.body.bodyId`)
and `params` without a `meta` object — the shape the future v1.1 importer must read
and migrate onto the new OperationRecord v2 (camelCase, adjacently-tagged
`{opType, params}`) described in the migration plan.

The new stack ships a **fresh v2 container**; the `.onecad` v1.1 importer is later
work (plan decision 3). These samples are the read-side conformance targets for that
importer's `MigrationRegistry` chain entry.
