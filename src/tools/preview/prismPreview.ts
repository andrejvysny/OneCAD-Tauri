/*
 * Prism preview geometry (PURE math, plane-local coords) — the shared core of
 * the two-level extrude preview (NEW_SPEC §15).
 *
 * A finished sketch region carries a fan triangulation in plane (u,v):
 * `previewTriangles.positions` = [centroidU, centroidV, ring0U, ring0V, …] and
 * `indices` fan the ring around the centroid (see mockSketch). We lift that flat
 * profile into a 3D prism authored in PLANE-LOCAL coordinates (x=u, y=v, z=w
 * along the plane normal):
 *
 *   - L1 (engine): a UNIT prism (cap at w=1) built once; a group carrying the
 *     plane basis scales `z` by the live depth, so the drag runs zero-allocation.
 *   - L2 (mock backend): the exact prism at the real depth, transformed to world
 *     and encoded as MESH1 (see mockMeshes.makeExtrudeBodyMesh).
 *
 * Both consume the SAME topology from here, so L1 and the exact result agree.
 */
import type { SketchRegion } from "@/ipc/types";

/** Boundary ring (u,v) + cap fan triangulation, extracted from a region. */
export interface PrismProfile {
  /** Boundary ring in plane (u,v), CCW, no repeated closing point. */
  ring: [number, number][];
  /** Fan cap: `positions` = flat (u,v) pairs (index 0 = centroid), `indices` = triples. */
  cap: { positions: number[]; indices: number[] };
}

/** (u,v) bounds + centroid of a region's profile — sizes the handle + the mock body. */
export interface RegionBounds {
  minU: number;
  maxU: number;
  minV: number;
  maxV: number;
  centroidU: number;
  centroidV: number;
}

/**
 * Extract the prism profile from a region's `previewTriangles`. Returns null when
 * the region has no triangulation (nothing to extrude). The ring is the cap
 * vertices AFTER the centroid (index 0), matching the fan layout.
 */
export function profileFromRegion(region: SketchRegion): PrismProfile | null {
  const tris = region.previewTriangles;
  if (!tris || tris.positions.length < 8) return null; // need centroid + ≥3 ring pts
  const ring: [number, number][] = [];
  for (let i = 2; i + 1 < tris.positions.length; i += 2) {
    ring.push([tris.positions[i], tris.positions[i + 1]]);
  }
  if (ring.length < 3) return null;
  return { ring, cap: { positions: [...tris.positions], indices: [...tris.indices] } };
}

/** (u,v) bounds + centroid of a profile. */
export function profileBounds(profile: PrismProfile): RegionBounds {
  let minU = Infinity,
    maxU = -Infinity,
    minV = Infinity,
    maxV = -Infinity;
  for (const [u, v] of profile.ring) {
    if (u < minU) minU = u;
    if (u > maxU) maxU = u;
    if (v < minV) minV = v;
    if (v > maxV) maxV = v;
  }
  // Centroid = cap vertex 0 (the fan hub), which mockSketch places at the mean.
  const centroidU = profile.cap.positions[0];
  const centroidV = profile.cap.positions[1];
  return { minU, maxU, minV, maxV, centroidU, centroidV };
}

/** A faceted prism in plane-local coords (x=u, y=v, z=w along normal). */
export interface PrismLocal {
  /** Flat (x,y,z) vertex triples. */
  positions: number[];
  /** Flat (x,y,z) per-vertex normals (same length as positions). */
  normals: number[];
  /** Faces in emission order; triangles index `positions`. bottom, top, side. */
  faces: { triangles: [number, number, number][] }[];
  /** Edge polylines as arrays of point indices into `positions`. */
  edges: number[][];
}

/**
 * Build the faceted prism (bottom cap `-z`, top cap `+z`, side walls) at `depth`.
 * Caps reuse the fan cap verts (shared, ±z normals); side walls get their own
 * duplicated ring verts (crease split, radial normals). Ring winding is assumed
 * CCW in (u,v), so the top cap fan is +z and the bottom is the reversed winding.
 */
export function prismLocal(profile: PrismProfile, depth: number): PrismLocal {
  const capPairs = profile.cap.positions.length / 2; // centroid + ring
  const ringN = profile.ring.length;
  const positions: number[] = [];
  const normals: number[] = [];

  const pushV = (x: number, y: number, z: number, nx: number, ny: number, nz: number) => {
    positions.push(x, y, z);
    normals.push(nx, ny, nz);
  };

  // 1) bottom cap verts (z=0, normal -z), indices [0 .. capPairs-1]
  for (let i = 0; i < capPairs; i++) {
    pushV(profile.cap.positions[i * 2], profile.cap.positions[i * 2 + 1], 0, 0, 0, -1);
  }
  // 2) top cap verts (z=depth, normal +z), indices [capPairs .. 2*capPairs-1]
  const topBase = capPairs;
  for (let i = 0; i < capPairs; i++) {
    pushV(profile.cap.positions[i * 2], profile.cap.positions[i * 2 + 1], depth, 0, 0, 1);
  }

  // Bottom cap triangles = fan indices with REVERSED winding (normal -z).
  const bottomTris: [number, number, number][] = [];
  for (let i = 0; i + 2 < profile.cap.indices.length; i += 3) {
    const a = profile.cap.indices[i];
    const b = profile.cap.indices[i + 1];
    const c = profile.cap.indices[i + 2];
    bottomTris.push([a, c, b]);
  }
  // Top cap triangles = fan indices as-is, offset to the top verts (normal +z).
  const topTris: [number, number, number][] = [];
  for (let i = 0; i + 2 < profile.cap.indices.length; i += 3) {
    topTris.push([
      topBase + profile.cap.indices[i],
      topBase + profile.cap.indices[i + 1],
      topBase + profile.cap.indices[i + 2],
    ]);
  }

  // 3) side verts: per ring vertex a bottom+top duplicate with a radial normal.
  const sideBase = positions.length / 3;
  for (let j = 0; j < ringN; j++) {
    const [u, v] = profile.ring[j];
    const nu = u - profile.cap.positions[0];
    const nv = v - profile.cap.positions[1];
    const len = Math.hypot(nu, nv) || 1;
    const rx = nu / len;
    const ry = nv / len;
    pushV(u, v, 0, rx, ry, 0); // bottom dup, even index
    pushV(u, v, depth, rx, ry, 0); // top dup, odd index
  }
  const sideTris: [number, number, number][] = [];
  for (let j = 0; j < ringN; j++) {
    const b0 = sideBase + j * 2;
    const t0 = b0 + 1;
    const jn = (j + 1) % ringN;
    const b1 = sideBase + jn * 2;
    const t1 = b1 + 1;
    sideTris.push([b0, b1, t1]);
    sideTris.push([b0, t1, t0]);
  }

  // Edges: bottom ring loop, top ring loop (closed), + verticals for small rings.
  const bottomLoop: number[] = [];
  const topLoop: number[] = [];
  for (let j = 0; j < ringN; j++) {
    bottomLoop.push(sideBase + j * 2);
    topLoop.push(sideBase + j * 2 + 1);
  }
  bottomLoop.push(sideBase); // close
  topLoop.push(sideBase + 1);
  const edges: number[][] = [bottomLoop, topLoop];
  if (ringN <= 12) {
    for (let j = 0; j < ringN; j++) edges.push([sideBase + j * 2, sideBase + j * 2 + 1]);
  } else {
    edges.push([sideBase, sideBase + 1]); // a single seam for round profiles
  }

  return {
    positions,
    normals,
    faces: [{ triangles: bottomTris }, { triangles: topTris }, { triangles: sideTris }],
    edges,
  };
}

/**
 * A UNIT-depth prism (cap at z=1) as merged indexed geometry, plane-local. The
 * engine builds this ONCE and scales z by the live depth (zero per-frame alloc).
 * No normals — L1 is a flat translucent accent, not lit.
 */
export function unitPrismGeometry(profile: PrismProfile): {
  positions: Float32Array;
  indices: Uint32Array;
} {
  const prism = prismLocal(profile, 1);
  const indices: number[] = [];
  for (const f of prism.faces) for (const t of f.triangles) indices.push(t[0], t[1], t[2]);
  return { positions: Float32Array.from(prism.positions), indices: Uint32Array.from(indices) };
}
