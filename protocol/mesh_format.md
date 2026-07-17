# MESH1 — Binary Mesh Format

Status: canonical. Version `1`. Produced by the C++ worker (`Tessellate`,
[`SCHEMA.md §7.6`](./SCHEMA.md#76-geometry)), forwarded **verbatim** by the Rust
core (Rust validates the header only and does not repack), parsed zero-copy in
TypeScript. All multi-byte fields are **little-endian**. Section data is **4-byte
aligned**.

MESH1 is a self-contained blob. In the OCW1 transport frame it rides either inline
in a `bin` section or split across a bulk chunk stream
([`SCHEMA.md §5.2`](./SCHEMA.md#52-chunked-bulk-streams)); the transport's
`bin`/chunk framing is a **separate** layer from MESH1's own internal section
table below. A MESH1 blob is reassembled from the stream **before** parsing.

IDs inside a MESH1 blob are **snapshot-scoped TopoKeys** (`"f:22"`, `"e:5"`) and,
where already minted, persistent **ElementIds**. TopoKeys are valid only within the
`snapshotId` that produced the mesh ([`SCHEMA.md §2`](./SCHEMA.md#2-identifier--scalar-types)).

---

## 1. Layout overview

```
[ 64-byte header ]
[ section table : sectionCount × 16-byte entries, sorted by offset ]
[ section data : each section 4-byte aligned ]
```

All offsets in the header/table are byte offsets **from the start of the MESH1
blob** (byte 0 = the `magic`).

---

## 2. Header (64 bytes)

Exactly 64 bytes (one cache line). Fields (LE):

| Offset | Size | Type | Field | Notes |
|-------:|-----:|------|-------|-------|
| 0x00 | 4 | u32 | `magic` | `0x4D455348` (the ASCII codes of `"MESH"`: `4D 45 53 48`). Stored/read as a **little-endian u32**, so the on-wire byte sequence is `48 53 45 4D`. Read back with a LE u32 load it equals `0x4D455348`. Compare the u32 value (LE), or the 4 stream bytes `48 53 45 4D` — not the ASCII string `"MESH"` against stream order. |
| 0x04 | 2 | u16 | `version` | `1` |
| 0x06 | 2 | u16 | `flags` | bitfield, see below |
| 0x08 | 4 | u32 | `vertexCount` | `V` |
| 0x0C | 4 | u32 | `triangleCount` | `T` |
| 0x10 | 4 | u32 | `faceCount` | `F` |
| 0x14 | 4 | u32 | `edgeCount` | `E` |
| 0x18 | 4 | u32 | `edgePointCount` | `P` (total polyline points across all edges) |
| 0x1C | 2 | u16 | `lod` | `0`=coarse, `1`=medium, `2`=fine |
| 0x1E | 2 | u16 | `sectionCount` | number of section-table entries |
| 0x20 | 4 | f32 | `bboxMinX` | |
| 0x24 | 4 | f32 | `bboxMinY` | |
| 0x28 | 4 | f32 | `bboxMinZ` | |
| 0x2C | 4 | f32 | `bboxMaxX` | |
| 0x30 | 4 | f32 | `bboxMaxY` | |
| 0x34 | 4 | f32 | `bboxMaxZ` | |
| 0x38 | 4 | u32 | `reserved0` | MUST be `0` |
| 0x3C | 4 | u32 | `reserved1` | MUST be `0` |

`flags` bits:

| Bit | Mask | Meaning |
|----:|------|---------|
| 0 | `0x0001` | `HAS_NORMALS` — NORMALS section present |
| 1 | `0x0002` | `HAS_EDGES` — EDGE_* sections present |
| 2 | `0x0004` | `HAS_FACE_BBOXES` — FACE_BBOXES section present |
| 3 | `0x0008` | `IDS_HAVE_ELEMENTIDS` — face/edge id tables contain minted ElementIds (else pure TopoKeys) |
| 4–15 | | reserved (MUST be 0) |

Buffers are **Z-up right-handed** (hard frontend invariant): positions are
consumed verbatim into GPU buffers, no axis swaps.

---

## 3. Section table

Immediately follows the header (starts at offset `0x40`). `sectionCount` entries,
**16 bytes each**, **sorted by `offset` ascending**:

| Offset in entry | Size | Type | Field | Notes |
|----------------:|-----:|------|-------|-------|
| +0x00 | 4 | u32 | `type` | section type, see §4 |
| +0x04 | 4 | u32 | `pad` | reserved, MUST be `0` |
| +0x08 | 4 | u32 | `offset` | byte offset of section data from blob start |
| +0x0C | 4 | u32 | `byteLen` | length of section data in bytes |

`offset` and `byteLen` are `u32` (not `u64`): a section lives inside one MESH1 blob
whose transport is capped by the frame `binLen ≤ 1 GiB`
([`SCHEMA.md §1`](./SCHEMA.md#1-frame-layout-ocw1)), so `u32` is sufficient and
keeps entries at the stated 16 bytes with uniform 4-byte words (ideal for zero-copy
`Uint32Array` reads). `offset` MUST be 4-byte aligned; the gap between a table/prev
section end and the next `offset` is zero-padding.

A section `type` MUST appear at most once. Unknown `type` values MUST be skipped
(forward-compat).

---

## 4. Section types

| `type` | Name | Element | Length | Contents |
|-------:|------|---------|--------|----------|
| 1 | `POSITIONS` | required | `12·V` | f32 × 3 × `V` — vertex xyz |
| 2 | `NORMALS` | if `HAS_NORMALS` | `12·V` | f32 × 3 × `V` — per-vertex smoothed normals |
| 3 | `INDICES` | required | `12·T` | u32 × 3 × `T` — triangle vertex indices, **grouped by face** (all of face 0's triangles, then face 1's, …) |
| 4 | `FACE_RANGES` | required | `8·F` | per face: `{ u32 firstTri, u32 triCount }` into INDICES (triangle units) |
| 5 | `FACE_ID_OFFS` | required | `4·(F+1)` | u32 prefix-sum offsets into FACE_ID_CHARS; id `i` = bytes `[offs[i], offs[i+1])` |
| 6 | `FACE_ID_CHARS` | required | `offs[F]` | UTF-8 bytes of all face ids concatenated (TopoKey `"f:22"` or ElementId) |
| 7 | `EDGE_RANGES` | if `HAS_EDGES` | `8·E` | per edge: `{ u32 firstPoint, u32 pointCount }` into EDGE_POSITIONS (point units) |
| 8 | `EDGE_POSITIONS` | if `HAS_EDGES` | `12·P` | f32 × 3 × `P` — polyline points, grouped by edge |
| 9 | `EDGE_ID_OFFS` | if `HAS_EDGES` | `4·(E+1)` | u32 prefix-sum offsets into EDGE_ID_CHARS |
| 10 | `EDGE_ID_CHARS` | if `HAS_EDGES` | `offs[E]` | UTF-8 bytes of all edge ids concatenated (`"e:5"` or ElementId) |
| 11 | `FACE_BBOXES` | optional | `24·F` | per face: f32 × 6 `{minX,minY,minZ,maxX,maxY,maxZ}` — pick accel |

Notes:
- Triangle → face lookup: binary-search FACE_RANGES for the triangle index (ranges
  are contiguous and ordered because INDICES is grouped by face). The face id is
  then `FACE_ID_CHARS[FACE_ID_OFFS[faceIndex] .. FACE_ID_OFFS[faceIndex+1]]`.
- Id tables use the offset+chars pattern so the whole id set is two contiguous
  arrays (one `TextDecoder` slice per pick, no per-id allocation up front).
- `POSITIONS` and `INDICES` are the only always-required data sections;
  `FACE_RANGES`/`FACE_ID_OFFS`/`FACE_ID_CHARS` are required so every triangle is
  attributable to a face id (picking).

---

## 5. Worked example — tiny 2-face mesh

A unit square in the XY plane (`z=0`), split into two triangles, each triangle its
own face, plus one edge (the diagonal) as a 2-point polyline.

Geometry:
- Vertices (`V=4`): `v0(0,0,0) v1(1,0,0) v2(1,1,0) v3(0,1,0)`
- Triangles (`T=2`): `T0=[0,1,2]` (face 0), `T1=[0,2,3]` (face 1)
- Faces (`F=2`): id `"f:7"`, id `"f:12"`
- Edges (`E=1`, `P=2`): id `"e:5"`, points `v0(0,0,0) → v2(1,1,0)`
- Normals: all `(0,0,1)`
- bbox: min `(0,0,0)`, max `(1,1,0)`
- `flags = HAS_NORMALS | HAS_EDGES = 0x0003`; `lod = 0`

Sections present (10): POSITIONS, NORMALS, INDICES, FACE_RANGES, FACE_ID_OFFS,
FACE_ID_CHARS, EDGE_RANGES, EDGE_POSITIONS, EDGE_ID_OFFS, EDGE_ID_CHARS →
`sectionCount = 10`.

### 5.1 Offset map

Header 64 B (`0x00–0x40`), table `10×16 = 160 B` (`0x40–0xE0`), data from `0xE0`:

| Section | type | offset (dec / hex) | byteLen | end |
|---------|-----:|-------------------:|--------:|----:|
| POSITIONS | 1 | 224 / 0x0E0 | 48 (`12·4`) | 272 |
| NORMALS | 2 | 272 / 0x110 | 48 | 320 |
| INDICES | 3 | 320 / 0x140 | 24 (`12·2`) | 344 |
| FACE_RANGES | 4 | 344 / 0x158 | 16 (`8·2`) | 360 |
| FACE_ID_OFFS | 5 | 360 / 0x168 | 12 (`4·3`) | 372 |
| FACE_ID_CHARS | 6 | 372 / 0x174 | 7 | 379 → pad→380 |
| EDGE_RANGES | 7 | 380 / 0x17C | 8 (`8·1`) | 388 |
| EDGE_POSITIONS | 8 | 388 / 0x184 | 24 (`12·2`) | 412 |
| EDGE_ID_OFFS | 9 | 412 / 0x19C | 8 (`4·2`) | 420 |
| EDGE_ID_CHARS | 10 | 420 / 0x1A4 | 3 | 423 → pad→424 |

Total blob length = **424 bytes** (last section padded to 4-byte boundary).
`FACE_ID_CHARS` ends at 379 and is padded to 380 (next `offset` is 4-aligned);
`EDGE_ID_CHARS` ends at 423, blob padded to 424.

### 5.2 Header bytes (`0x00–0x40`)

```
0x00: 48 53 45 4D            magic  = 0x4D455348 (LE u32; stream bytes 48 53 45 4D)
0x04: 01 00                  version = 1
0x06: 03 00                  flags   = 0x0003 (HAS_NORMALS|HAS_EDGES)
0x08: 04 00 00 00            vertexCount   = 4
0x0C: 02 00 00 00            triangleCount = 2
0x10: 02 00 00 00            faceCount     = 2
0x14: 01 00 00 00            edgeCount     = 1
0x18: 02 00 00 00            edgePointCount= 2
0x1C: 00 00                  lod          = 0
0x1E: 0A 00                  sectionCount = 10
0x20: 00 00 00 00            bboxMinX = 0.0
0x24: 00 00 00 00            bboxMinY = 0.0
0x28: 00 00 00 00            bboxMinZ = 0.0
0x2C: 00 00 80 3F            bboxMaxX = 1.0
0x30: 00 00 80 3F            bboxMaxY = 1.0
0x34: 00 00 00 00            bboxMaxZ = 0.0
0x38: 00 00 00 00            reserved0 = 0
0x3C: 00 00 00 00            reserved1 = 0
```

### 5.3 Section table (`0x40–0xE0`), first two + FACE entries

Each entry: `type(u32) pad(u32) offset(u32) byteLen(u32)`. Sorted by `offset`.

```
0x40: 01 00 00 00  00 00 00 00  E0 00 00 00  30 00 00 00   POSITIONS   off=224 len=48
0x50: 02 00 00 00  00 00 00 00  10 01 00 00  30 00 00 00   NORMALS     off=272 len=48
0x60: 03 00 00 00  00 00 00 00  40 01 00 00  18 00 00 00   INDICES     off=320 len=24
0x70: 04 00 00 00  00 00 00 00  58 01 00 00  10 00 00 00   FACE_RANGES off=344 len=16
0x80: 05 00 00 00  00 00 00 00  68 01 00 00  0C 00 00 00   FACE_ID_OFFS  off=360 len=12
0x90: 06 00 00 00  00 00 00 00  74 01 00 00  07 00 00 00   FACE_ID_CHARS off=372 len=7
0xA0: 07 00 00 00  00 00 00 00  7C 01 00 00  08 00 00 00   EDGE_RANGES off=380 len=8
0xB0: 08 00 00 00  00 00 00 00  84 01 00 00  18 00 00 00   EDGE_POSITIONS off=388 len=24
0xC0: 09 00 00 00  00 00 00 00  9C 01 00 00  08 00 00 00   EDGE_ID_OFFS  off=412 len=8
0xD0: 0A 00 00 00  00 00 00 00  A4 01 00 00  03 00 00 00   EDGE_ID_CHARS off=420 len=3
```

### 5.4 Selected section data

POSITIONS (`0xE0`, 48 B) — `f32×3×4`, `1.0f = 00 00 80 3F`:

```
v0(0,0,0): 00000000 00000000 00000000
v1(1,0,0): 0000803F 00000000 00000000
v2(1,1,0): 0000803F 0000803F 00000000
v3(0,1,0): 00000000 0000803F 00000000
```

INDICES (`0x140`, 24 B) — `u32×3×2`, grouped by face (`T0` then `T1`):

```
00000000 01000000 02000000   T0 = 0,1,2  (face 0)
00000000 02000000 03000000   T1 = 0,2,3  (face 1)
```

FACE_RANGES (`0x158`, 16 B) — `{firstTri,triCount}` per face:

```
00000000 01000000   face 0: firstTri=0, triCount=1
01000000 01000000   face 1: firstTri=1, triCount=1
```

FACE_ID_OFFS (`0x168`, 12 B) — prefix sums `[0, 3, 7]`:

```
00000000 03000000 07000000
```

FACE_ID_CHARS (`0x174`, 7 B, padded to 8) — `"f:7"` + `"f:12"`:

```
66 3A 37   66 3A 31 32   00       ('f'':''7' 'f'':''1''2' pad)
```

EDGE_ID_OFFS (`0x19C`, 8 B) — `[0, 3]`; EDGE_ID_CHARS (`0x1A4`, 3 B, padded to 4)
`"e:5"` = `65 3A 35 00`.

---

## 6. Parsing guidance

### 6.1 TypeScript (zero-copy)

Reassemble the full blob into one `ArrayBuffer` (from the bulk stream), then take
**typed-array views** without copying:

```ts
const dv = new DataView(buf);
if (dv.getUint32(0, true) !== 0x4d455348) throw new Error("bad MESH1 magic");
const version = dv.getUint16(4, true);          // 1
const flags = dv.getUint16(6, true);
const V = dv.getUint32(8, true), T = dv.getUint32(12, true), F = dv.getUint32(16, true);
const sectionCount = dv.getUint16(0x1e, true);

// section table: uniform u32 words, zero-copy
const table = new Uint32Array(buf, 0x40, sectionCount * 4);
const sec = (type: number) => {
  for (let i = 0; i < sectionCount; i++) {
    if (table[i * 4] === type) return { off: table[i * 4 + 2], len: table[i * 4 + 3] };
  }
  return null;
};

const pos = sec(1)!;   // POSITIONS
const positions = new Float32Array(buf, pos.off, (pos.len / 4) | 0);  // view, no copy
const idx = sec(3)!;
const indices = new Uint32Array(buf, idx.off, (idx.len / 4) | 0);     // view, no copy
```

- `positions`/`normals`/`indices` become GPU buffers directly (Z-up verbatim; no
  axis swap).
- **Do not** eagerly decode id strings. Keep `FACE_ID_OFFS`/`FACE_ID_CHARS` as
  `Uint32Array`/`Uint8Array` views and **lazily** `TextDecoder`-decode a single id
  **on pick**: find the face via binary search over `FACE_RANGES`, then
  `new TextDecoder().decode(chars.subarray(offs[i], offs[i+1]))`.
- Alignment note: `new Float32Array(buf, off, …)` requires `off % 4 === 0`; all
  section offsets are 4-aligned by construction, so views are always valid. If a
  producer ever violates alignment the view constructor throws — treat as a
  protocol error.

### 6.2 Rust (validate header, forward verbatim)

Rust **does not parse** the mesh body. It validates the header and forwards the
blob unchanged into the `MeshCache` (keyed `(BodyId, Lod, generation)`) and then
to the webview as a `tauri::ipc::Response` ArrayBuffer.

Validation (cheap, header-only):

```rust
fn validate_mesh1(buf: &[u8]) -> Result<(), ProtocolError> {
    if buf.len() < 64 { return Err(ProtocolError::Framing("mesh < 64 bytes")); }
    if u32::from_le_bytes(buf[0..4].try_into().unwrap()) != 0x4D45_5348 {
        return Err(ProtocolError::Framing("bad MESH1 magic"));
    }
    if u16::from_le_bytes(buf[4..6].try_into().unwrap()) != 1 {
        return Err(ProtocolError::Framing("unsupported MESH1 version"));
    }
    let section_count = u16::from_le_bytes(buf[0x1e..0x20].try_into().unwrap()) as usize;
    let table_end = 0x40 + section_count * 16;
    if buf.len() < table_end { return Err(ProtocolError::Framing("truncated section table")); }
    // optional: check each (offset,byteLen) lies within buf and offset % 4 == 0
    Ok(())
}
```

Rust MUST NOT reinterpret, reorder, or re-encode sections — the buffer the worker
produced is the buffer the webview receives (Invariant 5: parallelism/meshing
never changes IDs or bytes for the same inputs).
