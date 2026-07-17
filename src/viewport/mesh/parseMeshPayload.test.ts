/*
 * MESH1 parser conformance: worked-example golden (byte-identical to
 * mesh_format.md §5), mock synth ↔ parse round-trip, and the typed error surface
 * (truncated / bad-magic / misaligned / overlap / bad-length / container).
 */
import { describe, it, expect } from "vitest";
import {
  parseMeshPayload,
  parseMeshContainer,
  MeshParseError,
  MESH1_MAGIC,
  SEC,
  FLAG,
} from "./parseMeshPayload";
import { encodeMesh1, makeBoxMesh, makeCylinderMesh } from "@/ipc/mockMeshes";

// The exact §5 mesh: unit square, two 1-triangle faces, one diagonal edge.
function workedExample(): ArrayBuffer {
  return encodeMesh1({
    positions: [0, 0, 0, 1, 0, 0, 1, 1, 0, 0, 1, 0],
    normals: [0, 0, 1, 0, 0, 1, 0, 0, 1, 0, 0, 1],
    faces: [
      { triangles: [[0, 1, 2]], id: "f:7" },
      { triangles: [[0, 2, 3]], id: "f:12" },
    ],
    edges: [{ points: [[0, 0, 0], [1, 1, 0]], id: "e:5" }],
    lod: 0,
  });
}

/** Read the section table into a {type: {offset, len}} map. */
function readTable(buf: ArrayBuffer): Map<number, { offset: number; len: number }> {
  const dv = new DataView(buf);
  const count = dv.getUint16(0x1e, true);
  const map = new Map<number, { offset: number; len: number }>();
  for (let i = 0; i < count; i++) {
    const base = 64 + i * 16;
    map.set(dv.getUint32(base, true), {
      offset: dv.getUint32(base + 8, true),
      len: dv.getUint32(base + 12, true),
    });
  }
  return map;
}

describe("MESH1 worked example (§5) — byte-identical to the doc", () => {
  const buf = workedExample();
  const dv = new DataView(buf);

  it("blob is 424 bytes", () => {
    expect(buf.byteLength).toBe(424);
  });

  it("magic on-wire bytes are 48 53 45 4D and read back as the LE u32", () => {
    const u8 = new Uint8Array(buf);
    expect([u8[0], u8[1], u8[2], u8[3]]).toEqual([0x48, 0x53, 0x45, 0x4d]);
    expect(dv.getUint32(0, true)).toBe(MESH1_MAGIC);
  });

  it("header fields match §5.2", () => {
    expect(dv.getUint16(0x04, true)).toBe(1); // version
    expect(dv.getUint16(0x06, true)).toBe(FLAG.HAS_NORMALS | FLAG.HAS_EDGES); // 0x0003
    expect(dv.getUint32(0x08, true)).toBe(4); // V
    expect(dv.getUint32(0x0c, true)).toBe(2); // T
    expect(dv.getUint32(0x10, true)).toBe(2); // F
    expect(dv.getUint32(0x14, true)).toBe(1); // E
    expect(dv.getUint32(0x18, true)).toBe(2); // P
    expect(dv.getUint16(0x1c, true)).toBe(0); // lod
    expect(dv.getUint16(0x1e, true)).toBe(10); // sectionCount
    expect(dv.getFloat32(0x2c, true)).toBe(1); // bboxMaxX
    expect(dv.getFloat32(0x30, true)).toBe(1); // bboxMaxY
    expect(dv.getFloat32(0x34, true)).toBe(0); // bboxMaxZ
    expect(dv.getUint32(0x38, true)).toBe(0); // reserved0
    expect(dv.getUint32(0x3c, true)).toBe(0); // reserved1
  });

  it("section table offsets/lengths match §5.1 exactly", () => {
    const t = readTable(buf);
    expect(t.get(SEC.POSITIONS)).toEqual({ offset: 224, len: 48 });
    expect(t.get(SEC.NORMALS)).toEqual({ offset: 272, len: 48 });
    expect(t.get(SEC.INDICES)).toEqual({ offset: 320, len: 24 });
    expect(t.get(SEC.FACE_RANGES)).toEqual({ offset: 344, len: 16 });
    expect(t.get(SEC.FACE_ID_OFFS)).toEqual({ offset: 360, len: 12 });
    expect(t.get(SEC.FACE_ID_CHARS)).toEqual({ offset: 372, len: 7 });
    expect(t.get(SEC.EDGE_RANGES)).toEqual({ offset: 380, len: 8 });
    expect(t.get(SEC.EDGE_POSITIONS)).toEqual({ offset: 388, len: 24 });
    expect(t.get(SEC.EDGE_ID_OFFS)).toEqual({ offset: 412, len: 8 });
    expect(t.get(SEC.EDGE_ID_CHARS)).toEqual({ offset: 420, len: 3 });
  });

  it("FACE_ID_CHARS is 'f:7'+'f:12' UTF-8 (66 3A 37 66 3A 31 32)", () => {
    const u8 = new Uint8Array(buf, 372, 7);
    expect([...u8]).toEqual([0x66, 0x3a, 0x37, 0x66, 0x3a, 0x31, 0x32]);
  });

  it("parses back with correct counts, views, and lazy ids", () => {
    const v = parseMeshPayload(buf);
    expect(v.vertexCount).toBe(4);
    expect(v.triangleCount).toBe(2);
    expect(v.faceCount).toBe(2);
    expect(v.edgeCount).toBe(1);
    expect(v.edgePointCount).toBe(2);
    expect(v.hasNormals).toBe(true);
    expect(v.hasEdges).toBe(true);
    expect([...v.indices]).toEqual([0, 1, 2, 0, 2, 3]);
    expect([...v.faceRanges]).toEqual([0, 1, 1, 1]);
    expect([...v.faceIdOffsets]).toEqual([0, 3, 7]);
    expect(new TextDecoder().decode(v.faceIdChars.subarray(0, 3))).toBe("f:7");
    expect(new TextDecoder().decode(v.faceIdChars.subarray(3, 7))).toBe("f:12");
    // Views alias the original buffer (zero-copy).
    expect(v.positions.buffer).toBe(buf);
  });
});

describe("mock synth ↔ parse round-trip", () => {
  it("box: 6 faces / 12 edges / crease-split verts", () => {
    const v = parseMeshPayload(makeBoxMesh());
    expect(v.faceCount).toBe(6);
    expect(v.edgeCount).toBe(12);
    expect(v.vertexCount).toBe(24); // 6 faces × 4 crease-split verts
    expect(v.triangleCount).toBe(12);
    expect(v.hasNormals).toBe(true);
    expect(v.hasEdges).toBe(true);
    // Face id table decodes to f:0..f:5.
    const ids = [...Array(6)].map((_, i) =>
      new TextDecoder().decode(v.faceIdChars.subarray(v.faceIdOffsets[i], v.faceIdOffsets[i + 1])),
    );
    expect(ids).toEqual(["f:0", "f:1", "f:2", "f:3", "f:4", "f:5"]);
    // bbox spans the 80×60×30 extents centred at origin.
    expect(v.bboxMin).toEqual([-40, -30, -15]);
    expect(v.bboxMax).toEqual([40, 30, 15]);
  });

  it("cylinder: side + 2 caps, 3 edges, valid ranges", () => {
    const v = parseMeshPayload(makeCylinderMesh(25, 60, 24));
    expect(v.faceCount).toBe(3);
    expect(v.edgeCount).toBe(3);
    // FACE_RANGES are contiguous and cover every triangle.
    let expectedFirst = 0;
    for (let f = 0; f < v.faceCount; f++) {
      expect(v.faceRanges[f * 2]).toBe(expectedFirst);
      expectedFirst += v.faceRanges[f * 2 + 1];
    }
    expect(expectedFirst).toBe(v.triangleCount);
  });

  it("faceBboxes flag round-trips when requested", () => {
    const buf = encodeMesh1({
      positions: [0, 0, 0, 1, 0, 0, 1, 1, 0],
      faces: [{ triangles: [[0, 1, 2]], id: "f:0" }],
      faceBboxes: true,
    });
    const v = parseMeshPayload(buf);
    expect(v.hasFaceBboxes).toBe(true);
    expect(v.faceBboxes).not.toBeNull();
    expect([...v.faceBboxes!]).toEqual([0, 0, 0, 1, 1, 0]);
  });
});

describe("MESH1 parser error surface", () => {
  const good = () => workedExample();

  it("rejects a truncated blob (< 64 bytes)", () => {
    const err = catchParse(good().slice(0, 40));
    expect(err.kind).toBe("truncated");
  });

  it("rejects bad magic", () => {
    const buf = good();
    new DataView(buf).setUint8(0, 0x00);
    expect(catchParse(buf).kind).toBe("bad-magic");
  });

  it("rejects unsupported version", () => {
    const buf = good();
    new DataView(buf).setUint16(0x04, 2, true);
    expect(catchParse(buf).kind).toBe("unsupported-version");
  });

  it("rejects a non-4-aligned section offset (misaligned)", () => {
    const buf = good();
    // POSITIONS entry is table row 0; offset field at header+8.
    new DataView(buf).setUint32(64 + 0x08, 226, true);
    expect(catchParse(buf).kind).toBe("misaligned");
  });

  it("rejects a misaligned blob origin", () => {
    const buf = good();
    expect(catchParse(buf, 2).kind).toBe("misaligned");
  });

  it("rejects overlapping sections", () => {
    const buf = good();
    // Force NORMALS (row 1) to start at POSITIONS' offset → overlap.
    new DataView(buf).setUint32(64 + 16 + 0x08, 224, true);
    expect(catchParse(buf).kind).toBe("section-overlap");
  });

  it("rejects a section length that disagrees with the counts", () => {
    const buf = good();
    // POSITIONS len 48 → 44.
    new DataView(buf).setUint32(64 + 0x0c, 44, true);
    expect(catchParse(buf).kind).toBe("bad-length");
  });

  it("rejects a nonzero reserved header word", () => {
    const buf = good();
    new DataView(buf).setUint32(0x38, 1, true);
    expect(catchParse(buf).kind).toBe("reserved-nonzero");
  });

  it("rejects a set reserved flag bit", () => {
    const buf = good();
    new DataView(buf).setUint16(0x06, 0x0013, true); // bit 4 set
    expect(catchParse(buf).kind).toBe("reserved-nonzero");
  });
});

describe("parseMeshContainer (frontend multi-body convention)", () => {
  it("splits a concatenated container into per-blob views (zero-copy)", () => {
    const box = makeBoxMesh();
    const cyl = makeCylinderMesh();
    const container = buildContainer([box, cyl]);
    const views = parseMeshContainer(container);
    expect(views.length).toBe(2);
    expect(views[0].faceCount).toBe(6);
    expect(views[1].faceCount).toBe(3);
    // Second blob is viewed in place at a nonzero, 4-aligned origin.
    expect(views[1].blobByteOffset % 4).toBe(0);
    expect(views[1].positions.buffer).toBe(container);
  });
});

// ── helpers ──
function catchParse(buf: ArrayBuffer, off?: number): MeshParseError {
  try {
    parseMeshPayload(buf, off ?? 0);
  } catch (e) {
    if (e instanceof MeshParseError) return e;
    throw e;
  }
  throw new Error("expected parse to throw");
}

function buildContainer(blobs: ArrayBuffer[]): ArrayBuffer {
  const align4 = (n: number) => (n + 3) & ~3;
  const lenTable = 4 + blobs.length * 4;
  let total = lenTable;
  const padded = blobs.map((b) => {
    const off = total;
    total += align4(b.byteLength);
    return { b, off, len: align4(b.byteLength) };
  });
  const out = new ArrayBuffer(total);
  const dv = new DataView(out);
  const u8 = new Uint8Array(out);
  dv.setUint32(0, blobs.length, true);
  padded.forEach((p, i) => {
    dv.setUint32(4 + i * 4, p.len, true);
    u8.set(new Uint8Array(p.b), p.off);
  });
  return out;
}
