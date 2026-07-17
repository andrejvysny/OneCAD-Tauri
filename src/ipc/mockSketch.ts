/*
 * Mock sketch "solver" + region detection (MOCK-ONLY, no real geometry kernel).
 *
 * The real solver is the C++ worker's PlaneGCS actor (SCHEMA §7.4). Until it is
 * wired in, these pure functions give the frontend a plausible, deterministic
 * stand-in so the whole sketch UX (DOF badge, constraint state colors, extrude
 * profile preview) runs with no backend. LIMITS are documented per function.
 */
import type {
  SketchConstraint,
  SketchEntity,
  SketchPlane,
  SketchPlaneKind,
  SketchRegion,
  SketchSolveStatus,
} from "./types";

// ── Canonical planes — SCHEMA §7.3 EXACT bases (non-standard XY basis) ───────

const PLANES: Record<Exclude<SketchPlaneKind, "custom">, Omit<SketchPlane, "kind">> = {
  // User X → World Y+, User Y → World X− (ported verbatim from Sketch.h XY()).
  XY: { origin: [0, 0, 0], xAxis: [0, 1, 0], yAxis: [-1, 0, 0], normal: [0, 0, 1] },
  XZ: { origin: [0, 0, 0], xAxis: [0, 1, 0], yAxis: [0, 0, 1], normal: [1, 0, 0] },
  YZ: { origin: [0, 0, 0], xAxis: [-1, 0, 0], yAxis: [0, 0, 1], normal: [0, 1, 0] },
};

export function planeFor(kind: SketchPlaneKind): SketchPlane {
  const base = PLANES[kind === "custom" ? "XY" : kind];
  return { kind, ...structuredCloneLite(base) };
}

// ── Naive DOF heuristic (MOCK) ───────────────────────────────────────────────
//
// Real DOF comes from the constraint solver's Jacobian rank. The mock uses a
// coarse "2·points (+radius/angle terms) − removed" count: it moves the right
// direction (DOF falls as constraints are added, hits 0 when fully defined) but
// is NOT geometrically exact. It never reports Conflicting (that needs a real
// solver detecting contradictory equations).

/** Free parameters an entity contributes: 2 per point, +1 per radius/angle. */
export function entityFreedom(e: SketchEntity): number {
  switch (e.type) {
    case "Point":
      return 2; // 1 point
    case "Line":
      return 4; // 2 endpoints
    case "Circle":
      return 3; // center (2) + radius (1)
    case "Arc":
      return 5; // center (2) + radius (1) + 2 sweep angles
    default:
      return 0;
  }
}

/** DOFs one constraint removes (coarse: 2 for pairing/fixing, else 1). */
export function constraintFreedom(c: SketchConstraint): number {
  switch (c.type) {
    case "Coincident":
    case "Fixed":
    case "Symmetric":
      return 2;
    default:
      return 1;
  }
}

export function freeDegrees(entities: SketchEntity[]): number {
  return entities.reduce((sum, e) => sum + entityFreedom(e), 0);
}

export function removedDegrees(constraints: SketchConstraint[]): number {
  return constraints.reduce((sum, c) => sum + constraintFreedom(c), 0);
}

/** Solve → {dof (clamped ≥0), status}. Signed surplus decides the state. */
export function solveDof(
  entities: SketchEntity[],
  constraints: SketchConstraint[],
): { dof: number; status: SketchSolveStatus } {
  const surplus = freeDegrees(entities) - removedDegrees(constraints);
  const status: SketchSolveStatus =
    surplus > 0 ? "UnderConstrained" : surplus === 0 ? "FullyConstrained" : "OverConstrained";
  return { dof: Math.max(0, surplus), status };
}

// ── Region detection (MOCK — single closed loop, or circles) ─────────────────
//
// LIMITS: detects (a) each Circle as its own region and (b) a SINGLE closed loop
// of lines/arcs (every vertex degree 2, connected). No hole/nesting detection,
// no self-intersection handling. The real worker computes proper regions with
// holes (SCHEMA §7.4 SketchRegions). regionId here is a mock hash, NOT the
// normative FNV-1a-64 over 16-byte UUIDs the worker/Rust agree on.

const QUANT = 1e6; // 1e-6 endpoint-match tolerance
const key = (p: [number, number]): string =>
  `${Math.round(p[0] * QUANT)},${Math.round(p[1] * QUANT)}`;

/** Deterministic mock region id from member ids (NOT the normative scheme). */
export function mockRegionId(memberIds: string[]): string {
  let h = 0x811c9dc5; // FNV-1a-32
  for (const s of [...memberIds].sort()) {
    for (let i = 0; i < s.length; i++) {
      h ^= s.charCodeAt(i);
      h = Math.imul(h, 0x01000193);
    }
  }
  return `r_${(h >>> 0).toString(16).padStart(8, "0")}`;
}

interface Seg {
  id: string;
  a: [number, number];
  b: [number, number];
}

function segEndpoints(e: SketchEntity): Seg | null {
  if (e.type === "Line" && e.p0 && e.p1) return { id: e.id, a: e.p0, b: e.p1 };
  if (e.type === "Arc" && e.start && e.end) return { id: e.id, a: e.start, b: e.end };
  return null;
}

/** Walk segments into a single closed loop; ordered points or null if not one loop. */
export function orderedClosedLoop(entities: SketchEntity[]): { ids: string[]; points: [number, number][] } | null {
  const segs = entities.map(segEndpoints).filter((s): s is Seg => s !== null);
  if (segs.length < 3) return null;

  // Degree check: every endpoint vertex must have exactly two incident segments.
  const degree = new Map<string, number>();
  for (const s of segs) {
    degree.set(key(s.a), (degree.get(key(s.a)) ?? 0) + 1);
    degree.set(key(s.b), (degree.get(key(s.b)) ?? 0) + 1);
  }
  if ([...degree.values()].some((d) => d !== 2)) return null;
  if (degree.size !== segs.length) return null; // #vertices == #edges ⇒ single cycle

  // Walk the cycle.
  const remaining = new Set(segs.map((_, i) => i));
  const ids: string[] = [];
  const points: [number, number][] = [];
  let start = segs[0].a;
  let cursor = start;
  let steps = 0;
  while (remaining.size > 0 && steps <= segs.length) {
    steps++;
    let advanced = false;
    for (const i of remaining) {
      const s = segs[i];
      if (key(s.a) === key(cursor)) {
        ids.push(s.id);
        points.push(cursor);
        cursor = s.b;
        remaining.delete(i);
        advanced = true;
        break;
      }
      if (key(s.b) === key(cursor)) {
        ids.push(s.id);
        points.push(cursor);
        cursor = s.a;
        remaining.delete(i);
        advanced = true;
        break;
      }
    }
    if (!advanced) return null; // broken chain
  }
  return key(cursor) === key(start) ? { ids, points } : null;
}

/** Fan-triangulate a polygon (from centroid) into plane-local (u,v) preview tris. */
function polygonTriangles(points: [number, number][]): { positions: number[]; indices: number[] } {
  const cx = points.reduce((s, p) => s + p[0], 0) / points.length;
  const cy = points.reduce((s, p) => s + p[1], 0) / points.length;
  const positions = [cx, cy];
  for (const p of points) positions.push(p[0], p[1]);
  const indices: number[] = [];
  for (let i = 0; i < points.length; i++) {
    indices.push(0, 1 + i, 1 + ((i + 1) % points.length));
  }
  return { positions, indices };
}

/** Fan-triangulate a circle into plane-local (u,v) preview tris. */
function circleTriangles(center: [number, number], radius: number, segments = 32): {
  positions: number[];
  indices: number[];
} {
  const positions = [center[0], center[1]];
  for (let i = 0; i < segments; i++) {
    const a = (i / segments) * Math.PI * 2;
    positions.push(center[0] + radius * Math.cos(a), center[1] + radius * Math.sin(a));
  }
  const indices: number[] = [];
  for (let i = 0; i < segments; i++) indices.push(0, 1 + i, 1 + ((i + 1) % segments));
  return { positions, indices };
}

export function detectRegions(entities: SketchEntity[]): SketchRegion[] {
  const regions: SketchRegion[] = [];

  for (const e of entities) {
    if (e.type === "Circle" && e.center && e.radius && !e.construction) {
      regions.push({
        regionId: mockRegionId([e.id]),
        outerLoop: [e.id],
        holes: [],
        previewTriangles: circleTriangles(e.center, e.radius),
      });
    }
  }

  const loop = orderedClosedLoop(entities.filter((e) => !e.construction));
  if (loop) {
    regions.push({
      regionId: mockRegionId(loop.ids),
      outerLoop: loop.ids,
      holes: [],
      previewTriangles: polygonTriangles(loop.points),
    });
  }

  return regions;
}

// ── util ─────────────────────────────────────────────────────────────────────

function structuredCloneLite<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T;
}
