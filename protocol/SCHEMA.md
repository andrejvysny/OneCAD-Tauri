# OneCAD Worker Protocol — Wire Contract (SCHEMA)

Status: canonical. Protocol version `1`. Both the C++ sidecar (`worker/`) and the
Rust core (`src-tauri/crates/onecad-protocol`) implement against this document
verbatim. Any change requires a fixture bump + cross-track sign-off (see
[§13 Versioning](#13-versioningchange-policy)).

The transport is **stdio between the Rust core (parent) and one C++ worker
process (child)**. `stdout` carries frames only; all logs go to `stderr`
(grep-gated). There is **no JavaScript on this path** — only `serde_json` (Rust)
and `nlohmann::json` (C++) parse envelopes — so `u64` integers are safe as JSON
numbers (both round-trip `u64` losslessly). The frontend never sees a raw
envelope; it receives projection DTOs from Rust.

All multi-byte integers and floats are **little-endian**.

Reference: this contract realizes the decisions in the migration plan
`~/.claude/plans/act-as-senior-software-transient-popcorn.md` ("Key protocol
decisions", "Architecture (final)"). The 7 invariants in [§11](#11-invariants)
are the correctness spine; every verb below is defined so as not to violate them.

---

## Table of contents

1. [Frame layout (OCW1)](#1-frame-layout-ocw1)
2. [Identifier & scalar types](#2-identifier--scalar-types)
3. [Envelope shapes](#3-envelope-shapes)
4. [JSON encoding rules](#4-json-encoding-rules)
5. [Logical lanes, chunking & flow control](#5-logical-lanes-chunking--flow-control)
6. [Handshake](#6-handshake)
7. [Verb catalogue](#7-verb-catalogue)
8. [Error taxonomy](#8-error-taxonomy)
9. [NeedsRepair payload](#9-needsrepair-payload)
10. [Resolution ladder](#10-resolution-ladder)
11. [Invariants](#11-invariants)
12. [Signatures](#12-signatures)
13. [Versioning/change policy](#13-versioningchange-policy)
14. [Changelog](#14-changelog)

---

## 1. Frame layout (OCW1)

Every message — control or bulk — is one **frame**:

```
offset  size            field
0        4 bytes        magic   = "OCW1" = 0x4F 0x43 0x57 0x31  (u32 LE 0x3157434F*)
4        4 bytes        jsonLen (u32 LE)   length of the JSON envelope in bytes
8        4 bytes        binLen  (u32 LE)   length of the binary tail in bytes
12       jsonLen bytes  json    UTF-8 JSON envelope (no BOM, no trailing NUL)
12+jsonLen  binLen bytes binary tail (raw bytes; addressed by the envelope "bin" table)
```

\* The magic is the ASCII bytes `O C W 1` in stream order. Written/read as the
4-byte sequence `4F 43 57 31`. Implementations MUST compare the 4 bytes, not an
endian-decoded integer, to avoid endianness confusion. (`"OCW1"` as a `u32` read
little-endian is `0x3157434F`; read big-endian it is `0x4F435731`. The byte
sequence is authoritative.)

### Caps

- `jsonLen` ≤ **16 MiB** (`16 * 1024 * 1024 = 16777216`).
- `binLen` ≤ **1 GiB** (`1024 * 1024 * 1024 = 1073741824`).

A frame that declares a length over cap is a fatal `PROTOCOL_ERROR`. There is **no
resync** after a malformed frame: the reader stops, the connection is torn down,
and the worker is **restarted** (see [§8](#8-error-taxonomy)). Readers MUST NOT
attempt to scan forward for the next magic.

### Binary tail addressing

The binary tail is a flat byte region. Named sections inside it are described by a
`bin` array in the JSON envelope:

```json
"bin": [
  { "name": "mesh:body_3", "off": 0,      "len": 524288 },
  { "name": "brep:body_3", "off": 524288, "len": 91234 }
]
```

- `off` and `len` are byte offsets/lengths **relative to the start of the binary
  tail** (i.e. relative to byte `12 + jsonLen`). Both `u32`.
- Sections MUST NOT overlap and MUST lie within `[0, binLen)`.
- Section `name` is a UTF-8 string, unique within the frame.
- The order of the `bin` array is not significant; addressing is by `off`/`len`.
- The concatenation of all sections need not cover the whole tail (gaps for
  4-byte alignment are permitted); readers address only named sections.

A frame with no binary payload sets `binLen = 0` and omits `bin` (or sets `bin:
[]`).

---

## 2. Identifier & scalar types

| Type | Wire form | Notes |
|------|-----------|-------|
| `id` | JSON integer (`u64`) | Correlation id. **Rust-assigned, strictly monotonic** per connection. One request → one terminal `resp` with the same `id`. |
| `seq` | JSON integer (`u64`) | Worker's global output sequence number. Monotonic across **every** frame the worker emits on the connection. Lets Rust detect drops/reordering. |
| `documentRevision` | JSON integer (`u64`) | **Rust-owned document revision (an edit counter); ADVISORY stamp, NOT a fencing token (D4).** The worker MUST NOT reject a request on `documentRevision` (a post-edit regen legitimately runs ahead of the worker's last-accepted head). It is carried in `ExecutePlan.args`, stored in the prepared scratch, and **adopted** as the worker head's `documentRevision` at `AcceptPrepared` (worker frame stamps thereafter echo it). See [§7.2](#72-regen--executeplan). |
| `workerEpoch` | JSON integer (`u64`) | Incremented by Rust on every worker (re)start / `ResetSession`. **Fencing token.** |
| `snapshotId` | JSON integer (`u64`) | Identifies one published geometry snapshot. Bodies/maps/signatures/meshes of one publish share it (Invariant 4). |
| `jobId` | JSON integer (`u64`) | Rust-assigned id for one `ExecutePlan` job. Idempotent: re-sending the same `jobId` is a no-op if already prepared. |
| `sketchRevision` | JSON integer (`u64`) | Rust-owned sketch revision. |
| `gestureId` | JSON integer (`u64`) | Rust-assigned drag-gesture id. |
| `streamId` | JSON integer (`u64`) | Worker-assigned bulk-stream id, unique per connection. |
| `BodyId` | JSON string | Opaque, globally unique (e.g. `"body_7"`). **Minting is split (D1):** a **NewBody** body is **worker-minted deterministic** `body_<opId>` (the `opId` is the Rust-minted op record id, so replay is stable); a future split mints `body_<opId>:<k>` with deterministic `k`-ordering. Rust **adopts** these ids from `planStep` `bodyEvents` at `AcceptPrepared` time, validating format (`body_` prefix + a known `opId` in the plan) and uniqueness, and **rejects** the prepared plan (`PROTOCOL_ERROR`, discard — never publish) on malformation/collision. All *other* body ids (loaded/imported bodies) stay Rust-minted. See [§7.2](#72-regen--executeplan). |
| `ElementId` | JSON string | Opaque, Rust-minted, **globally unique and DOES NOT embed BodyId** (e.g. `"el_00000000000004a1"`). Partition membership (which body an element belongs to) is a *mapping*, never encoded in the id. |
| `TopoKey` | JSON string | **Snapshot-scoped** topology address: `"<kind>:<index>"`, kind ∈ `f` (face) / `e` (edge) / `v` (vertex) / `b` (body). Example `"f:22"`. Valid only within the `snapshotId` that produced it. NEW scheme (OneCAD-CPP used path-style ids; this protocol uses compact snapshot-scoped TopoKeys promoted on demand to `ElementId`). |
| hash | JSON string, lowercase hex, no `0x` | 64-bit hash → 16 hex chars (e.g. `"cbf29ce484222325"`). SHA-256 → 64 hex chars. Applies to `expectedBaseHash`, `historyPrefixHash`, all signatures, `brepContentHash`, `contentHash`, `tolerancePolicyHash`, `solverPolicyHash`, `occtFingerprint`, chunk `sha256`. |
| coordinate / scalar geometry | JSON number (`f64`) | Subject to [§4](#4-json-encoding-rules) float rules. |

**Fencing (D4).** A session-mutating request is fenced on **`workerEpoch` +
`expectedBaseHash` ONLY**: the worker rejects with `PROTOCOL_ERROR` (Rust reconciles
via `GetWorkerHead`) when the request's `workerEpoch` does not match the head epoch,
or its `expectedBaseHash` does not equal the head `historyPrefixHash`.
`documentRevision` is **NOT** a fencing token — it is a Rust-owned advisory stamp
(an edit counter) that the worker stores and **adopts** as its head at
`AcceptPrepared` (see [§7.2](#72-regen--executeplan)). Fencing on it would reject
every legitimate post-edit regen, whose `documentRevision` runs ahead of the
worker's last-accepted head.

---

## 3. Envelope shapes

Every envelope is a JSON object with `v` (protocol version, `1`) and `t` (frame
type). Types: `hello`, `req`, `resp`, `progress`, `event`, `cancel`, `credit`,
`chunk`.

Frames **originating from the worker** (`resp`, `progress`, `event`, `chunk`)
carry a **stamp**: `documentRevision`, `workerEpoch`, `snapshotId`, `seq`, and
`jobId` where a job is in flight. Frames from Rust (`req`, `cancel`, `credit`)
never carry the stamp.

### 3.1 `req` (Rust → worker)

```json
{
  "v": 1,
  "t": "req",
  "id": 42,
  "verb": "Tessellate",
  "lane": "control",
  "args": { "...": "verb-specific" },
  "bin": []
}
```

- `lane`: `"control"` (default) or `"bulk"`. Omitted ⇒ `"control"`.
- `bin`: optional; present when the request carries a binary payload (e.g.
  `LoadBodies`).

### 3.2 `resp` (worker → Rust, terminal — exactly one per request `id`)

```json
{
  "v": 1,
  "t": "resp",
  "id": 42,
  "ok": true,
  "result": { "...": "verb-specific" },
  "documentRevision": 17,
  "workerEpoch": 3,
  "snapshotId": 5012,
  "jobId": 88,
  "seq": 921,
  "bin": []
}
```

On failure:

```json
{
  "v": 1, "t": "resp", "id": 42, "ok": false,
  "error": { "code": "OP_FAILED", "message": "…", "detail": { }, "retriable": false },
  "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012, "seq": 922
}
```

Exactly one terminal `resp` is emitted per request `id`. `ok` MUST be present.
`result` present iff `ok:true`; `error` present iff `ok:false`. `jobId` present
only where a job is associated.

### 3.3 `progress` (worker → Rust, non-terminal)

```json
{
  "v": 1, "t": "progress", "id": 42,
  "phase": "tessellating", "fraction": 0.4, "message": "body 2/5",
  "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012, "seq": 900
}
```

`fraction` ∈ `[0,1]` optional. Progress frames are informational and MUST NOT be
required for correctness.

### 3.4 `event` (worker → Rust, non-terminal)

Structured, correlation-scoped domain events. Used by `ExecutePlan` for per-step
results (see [§7.2](#72-regen--executeplan)).

```json
{
  "v": 1, "t": "event", "id": 42, "event": "planStep",
  "stepIndex": 3, "payload": { "...": "event-specific" },
  "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012, "jobId": 88, "seq": 905
}
```

### 3.5 `cancel` (Rust → worker)

```json
{ "v": 1, "t": "cancel", "id": 42 }
```

Cancels the in-flight request `id`. The worker MUST still emit a terminal `resp`
for `id` with `error.code = "CANCELLED"` (cancellation is cooperative; the
terminal frame is **never dropped**). If `id` is already complete, `cancel` is a
no-op.

### 3.6 `credit` (Rust → worker) — bulk flow control

```json
{ "v": 1, "t": "credit", "lane": "bulk", "bytes": 4194304 }
```

Grants `bytes` of additional bulk-lane byte budget. See
[§5.3](#53-byte-budget-flow-control).

### 3.7 `chunk` (worker → Rust) — bulk stream frame

Two kinds, discriminated by `kind`:

Manifest (first frame of a stream):

```json
{
  "v": 1, "t": "chunk", "id": 42, "streamId": 700, "kind": "manifest",
  "purpose": "mesh", "count": 8, "totalBytes": 4194304,
  "sha256": "…64 hex…", "meta": { "bodyId": "body_3", "lod": "coarse", "format": "MESH1" },
  "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012, "jobId": 88, "seq": 906
}
```

Data (frames `index` 0…`count`-1):

```json
{
  "v": 1, "t": "chunk", "id": 42, "streamId": 700, "kind": "data",
  "index": 0, "byteOffset": 0,
  "bin": [ { "name": "chunk", "off": 0, "len": 524288 } ],
  "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012, "jobId": 88, "seq": 907
}
```

The receiver assembles the payload by `byteOffset`, verifies assembled length ==
`totalBytes` and SHA-256 == `sha256`, then hands the buffer off. `purpose` ∈
`"mesh"` | `"brep"`. See [§5.2](#52-chunked-bulk-streams).

---

## 4. JSON encoding rules

- **camelCase** for all object keys and enum-tag string values (e.g. `opType`,
  `documentRevision`, `"throughAll"` — but see op `opType`/`kind` tags which keep
  their PascalCase spelling as domain type names, e.g. `"Extrude"`,
  `"ThroughAll"`, `"Union"`, matching OneCAD-CPP `operationTypeName`).
- **64-bit hashes are hex strings**, never numbers ([§2](#2-identifier--scalar-types)).
- **`NaN`, `+Infinity`, `-Infinity` are rejected** on read → `PROTOCOL_ERROR`.
  Producers MUST NOT emit them.
- **`-0` is normalized to `0`** by producers; readers treat `-0.0` and `0.0` as
  equal.
- No trailing whitespace requirement; parsers MUST accept minified JSON.
- Unknown object keys are **ignored** by readers (forward-compat), except inside a
  frame header (`v`,`t`,`id`,`ok`) where they are errors. Op params carry unknown
  keys forward (Rust `flatten extra`); the worker ignores keys it does not know.
- Duplicate keys in one object → `PROTOCOL_ERROR`.
- Integers that exceed their declared width (`u64`) → `PROTOCOL_ERROR`.

---

## 5. Logical lanes, chunking & flow control

### 5.1 Lanes

Two **logical** lanes multiplex over the single stdio frame stream:

- **control** — requests, responses, progress, events, cancel, credit,
  handshake, all diagnostics and NeedsRepair state. Never blocked by flow
  control. Small, latency-sensitive.
- **bulk** — chunk streams carrying MESH1 meshes and BREP blobs. Subject to
  byte-budget credit.

Because meshes/BREP are **chunked**, control frames (cancel, progress, credit)
**interleave** with bulk frames: a cancel or a solver response is never stuck
behind a 50 MB mesh. A worker that has bulk data to send MUST yield the writer
between bulk frames so pending control frames go out first.

### 5.2 Chunked bulk streams

A bulk payload is transmitted as one stream: a **manifest** frame followed by
`count` **data** frames ([§3.7](#37-chunk-worker--rust--bulk-stream-frame)). The
terminal `resp` of the producing verb references the stream(s) it emitted, e.g.:

```json
"result": { "meshes": [ { "bodyId": "body_3", "streamId": 700,
  "format": "MESH1", "totalBytes": 4194304, "sha256": "…" } ] }
```

The worker MAY inline a small bulk payload (≤ negotiated `chunkSize`, default
**1 MiB**) directly in the terminal `resp`'s binary tail instead of opening a
stream; in that case the `resp` result references a `bin` section name rather than
a `streamId`. Payloads larger than `chunkSize` MUST be chunked so control frames
interleave.

Stream integrity: the manifest's `sha256` is the SHA-256 of the concatenated
payload bytes (all data frames in `byteOffset` order). A mismatch is a
`PROTOCOL_ERROR` → restart.

### 5.3 Byte-budget flow control

Bulk data flows worker → Rust. Rust grants credit; the worker spends it:

- Rust sends `credit{lane:"bulk", bytes:N}` control frames.
- The worker MUST NOT have more than the outstanding-credit total of **bulk
  payload bytes** (sum of data-frame `bin` lengths) in flight beyond what has been
  granted. When credit is exhausted it pauses the bulk stream (control frames keep
  flowing).
- Rust replenishes credit as it consumes/assembles. Initial credit is granted at
  handshake (`initialBulkCredit`, default **8 MiB**).
- Manifest and control frames do **not** consume bulk credit.

### 5.4 Never-dropped classes

Cancellation acknowledgements, terminal `resp` frames (including error terminals),
and NeedsRepair state are control-lane and MUST NEVER be dropped or coalesced away
by flow control or backpressure.

---

## 6. Handshake

Immediately after spawn, the worker emits an unsolicited `hello` frame (it is the
only worker frame with `t:"hello"` and no request `id`). Rust reads it before
sending any `req`.

Worker → Rust:

```json
{
  "v": 1,
  "t": "hello",
  "seq": 0,
  "result": {
    "protocolVersion": 1,
    "workerVersion": "0.1.0",
    "occt": { "version": "7.9.3", "fingerprint": "9a1c33f0e7b24d10" },
    "quantizationVersion": 1,
    "solverPolicyVersion": 1,
    "capabilities": [
      "op.sketch", "op.extrude", "op.revolve", "op.fillet", "op.chamfer",
      "op.boolean", "solver.planegcs", "tessellate.mesh1", "io.step",
      "io.stl", "io.obj", "checkpoint.v1"
    ],
    "limits": { "chunkSize": 1048576, "initialBulkCredit": 8388608 }
  }
}
```

Rust verifies `protocolVersion == 1` and applies the fingerprint policy
(migration plan; V1/V2 §8): matching fingerprint ⇒ proceed; mismatch ⇒ warn →
deterministic rebuild → read-only on failure. Rust then drives
[`OpenSession`](#71-lifecycle).

- `occt.fingerprint`: 64-bit hash of `{occtVersion, build flags, relevant
  algorithm knobs}`. Governs BREP/checkpoint cache compatibility.
- `quantizationVersion`: descriptor quantization scheme (currently `1` = `1e-6`
  quantization, FNV-1a 64-bit; see [§10](#10-resolution-ladder)).
- `solverPolicyVersion`: PlaneGCS policy/tuning revision.

---

## 7. Verb catalogue

Conventions: each verb shows `args` (request) and `result` (success response).
Only the verb-specific bodies are shown; the outer frame wrapping is per
[§3](#3-envelope-shapes). Fencing tokens `documentRevision`/`workerEpoch` appear
in `args` for every session-mutating verb.

### 7.1 Lifecycle

#### Hello
Emitted unsolicited by the worker; see [§6](#6-handshake). Not a `req`.

#### Shutdown
Graceful stop. Worker flushes, replies, then exits 0.

```json
// req.args
{}
// result
{ "goodbye": true }
```

#### OpenSession

```json
// req.args
{
  "documentId": "doc_1",
  "documentRevision": 0,
  "workerEpoch": 3,
  "tolerancePolicy": { "linear": 1e-7, "angular": 1e-9, "tolerancePolicyHash": "b2c9…" },
  "mode": "determinism"
}
// result
{ "sessionOpen": true, "workerHead": { "documentRevision": 0, "snapshotId": 0 } }
```

`mode` ∈ `"determinism"` (single-threaded OCCT, `parallel:false`, reproducible)
| `"fast"` (parallelism permitted; must still satisfy Invariant 5 — never change
IDs/mappings, only performance). One session per document (V1).

#### CloseSession

```json
// req.args
{ "documentId": "doc_1", "workerEpoch": 3 }
// result
{ "sessionClosed": true }
```

#### ResetSession
Drops all session + scratch state, **increments `workerEpoch`** (Rust echoes the
new epoch in subsequent requests), keeps the process alive.

```json
// req.args
{ "documentId": "doc_1", "workerEpoch": 3 }
// result
{ "reset": true, "workerEpoch": 4 }
```

#### GetWorkerHead
Reconciliation probe after a suspected desync (no side effects).

```json
// req.args
{}
// result
{ "documentRevision": 17, "workerEpoch": 3, "snapshotId": 5012,
  "historyPrefixHash": "7f1a…", "hasScratch": false }
```

### 7.2 Regen — ExecutePlan

Regen is an **ExecutePlan** model (NOT per-op). Rust compiles an immutable plan;
the worker executes step-by-step into **scratch job state** (never mutating the
active session mid-plan), streams per-step `event`s, stops at the first
failure/NeedsRepair preparing snapshot `m−1`, and ends with a terminal
`PlanPrepared` resp. Rust then publishes (`AcceptPrepared`) or drops
(`DiscardPrepared`). An interactive single-op commit is a plan of length 1.

#### ExecutePlan

```json
// req.args
{
  "jobId": 88,
  "documentRevision": 17,
  "workerEpoch": 3,
  "expectedBaseHash": "7f1a2b3c4d5e6f70",     // opaque base token (Rust-minted)
  "prefixHashes": [ "a1b2…", "c3d4…", "e5f6…" ],  // opaque per-executed-op tokens
  "baseCheckpoint": { "stepIndex": 2, "checkpointId": "ckpt_9" },  // optional
  "policyVersions": { "quantizationVersion": 1, "solverPolicyVersion": 1,
                      "descriptorVersion": 1, "resolverVersion": 1, "signatureVersion": 1 },
  "targetStep": 6,
  "artifacts": { "tessellate": { "lod": "coarse", "includeEdges": true } },
  "ops": [ /* ordered op payloads — see §7.3 */ ]
}
```

- **Fencing (D4) — `workerEpoch` + `expectedBaseHash` ONLY.** The worker fences an
  `ExecutePlan` on its `workerEpoch` (must match the head epoch) and its
  `expectedBaseHash` (must equal the head `historyPrefixHash`); either mismatch ⇒
  `error.code = "PROTOCOL_ERROR"` (Rust reconciles via `GetWorkerHead`).
  `documentRevision` is a **Rust-owned advisory stamp (an edit counter) and is NEVER
  fenced** — the worker stores the plan's `documentRevision` in the prepared scratch
  and **adopts** it as its head `documentRevision` at `AcceptPrepared` (rather than
  incrementing a worker-owned accept counter). A post-edit regen legitimately carries
  a `documentRevision` ahead of the worker's last-accepted head, so rejecting on it
  would break every such regen.
- **From-0 plans are always base-valid (D5).** A **from-0 plan** — one with **no
  `baseCheckpoint`** AND `expectedBaseHash` == the empty-prefix anchor (`e3b0c442…`)
  — is ALWAYS base-valid: the worker **SKIPS the `expectedBaseHash` head-hash
  comparison**, builds the scratch from a **genuinely empty base** (no bodies, no
  partitions — discarding any prior head state), and on `AcceptPrepared` **REPLACES
  the head wholesale** (bodies, partitions, `historyPrefixHash` = the echoed last
  prefix token, adopted `documentRevision`, bumped `snapshotId`). Rationale:
  `expectedBaseHash` pins the base state a plan builds on; a from-0 plan's base IS
  empty by definition, so the precondition is satisfiable regardless of the head. V1
  is full-replay + wholesale-publish (the `RegenPlanner` always emits from-0 plans,
  and checkpoints are UNSUPPORTED — [§7.7](#77-checkpoints)); after the first
  `AcceptPrepared` the head token is nonzero, so without this rule every subsequent
  regen would fail the `expectedBaseHash` precondition. `workerEpoch` fencing and all
  `AcceptPrepared`/`DiscardPrepared` fencing are **unchanged**. **Incremental plans**
  (`expectedBaseHash` != the empty anchor, e.g. a checkpoint-accelerated regen) keep
  the **strict head-hash fence** exactly as above.
- **Hash provenance — Rust is the sole hash authority.** `expectedBaseHash` and
  every entry of `prefixHashes` are **opaque tokens minted by Rust**. Rust computes
  them from the **geometry-relevant canonical wire-op form** of each op — the
  sorted-key JSON of `{opId, opType, stepIndex, inputs, params, determinism}`,
  SHA-256 over the newline-joined lines, lowercase hex (the empty base is the
  SHA-256 anchor `e3b0c442…`). The form deliberately **excludes** record-level
  cosmetics (`name`, record `extra`, the `suppressed` flag) so a rename never
  invalidates a checkpoint while any geometry-affecting edit does. **The worker
  MUST store/compare/echo these tokens verbatim and MUST NOT recompute them** — it
  has no visibility into the Rust record shape and any independent computation
  would diverge.
- `expectedBaseHash`: the worker compares its restored/replayed base against this
  opaque token before executing; mismatch ⇒ `error.code = "PROTOCOL_ERROR"` (Rust
  reconciles). Precondition enforcement (migration plan defenses).
- `prefixHashes`: one opaque token **per executed op**, in `ops` order —
  `prefixHashes[i]` is the history-prefix token **after executing `ops[i]`**.
  Suppressed steps are not in `ops`, so this array is indexed by execution order,
  not timeline step index. On `PlanPrepared` the worker echoes the token for its
  **last executed op** (or `expectedBaseHash` when only the base is valid) as
  `historyPrefixHash`; Rust verifies that echo (mismatch ⇒ `PROTOCOL_ERROR`).
- `baseCheckpoint`: optional; if present the worker restores it as the base
  instead of replaying from empty.
- `ops`: the ordered op slice; each op is executed on the **exact snapshot
  produced by its predecessor** (Invariant 3).

Per-step `event`s (`event:"planStep"`), one per executed step:

```json
{
  "stepIndex": 3,
  "bodyEvents": [ { "kind": "created", "bodyId": "body_3" },
                  { "kind": "modified", "bodyId": "body_1" } ],
  "elementMapDelta": { "added": [ /* {elementId, topoKey, kind, bodyId} */ ],
                       "removed": [ /* elementId */ ],
                       "relabeled": [ /* {elementId, topoKey, kind, bodyId} */ ] },
  "needsRepair": [ /* NeedsRepair payloads — §9 — STATE, not error */ ],
  "signatures": { "geometry": "aa11…", "bodyLifecycle": "bb22…", "referencedBinding": "cc33…" },
  "diagnostics": [ { "severity": "warning", "code": "…", "message": "…" } ]
}
```

- `elementMapDelta.added` / `.relabeled` entries carry a **REQUIRED `bodyId`**:
  `{ elementId, topoKey, kind, bodyId }`. A single step can create/modify several
  bodies, so each element names its owning body **explicitly** — Rust folds the
  partition from this field. (Without it Rust would have to guess the body, which
  mis-partitioned elements when one step produced two bodies.) `bodyId` is the
  partition the element currently maps to; an element's *identity* (`elementId`)
  never changes because geometry changed (Invariant 1) — only its `bodyId`/`topoKey`
  moves across split/merge.
- **`bodyEvents` NewBody id minting + adoption (D1).** A `{ "kind": "created" }`
  event's `bodyId` is **worker-minted deterministic** `body_<opId>` — `<opId>` is
  the Rust-minted op record id of the step that produced the body, so the id is a
  pure function of the (Rust-owned) plan and replay is stable across worker
  processes. Rust **adopts** each `created` id: at `AcceptPrepared` it validates the
  `body_` prefix, that `<opId>` is a **known op in the plan**, and **uniqueness**
  (no collision with a session body or a duplicate earlier in the same plan); a
  malformed or colliding id **rejects** the prepared plan (the worker's terminal is
  treated as `PROTOCOL_ERROR`, the scratch is **discarded, never published**).
  `modified`/`deleted`/`split`/`merged` events reference bodies that already exist
  (a split's surviving child keeps the parent id; new split children `body_<opId>:<k>`
  are deferred). This is a normative refinement of the §2 `BodyId` "Rust-minted"
  note: for NewBody the worker mints and Rust adopts+fences, rather than Rust
  pre-minting (split/merge body counts are unknowable before OCCT executes, so
  pre-minting could never cover them anyway).

Terminal resp — `PlanPrepared`:

```json
// result
{
  "planPrepared": true,
  "preparedSnapshotId": 5013,
  "lastValidStep": 6,          // = targetStep on full success; < targetStep if stopped early
  "stoppedReason": "completed", // "completed" | "opFailed" | "needsRepair"
  "perStepResults": [
    { "stepIndex": 0, "status": "ok",          "bodyIds": ["body_3"] },
    { "stepIndex": 1, "status": "ok",          "bodyIds": ["body_3"] },
    { "stepIndex": 6, "status": "needsRepair", "refCount": 1 }
  ],
  "historyPrefixHash": "9c4d…"
}
```

The prepared snapshot is held in scratch, NOT published. `preparedSnapshotId`
becomes live only after `AcceptPrepared`.

#### AcceptPrepared
Publishes the prepared scratch snapshot into the active session atomically. The
worker re-fences **`workerEpoch` ONLY** (a restart between prepare and accept bumps
the epoch — Rust catches that here); `documentRevision` is not fenced. On publish the
worker **adopts the accepted plan's `documentRevision`** (the value carried in the
originating `ExecutePlan`, held in the scratch) as its head `documentRevision` —
**not** a worker-owned `+1` accept counter (D4). The result echoes the adopted head
revision.

```json
// req.args  (documentRevision = the plan's Rust-owned edit counter)
{ "jobId": 88, "documentRevision": 17, "workerEpoch": 3 }
// result  (head ADOPTS the plan's documentRevision — echoes 17, not 18)
{ "accepted": true, "snapshotId": 5013, "documentRevision": 17 }
```

#### DiscardPrepared
Drops the scratch job state; session unchanged.

```json
// req.args
{ "jobId": 88 }
// result
{ "discarded": true }
```

#### Double-`ExecutePlan` while prepared (idempotency rule)

A worker holds **at most one** prepared scratch job at a time. When an
`ExecutePlan` arrives while a job is already prepared (awaiting
`AcceptPrepared`/`DiscardPrepared`):

- **Same `jobId`** — the request is a **retransmit** (Rust job ids are idempotent).
  The worker MUST NOT re-execute; it replies with the **cached `PlanPrepared`** for
  that job (byte-identical `preparedSnapshotId`/`historyPrefixHash`/`perStepResults`).
- **Different `jobId`** — a second plan cannot prepare over an outstanding one. The
  worker replies `error.code = "PROTOCOL_ERROR"` with
  `detail = { "preparedJobId": <held>, "requestedJobId": <new> }` and leaves the
  held prepared job untouched. Rust must `AcceptPrepared`/`DiscardPrepared` the
  outstanding job before sending a new plan.

### 7.3 Op payload schemas (vertical slice)

Each op in `ExecutePlan.ops` is:

```json
{
  "opType": "Extrude",
  "opId": "op_5",
  "inputs": [ /* semantic refs — see below */ ],
  "params": { /* opType-specific */ },
  "determinism": {
    "parallel": false,
    "occtOptions": { "fuzzyValue": 0.0, "useOBB": false },
    "tolerancePolicyHash": "b2c9…"
  }
}
```

`opType` ∈ `Sketch` | `Extrude` | `Revolve` | `Fillet` | `Chamfer` | `Boolean`
| `Shell` | `LinearPattern` | `CircularPattern` | `MirrorBody` (the M6a breadth
ops extend the original vertical slice — see the [Changelog](#14-changelog)).
`Loft` and `Sweep` remain **`UNSUPPORTED`** ([§8](#8-error-taxonomy)). Values keep
OneCAD-CPP `operationTypeName` spelling (PascalCase).

**Scalar / dimension fields.** Every dimensional param (`distance`, `radius`,
`angleDeg`, `thickness`, `spacing`, …) is a **scalar**: it MAY be either a bare
JSON number (`"distance": 25.0`, as the examples below spell it for brevity) **or
a `{ "value": <number>, "expr"?: <string> }` object** (`expr` = a bare V1 variable
name). Readers — this worker AND the Rust core — MUST accept both forms. The Rust
core **normalizes to the object form on write**, so an `ExecutePlan` op authored
by the core arrives here as `{ "value": … }`; hand-authored/legacy payloads may
carry a bare number. `NaN`/`±Infinity` are rejected either way ([§4](#4-json-encoding-rules)).

**Semantic reference** (`inputs[]` element) — the topological input to an op,
carried as evidence + identity so the resolution ladder can rebind after edits
(Invariant 2/3):

```json
{
  "primary": { "bodyId": "body_1", "elementId": "el_…4a1", "kind": "face" },
  "intent":  { "version": 1, "kind": "face",
               "descriptor": { /* see §10 descriptor fields */ } },
  "anchor":  { "worldPoint": [12.0, 3.5, 0.0],
               "surfaceUv":  [0.25, 0.75],
               "localFrame": { "origin": [12.0,3.5,0.0], "x": [1,0,0], "y": [0,1,0], "z": [0,0,1] },
               "adjacencyHint": "d41d8cd98f00b204" }
}
```

- `primary.kind` ∈ `body` | `face` | `edge` | `vertex`.
- `intent.descriptor` is the frozen descriptor captured when the ref was authored;
  it is **evidence, never identity** (Invariant 2). The worker MUST NOT overwrite
  the stored anchor with an op's own output (Invariant 3).

**Sketch** (`op.sketch`) — materializes a sketch feature; sketch geometry is
authored/solved in the [solver lane](#74-sketch-solver-lane) but a plan carries
the full authoritative sketch so replay is deterministic.

```json
// params
{
  "sketchId": "sk_1",
  "plane": {
    "kind": "XY",
    "origin": [0,0,0], "xAxis": [0,1,0], "yAxis": [-1,0,0], "normal": [0,0,1]
  },
  "entities": [
    { "id": "e1", "type": "Line",   "p0": [0,0], "p1": [40,0] },
    { "id": "e2", "type": "Line",   "p0": [40,0], "p1": [40,20] },
    { "id": "e3", "type": "Arc",    "center": [0,20], "radius": 40, "start": [40,20], "end": [0,60] },
    { "id": "e4", "type": "Circle", "center": [10,10], "radius": 3 }
  ],
  "constraints": [
    { "id": "c1", "type": "Horizontal", "entities": ["e1"] },
    { "id": "c2", "type": "Coincident", "entities": ["e1", "e2"], "positions": ["End","Start"] },
    { "id": "c3", "type": "Distance",   "entities": ["e1"], "value": 40.0 }
  ]
}
```

- `plane.kind` ∈ `XY` | `XZ` | `YZ` | `custom`. **Hard invariant — non-standard
  XY basis** (ported verbatim from OneCAD-CPP `Sketch.h` `SketchPlane::XY()`):
  `xAxis = (0,1,0)`, `yAxis = (−1,0,0)`, `normal = (0,0,1)` (User X → World Y+,
  User Y → World X−). `XZ` = `{x:(0,1,0), y:(0,0,1), n:(1,0,0)}`; `YZ` =
  `{x:(−1,0,0), y:(0,0,1), n:(0,1,0)}`. Producers MUST send these exact bases for
  the named planes; readers MUST lock-test them.
- `entities[].type` ∈ `Point` | `Line` | `Arc` | `Circle` | `Ellipse` | `Spline`.
- `constraints[].type` ∈ the 18 kinds (verbatim from OneCAD-CPP
  `SketchTypes.h ConstraintType`): `Coincident`, `Horizontal`, `Vertical`,
  `Fixed`, `Midpoint`, `OnCurve`, `Parallel`, `Perpendicular`, `Tangent`,
  `Concentric`, `Equal`, `Distance`, `HorizontalDistance`, `VerticalDistance`,
  `Angle`, `Radius`, `Diameter`, `Symmetric`.

**Extrude** (`op.extrude`) — end conditions `Blind` / `ThroughAll` / `Symmetric`
/ `ToNext` / `ToFace`, optional two directions. Field names ported from
OneCAD-CPP `ExtrudeParams`.

```json
// inputs: [ semanticRef to a SketchRegion (kind "face"/region) ]
// params
{
  "distance": 25.0,
  "draftAngleDeg": 0.0,
  "extrudeMode": "Blind",         // Blind | ThroughAll | Symmetric | ToNext | ToFace
  "booleanMode": "NewBody",       // NewBody | Add | Cut | Intersect
  "targetBodyId": "",             // for Add/Cut/Intersect
  "twoDirections": false,
  "extrudeMode2": "Blind",        // direction-2 end condition (when twoDirections)
  "distance2": 0.0
  // For ToFace, add "targetFace" (direction 1) and/or "targetFace2"
  // (direction 2) — a **semantic reference** object (same {primary, intent,
  // anchor} shape as the "Semantic reference" above and the fillet edge refs):
  // "targetFace": {
  //   "primary": { "bodyId": "body_1", "elementId": "el_…", "kind": "face" },
  //   "intent":  { "version": 1, "kind": "face", "descriptor": { /* §10 */ } },
  //   "anchor":  { "worldPoint": [12.0,3.5,0.0], "surfaceUv": [0.25,0.75] }
  // }
}
```

- `targetFace`/`targetFace2` are **typed semantic refs**, not bare ids
  (amended 2026-07-16 — see [Changelog](#14-changelog)). A bare `targetFaceId`
  string could carry no anchor/intent, so a ToFace target would be
  **un-repairable** across parametric edits, violating Invariants 2/3; the typed
  ref lets the resolution ladder rebind it. Absent for non-`ToFace` extrudes.

**Revolve** (`op.revolve`) — field names from OneCAD-CPP `RevolveParams`.

```json
// inputs: [ semanticRef to a SketchRegion ]
// params
{
  "angleDeg": 360.0,
  "axis": { "kind": "sketchLine", "sketchId": "sk_1", "lineId": "e1" },
              // axis.kind ∈ "sketchLine" {sketchId,lineId} | "edge" {bodyId,edgeId} | "none"
  "booleanMode": "NewBody",       // NewBody | Add | Cut | Intersect
  "targetBodyId": ""
}
```

**Fillet** (`op.fillet`) and **Chamfer** (`op.chamfer`) — split ops sharing the
OneCAD-CPP `FilletChamferParams` shape (`mode` distinguishes; radius doubles as
chamfer distance).

```json
// Fillet params
{ "mode": "Fillet", "radius": 2.0, "edgeIds": ["e:14", "e:15"], "chainTangentEdges": true }
// Chamfer params
{ "mode": "Chamfer", "radius": 1.0, "edgeIds": ["e:14"], "chainTangentEdges": true }
```

`edgeIds` entries are TopoKeys (snapshot-scoped) or `ElementId`s; the worker
resolves each through the ladder ([§10](#10-resolution-ladder)). The `inputs[]`
array carries the corresponding semantic refs (one per edge) supplying descriptor
+ anchor evidence.

**Boolean** (`op.boolean`) — standalone body-body boolean. Field names from
OneCAD-CPP `BooleanParams` (`operation` ∈ Union/Cut/Intersect; distinct from the
`booleanMode` fused into feature ops).

```json
// inputs: [ semanticRef(target body), semanticRef(tool body) ]
// params
{ "operation": "Union", "targetBodyId": "body_1", "toolBodyId": "body_2" }
```

`operation` ∈ `Union` | `Cut` | `Intersect`.

**Shell** (`op.shell`) — hollow a body, removing (opening) selected faces. Field
names from OneCAD-CPP `ShellParams`. Added M6a (see the [Changelog](#14-changelog)).

```json
// inputs: [ semanticRef(face) per open face — kind "face" ]
// params
{ "thickness": 2.0, "targetBodyId": "body_1", "openFaces": ["el_…7c", "el_…8d"] }
```

- `thickness` is the (positive) wall thickness; the worker offsets **inward**
  (`BRepOffsetAPI_MakeThickSolid::MakeThickSolidByJoin(target, removed,
  −thickness, …)`, OneCAD-CPP parity). `thickness < 1e-3` ⇒ recoverable
  `OP_FAILED` ("Shell thickness too small").
- `openFaces` entries are `ElementId`s (bare). **Unlike Fillet/Chamfer edges, the
  frozen `ShellParams` carries no per-face typed ref**, so the `inputs[]` face refs
  are **element-only** (no `intent`/`anchor`). The worker resolves each on the
  predecessor snapshot via the partition-tracked binding (an id already minted by
  an earlier op / this plan's `resolve_input_refs`) OR the descriptor+anchor
  ladder ([§10](#10-resolution-ladder)); a face that resolves via neither ⇒
  **NeedsRepair** ([§9](#9-needsrepair-payload)), never a wrong bind. The result
  **replaces** the shelled body (id preserved; OCCT history folds into its
  partition).

**LinearPattern** (`op.linearPattern`) — `count` copies of a source body translated
`spacing` along `direction`. Field names from OneCAD-CPP `LinearPatternParams`
(the C++ flat `dirX/Y/Z` is a single `direction: [x,y,z]`). Added M6a.

```json
// inputs: [ semanticRef(source body) ]
// params
{ "sourceBodyId": "body_1", "direction": [1,0,0], "spacing": 40.0, "count": 3, "fuseResult": true }
```

- `count ≥ 2` (else recoverable `OP_FAILED`); `|spacing| ≥ 1e-9`; `direction`
  non-zero (normalized). Instance `i ∈ [1, count)` is translated `direction·spacing·i`.
- `fuseResult` (default `true`): `true` ⇒ source + instances FUSED into one solid;
  `false` ⇒ gathered into one compound. **Either way the op produces ONE new body
  `body_<opId>`** (NewBody lineage — the source body is preserved). The result
  INCLUDES the source geometry (OneCAD-CPP parity). Empty `elementMapDelta`
  (ID-on-demand; a pattern face is minted when first referenced).

**CircularPattern** (`op.circularPattern`) — `count` copies rotated about an axis.
Field names from OneCAD-CPP `CircularPatternParams` (flat `axisX/Y/Z` +
`axisDirX/Y/Z` → `axisOrigin` + `axisDirection`). Added M6a.

```json
// inputs: [ semanticRef(source body) ]
// params
{ "sourceBodyId": "body_1", "axisOrigin": [0,0,0], "axisDirection": [0,0,1],
  "angleDeg": 360.0, "count": 3, "fuseResult": true }
```

- `count ≥ 2`; `axisDirection` non-zero. The per-instance step angle is
  `angleDeg / count` (OneCAD-CPP parity — divides by `count`, **not** `count−1`);
  instance `i ∈ [1, count)` is rotated `step·i` about `(axisOrigin, axisDirection)`.
- `fuseResult` + lineage identical to LinearPattern.

**MirrorBody** (`op.mirrorBody`) — reflect a source body across a plane. Field names
from OneCAD-CPP `MirrorBodyParams` (flat `planePointX/Y/Z` + `planeNormalX/Y/Z` →
`planePoint` + `planeNormal`). Added M6a.

```json
// inputs: [ semanticRef(source body) ]
// params
{ "sourceBodyId": "body_1", "planePoint": [0,0,0], "planeNormal": [1,0,0], "fuseWithOriginal": false }
```

- The mirror plane passes through `planePoint` perpendicular to `planeNormal`
  (`gp_Trsf::SetMirror(gp_Ax2(planePoint, planeNormal))`).
- `fuseWithOriginal` (default `false`): `true` ⇒ source + mirror image FUSED into
  one solid; `false` ⇒ the mirror image alone. Either way ONE new body
  `body_<opId>` (NewBody lineage; source preserved). Empty `elementMapDelta`.

### 7.4 Sketch solver lane

A **separate worker thread/actor** runs PlaneGCS. It follows a **latest-wins**
mailbox: drags never queue behind OCCT ops (migration plan — solver lane in V1).
Requests here are ordinary `req` frames; the worker routes them to the solver
thread by verb.

#### SketchUpsert
Upserts the authoritative sketch (plane + entities + constraints). Increments
`sketchRevision`.

```json
// req.args  (entities/constraints as in the Sketch op params, §7.3)
{ "sketchId": "sk_1", "plane": { "kind": "XY", "...": "..." },
  "entities": [ … ], "constraints": [ … ] }
// result
{ "sketchId": "sk_1", "sketchRevision": 4, "dof": 2,
  "state": "UnderConstrained" }   // state ∈ UnderConstrained|FullyConstrained|OverConstrained|Conflicting
```

#### BeginGesture
Opens a drag gesture against a specific sketch revision.

```json
// req.args
{ "sketchId": "sk_1", "sketchRevision": 4, "gestureId": 51, "solverPolicyHash": "3e9a…" }
// result
{ "gestureId": 51, "ready": true }
```

#### SolveDrag
Latest-wins incremental solve. Superseded in-flight drags may be dropped; only the
newest `seq` per gesture must resolve.

```json
// req.args
{ "gestureId": 51, "seq": 129, "pointId": "e3.start", "target": [42.0, 19.5] }
// result
{
  "gestureId": 51, "seq": 129,
  "status": "success",       // success | partial | conflicting | redundant
  "dof": 1,
  "conflicting": [],         // constraint ids in conflict (when status=conflicting)
  "positions": { "e3.start": [42.0, 19.5], "e2.p1": [40.0, 19.5] },  // CHANGED points only
  "solveMicros": 1840
}
```

#### EndGesture
Pointer-up: performs the final **exact** solve (Rust commits one undo command from
its result).

```json
// req.args
{ "gestureId": 51 }
// result
{ "gestureId": 51, "status": "success", "dof": 0,
  "positions": { /* final exact positions, changed since BeginGesture */ },
  "sketchRevision": 5 }
```

#### SketchRegions
Computes closed profile regions for a sketch (for extrude/revolve selection and
preview fill).

```json
// req.args
{ "sketchId": "sk_1" }
// result
{
  "sketchId": "sk_1", "sketchRevision": 5,
  "regions": [
    {
      "regionId": "r0",
      "outerLoop": ["e1", "e2", "e3"],
      "holes": [ ["e4"] ],
      "previewTriangles": { "format": "f32xyz+u32idx", "vertexCount": 8,
        "triangleCount": 6, "bin": "region:r0" }
    }
  ]
}
// bin: [ { "name": "region:r0", "off": 0, "len": … } ]  // f32 positions then u32 indices
```

- **`regionId` derivation is NORMATIVE** (worker and Rust core MUST agree so a
  region id is reproducible from loop membership alone, without shared mutable
  state). It is **FNV-1a-64** (offset `0xcbf29ce484222325`, prime
  `0x100000001b3`) over: each loop-member entity UUID as its **16 raw bytes**,
  taken in **ascending sorted order of the 16-byte arrays** (so the id is
  independent of member ordering), followed by **one winding byte**
  (`0` = CCW / outer, `1` = CW / hole). The 64-bit result is rendered
  `"r_%016x"` (lowercase hex, e.g. `"r_0123456789abcdef"`). The examples above
  use short placeholders (`"r0"`) for readability. The C++ worker MUST produce
  byte-identical ids; the reference implementation is onecad-core
  `sketch/mod.rs::derive_region_id` (Rust). Regions are a rebuildable cache, not
  authoritative identity — a hash collision only costs a recomputed cache entry,
  never correctness.

### 7.5 Element identity

#### AcquireElementIds
Promotes snapshot-scoped TopoKeys to persistent, globally-unique `ElementId`s
(**ID-on-demand**). ElementIds do **not** embed `BodyId`.

```json
// req.args
{ "snapshotId": 5012, "bodyId": "body_3",
  "picks": [ { "topoKey": "f:22", "anchor": { "worldPoint": [1,2,3], "surfaceUv": [0.5,0.5] } } ] }
// result
{ "ids": [ { "topoKey": "f:22", "elementId": "el_00000000000004a1", "kind": "face" } ] }
```

Note: `elementId` is **minted by Rust**, not the worker — the worker returns the
resolved `topoKey → (kind, descriptor, anchor)` binding and Rust assigns/echoes
the persistent id it owns. When Rust already holds an id for that stable element,
the worker's response includes the existing binding so Rust returns the same id
(Invariant 1: an ElementId never changes because geometry changed).

#### QueryElement
Looks up an element's current binding within a snapshot (no mutation).

```json
// req.args
{ "snapshotId": 5012, "elementId": "el_…4a1" }   // or { "snapshotId", "topoKey", "bodyId" }
// result
{ "elementId": "el_…4a1", "topoKey": "f:22", "bodyId": "body_3", "kind": "face",
  "descriptor": { … }, "anchor": { … }, "present": true }
```

#### ResolveRefs
**Dry-run** ladder execution for repair dialogs — returns full evidence per ref
without binding anything.

```json
// req.args
{ "snapshotId": 5012,
  "refs": [ { "refId": "op_5.input0", "primary": {…}, "intent": {…}, "anchor": {…} } ] }
// result
{ "resolutions": [
    { "refId": "op_5.input0", "outcome": "autoBind",   "elementId": "el_…", "score": 0.94, "margin": 0.31 },
    { "refId": "op_5.input1", "outcome": "needsRepair", "needsRepair": { /* §9 */ } }
] }
```

`outcome` ∈ `autoBind` | `needsRepair` | `unchanged`.

### 7.6 Geometry

#### Tessellate
Produces MESH1 meshes; large meshes stream on the bulk lane
([§5.2](#52-chunked-bulk-streams)). `mesh_format.md` defines MESH1.

```json
// req.args
{ "bodyIds": "all", "lod": "coarse", "includeEdges": true }
       // bodyIds: "all" | ["body_1","body_3"];  lod: "coarse"|"medium"|"fine"
// result
{ "meshes": [
    { "bodyId": "body_1", "streamId": 700, "format": "MESH1",
      "totalBytes": 4194304, "sha256": "…", "snapshotId": 5012 }
] }
```

Meshes label faces/edges with snapshot-scoped TopoKeys (`"f:22"`) and persistent
`ElementId`s where already minted. Meshing parallelism never affects IDs
(Invariant 5).

#### GetBodies
Returns BREP blobs (OCCT `BinTools`) for the given bodies; streams on bulk lane.

```json
// req.args
{ "bodyIds": ["body_1"], "snapshotId": 5012 }
// result
{ "bodies": [ { "bodyId": "body_1", "streamId": 701, "format": "BREP",
  "brepContentHash": "…", "totalBytes": 91234, "sha256": "…" } ] }
```

#### LoadBodies
Loads BREP blobs into the session (input via request `bin`/stream).

```json
// req.args
{ "bodies": [ { "bodyId": "body_1", "bin": "brep:body_1", "brepContentHash": "…" } ] }
// bin: [ { "name": "brep:body_1", "off": 0, "len": 91234 } ]
// result
{ "loaded": ["body_1"], "snapshotId": 5014 }
```

### 7.7 Checkpoints

#### SaveCheckpoint
Emits an **atomic artifact set** for a step: BREP blobs (BinTools) + ElementMap
partition JSON + the 3 signatures + `historyPrefixHash`, each wrapped in a
Rust-readable envelope. Blobs stream on the bulk lane.

```json
// req.args
{ "stepIndex": 4 }
// result
{
  "checkpointId": "ckpt_9",
  "stepIndex": 4,
  "artifacts": [
    {
      "envelope": {
        "artifactSchemaVersion": 1,
        "bodyId": "body_3",
        "step": 4,
        "historyPrefixHash": "9c4d…",
        "brepContentHash": "aa11…",
        "occtFingerprint": "9a1c33f0e7b24d10",
        "descriptorVersion": 1,
        "resolverVersion": 1,
        "quantizationVersion": 1,
        "signatureVersion": 1,
        "codec": "brep-bintools",
        "size": 91234,
        "contentHash": "bb22…"
      },
      "streamId": 702
    }
  ],
  "elementMapPartition": { "streamId": 703, "format": "elementmap-json", "sha256": "…" },
  "signatures": { "geometry": "…", "bodyLifecycle": "…", "referencedBinding": "…" }
}
```

Checkpoints are **disposable caches**: an envelope whose versions/fingerprint are
incompatible is discarded + replayed; a checkpoint never blocks opening the
authoritative JSON (Invariant 7).

#### RestoreCheckpoint
Restores a checkpoint as base state; verifies the envelope signature and reports
drift.

```json
// req.args
{ "checkpointId": "ckpt_9", "expectedHistoryPrefixHash": "9c4d…" }
// bin/streams: the artifact blobs Rust supplies back
// result
{ "restored": true, "snapshotId": 5015, "driftDetected": false,
  "driftDetail": null }   // when driftDetected: { signature: "geometry"|"bodyLifecycle"|"referencedBinding", expected, actual }
```

### 7.8 IO

Paths are **Rust-provided temp paths** (the webview has zero fs capability; Rust
does all IO and handles hostile files in the isolated worker).

#### ImportStep

```json
// req.args
{ "path": "/tmp/onecad/import_ab12.step" }
// result
{ "bodyIds": ["body_10","body_11"], "snapshotId": 5016,
  "diagnostics": [ { "severity": "warning", "code": "STEP_HEALED", "message": "…" } ] }
```

#### ExportStep

```json
// req.args
{ "path": "/tmp/onecad/export_cd34.step", "bodyIds": ["body_3"], "schema": "AP214IS" }
// result
{ "written": true, "bytes": 40211 }
```

`schema` currently `"AP214IS"`.

#### ExportStl

```json
// req.args
{ "path": "/tmp/onecad/out.stl", "bodyIds": ["body_3"], "binary": true, "lod": "fine" }
// result
{ "written": true, "bytes": 120344, "triangleCount": 4012 }
```

#### ExportObj

```json
// req.args
{ "path": "/tmp/onecad/out.obj", "bodyIds": ["body_3"], "lod": "fine" }
// result
{ "written": true, "bytes": 98211 }
```

---

## 8. Error taxonomy

Errors are returned in a terminal `resp` with `ok:false` and an `error` object:

```json
{ "code": "OP_FAILED", "message": "human-readable", "detail": { … }, "retriable": false }
```

| Class | `code` | Session effect | Recovery |
|-------|--------|----------------|----------|
| Recoverable op failure | `OP_FAILED` | scratch only — **session intact** | Rust discards scratch; user edits and retries |
| Reference unresolved | `REF_UNRESOLVED` | scratch only | as above (distinct from NeedsRepair — this is a hard resolve failure, e.g. input body missing) |
| Invalid geometry produced | `GEOMETRY_INVALID` | scratch only | as above |
| Unsupported op/param (known verb) | `UNSUPPORTED` | none | Rust falls back / freezes node (the remaining un-shipped ops `opType:"Loft"` / `"Sweep"`; the M6a breadth ops Shell/LinearPattern/CircularPattern/MirrorBody are now supported, [§7.3](#73-op-payload-schemas-vertical-slice)) |
| Cooperative cancellation | `CANCELLED` | in-flight job dropped; session intact | terminal frame always sent ([§3.5](#35-cancel-rust--worker)) |
| Protocol violation | `PROTOCOL_ERROR` | fatal | **restart worker** (no resync) |
| Worker crash / abnormal exit | *(no frame)* | fatal | **restart + replay** from last checkpoint/head; crash **circuit breaker** on repeated `(historyPrefixHash, opId, occtFingerprint)` |
| Timeout | *(Rust-side)* | Rust-enforced | see below |

`PROTOCOL_ERROR` covers two sub-cases:
- **Framing / envelope violation** (bad magic, over-cap length, malformed JSON,
  `NaN`/`Inf`, duplicate keys, chunk SHA-256 mismatch): the frame stream is
  unparseable — the reader tears down without resync; a terminal frame may not be
  produced.
- **Well-framed but protocol-illegal request** (**unknown verb**, mismatched
  `workerEpoch` or `expectedBaseHash` — the D4 fencing tokens; a stale
  `documentRevision` is **not** an error, [§2](#2-identifier--scalar-types)/[§7.2](#72-regen--executeplan) — malformed `args`): the frame parsed, so the worker
  replies with a terminal `resp` `ok:false` `error.code:"PROTOCOL_ERROR"`. Rust
  reconciles (`GetWorkerHead`) or restarts per severity.

**Timeouts** are enforced by **Rust**, not the worker:
- `SolveDrag`: **250 ms**. On timeout Rust drops the stale drag (latest-wins) and
  keeps the gesture; the frontend keeps its 120 Hz preview.
- `Tessellate`: **30 s**. On timeout Rust cancels the request and may retry at a
  coarser LOD.
- Hung worker: ping every **5 s**, ×2 misses → `SIGKILL` → restart.

**`OP_FAILED`, `REF_UNRESOLVED`, `GEOMETRY_INVALID`, `UNSUPPORTED` are
*recoverable*: the worker's active session is untouched (all work was in scratch).
Rust reports the failure and the document stays editable.**

**NeedsRepair is NOT an error.** It is per-step **state** inside `PlanPrepared`
(`perStepResults[].status = "needsRepair"`, payload in the step `event`'s
`needsRepair[]`). It is never returned in an `error` object, in any of the three
languages. A plan that hits NeedsRepair at step `m` still prepares snapshot `m−1`
and returns a successful `PlanPrepared` (`stoppedReason:"needsRepair"`).

---

## 9. NeedsRepair payload

Emitted in a `planStep` event's `needsRepair[]` and echoed by `ResolveRefs`. It is
STATE (see [§8](#8-error-taxonomy)).

```json
{
  "refId": "op_5.input0",
  "elementId": "el_…4a1",
  "ladderFailed": "descriptor",          // "history" | "descriptor"
  "reason": "ambiguous",                 // "ambiguous" | "no-candidates" | "low-confidence"
  "scoringVersion": 1,                   // = resolverVersion the scores were computed under
  "candidates": [
    {
      "topoKey": "f:31",
      "score": 0.91,                     // normalized [0,1], versioned (§10)
      "margin": 0.00,                    // score1 − score2
      "worldPos": [12.0, 3.5, 0.0],
      "summary": "planar face, area≈120mm²",
      "featureContributions": { "surfaceType": 0.2, "area": 0.25, "normal": 0.2,
                                "adjacency": 0.15, "anchor": 0.11 }
    },
    { "topoKey": "f:44", "score": 0.91, "margin": 0.00, "worldPos": [12.0,-3.5,0.0],
      "summary": "planar face, area≈120mm²", "featureContributions": { } }
  ],
  "anchor": { "worldPoint": [12.0,3.5,0.0], "surfaceUv": [0.25,0.75],
              "localFrame": { … }, "adjacencyHint": "d41d8cd9…" },
  "uiLabel": "Fillet edge on right pocket"
}
```

- `ladderFailed`: the ladder level that could not decide (`history` = OCCT history
  gave no/ambiguous mapping; `descriptor` = descriptor+anchor matching was
  ambiguous/low-confidence).
- `scoringVersion`: the `resolverVersion` (§10) the candidate scores were computed
  under. Present on every NeedsRepair evidence payload so a repair UI / a
  Rust-side policy knows which normalized-scoring scheme produced the numbers.
- `candidates[]` is sorted by `score` descending; a symmetric tie (equal scores,
  `margin` below the policy margin) MUST produce NeedsRepair, never a guess (false
  positive is worse than false negative).
- Repair is performed by **Rust** (rewrite the OperationRecord ref + re-regen);
  there is **no worker `BindRepair` verb**.

---

## 10. Resolution ladder

Worker-side, executed inside each plan step's input binding. Returns full typed
evidence so the policy can later move to Rust.

**Ladder:**

1. **OCCT history** — consult the modified/generated maps of **all** ops in the
   step's builders (not just booleans); builder objects are kept alive for the
   step. A unique history image auto-binds.
2. **Descriptor matching with anchor narrowing** — for unresolved refs, match the
   frozen `intent.descriptor` against candidate elements; narrow ambiguity using
   the `anchor` (world point, surface UV, local frame, adjacency hint).
3. **Confidence gate → NeedsRepair** — if no confident unique match, emit
   NeedsRepair ([§9](#9-needsrepair-payload)).

**Descriptor** (evidence, never identity — Invariant 2). Ported from OneCAD-CPP
`ElementMap.h`: an `ElementDescriptor` of `{shapeType, center, size (bbox
diagonal), magnitude (area/length/volume), surfaceType, curveType, normal,
tangent, hasNormal, hasTangent, adjacencyHash}`, quantized into a match key
(shape/surface/curve type + quantized center xyz + normal xyz + tangent xyz +
size + magnitude + adjacencyHash). Quantization step **`1e-6`**
(`llround(value / 1e-6)`). Hashing **FNV-1a 64-bit** (offset basis
`14695981039346656037`, prime `1099511628211`). `adjacencyHash` is FNV-1a over
sorted quantized incident-edge lengths (faces) or magnitude + vertex offsets +
count (edges). This is `quantizationVersion = 1` / `descriptorVersion = 1`.

**Scoring (REDESIGNED — normalized).** OneCAD-CPP's `score()` is an unbounded,
scale-dependent cost that cannot express the locked policy; this protocol replaces
it with a **normalized `[0,1]` versioned confidence** (`resolverVersion = 1`).
Higher = better match. Policy:

- **Auto-bind iff** `score1 ≥ 0.85` **AND** `(score1 − score2) ≥ 0.10`
  (score1/score2 = best/second-best candidate).
- Otherwise, attempt anchor narrowing; if still not confident ⇒ NeedsRepair.
- For a set of referenced elements, use **min-cost assignment** over the
  **referenced-only** candidate sets (greedy is a documented counterexample —
  never greedy).
- **Lineage semantics for split/merge are explicit: no forced 1:1.** A split may
  map one prior element to several successors; a merge, several to one. The
  assignment respects declared lineage rather than forcing bijection.
- A **symmetric tie** (e.g. `0.91` vs `0.91`, margin `< 0.10`) ⇒ NeedsRepair. A
  false positive (wrong silent bind) is strictly worse than a false negative
  (asking the user).

The worker returns, per ref: candidates, `featureContributions`, `score`,
`margin`, and the ladder level reached — full evidence for repair UI and for
moving policy to Rust later.

---

## 11. Invariants

Copied verbatim from the migration plan ("Invariants (test-enforced)"). Every verb
in this contract is defined so as not to violate them; the golden fixtures enforce
them.

1. ElementId never changes because geometry changed.
2. Descriptors are evidence never identity.
3. Every op resolves inputs on its exact predecessor snapshot (never overwrite
   stored input anchor with op's own output).
4. Published bodies/maps/signatures/meshes share one snapshot id.
5. Same plan+base+policies+fingerprint ⇒ identical lifecycle/mappings/quantized
   signatures.
6. Failure at m publishes ≤ m−1.
7. Incompatible cache degrades performance never correctness.

---

## 12. Signatures

**Three** signatures per step (counts alone cannot detect symmetric ElementId
swaps). All are 64-bit FNV-1a hex strings (`signatureVersion = 1`):

- `geometry` — over per-body counts (faces/edges/vertices), quantized bbox, and
  adjacency structure.
- `bodyLifecycle` — over the ordered body create/modify/delete/split/merge events
  of the step.
- `referencedBinding` — over the `(refId → ElementId)` bindings the step produced
  for **referenced** elements (catches symmetric swaps that leave counts intact).

They appear in `planStep` events, `SaveCheckpoint`, and `PlanPrepared` summaries,
and back Invariant 5 and checkpoint drift detection.

---

## 13. Versioning/change policy

- `protocolVersion` is `1`. A wire-incompatible change bumps it; the handshake
  negotiates and Rust refuses an unknown major.
- The independent version axes carried in the handshake and checkpoint envelopes —
  `quantizationVersion`, `descriptorVersion`, `resolverVersion`,
  `signatureVersion`, `solverPolicyVersion`, `occtFingerprint`,
  `artifactSchemaVersion` — evolve separately; a mismatch degrades caches to
  replay, never correctness (Invariant 7).
- **Any change to this file, to `mesh_format.md`, to the `Descriptor.*`
  computation, or to a serde/nlohmann schema requires a fixture bump
  (`protocol/fixtures/`) + cross-track sign-off (worker + Rust + orchestrator).**
- Golden fixtures in `protocol/fixtures/` are the executable form of this
  contract; both sides run them in CI.

---

## 14. Changelog

`protocolVersion` stays **1** for all entries below — these are pre-implementation
contract refinements (no worker has shipped against the prior text), so they are
edits to version 1 rather than a version bump. They still fall under the
[§13](#13-versioningchange-policy) change policy (fixture bump + cross-track
sign-off) once fixtures exist.

- **2026-07-19 — M6a: breadth ops Shell / LinearPattern / CircularPattern /
  MirrorBody implemented (worker + Rust wire)** (§7.3; **orchestrator sign-off
  PENDING**). [§7.3](#73-op-payload-schemas-vertical-slice), [§8](#8-error-taxonomy).
  Additive extension of the §7.3 op catalogue: four op payload schemas derived
  1:1 from the Rust serde param shapes (`onecad-core` `ShellParams` /
  `LinearPatternParams` / `CircularPatternParams` / `MirrorBodyParams` — the wire
  truth). `opType` now also accepts these four; **`Loft`/`Sweep` remain
  `UNSUPPORTED`** ([§8](#8-error-taxonomy) table updated). The worker ports the
  OneCAD-CPP `RegenerationEngine` construction verbatim (Shell:
  `BRepOffsetAPI_MakeThickSolid::MakeThickSolidByJoin` with a **negative** offset;
  patterns: `BRepBuilderAPI_Transform` + chained `BRepAlgoAPI_Fuse` or a compound,
  step angle `angleDeg/count`; mirror: `gp_Trsf::SetMirror`). **Shell** replaces
  its body (Modify lineage; OCCT history → partition) and resolves its **bare**
  open-face refs (frozen `ShellParams` carries no per-face anchor) via the
  partition-tracked binding or the [§10](#10-resolution-ladder) ladder — a face
  that resolves via neither ⇒ NeedsRepair ([§9](#9-needsrepair-payload)).
  **Patterns/MirrorBody** mint ONE new `body_<opId>` (NewBody lineage; source
  preserved; empty `elementMapDelta`). No `protocolVersion` bump (still 1 —
  pre-implementation contract extension). Fixtures:
  `worker/tests/fixtures/executeplan_linearpattern.ndjson` (full apply),
  `worker/tests/fixtures/executeplan_shell.ndjson` (bare-ref → NeedsRepair). No
  canonical `protocol/fixtures/` change. Tests: worker `m6a_ops` (exact box
  arithmetic — shell 4112, patterns 30000, mirror 20000, guards, NeedsRepair) +
  Rust `src-tauri/tests/breadth_ops.rs` (real-worker exact volumes, determinism
  across two processes, upstream-edit re-run).
- **2026-07-19 — M5a: SaveCheckpoint/RestoreCheckpoint implemented (worker-retained,
  in-session); mesh export (ExportStl/ExportObj) shipped** (§7.7/§7.8;
  **orchestrator-approved 2026-07-19 for the §7.7 divergences**). [§7.7](#77-checkpoints),
  [§7.8](#78-io). **Checkpoints.** `SaveCheckpoint` serializes the session head
  (per-body BREP via **BinTools** + the 3 signatures + `historyPrefixHash`) into the
  resp binary tail AND **retains the head in-session** (keyed by step); Rust stores the
  bytes + parsed envelope metadata and persists them into the `.onecad` container's
  `checkpoints/` layout. `RestoreCheckpoint` **rolls the in-session head back** to the
  step (fenced on `workerEpoch`; a stale `expectedHistoryPrefixHash` ⇒
  `driftDetected`). The Rust `WorkerManager` reconstructs the base `BodyRegistry` +
  `ElementIndex` from the stored artifacts (executor seeds scratch from the immutable
  checkpoint, review F3); the planner selects a compatible checkpoint at/below the
  dirty floor so a post-checkpoint edit regens **incrementally**. Determinism proven:
  an incremental regen (RestoreCheckpoint + incremental plan) yields a head
  byte-identical to a from-0 replay (`src-tauri/tests/checkpoints.rs`).
  ***Divergences to sign off:*** **(a)** the artifact blobs ride **inline in the resp
  tail** (`bin` sections), NOT on the bulk lane / `streamId` the §7.7 example shows —
  sound for the small V1 artifacts; **(b)** restore is **in-session only** — the OCW1
  request path carries no binary, so RestoreCheckpoint cannot re-ship BREP; a worker
  that no longer retains the step (post-restart, post-reopen) reports `restored:false`
  and the executor **replays from 0** (Invariant 7 — the cache degrades to replay,
  never a wrong result). The checkpoint bytes ARE persisted to the container for a
  future cross-restart restore (transport request-binary is the follow-up);
  **(c)** the persisted `elementMapPartition` is a **placeholder** JSON (the in-session
  restore uses the retained partition) and the container form stores the whole
  `CheckpointArtifacts` as JSON (BREP bytes inline) rather than split json/bin — size
  inefficiency, sound. A minor **latent bug fixed:** the Rust `wire_op` omitted
  `stepIndex`, so the worker used the execution-order index; harmless for from-0 plans
  (exec index == step index) but wrong for the incremental plans checkpoints enable —
  now sent per [§7.3](#73-op-payload-schemas-vertical-slice).
  **Mesh export.** `ExportStl` (`{path, bodyIds, binary, lod}` → `{written, bytes,
  triangleCount}`) and `ExportObj` (`{path, bodyIds, lod}` → `{written, bytes}`) reuse
  the worker's tessellation (`tess::tessellate_raw`, same BRepMesh params/winding as the
  viewport mesh ⇒ the STL triangle count equals the tessellation), binary+ASCII STL and
  ASCII OBJ. stdout-hygiene asserted (`test_wp6_meshexport`). Fixtures:
  `worker/tests/fixtures/export_mesh.ndjson`, `checkpoint_roundtrip.ndjson`. No
  canonical `protocol/fixtures/` change.

- **2026-07-19 — M5a: boolean split children `body_<opId>:<k>` are minted, adopted +
  fenced; the Rust `BodyId` is a deterministic derived uuid** (D1 extension;
  **orchestrator-approved 2026-07-19**). [§2](#2-identifier--scalar-types), [§7.2](#72-regen--executeplan).
  When a Boolean (or a boolean-mode Extrude **Cut**) yields **multiple solids**, the
  worker deletes the parent + tool and mints a deterministic child `body_<opId>:<k>`
  per solid, ordered by a **quantized geometric key** (volume, then centroid x/y/z,
  then face count, at 1e-6 — never unordered `TopExp` iteration), emitting a `Created`
  `bodyEvent` per child. Rust **adopts** them at `AcceptPrepared` (`validate_created`):
  the wire id parses to `(opId, k)`, `opId` must be a **known op in the plan**, the
  per-op ordinals must be **contiguous from 0**, and the id must be **unique** (else
  `PROTOCOL_ERROR`, discard). **Rust-side representation (flagged for sign-off):**
  `BodyId` is a `Uuid` newtype, so a `:<k>` child cannot reuse the opId uuid; it maps
  to a **deterministic derived uuid** = first 16 bytes of `SHA-256("onecad.body.split.v1:"
  ‖ "<opId>:<k>")` (a uuid5-style stable hash — `uuid`'s `v5` feature is off; `sha2` is
  already a dep; the derivation lives in `onecad-core` so both the wire layer and the
  registry share it). The derivation is one-way, so the wire layer keeps a small
  **interner** `derived → "body_<opId>:<k>"`. **Cross-process persistence (orchestrator
  review fix):** the interner alone is only warm within the minting process, but a
  from-0 replay compiles the WHOLE plan *before* the worker re-mints anything — so a
  downstream op that references a persisted split child (e.g. a Cut targeting `:1`)
  would, on reopen in a fresh process, render a bare derived uuid the worker never
  minted (`REF_UNRESOLVED`). Fix: the core `BodyMeta` carries an **additive**
  `splitOf: {op, k}` (serde `skip_serializing_if=None` ⇒ non-split documents are
  byte-identical; `schema_freeze` stays green), populated at adoption/fold time and
  persisted in `document.json`; `DocumentRuntime::open` (and checkpoint restore, via the
  open path) walks the registry and **re-interns every split entry before any plan
  compiles**. (A split child can only be *referenced* by an op added AFTER a regen
  created it — you cannot select a body a not-yet-run op will mint — so within one
  session the interner is warm at compile time; the persisted `splitOf` covers the
  reopen.) The mapping stays deterministic + replay/persistence-stable: the document
  stores the derived uuid and a from-0 replay re-mints the SAME id.
  *Divergence to sign off:* the §2 note called split minting "deferred to W-WP6" and
  said BodyIds "DOES NOT embed BodyId"; this ships the split-child id form and the
  derived-uuid representation (an implementation choice §2 did not pin — §2 only fixed
  the *wire* string `body_<opId>:<k>`, which is honored exactly). On a split, the
  parent's referenced-element partition entries are **dropped** (a rebuildable
  ID-on-demand cache; a later ref re-mints against a child or NeedsRepairs) — no
  confident 1:1 child assignment exists. *Tests:* worker `test_wp6_split` (in-process
  bisecting Cut → 2 ordered children, exact volumes, ids stable), Rust
  `wire_contract::boolean_split_children_adopted` (real worker, 2 children adopted,
  volumes 7500 each, ids stable across replay), `validate_created` unit tests
  (contiguity / unknown-op / collision). No canonical `protocol/fixtures/` change (they
  carry no multi-solid boolean).

- **2026-07-19 — M4a: ResolveRefs `autoBind` returns `elementId` in its own slot;
  `topoKey` is evidence** (code-to-spec; **orchestrator-approved 2026-07-19**).
  [§7.5](#75-element-identity), [§9](#9-needsrepair-payload). The worker's
  `ResolveRefs` `autoBind` resolution now carries the **Rust-minted `elementId`** in
  the [§7.5](#75-element-identity) `elementId` slot (**empty** when the resolved
  element is not yet in the partition — a dry run binds nothing, so Rust would mint at
  real bind time) and the bound `topoKey` as **evidence** *alongside* it
  ([§9](#9-needsrepair-payload): a snapshot-scoped `topoKey` is evidence never
  identity, so it must not occupy the `elementId` slot). Previously the worker put the
  `topoKey` in the `elementId` slot (R-WP12 flag). The Rust parser now reads the
  `elementId` slot strictly (with a one-release tolerance: a legacy `topoKey`-only
  `autoBind` still parses, the `topoKey` landing as evidence). *Fixture bump:*
  `worker/tests/fixtures/resolve_refs.ndjson` `r_autobind` now asserts the `elementId`
  slot present beside `topoKey`. No wire-shape change beyond code-to-spec (§7.5 already
  specified `elementId`); no canonical `protocol/fixtures/` change (they carry no
  `ResolveRefs` `autoBind` flow).

- **2026-07-19 — M4a: Extrude/Revolve profile carries `params.sketchId` +
  `params.regionId`; the worker selects the region by normative FNV id**
  (code-to-spec; **orchestrator-approved 2026-07-19**).
  [§7.3](#73-op-payload-schemas-vertical-slice), [§7.4](#74-sketch-solver-lane). The
  Rust wire layer lifts the core-only `profile` (`SketchRegionRef {sketchId,
  regionId}`) to top-level `params.sketchId` + `params.regionId` (dropping the
  `profile` wrapper — §7.3 has no `profile`; Extrude AND Revolve), and the worker's
  `build_profile_face` selects the closed region whose normative FNV `regionId`
  ([§7.4](#74-sketch-solver-lane) `derive_region_id`, `r_<hash>`) matches. **Strict
  semantics:** a **non-empty** `regionId` MUST match a detected region — **no match is
  a deterministic `OP_FAILED`** (the `perStepResults` message names the requested id +
  the available ids; downstream is blocked, publish ≤ m−1), **never** a silent fallback
  to a different region (a stale id after a sketch edit must fail loudly, not extrude a
  wrong profile — the "never a silent wrong bind" principle). An **empty/absent**
  `regionId` keeps the V1 **first-region** fallback (single-region sketches; the
  region-selection micro-slice does not yet author a real id everywhere). Additive:
  `perStepResults[].message` on a failed step carries the §8 recoverable reason (a
  failed step emits no `planStep`, so this is its only channel to Rust; readers ignore
  unknown keys, §4). This closes the M2 `last_sketch_id` + first-region binding gap
  (multi-region / multi-sketch). *Fixture:* `worker/tests/fixtures/executeplan_region_nomatch.ndjson`
  (new) pins the no-match `OP_FAILED`. No canonical `protocol/fixtures/` change.

- **2026-07-19 — Rust wire layer conformance fix: params body-bearing fields now
  rendered in [§2](#2-identifier--scalar-types) `body_<uuid>` wire form**
  (code-to-spec; **no schema semantic change**). [§7.3](#73-op-payload-schemas-vertical-slice).
  The Rust wire translator now renders every body-bearing op-`params` field
  (`targetBodyId`, `toolBodyId`, `axis.bodyId`, and `targetFace(2).primary.bodyId`)
  in the worker's `body_<uuid>` id form on the way out, matching the `inputs[]`
  semantic-ref rendering that already did so. The `BodyId` core wire encoding was
  a bare uuid (the frozen document schema), which the worker's `body_<opId>`-keyed
  `BodyStore` could never resolve (standalone Boolean / Extrude-Cut/Add / ToFace all
  failed `REF_UNRESOLVED` / NeedsRepair). *This bullet aligns the code with the
  already-normative [§2](#2-identifier--scalar-types) `BodyId` wire form; the wire
  shape the worker parses is unchanged, so no `protocol/fixtures/` file changes (they
  already carry `body_<opId>`-form ids, never bare uuids). Orchestrator-approved
  2026-07-19.*

- **2026-07-18 — a from-0 plan is always base-valid; accept replaces the head
  wholesale** (D5, orchestrator-approved; R-WP11.2). [§7.2](#72-regen--executeplan).
  A **from-0 plan** — no `baseCheckpoint` AND `expectedBaseHash` == the empty-prefix
  anchor (`e3b0c442…`) — is now ALWAYS base-valid: the worker **skips the
  `expectedBaseHash` head-hash comparison**, builds the scratch from a **genuinely
  empty base**, and on `AcceptPrepared` **replaces the head wholesale** (bodies,
  partitions, `historyPrefixHash` = echoed last prefix token, adopted
  `documentRevision`, bumped `snapshotId`). `workerEpoch` fencing and all
  `AcceptPrepared`/`DiscardPrepared` fencing are unchanged; **incremental** plans
  (nonzero `expectedBaseHash`) keep the strict head-hash fence. *Reason:*
  `expectedBaseHash` pins the base a plan builds on, and a from-0 plan's base IS empty
  by definition, so its precondition is satisfiable regardless of the head. V1 is
  full-replay + wholesale-publish (the `RegenPlanner` always emits from-0 plans;
  checkpoints are UNSUPPORTED, [§7.7](#77-checkpoints)); once the head token advanced
  past the empty anchor at the first `AcceptPrepared`, the strict head-hash fence
  rejected every subsequent regen (the sequential-regen blocker before the M2 gate).
  No canonical `protocol/fixtures/` file embeds the old from-0 fencing rule (they
  carry no post-accept sequential-regen flow), so no canonical fixture bump is
  required; the worker's local `test_wp5_plan` + the Rust `worker_chaos` /
  `document_runtime` / `real_worker_smoke` tests cover the D5 sequence, and the
  `worker_chaos` F4 negative was retargeted from the empty anchor (now the from-0
  exemption) to a nonzero wrong hash.

- **2026-07-18 — `documentRevision` is a Rust-owned advisory stamp, not a fencing
  token; fencing = `expectedBaseHash` + `workerEpoch`** (D4, orchestrator-approved;
  R-WP11.1). [§2](#2-identifier--scalar-types), [§7.2](#72-regen--executeplan),
  [§8](#8-error-taxonomy). `ExecutePlan` and `AcceptPrepared` fencing drops the
  `documentRevision` equality check: a session-mutating request is fenced on
  `workerEpoch` (must match the head) **and** `expectedBaseHash` (must equal the head
  `historyPrefixHash`) only. `documentRevision` is a Rust-owned edit counter carried
  as an advisory stamp; the worker stores the plan's `documentRevision` in the
  prepared scratch and **adopts** it as its head `documentRevision` at
  `AcceptPrepared` (worker frame stamps thereafter echo it), instead of incrementing a
  worker-owned accept counter. *Reason:* the pre-D4 worker advanced `documentRevision`
  as its own accept counter, which diverged from Rust's edit counter; every post-edit
  regen (whose `documentRevision` runs ahead of the worker's last-accepted head) was
  then rejected with `PROTOCOL_ERROR`. Making `documentRevision` advisory + adopted
  fixes that while keeping the real precondition guards (`workerEpoch` +
  `expectedBaseHash`). No canonical `protocol/fixtures/` file embeds the old
  `documentRevision` fencing (they carry no `ExecutePlan` fencing flow), so no
  canonical fixture bump is required; the worker's local `executeplan_*` harness
  fixtures were updated to the D4 rule.

- **2026-07-17 — NeedsRepair evidence carries `scoringVersion`** (W-WP6,
  orchestrator sign-off pending). [§9](#9-needsrepair-payload) every NeedsRepair
  payload (in `planStep.needsRepair[]`, `ResolveRefs`, and the history-stage split
  ambiguity) now stamps `scoringVersion` = the `resolverVersion`
  ([§10](#10-resolution-ladder)) under which the candidate
  `score`/`margin`/`featureContributions` were computed. *Reason:* the normalized
  [0,1] scoring is versioned; a repair UI or a future Rust-side policy must know
  which scheme produced the numbers to compare or re-evaluate them. Additive +
  forward-compatible (readers ignore unknown keys per [§4](#4-json-encoding-rules));
  no shape change to existing fields.

- **2026-07-17 — NewBody `BodyId`s are worker-minted deterministic `body_<opId>`,
  adopted+fenced by Rust** (D1, orchestrator-approved; R-WP10). [§2](#2-identifier--scalar-types)
  and [§7.2](#72-regen--executeplan). A `bodyEvents` `created` id is now
  worker-minted `body_<opId>` (`<opId>` = the Rust-minted op record id, so the id is
  a pure function of the plan and replay is stable); Rust **adopts** it at
  `AcceptPrepared`, validating the `body_` prefix + a **known opId** + **uniqueness**,
  and **rejects** the prepared plan (`PROTOCOL_ERROR`, discard) on malformation or
  collision. A future split mints `body_<opId>:<k>` (deferred to W-WP6). *Reason:*
  split/merge body counts are unknowable before OCCT executes, so Rust could never
  pre-mint them; `opId` is Rust-owned, so determinism and replay stability hold with
  worker minting + Rust adoption. This refines the §2 `BodyId` "Rust-minted" note
  (loaded/imported bodies stay Rust-minted; only NewBody flips to worker-mint +
  adopt). No fixture embeds a contrary minting assumption (the current fixtures use
  `body_1` only as a loaded-body example), so no fixture bump is required.

- **2026-07-17 — Rust is the sole hash authority; `ExecutePlan` gains
  `prefixHashes`** (X-WP1, orchestrator-signed). [§7.2](#72-regen--executeplan)
  `ExecutePlan.args` adds `prefixHashes: [hex64, …]` (one opaque token per executed
  op, in `ops` order) alongside the existing `expectedBaseHash`. **Both are
  Rust-minted opaque tokens the worker stores/compares/echoes but NEVER computes.**
  Their provenance is now documented: the SHA-256 over the newline-joined
  *geometry-relevant canonical wire-op form* (`{opId, opType, stepIndex, inputs,
  params, determinism}`, sorted-key JSON), which **excludes** record-level cosmetics
  (`name`, record `extra`, `suppressed`) so a rename never invalidates a checkpoint
  while any geometry-affecting edit does. On `PlanPrepared` the worker echoes the
  token for its last executed op (or `expectedBaseHash` for a base-only prepare) as
  `historyPrefixHash`; Rust verifies the echo (mismatch ⇒ `PROTOCOL_ERROR`). *Reason:*
  the worker cannot see the Rust record shape, so an independently-computed hash
  would diverge; making the token opaque removes a class of false `PROTOCOL_ERROR`s
  and lets a rename/cosmetic edit reuse a checkpoint.

- **2026-07-17 — `elementMapDelta` entries require `bodyId`** (R-WP7.1 review F19,
  orchestrator-signed). [§7.2](#72-regen--executeplan) each `elementMapDelta.added`
  / `.relabeled` entry is now `{ elementId, topoKey, kind, bodyId }` — the owning
  body is **REQUIRED**, not inferred. *Reason:* a single step can create/modify
  several bodies; without an explicit `bodyId` the partition mapping had to guess
  (the "most-recently-created body" heuristic), which mis-partitioned elements when
  one step produced two bodies. The §7.2 example JSON was updated. `bodyId` is
  partition membership only — an element's identity (`elementId`) never changes
  because geometry changed (Invariant 1).

- **2026-07-17 — double-`ExecutePlan`-while-prepared rule** (W-WP4 → recorded here).
  [§7.2](#72-regen--executeplan) pins worker behaviour when an `ExecutePlan` arrives
  while a scratch job is already prepared: **same `jobId`** ⇒ idempotent retransmit,
  reply with the **cached `PlanPrepared`** (no re-execution); **different `jobId`** ⇒
  `PROTOCOL_ERROR` with `detail = { preparedJobId, requestedJobId }`, the held job
  left untouched. *Reason:* a worker holds at most one prepared job; the rule makes
  request retransmission safe and forbids clobbering an outstanding prepare.

- **2026-07-16 — Extrude ToFace targets are typed semantic refs** (R-WP2.1,
  orchestrator-signed). [§7.3](#73-op-payload-schemas-vertical-slice) Extrude
  replaces the bare-string `targetFaceId` / `targetFaceId2` with
  `targetFace` / `targetFace2` **semantic reference** objects (`{primary, intent,
  anchor}`, the shape already used by fillet edge refs). *Reason:* a bare id
  carries no anchor/intent, so a ToFace target could not be rebound by the
  resolution ladder after a parametric edit — it would be un-repairable,
  violating Invariants 2/3. The example JSON was updated accordingly. No other
  §7.3 op needed the same treatment: the Revolve `axis` is already a structured
  ref (`sketchLine`/`edge` with typed subfields), Boolean `targetBodyId`/
  `toolBodyId` reference whole **bodies** (referenced directly by id, not
  ladder-resolved sub-elements), and Fillet/Chamfer `edgeIds` stay bare strings
  because their per-edge repair evidence already rides in the op's `inputs[]`
  semantic refs (mirrored in the Rust core by `FilletParams.edges`).

- **2026-07-16 — Scalar/dimension fields accept number OR object** (R-WP2.1).
  [§7.3](#73-op-payload-schemas-vertical-slice) now states explicitly that a
  dimensional param may be a bare number or a `{value, expr?}` object and that
  both producers/readers must accept either; the Rust core normalizes to the
  object form on write. Documents the file↔wire form already in effect; no shape
  change to the examples (which keep the bare-number spelling).

- **2026-07-16 — `regionId` derivation made normative** (R-WP3 → recorded here).
  [§7.4](#74-sketch-solver-lane) SketchRegions now pins the exact FNV-1a-64
  algorithm (sorted 16-byte member UUIDs + winding byte, rendered `"r_%016x"`) so
  the C++ worker and Rust core produce identical region ids; reference impl is
  onecad-core `sketch/mod.rs::derive_region_id`.
