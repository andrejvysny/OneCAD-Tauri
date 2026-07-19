/*
 * Frontend snap detection (PURE, NEW_SPEC §14 — the frontend owns snapping).
 *
 * Given the raw pointer position in plane (u,v) coords plus the current sketch
 * entities, returns the snapped point, the indicator kind (for the in-canvas
 * marker + hint chip), and any H/V alignment guide lines to draw.
 *
 * PRIORITY LADDER (highest wins). Ported from OneCAD-CPP `SnapManager` — the C++
 * `SnapType` enum orders snaps by priority (lower enum = higher priority) and
 * `SnapResult::operator<` sorts candidates by TYPE first, then distance. We mirror
 * that ordering with a per-kind `tier`; within a tier the nearest candidate wins.
 * All point-like snaps are gated to ~8px on screen (the pixel-space analogue of the
 * C++ 2mm sketch-coord snap radius — see the mapping note below):
 *
 *   1. Alt held           → no snap (raw point)                        [suppress]
 *   2. endpoint (tier 0)   line/arc endpoints                          [nearest]
 *   3. midpoint (tier 1)   line/arc midpoints
 *   4. center   (tier 2)   circle/arc centers
 *   5. quadrant (tier 3)   circle/arc 0/90/180/270° points (arc: in-extent only)
 *   6. intersection (t4)   line-line / line-circle / circle-circle crossings
 *   7. onCurve  (tier 5)   nearest point ON a line/circle/arc          [lowest point tier]
 *   8. H/V alignment guide  align to a reference point's x and/or y     [within 8px]
 *   9. grid                 round to the nearest grid step
 *  10. none                 raw point
 *
 * 2mm↔8px mapping: the C++ app snaps in sketch millimetres (radius 2mm); the
 * frontend snaps in SCREEN pixels (radius 8px) so the target stays constant on
 * screen at any zoom. `threshold = SNAP_PX * pixelWorld` converts 8px into world
 * units at the cursor, so both models gate on the same on-screen distance.
 *
 * Everything is pure so priority + guide math is unit-tested; the engine renders
 * the returned indicator/guides and the tool machines consume `point`.
 */
import type { SketchEntity } from "@/ipc/types";
import type { Point2 } from "@/viewport/engine/sketchBasis";

export type SnapKind =
  | "none"
  | "grid"
  | "endpoint"
  | "midpoint"
  | "center"
  | "quadrant"
  | "intersection"
  | "onCurve"
  | "alignH"
  | "alignV"
  | "alignHV";

/** A dashed alignment guide: vertical ⇒ constant x, horizontal ⇒ constant y. */
export interface GuideLine {
  orientation: "vertical" | "horizontal";
  value: number;
}

export interface SnapResult {
  point: Point2;
  kind: SnapKind;
  /** Hint chip text (null when nothing to hint). */
  label: string | null;
  guides: GuideLine[];
  snapped: boolean;
}

export interface SnapOptions {
  /** World units between grid snap lines. */
  gridStep: number;
  /** World units per screen pixel at the cursor (sizes the 8px threshold). */
  pixelWorld: number;
  enableGrid: boolean;
  enableGuideLines: boolean;
  enableGuidePoints: boolean;
  /** Circle/arc 0/90/180/270° quadrant snaps (default on). */
  enableQuadrant?: boolean;
  /** Entity-entity intersection snaps (default on). */
  enableIntersection?: boolean;
  /** Nearest-point-on-curve snaps (default on). */
  enableOnCurve?: boolean;
  /** Alt held ⇒ raw point, no snap. */
  suppress: boolean;
  /** Extra reference points for H/V alignment (e.g. the current chain anchor). */
  recentPoints?: Point2[];
}

const SNAP_PX = 8;
const EPS = 1e-9;
const TWO_PI = Math.PI * 2;

const dist = (a: Point2, b: Point2): number => Math.hypot(a.x - b.x, a.y - b.y);

interface PointCandidate {
  point: Point2;
  kind: "endpoint" | "midpoint" | "center";
}

/** All snappable geometry points of an entity (endpoints, midpoint, center). */
export function entitySnapPoints(e: SketchEntity): PointCandidate[] {
  const out: PointCandidate[] = [];
  if (e.type === "Point" && e.p0) out.push({ point: xy(e.p0), kind: "endpoint" });
  if (e.type === "Line" && e.p0 && e.p1) {
    out.push({ point: xy(e.p0), kind: "endpoint" });
    out.push({ point: xy(e.p1), kind: "endpoint" });
    out.push({ point: { x: (e.p0[0] + e.p1[0]) / 2, y: (e.p0[1] + e.p1[1]) / 2 }, kind: "midpoint" });
  }
  if (e.type === "Circle" && e.center) out.push({ point: xy(e.center), kind: "center" });
  if (e.type === "Arc") {
    if (e.center) out.push({ point: xy(e.center), kind: "center" });
    if (e.start) out.push({ point: xy(e.start), kind: "endpoint" });
    if (e.end) out.push({ point: xy(e.end), kind: "endpoint" });
  }
  return out;
}

// ── Geometry primitives (PURE, ported verbatim from SnapManager.cpp) ──────────

/** Angle of a point relative to a center, in [0, 2π). */
function angleOf(center: [number, number], p: [number, number]): number {
  const a = Math.atan2(p[1] - center[1], p[0] - center[0]);
  return a < 0 ? a + TWO_PI : a;
}

/** True if `angle` (radians) lies on the CCW arc sweep start→end (frontend arcs
 *  sweep CCW from `start` to `end`, matching arcTool). */
export function arcContainsAngle(
  center: [number, number],
  start: [number, number],
  end: [number, number],
  angle: number,
): boolean {
  const a0 = angleOf(center, start);
  const a1 = angleOf(center, end);
  let sweep = a1 - a0;
  if (sweep < 0) sweep += TWO_PI;
  if (sweep < EPS) sweep = TWO_PI; // full turn (start==end) ⇒ whole circle
  let rel = ((angle % TWO_PI) + TWO_PI) % TWO_PI - a0;
  if (rel < 0) rel += TWO_PI;
  return rel <= sweep + 1e-9;
}

const QUADRANTS = [0, Math.PI / 2, Math.PI, (3 * Math.PI) / 2];

/** Circle quadrant points (0/90/180/270°). */
export function circleQuadrantPoints(center: [number, number], radius: number): Point2[] {
  return QUADRANTS.map((a) => ({ x: center[0] + radius * Math.cos(a), y: center[1] + radius * Math.sin(a) }));
}

/** Arc quadrant points — only those inside the arc's angular extent. */
export function arcQuadrantPoints(
  center: [number, number],
  radius: number,
  start: [number, number],
  end: [number, number],
): Point2[] {
  const out: Point2[] = [];
  for (const a of QUADRANTS) {
    if (arcContainsAngle(center, start, end, a)) {
      out.push({ x: center[0] + radius * Math.cos(a), y: center[1] + radius * Math.sin(a) });
    }
  }
  return out;
}

/** Nearest point on a bounded line segment (clamped to [0,1]). */
export function nearestOnSegment(p: Point2, a: [number, number], b: [number, number]): Point2 {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len2 = dx * dx + dy * dy;
  if (len2 < 1e-12) return { x: a[0], y: a[1] };
  let t = ((p.x - a[0]) * dx + (p.y - a[1]) * dy) / len2;
  t = Math.max(0, Math.min(1, t));
  return { x: a[0] + t * dx, y: a[1] + t * dy };
}

/** Nearest point on a full circle. */
export function nearestOnCircle(p: Point2, center: [number, number], radius: number): Point2 {
  const dx = p.x - center[0];
  const dy = p.y - center[1];
  const d = Math.hypot(dx, dy);
  if (d < 1e-12) return { x: center[0] + radius, y: center[1] };
  return { x: center[0] + (radius * dx) / d, y: center[1] + (radius * dy) / d };
}

/**
 * Segment-segment intersection (both parameters within [0,1]); null if parallel
 * or the crossing lies off either segment. Verbatim from
 * `SnapManager::lineLineIntersection`.
 */
export function segSegIntersection(
  p1: [number, number],
  p2: [number, number],
  p3: [number, number],
  p4: [number, number],
): Point2 | null {
  const d1x = p2[0] - p1[0];
  const d1y = p2[1] - p1[1];
  const d2x = p4[0] - p3[0];
  const d2y = p4[1] - p3[1];
  const cross = d1x * d2y - d1y * d2x;
  if (Math.abs(cross) < 1e-12) return null; // parallel / collinear
  const dx = p3[0] - p1[0];
  const dy = p3[1] - p1[1];
  const t = (dx * d2y - dy * d2x) / cross;
  const u = (dx * d1y - dy * d1x) / cross;
  if (t < 0 || t > 1 || u < 0 || u > 1) return null;
  return { x: p1[0] + t * d1x, y: p1[1] + t * d1y };
}

/**
 * Segment-circle intersection points (segment parameter within [0,1]). Verbatim
 * from `SnapManager::lineCircleIntersection`.
 */
export function segCircleIntersections(
  a: [number, number],
  b: [number, number],
  center: [number, number],
  radius: number,
): Point2[] {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const fx = a[0] - center[0];
  const fy = a[1] - center[1];
  const qa = dx * dx + dy * dy;
  const qb = 2 * (fx * dx + fy * dy);
  const qc = fx * fx + fy * fy - radius * radius;
  let disc = qb * qb - 4 * qa * qc;
  if (disc < 0 || qa < 1e-12) return [];
  disc = Math.sqrt(disc);
  const t1 = (-qb - disc) / (2 * qa);
  const t2 = (-qb + disc) / (2 * qa);
  const out: Point2[] = [];
  if (t1 >= 0 && t1 <= 1) out.push({ x: a[0] + t1 * dx, y: a[1] + t1 * dy });
  if (t2 >= 0 && t2 <= 1 && Math.abs(t2 - t1) > 1e-12) out.push({ x: a[0] + t2 * dx, y: a[1] + t2 * dy });
  return out;
}

/**
 * Circle-circle intersection points (0, 1, or 2). Verbatim from
 * `SnapManager::circleCircleIntersection` (tangent ⇒ 1 point, disjoint /
 * contained / concentric ⇒ 0).
 */
export function circleCircleIntersections(
  c1: [number, number],
  r1: number,
  c2: [number, number],
  r2: number,
): Point2[] {
  const dx = c2[0] - c1[0];
  const dy = c2[1] - c1[1];
  const d = Math.hypot(dx, dy);
  if (d > r1 + r2 + 1e-12 || d < Math.abs(r1 - r2) - 1e-12 || d < 1e-12) return [];
  const a = (r1 * r1 - r2 * r2 + d * d) / (2 * d);
  let h2 = r1 * r1 - a * a;
  if (h2 < 0) h2 = 0;
  const h = Math.sqrt(h2);
  const px = c1[0] + (a * dx) / d;
  const py = c1[1] + (a * dy) / d;
  const rx = -dy / d;
  const ry = dx / d;
  const out: Point2[] = [{ x: px + h * rx, y: py + h * ry }];
  if (h > 1e-12) out.push({ x: px - h * rx, y: py - h * ry });
  return out;
}

interface CurveInfo {
  kind: "line" | "circle" | "arc";
  a?: [number, number];
  b?: [number, number];
  center?: [number, number];
  radius?: number;
  start?: [number, number];
  end?: [number, number];
}

function curveOf(e: SketchEntity): CurveInfo | null {
  if (e.type === "Line" && e.p0 && e.p1) return { kind: "line", a: e.p0, b: e.p1 };
  if (e.type === "Circle" && e.center && e.radius !== undefined)
    return { kind: "circle", center: e.center, radius: e.radius };
  if (e.type === "Arc" && e.center && e.radius !== undefined && e.start && e.end)
    return { kind: "arc", center: e.center, radius: e.radius, start: e.start, end: e.end };
  return null;
}

/** Angle-gate a candidate on an arc curve; lines/circles always pass. */
function onCurveAngleOk(c: CurveInfo, p: Point2): boolean {
  if (c.kind !== "arc") return true;
  return arcContainsAngle(c.center!, c.start!, c.end!, Math.atan2(p.y - c.center![1], p.x - c.center![0]));
}

/** All intersection points between two entities (line/circle/arc). */
export function entityIntersections(e1: SketchEntity, e2: SketchEntity): Point2[] {
  const c1 = curveOf(e1);
  const c2 = curveOf(e2);
  if (!c1 || !c2) return [];

  const raw: Point2[] = [];
  const seg = (c: CurveInfo): [[number, number], [number, number]] => [c.a!, c.b!];

  if (c1.kind === "line" && c2.kind === "line") {
    const hit = segSegIntersection(...seg(c1), ...seg(c2));
    if (hit) raw.push(hit);
  } else if (c1.kind === "line" && (c2.kind === "circle" || c2.kind === "arc")) {
    raw.push(...segCircleIntersections(c1.a!, c1.b!, c2.center!, c2.radius!));
  } else if ((c1.kind === "circle" || c1.kind === "arc") && c2.kind === "line") {
    raw.push(...segCircleIntersections(c2.a!, c2.b!, c1.center!, c1.radius!));
  } else {
    // circle/arc × circle/arc
    raw.push(...circleCircleIntersections(c1.center!, c1.radius!, c2.center!, c2.radius!));
  }

  // Keep only points that lie within BOTH entities' angular extents (arcs).
  return raw.filter((p) => onCurveAngleOk(c1, p) && onCurveAngleOk(c2, p));
}

/** Nearest point ON an entity's curve (segment / circle / arc). */
export function nearestOnCurve(p: Point2, e: SketchEntity): Point2 | null {
  const c = curveOf(e);
  if (!c) return null;
  if (c.kind === "line") return nearestOnSegment(p, c.a!, c.b!);
  if (c.kind === "circle") return nearestOnCircle(p, c.center!, c.radius!);
  // arc: nearest on the full circle, snapping to the closer endpoint when outside.
  const onCircle = nearestOnCircle(p, c.center!, c.radius!);
  if (onCurveAngleOk(c, onCircle)) return onCircle;
  const ds = dist(p, xy(c.start!));
  const de = dist(p, xy(c.end!));
  return ds <= de ? xy(c.start!) : xy(c.end!);
}

// ── Ranked candidate ladder ───────────────────────────────────────────────────

const KIND_LABEL: Record<string, string> = {
  endpoint: "Endpoint",
  midpoint: "Midpoint",
  center: "Center",
  quadrant: "Quadrant",
  intersection: "Intersection",
  onCurve: "On Curve",
};

// Tier = C++ SnapType priority (lower = higher priority). Endpoint > Midpoint >
// Center > Quadrant > Intersection > OnCurve, then guides, then grid.
const KIND_TIER: Record<string, number> = {
  endpoint: 0,
  midpoint: 1,
  center: 2,
  quadrant: 3,
  intersection: 4,
  onCurve: 5,
};

interface Ranked {
  point: Point2;
  kind: Exclude<SnapKind, "none" | "grid" | "alignH" | "alignV" | "alignHV">;
  d: number;
  tier: number;
}

function collectPointCandidates(raw: Point2, entities: SketchEntity[], opts: SnapOptions, threshold: number): Ranked[] {
  const out: Ranked[] = [];
  const consider = (point: Point2, kind: Ranked["kind"]): void => {
    const d = dist(raw, point);
    if (d <= threshold) out.push({ point, kind, d, tier: KIND_TIER[kind] });
  };

  // endpoint / midpoint / center (gated by sketch-guide-points).
  if (opts.enableGuidePoints) {
    for (const e of entities) for (const c of entitySnapPoints(e)) consider(c.point, c.kind);
  }
  // quadrant.
  if (opts.enableQuadrant ?? true) {
    for (const e of entities) {
      if (e.type === "Circle" && e.center && e.radius !== undefined) {
        for (const q of circleQuadrantPoints(e.center, e.radius)) consider(q, "quadrant");
      } else if (e.type === "Arc" && e.center && e.radius !== undefined && e.start && e.end) {
        for (const q of arcQuadrantPoints(e.center, e.radius, e.start, e.end)) consider(q, "quadrant");
      }
    }
  }
  // intersection (all pairs; only crossings near the cursor survive the threshold).
  if (opts.enableIntersection ?? true) {
    for (let i = 0; i < entities.length; i++) {
      for (let j = i + 1; j < entities.length; j++) {
        for (const p of entityIntersections(entities[i], entities[j])) consider(p, "intersection");
      }
    }
  }
  // onCurve (lowest point tier).
  if (opts.enableOnCurve ?? true) {
    for (const e of entities) {
      const p = nearestOnCurve(raw, e);
      if (p) consider(p, "onCurve");
    }
  }
  return out;
}

export function computeSnap(
  raw: Point2,
  entities: SketchEntity[],
  opts: SnapOptions,
): SnapResult {
  if (opts.suppress) {
    return { point: raw, kind: "none", label: null, guides: [], snapped: false };
  }

  const threshold = SNAP_PX * opts.pixelWorld;

  // 2. Point-like snaps, ranked by (tier, distance) — C++ SnapResult::operator<.
  const candidates = collectPointCandidates(raw, entities, opts, threshold);
  if (candidates.length > 0) {
    let best = candidates[0];
    for (const c of candidates) {
      if (c.tier < best.tier || (c.tier === best.tier && c.d < best.d - EPS)) best = c;
    }
    return { point: best.point, kind: best.kind, label: KIND_LABEL[best.kind], guides: [], snapped: true };
  }

  // 3. H/V alignment guides from reference points.
  if (opts.enableGuideLines) {
    const refs = referencePoints(entities, opts.recentPoints);
    let vGuide: number | null = null; // constant x
    let hGuide: number | null = null; // constant y
    let vBest = threshold;
    let hBest = threshold;
    for (const r of refs) {
      const dx = Math.abs(raw.x - r.x);
      if (dx <= vBest) {
        vBest = dx;
        vGuide = r.x;
      }
      const dy = Math.abs(raw.y - r.y);
      if (dy <= hBest) {
        hBest = dy;
        hGuide = r.y;
      }
    }
    if (vGuide !== null || hGuide !== null) {
      const point = { x: vGuide ?? raw.x, y: hGuide ?? raw.y };
      const guides: GuideLine[] = [];
      if (vGuide !== null) guides.push({ orientation: "vertical", value: vGuide });
      if (hGuide !== null) guides.push({ orientation: "horizontal", value: hGuide });
      const kind: SnapKind = vGuide !== null && hGuide !== null ? "alignHV" : vGuide !== null ? "alignV" : "alignH";
      const label = vGuide !== null && hGuide !== null ? "Aligned" : vGuide !== null ? "Vertical" : "Horizontal";
      return { point, kind, label, guides, snapped: true };
    }
  }

  // 4. Grid snap.
  if (opts.enableGrid && opts.gridStep > 0) {
    const point = {
      x: Math.round(raw.x / opts.gridStep) * opts.gridStep,
      y: Math.round(raw.y / opts.gridStep) * opts.gridStep,
    };
    return { point, kind: "grid", label: "Grid", guides: [], snapped: true };
  }

  return { point: raw, kind: "none", label: null, guides: [], snapped: false };
}

// ── helpers ───────────────────────────────────────────────────────────────

function xy(p: [number, number]): Point2 {
  return { x: p[0], y: p[1] };
}

function referencePoints(entities: SketchEntity[], recent: Point2[] = []): Point2[] {
  const refs: Point2[] = [...recent];
  for (const e of entities) {
    for (const c of entitySnapPoints(e)) refs.push(c.point);
  }
  return refs;
}
