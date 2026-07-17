/*
 * MESH1 zero-copy parser — NORMATIVE implementation of protocol/mesh_format.md.
 *
 * A MESH1 blob is: [64-byte header][sectionCount × 16-byte table entries]
 * [section data, each 4-byte aligned]. All multi-byte fields little-endian.
 * Buffers are Z-up right-handed and consumed verbatim (no axis swap).
 *
 * We take TYPED-ARRAY VIEWS directly over the incoming ArrayBuffer — no copy.
 * Float32Array/Uint32Array views require a 4-byte-aligned byteOffset; every
 * section offset is 4-aligned by construction, and we validate the blob origin
 * is 4-aligned too, so views are always valid. ID char tables stay as
 * Uint8Array views and are decoded LAZILY, one id at a time, on pick (see
 * faceRangeIndex.ts) — never eagerly.
 *
 * ── Multi-body framing ──────────────────────────────────────────────────────
 * The REAL path is one MESH1 blob per `get_mesh` call: Rust's get_mesh returns a
 * single-body blob verbatim (mesh_format.md §6.2), so parseMeshPayload(buf) at
 * offset 0 is fully zero-copy. `parseMeshContainer` is a FRONTEND-SIDE CONVENTION
 * used only by the mock / multi-fetch to carry several bodies in one ArrayBuffer:
 *
 *     [ u32 count ][ count × u32 blobByteLen (each 4-aligned) ][ blob0 ][ blob1 ]…
 *
 * Each blob is itself a verbatim MESH1 blob and stays 4-aligned inside the
 * container, so parseMeshContainer views every blob in place — still zero-copy.
 * The real worker never emits a container; keep it simple and mock-only.
 */

// ── Section types (mesh_format.md §4) ───────────────────────────────────────
export const SEC = {
  POSITIONS: 1,
  NORMALS: 2,
  INDICES: 3,
  FACE_RANGES: 4,
  FACE_ID_OFFS: 5,
  FACE_ID_CHARS: 6,
  EDGE_RANGES: 7,
  EDGE_POSITIONS: 8,
  EDGE_ID_OFFS: 9,
  EDGE_ID_CHARS: 10,
  FACE_BBOXES: 11,
} as const;

// ── Header flag bits (mesh_format.md §2) ────────────────────────────────────
export const FLAG = {
  HAS_NORMALS: 0x0001,
  HAS_EDGES: 0x0002,
  HAS_FACE_BBOXES: 0x0004,
  IDS_HAVE_ELEMENTIDS: 0x0008,
} as const;
const FLAG_RESERVED_MASK = 0xfff0; // bits 4–15 MUST be 0

export const MESH1_MAGIC = 0x4d455348; // LE u32; on-wire bytes 48 53 45 4D
export const MESH1_VERSION = 1;
const HEADER_BYTES = 64;
const TABLE_ENTRY_BYTES = 16;

export type MeshParseErrorKind =
  | "truncated"
  | "bad-magic"
  | "unsupported-version"
  | "misaligned"
  | "section-overlap"
  | "section-bounds"
  | "missing-section"
  | "bad-length"
  | "reserved-nonzero"
  | "duplicate-section"
  | "bad-container";

/** Typed, catchable parse failure. `kind` classifies the protocol violation. */
export class MeshParseError extends Error {
  constructor(
    readonly kind: MeshParseErrorKind,
    message: string,
  ) {
    super(message);
    this.name = "MeshParseError";
  }
}

/** Zero-copy view over one parsed MESH1 blob. All arrays alias `buffer`. */
export interface BodyMeshView {
  readonly buffer: ArrayBuffer;
  /** Byte offset of this blob's `magic` within `buffer` (0 for a lone blob). */
  readonly blobByteOffset: number;

  readonly version: number;
  readonly flags: number;
  readonly lod: number;

  readonly vertexCount: number; // V
  readonly triangleCount: number; // T
  readonly faceCount: number; // F
  readonly edgeCount: number; // E
  readonly edgePointCount: number; // P

  readonly bboxMin: readonly [number, number, number];
  readonly bboxMax: readonly [number, number, number];

  readonly hasNormals: boolean;
  readonly hasEdges: boolean;
  readonly hasFaceBboxes: boolean;
  readonly idsHaveElementIds: boolean;

  // Required data (views, no copy).
  readonly positions: Float32Array; // 3·V
  readonly indices: Uint32Array; // 3·T
  readonly faceRanges: Uint32Array; // 2·F  {firstTri, triCount}
  readonly faceIdOffsets: Uint32Array; // F+1 prefix sums
  readonly faceIdChars: Uint8Array; // UTF-8 face ids, decoded lazily

  // Optional data (null when the corresponding flag is clear).
  readonly normals: Float32Array | null; // 3·V
  readonly edgeRanges: Uint32Array | null; // 2·E  {firstPoint, pointCount}
  readonly edgePositions: Float32Array | null; // 3·P
  readonly edgeIdOffsets: Uint32Array | null; // E+1
  readonly edgeIdChars: Uint8Array | null; // UTF-8 edge ids
  readonly faceBboxes: Float32Array | null; // 6·F
}

interface SectionRec {
  type: number;
  offset: number; // relative to blob start
  byteLen: number;
}

/**
 * Parse one MESH1 blob living at `[blobByteOffset, blobByteOffset+blobByteLength)`
 * inside `buffer`. Defaults parse a lone blob at offset 0. Returns zero-copy views.
 * Throws {@link MeshParseError} on any protocol violation.
 */
export function parseMeshPayload(
  buffer: ArrayBuffer,
  blobByteOffset = 0,
  blobByteLength: number = buffer.byteLength - blobByteOffset,
): BodyMeshView {
  if (blobByteOffset % 4 !== 0) {
    throw new MeshParseError(
      "misaligned",
      `blob origin ${blobByteOffset} is not 4-byte aligned`,
    );
  }
  if (blobByteLength < HEADER_BYTES || blobByteOffset + blobByteLength > buffer.byteLength) {
    throw new MeshParseError(
      "truncated",
      `blob shorter than 64-byte header (len=${blobByteLength})`,
    );
  }

  const dv = new DataView(buffer, blobByteOffset, blobByteLength);

  if (dv.getUint32(0x00, true) !== MESH1_MAGIC) {
    throw new MeshParseError("bad-magic", "bad MESH1 magic");
  }
  const version = dv.getUint16(0x04, true);
  if (version !== MESH1_VERSION) {
    throw new MeshParseError("unsupported-version", `unsupported MESH1 version ${version}`);
  }
  const flags = dv.getUint16(0x06, true);
  if ((flags & FLAG_RESERVED_MASK) !== 0) {
    throw new MeshParseError("reserved-nonzero", `reserved flag bits set: 0x${flags.toString(16)}`);
  }
  const vertexCount = dv.getUint32(0x08, true);
  const triangleCount = dv.getUint32(0x0c, true);
  const faceCount = dv.getUint32(0x10, true);
  const edgeCount = dv.getUint32(0x14, true);
  const edgePointCount = dv.getUint32(0x18, true);
  const lod = dv.getUint16(0x1c, true);
  const sectionCount = dv.getUint16(0x1e, true);
  const bboxMin: [number, number, number] = [
    dv.getFloat32(0x20, true),
    dv.getFloat32(0x24, true),
    dv.getFloat32(0x28, true),
  ];
  const bboxMax: [number, number, number] = [
    dv.getFloat32(0x2c, true),
    dv.getFloat32(0x30, true),
    dv.getFloat32(0x34, true),
  ];
  if (dv.getUint32(0x38, true) !== 0 || dv.getUint32(0x3c, true) !== 0) {
    throw new MeshParseError("reserved-nonzero", "header reserved0/reserved1 must be 0");
  }

  const tableEnd = HEADER_BYTES + sectionCount * TABLE_ENTRY_BYTES;
  if (tableEnd > blobByteLength) {
    throw new MeshParseError("truncated", "section table exceeds blob length");
  }

  // Section table: uniform u32 words, read as a zero-copy Uint32Array.
  const table = new Uint32Array(buffer, blobByteOffset + HEADER_BYTES, sectionCount * 4);
  const byType = new Map<number, SectionRec>();
  let prevEnd = tableEnd; // first section must start at/after the table end
  for (let i = 0; i < sectionCount; i++) {
    const type = table[i * 4 + 0];
    const pad = table[i * 4 + 1];
    const offset = table[i * 4 + 2];
    const byteLen = table[i * 4 + 3];
    if (pad !== 0) {
      throw new MeshParseError("reserved-nonzero", `section entry ${i} pad must be 0`);
    }
    if (offset % 4 !== 0) {
      throw new MeshParseError("misaligned", `section ${type} offset ${offset} not 4-aligned`);
    }
    // Table is sorted by offset ascending; consecutive sections must not overlap
    // (a gap of ≤3 padding bytes between prevEnd and offset is allowed).
    if (offset < prevEnd) {
      throw new MeshParseError(
        "section-overlap",
        `section ${type} offset ${offset} overlaps/precedes previous end ${prevEnd}`,
      );
    }
    const end = offset + byteLen;
    if (end > blobByteLength || end < offset) {
      throw new MeshParseError(
        "section-bounds",
        `section ${type} [${offset},${end}) out of blob bounds ${blobByteLength}`,
      );
    }
    // Unknown types are skipped (forward-compat) but still occupy their bytes.
    if (byType.has(type)) {
      throw new MeshParseError("duplicate-section", `section type ${type} appears twice`);
    }
    byType.set(type, { type, offset, byteLen });
    prevEnd = end;
  }

  const f32 = (rec: SectionRec, expectedFloats: number, name: string): Float32Array => {
    if (rec.byteLen !== expectedFloats * 4) {
      throw new MeshParseError(
        "bad-length",
        `${name} byteLen ${rec.byteLen} != ${expectedFloats * 4}`,
      );
    }
    return new Float32Array(buffer, blobByteOffset + rec.offset, expectedFloats);
  };
  const u32 = (rec: SectionRec, expectedWords: number, name: string): Uint32Array => {
    if (rec.byteLen !== expectedWords * 4) {
      throw new MeshParseError(
        "bad-length",
        `${name} byteLen ${rec.byteLen} != ${expectedWords * 4}`,
      );
    }
    return new Uint32Array(buffer, blobByteOffset + rec.offset, expectedWords);
  };
  const req = (type: number, name: string): SectionRec => {
    const rec = byType.get(type);
    if (!rec) throw new MeshParseError("missing-section", `required section ${name} absent`);
    return rec;
  };

  // ── Required sections ──
  const positions = f32(req(SEC.POSITIONS, "POSITIONS"), 3 * vertexCount, "POSITIONS");
  const indices = u32(req(SEC.INDICES, "INDICES"), 3 * triangleCount, "INDICES");
  const faceRanges = u32(req(SEC.FACE_RANGES, "FACE_RANGES"), 2 * faceCount, "FACE_RANGES");
  const faceIdOffsets = u32(req(SEC.FACE_ID_OFFS, "FACE_ID_OFFS"), faceCount + 1, "FACE_ID_OFFS");
  const faceCharsRec = req(SEC.FACE_ID_CHARS, "FACE_ID_CHARS");
  const faceCharsLen = faceIdOffsets[faceCount];
  if (faceCharsRec.byteLen !== faceCharsLen) {
    throw new MeshParseError(
      "bad-length",
      `FACE_ID_CHARS byteLen ${faceCharsRec.byteLen} != offs[F] ${faceCharsLen}`,
    );
  }
  const faceIdChars = new Uint8Array(buffer, blobByteOffset + faceCharsRec.offset, faceCharsLen);

  const hasNormals = (flags & FLAG.HAS_NORMALS) !== 0;
  const hasEdges = (flags & FLAG.HAS_EDGES) !== 0;
  const hasFaceBboxes = (flags & FLAG.HAS_FACE_BBOXES) !== 0;
  const idsHaveElementIds = (flags & FLAG.IDS_HAVE_ELEMENTIDS) !== 0;

  // ── Optional sections ──
  let normals: Float32Array | null = null;
  if (hasNormals) normals = f32(req(SEC.NORMALS, "NORMALS"), 3 * vertexCount, "NORMALS");

  let edgeRanges: Uint32Array | null = null;
  let edgePositions: Float32Array | null = null;
  let edgeIdOffsets: Uint32Array | null = null;
  let edgeIdChars: Uint8Array | null = null;
  if (hasEdges) {
    edgeRanges = u32(req(SEC.EDGE_RANGES, "EDGE_RANGES"), 2 * edgeCount, "EDGE_RANGES");
    edgePositions = f32(req(SEC.EDGE_POSITIONS, "EDGE_POSITIONS"), 3 * edgePointCount, "EDGE_POSITIONS");
    edgeIdOffsets = u32(req(SEC.EDGE_ID_OFFS, "EDGE_ID_OFFS"), edgeCount + 1, "EDGE_ID_OFFS");
    const edgeCharsRec = req(SEC.EDGE_ID_CHARS, "EDGE_ID_CHARS");
    const edgeCharsLen = edgeIdOffsets[edgeCount];
    if (edgeCharsRec.byteLen !== edgeCharsLen) {
      throw new MeshParseError(
        "bad-length",
        `EDGE_ID_CHARS byteLen ${edgeCharsRec.byteLen} != offs[E] ${edgeCharsLen}`,
      );
    }
    edgeIdChars = new Uint8Array(buffer, blobByteOffset + edgeCharsRec.offset, edgeCharsLen);
  }

  let faceBboxes: Float32Array | null = null;
  if (hasFaceBboxes) faceBboxes = f32(req(SEC.FACE_BBOXES, "FACE_BBOXES"), 6 * faceCount, "FACE_BBOXES");

  return {
    buffer,
    blobByteOffset,
    version,
    flags,
    lod,
    vertexCount,
    triangleCount,
    faceCount,
    edgeCount,
    edgePointCount,
    bboxMin,
    bboxMax,
    hasNormals,
    hasEdges,
    hasFaceBboxes,
    idsHaveElementIds,
    positions,
    indices,
    faceRanges,
    faceIdOffsets,
    faceIdChars,
    normals,
    edgeRanges,
    edgePositions,
    edgeIdOffsets,
    edgeIdChars,
    faceBboxes,
  };
}

/**
 * Parse a frontend-side multi-body container (see file header). Returns one
 * {@link BodyMeshView} per blob, each viewing its slice of `buffer` in place.
 */
export function parseMeshContainer(buffer: ArrayBuffer): BodyMeshView[] {
  if (buffer.byteLength < 4) {
    throw new MeshParseError("bad-container", "container shorter than 4-byte count");
  }
  const head = new DataView(buffer);
  const count = head.getUint32(0, true);
  const lenTableEnd = 4 + count * 4;
  if (lenTableEnd > buffer.byteLength) {
    throw new MeshParseError("bad-container", "container length table exceeds buffer");
  }
  const lengths = new Uint32Array(buffer, 4, count);
  const views: BodyMeshView[] = [];
  let cursor = lenTableEnd; // first blob starts after the length table (4-aligned)
  for (let i = 0; i < count; i++) {
    const len = lengths[i];
    if (len % 4 !== 0) {
      throw new MeshParseError("bad-container", `container blob ${i} length ${len} not 4-aligned`);
    }
    if (cursor + len > buffer.byteLength) {
      throw new MeshParseError("bad-container", `container blob ${i} exceeds buffer`);
    }
    views.push(parseMeshPayload(buffer, cursor, len));
    cursor += len;
  }
  return views;
}
