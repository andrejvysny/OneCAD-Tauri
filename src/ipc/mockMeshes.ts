/*
 * Synthesize REAL MESH1 binary blobs byte-by-byte (protocol/mesh_format.md).
 *
 * These are the exact bytes a C++ worker would emit, so the mock drives the full
 * parse → registry → scene → pick pipeline with no backend. `encodeMesh1` is the
 * shared DataView writer; `makeBoxMesh` / `makeCylinderMesh` build the two demo
 * bodies. The writer lays present sections in ascending `type` order (== ascending
 * offset), each 4-byte aligned — matching the worked example in §5 verbatim.
 */
import { FLAG, SEC } from "@/viewport/mesh/parseMeshPayload";
import { prismLocal, type PrismProfile } from "@/tools/preview/prismPreview";
import type { SketchPlane } from "./types";

const HEADER_BYTES = 64;
const TABLE_ENTRY_BYTES = 16;

const align4 = (n: number): number => (n + 3) & ~3;

export interface FaceSource {
  /** Triangles as triples of indices into `positions` (grouped under this face). */
  triangles: ReadonlyArray<readonly [number, number, number]>;
  /** Face id — TopoKey (`"f:0"`) or minted ElementId. */
  id: string;
}

export interface EdgeSource {
  /** Polyline points (world xyz); consecutive points form segments. */
  points: ReadonlyArray<readonly [number, number, number]>;
  id: string;
}

export interface Mesh1Source {
  /** Vertex positions, flat xyz (length 3·V). */
  positions: number[];
  /** Optional per-vertex smoothed normals, flat xyz (length 3·V). */
  normals?: number[];
  /** Faces in emission order; their triangles are concatenated into INDICES. */
  faces: FaceSource[];
  edges?: EdgeSource[];
  lod?: number;
  /** Override bbox; otherwise computed from `positions`. */
  bbox?: { min: [number, number, number]; max: [number, number, number] };
  /** Set IDS_HAVE_ELEMENTIDS (ids are minted ElementIds, not pure TopoKeys). */
  idsHaveElementIds?: boolean;
  /** Emit the optional FACE_BBOXES section (computed per face). */
  faceBboxes?: boolean;
}

interface SectionBytes {
  type: number;
  bytes: Uint8Array;
}

const utf8 = new TextEncoder();

/** Encode a Mesh1Source into an exact MESH1 ArrayBuffer. */
export function encodeMesh1(src: Mesh1Source): ArrayBuffer {
  const V = src.positions.length / 3;
  if (!Number.isInteger(V)) throw new Error("positions length not a multiple of 3");

  // ── INDICES (grouped by face) + FACE_RANGES ──
  const indices: number[] = [];
  const faceRanges: number[] = []; // firstTri, triCount per face
  for (const face of src.faces) {
    const firstTri = indices.length / 3;
    for (const [a, b, c] of face.triangles) indices.push(a, b, c);
    faceRanges.push(firstTri, face.triangles.length);
  }
  const T = indices.length / 3;
  const F = src.faces.length;

  // ── FACE_ID_OFFS / FACE_ID_CHARS ──
  const faceIdBytes = src.faces.map((f) => utf8.encode(f.id));
  const faceIdOffs: number[] = [0];
  for (const b of faceIdBytes) faceIdOffs.push(faceIdOffs[faceIdOffs.length - 1] + b.length);
  const faceChars = concatBytes(faceIdBytes, faceIdOffs[F]);

  // ── EDGES ──
  const edges = src.edges ?? [];
  const E = edges.length;
  const edgePositions: number[] = [];
  const edgeRanges: number[] = []; // firstPoint, pointCount per edge
  for (const edge of edges) {
    const firstPoint = edgePositions.length / 3;
    for (const [x, y, z] of edge.points) edgePositions.push(x, y, z);
    edgeRanges.push(firstPoint, edge.points.length);
  }
  const P = edgePositions.length / 3;
  const edgeIdBytes = edges.map((e) => utf8.encode(e.id));
  const edgeIdOffs: number[] = [0];
  for (const b of edgeIdBytes) edgeIdOffs.push(edgeIdOffs[edgeIdOffs.length - 1] + b.length);
  const edgeChars = concatBytes(edgeIdBytes, edgeIdOffs[E]);

  const hasNormals = src.normals !== undefined;
  const hasEdges = E > 0;
  const hasFaceBboxes = src.faceBboxes === true;

  let flags = 0;
  if (hasNormals) flags |= FLAG.HAS_NORMALS;
  if (hasEdges) flags |= FLAG.HAS_EDGES;
  if (hasFaceBboxes) flags |= FLAG.HAS_FACE_BBOXES;
  if (src.idsHaveElementIds) flags |= FLAG.IDS_HAVE_ELEMENTIDS;

  // ── Section byte payloads, in ascending type order ──
  const sections: SectionBytes[] = [];
  sections.push({ type: SEC.POSITIONS, bytes: f32Bytes(src.positions) });
  if (hasNormals) sections.push({ type: SEC.NORMALS, bytes: f32Bytes(src.normals!) });
  sections.push({ type: SEC.INDICES, bytes: u32Bytes(indices) });
  sections.push({ type: SEC.FACE_RANGES, bytes: u32Bytes(faceRanges) });
  sections.push({ type: SEC.FACE_ID_OFFS, bytes: u32Bytes(faceIdOffs) });
  sections.push({ type: SEC.FACE_ID_CHARS, bytes: faceChars });
  if (hasEdges) {
    sections.push({ type: SEC.EDGE_RANGES, bytes: u32Bytes(edgeRanges) });
    sections.push({ type: SEC.EDGE_POSITIONS, bytes: f32Bytes(edgePositions) });
    sections.push({ type: SEC.EDGE_ID_OFFS, bytes: u32Bytes(edgeIdOffs) });
    sections.push({ type: SEC.EDGE_ID_CHARS, bytes: edgeChars });
  }
  if (hasFaceBboxes) {
    sections.push({ type: SEC.FACE_BBOXES, bytes: f32Bytes(computeFaceBboxes(src.positions, src.faces)) });
  }

  // ── Lay sections out with 4-byte alignment ──
  const tableEnd = HEADER_BYTES + sections.length * TABLE_ENTRY_BYTES;
  let cursor = tableEnd;
  const placed = sections.map((s) => {
    const offset = align4(cursor);
    cursor = offset + s.bytes.length;
    return { ...s, offset };
  });
  const totalLen = align4(cursor);

  const buffer = new ArrayBuffer(totalLen);
  const dv = new DataView(buffer);
  const u8 = new Uint8Array(buffer);

  // Header (64 B).
  dv.setUint32(0x00, 0x4d455348, true); // magic
  dv.setUint16(0x04, 1, true); // version
  dv.setUint16(0x06, flags, true);
  dv.setUint32(0x08, V, true);
  dv.setUint32(0x0c, T, true);
  dv.setUint32(0x10, F, true);
  dv.setUint32(0x14, E, true);
  dv.setUint32(0x18, P, true);
  dv.setUint16(0x1c, src.lod ?? 0, true);
  dv.setUint16(0x1e, sections.length, true);
  const bbox = src.bbox ?? computeBbox(src.positions);
  dv.setFloat32(0x20, bbox.min[0], true);
  dv.setFloat32(0x24, bbox.min[1], true);
  dv.setFloat32(0x28, bbox.min[2], true);
  dv.setFloat32(0x2c, bbox.max[0], true);
  dv.setFloat32(0x30, bbox.max[1], true);
  dv.setFloat32(0x34, bbox.max[2], true);
  // reserved0/1 already zero.

  // Section table (16 B each, sorted by offset == type order here).
  placed.forEach((s, i) => {
    const base = HEADER_BYTES + i * TABLE_ENTRY_BYTES;
    dv.setUint32(base + 0x00, s.type, true);
    dv.setUint32(base + 0x04, 0, true); // pad
    dv.setUint32(base + 0x08, s.offset, true);
    dv.setUint32(base + 0x0c, s.bytes.length, true);
  });

  // Section data.
  for (const s of placed) u8.set(s.bytes, s.offset);

  return buffer;
}

// ── byte helpers ────────────────────────────────────────────────────────────

function f32Bytes(values: number[]): Uint8Array {
  return new Uint8Array(Float32Array.from(values).buffer);
}
function u32Bytes(values: number[]): Uint8Array {
  return new Uint8Array(Uint32Array.from(values).buffer);
}
function concatBytes(parts: Uint8Array[], total: number): Uint8Array {
  const out = new Uint8Array(total);
  let o = 0;
  for (const p of parts) {
    out.set(p, o);
    o += p.length;
  }
  return out;
}
function computeBbox(positions: number[]): { min: [number, number, number]; max: [number, number, number] } {
  const min: [number, number, number] = [Infinity, Infinity, Infinity];
  const max: [number, number, number] = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i < positions.length; i += 3) {
    for (let a = 0; a < 3; a++) {
      const v = positions[i + a];
      if (v < min[a]) min[a] = v;
      if (v > max[a]) max[a] = v;
    }
  }
  return { min, max };
}
function computeFaceBboxes(positions: number[], faces: FaceSource[]): number[] {
  const out: number[] = [];
  for (const face of faces) {
    const min = [Infinity, Infinity, Infinity];
    const max = [-Infinity, -Infinity, -Infinity];
    for (const tri of face.triangles) {
      for (const vi of tri) {
        for (let a = 0; a < 3; a++) {
          const v = positions[vi * 3 + a];
          if (v < min[a]) min[a] = v;
          if (v > max[a]) max[a] = v;
        }
      }
    }
    out.push(min[0], min[1], min[2], max[0], max[1], max[2]);
  }
  return out;
}

// ── Box (80×60×30, centred at origin — matches the retired demo box) ─────────

/** Six crease-split faces `f:0..f:5`, twelve edges `e:0..e:11`. */
export function makeBoxMesh(sizeX = 80, sizeY = 60, sizeZ = 30, lod = 0): ArrayBuffer {
  const hx = sizeX / 2;
  const hy = sizeY / 2;
  const hz = sizeZ / 2;

  const positions: number[] = [];
  const normals: number[] = [];
  const faces: FaceSource[] = [];

  // Each face: 4 own vertices (crease split) + 2 triangles, normal = face dir.
  const addFace = (corners: [number, number, number][], normal: [number, number, number], id: string) => {
    const base = positions.length / 3;
    for (const c of corners) {
      positions.push(c[0], c[1], c[2]);
      normals.push(normal[0], normal[1], normal[2]);
    }
    faces.push({
      triangles: [
        [base, base + 1, base + 2],
        [base, base + 2, base + 3],
      ],
      id,
    });
  };

  addFace(
    [ [hx, -hy, -hz], [hx, hy, -hz], [hx, hy, hz], [hx, -hy, hz] ],
    [1, 0, 0],
    "f:0",
  );
  addFace(
    [ [-hx, hy, -hz], [-hx, -hy, -hz], [-hx, -hy, hz], [-hx, hy, hz] ],
    [-1, 0, 0],
    "f:1",
  );
  addFace(
    [ [hx, hy, -hz], [-hx, hy, -hz], [-hx, hy, hz], [hx, hy, hz] ],
    [0, 1, 0],
    "f:2",
  );
  addFace(
    [ [-hx, -hy, -hz], [hx, -hy, -hz], [hx, -hy, hz], [-hx, -hy, hz] ],
    [0, -1, 0],
    "f:3",
  );
  addFace(
    [ [-hx, -hy, hz], [hx, -hy, hz], [hx, hy, hz], [-hx, hy, hz] ],
    [0, 0, 1],
    "f:4",
  );
  addFace(
    [ [hx, -hy, -hz], [-hx, -hy, -hz], [-hx, hy, -hz], [hx, hy, -hz] ],
    [0, 0, -1],
    "f:5",
  );

  // 8 corners → 12 edges.
  const c: Record<string, [number, number, number]> = {
    "000": [-hx, -hy, -hz], "100": [hx, -hy, -hz], "110": [hx, hy, -hz], "010": [-hx, hy, -hz],
    "001": [-hx, -hy, hz], "101": [hx, -hy, hz], "111": [hx, hy, hz], "011": [-hx, hy, hz],
  };
  const edgePairs: [string, string][] = [
    ["000", "100"], ["100", "110"], ["110", "010"], ["010", "000"], // bottom
    ["001", "101"], ["101", "111"], ["111", "011"], ["011", "001"], // top
    ["000", "001"], ["100", "101"], ["110", "111"], ["010", "011"], // verticals
  ];
  const edges: EdgeSource[] = edgePairs.map(([a, b], i) => ({
    points: [c[a], c[b]],
    id: `e:${i}`,
  }));

  return encodeMesh1({ positions, normals, faces, edges, lod });
}

// ── Cylinder (radial segments; side face + 2 caps) ───────────────────────────

/** Side face `f:0` + top cap `f:1` + bottom cap `f:2`; top/bottom circle + seam edges. */
export function makeCylinderMesh(radius = 25, height = 60, segments = 24, lod = 0): ArrayBuffer {
  const zTop = height / 2;
  const zBot = -height / 2;
  const positions: number[] = [];
  const normals: number[] = [];
  const faces: FaceSource[] = [];

  const push = (x: number, y: number, z: number, nx: number, ny: number, nz: number): number => {
    const idx = positions.length / 3;
    positions.push(x, y, z);
    normals.push(nx, ny, nz);
    return idx;
  };
  const ang = (i: number) => (i / segments) * Math.PI * 2;

  // Side face: (segments+1) bottom + top verts (seam duplicated) with radial normals.
  const sideBot: number[] = [];
  const sideTop: number[] = [];
  for (let i = 0; i <= segments; i++) {
    const a = ang(i);
    const cx = Math.cos(a);
    const cy = Math.sin(a);
    sideBot.push(push(radius * cx, radius * cy, zBot, cx, cy, 0));
    sideTop.push(push(radius * cx, radius * cy, zTop, cx, cy, 0));
  }
  const sideTris: [number, number, number][] = [];
  for (let i = 0; i < segments; i++) {
    sideTris.push([sideBot[i], sideBot[i + 1], sideTop[i + 1]]);
    sideTris.push([sideBot[i], sideTop[i + 1], sideTop[i]]);
  }
  faces.push({ triangles: sideTris, id: "f:0" });

  // Top cap (normal +Z): fan around a centre vertex.
  const topCenter = push(0, 0, zTop, 0, 0, 1);
  const topRing: number[] = [];
  for (let i = 0; i < segments; i++) {
    const a = ang(i);
    topRing.push(push(radius * Math.cos(a), radius * Math.sin(a), zTop, 0, 0, 1));
  }
  const topTris: [number, number, number][] = [];
  for (let i = 0; i < segments; i++) {
    topTris.push([topCenter, topRing[i], topRing[(i + 1) % segments]]);
  }
  faces.push({ triangles: topTris, id: "f:1" });

  // Bottom cap (normal -Z): fan wound the other way.
  const botCenter = push(0, 0, zBot, 0, 0, -1);
  const botRing: number[] = [];
  for (let i = 0; i < segments; i++) {
    const a = ang(i);
    botRing.push(push(radius * Math.cos(a), radius * Math.sin(a), zBot, 0, 0, -1));
  }
  const botTris: [number, number, number][] = [];
  for (let i = 0; i < segments; i++) {
    botTris.push([botCenter, botRing[(i + 1) % segments], botRing[i]]);
  }
  faces.push({ triangles: botTris, id: "f:2" });

  // Edges: top circle, bottom circle (closed polylines), one vertical seam.
  const circle = (z: number): [number, number, number][] => {
    const pts: [number, number, number][] = [];
    for (let i = 0; i <= segments; i++) {
      const a = ang(i);
      pts.push([radius * Math.cos(a), radius * Math.sin(a), z]);
    }
    return pts;
  };
  const edges: EdgeSource[] = [
    { points: circle(zTop), id: "e:0" },
    { points: circle(zBot), id: "e:1" },
    { points: [ [radius, 0, zBot], [radius, 0, zTop] ], id: "e:2" },
  ];

  return encodeMesh1({ positions, normals, faces, edges, lod });
}

// ── Extrude body (prism from a sketch region × depth) — the mock L2 body ──────

/**
 * Synthesize the exact extrude body: lift a region profile into a prism (shared
 * plane-local topology from prismPreview), transform to WORLD via the sketch
 * plane basis, and encode as MESH1. Faces `f:0` (bottom) / `f:1` (top) / `f:2`
 * (sides); edges are the top/bottom boundary loops + verticals. Sized by the
 * region (u,v) bbox × `depth`, so the emitted mesh's bbox scales with the drag —
 * exactly what the 60fps gate asserts against the final params.
 *
 * MOCK LIMIT: booleanMode (Add/Cut/Intersect) does not actually fuse/subtract
 * against the target body — the mock always emits the fresh prism as its own body
 * (the real fusion is OCCT's job in F-WP8).
 */
export function makeExtrudeBodyMesh(
  profile: PrismProfile,
  plane: SketchPlane,
  depth: number,
  lod = 0,
): ArrayBuffer {
  const d = Math.abs(depth) < 1e-4 ? (depth < 0 ? -1e-4 : 1e-4) : depth;
  const local = prismLocal(profile, d);
  const [ox, oy, oz] = plane.origin;
  const [xx, xy, xz] = plane.xAxis;
  const [yx, yy, yz] = plane.yAxis;
  const [nx, ny, nz] = plane.normal;

  const P = local.positions.length / 3;
  const worldPositions: number[] = new Array(P * 3);
  const worldNormals: number[] = new Array(P * 3);
  for (let i = 0; i < P; i++) {
    const u = local.positions[i * 3];
    const v = local.positions[i * 3 + 1];
    const w = local.positions[i * 3 + 2];
    worldPositions[i * 3] = ox + u * xx + v * yx + w * nx;
    worldPositions[i * 3 + 1] = oy + u * xy + v * yy + w * ny;
    worldPositions[i * 3 + 2] = oz + u * xz + v * yz + w * nz;
    const lx = local.normals[i * 3];
    const ly = local.normals[i * 3 + 1];
    const lz = local.normals[i * 3 + 2];
    worldNormals[i * 3] = lx * xx + ly * yx + lz * nx;
    worldNormals[i * 3 + 1] = lx * xy + ly * yy + lz * ny;
    worldNormals[i * 3 + 2] = lx * xz + ly * yz + lz * nz;
  }

  const faces: FaceSource[] = local.faces.map((f, i) => ({ triangles: f.triangles, id: `f:${i}` }));
  const edges: EdgeSource[] = local.edges.map((loop, i) => ({
    points: loop.map(
      (vi) => [worldPositions[vi * 3], worldPositions[vi * 3 + 1], worldPositions[vi * 3 + 2]] as [number, number, number],
    ),
    id: `e:${i}`,
  }));

  return encodeMesh1({ positions: worldPositions, normals: worldNormals, faces, edges, lod });
}
