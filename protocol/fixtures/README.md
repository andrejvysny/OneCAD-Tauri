# Protocol Fixtures

Executable form of [`../SCHEMA.md`](../SCHEMA.md) and
[`../mesh_format.md`](../mesh_format.md). Both sides run these in CI: the Rust
`onecad-protocol` harness drives the C++ `onecad-worker` (and the
`onecad-worker-stub`) through each fixture and asserts the exchange. A protocol
change is not done until its fixtures are updated (fixture bump — see
[`SCHEMA.md §13`](../SCHEMA.md#13-versioningchange-policy)).

## File format

A fixture is an **NDJSON** file (`*.ndjson`): one JSON object per line, executed
top to bottom. Blank lines and lines whose first non-space char is `#` are
ignored. Each object is exactly one of the directives below, discriminated by its
sole leading key.

### `send` — driver → worker

```json
{ "send": { "v": 1, "t": "req", "id": 1, "verb": "OpenSession", "args": { … } } }
```

The value is a full frame envelope ([`SCHEMA.md §3`](../SCHEMA.md#3-envelope-shapes)).
The harness serializes the JSON, frames it as OCW1, and writes it. `t` may be
`req`, `cancel`, `credit` (driver-originated frames only). Binary sections are
attached via `@file` (below).

### `expect` — match the next worker frame

```json
{ "expect": { "t": "resp", "id": 1, "ok": true, "result": { "sessionOpen": true } } }
```

The harness reads the next worker frame and asserts a **subset match**: every key
present in the matcher must be present and equal in the actual frame; keys absent
from the matcher are not checked (so worker-assigned stamps like `seq`,
`snapshotId` need not be enumerated). Arrays match element-by-element, each element
subset-matched.

Matcher leaf values:

| Value | Meaning |
|-------|---------|
| literal (string/number/bool/null) | must equal (numbers per tolerance below) |
| `"$any"` | key must be present, any value |
| `"$capture:<name>"` | must be present; bind its value to `<name>` for later `$ref` |
| `"$ref:<name>"` | must equal a previously captured `<name>` |
| `"$hex64"` | must be a 16-char lowercase-hex string (a 64-bit hash) |
| `"$hex256"` | must be a 64-char lowercase-hex string (a SHA-256) |

Ordering: `expect` consumes exactly one worker frame. To skip non-terminal frames
(`progress`, `event`, `chunk`) use `expectAny`/`drain` (below) or match them
explicitly in order.

### Float tolerance

Numeric leaves are compared with tolerance. Default is **exact** (`abs 0`). A
fixture may relax it per file or per `expect`:

```json
{ "tolerance": { "abs": 1e-9, "rel": 1e-6 } }
```

- As a standalone directive line it sets the file default from that point on.
- As a key inside an `expect` object it applies to that matcher only.
- A numeric leaf matches iff `|actual − expected| ≤ max(abs, rel · |expected|)`.
- `-0` and `0` compare equal; `NaN`/`Inf` in an actual frame is always a failure
  (they are rejected on the wire).

### `@file` — binary payload reference

Inside a `send` frame's `bin` section, replace inline bytes with a file reference
resolved relative to the fixture file:

```json
{ "send": { "v":1, "t":"req", "id":5, "verb":"LoadBodies",
  "args": { "bodies": [ { "bodyId":"body_1", "bin":"brep:body_1", "brepContentHash":"$hex64" } ] },
  "bin": [ { "name":"brep:body_1", "@file":"blobs/cube.brep" } ] } }
```

The harness loads `blobs/cube.brep`, places it in the binary tail, and fills the
section `off`/`len`. `@file` is mutually exclusive with an inline byte array.

### Binary assertions on worker output

Worker frames carrying binary (inline `bin` or a reassembled bulk stream) are
asserted by SHA-256, not by byte inlining:

```json
{ "expect": { "t":"resp", "id":6, "ok":true,
  "result": { "bodies": [ { "bodyId":"body_1", "streamId":"$any", "sha256":"$hex256" } ] },
  "binSha256": { "brep:body_1": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" } } }
```

`binSha256` maps a section name (inline) or a `streamId`/mesh key (bulk) to the
expected SHA-256 of its reassembled bytes. For a bulk stream the harness first
reassembles per [`SCHEMA.md §5.2`](../SCHEMA.md#52-chunked-bulk-streams) (verifying
the manifest `sha256`), then checks `binSha256`.

### `drain` — consume non-terminal frames

```json
{ "drain": { "until": { "t": "resp", "id": 1 }, "collect": ["event", "progress"] } }
```

Reads and (optionally) records frames until one subset-matches `until`; that
terminal frame is left for the following `expect`. Use for `ExecutePlan`, whose
`planStep` events precede the `PlanPrepared` resp.

## Determinism rules for fixtures

- Driver-assigned `id` values are fixed in the fixture. Worker-assigned `seq`,
  `snapshotId`, `documentRevision`, `streamId`, `jobId` are `$any`/`$capture`
  (never hard-coded to a literal unless the value is contractually fixed, e.g.
  the initial `hello` has `seq: 0`).
- Hashes use `$hex64`/`$hex256` unless a fixture pins an exact golden hash for
  determinism-suite coverage.
- One fixture = one connected session unless it explicitly `send`s
  `ResetSession`/`CloseSession`.

## Example fixtures in this directory

| File | Covers |
|------|--------|
| [`hello.ndjson`](./hello.ndjson) | handshake happy path: unsolicited `hello` with all version fields, then `OpenSession` |
| [`echo_error.ndjson`](./echo_error.ndjson) | unknown verb → `PROTOCOL_ERROR` terminal `resp` shape |

Planned (per plan M0/M1 co-sign gate): `capabilities`, `cancellation`,
`malformed_frame`, `chunked_mesh`, `crash_restart`, `execute_plan_prefix_atomic`,
`needs_repair_symmetric`.
