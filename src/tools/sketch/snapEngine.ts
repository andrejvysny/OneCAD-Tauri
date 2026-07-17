/*
 * Frontend snap detection (PURE, NEW_SPEC §14 — the frontend owns snapping).
 *
 * Given the raw pointer position in plane (u,v) coords plus the current sketch
 * entities, returns the snapped point, the indicator kind (for the in-canvas
 * marker + hint chip), and any H/V alignment guide lines to draw.
 *
 * PRIORITY (highest wins), all point snaps gated to ~8px on screen:
 *   1. Alt held           → no snap (raw point)                    [suppress]
 *   2. Geometry point      endpoint > midpoint > center            [nearest]
 *   3. H/V alignment guide  align to a reference point's x and/or y [within 8px]
 *   4. Grid                 round to the nearest grid step
 *   5. none                 raw point
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
  /** Alt held ⇒ raw point, no snap. */
  suppress: boolean;
  /** Extra reference points for H/V alignment (e.g. the current chain anchor). */
  recentPoints?: Point2[];
}

const SNAP_PX = 8;

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

const KIND_LABEL: Record<PointCandidate["kind"], string> = {
  endpoint: "Endpoint",
  midpoint: "Midpoint",
  center: "Center",
};

// endpoint beats midpoint beats center on a tie.
const KIND_RANK: Record<PointCandidate["kind"], number> = { endpoint: 0, midpoint: 1, center: 2 };

export function computeSnap(
  raw: Point2,
  entities: SketchEntity[],
  opts: SnapOptions,
): SnapResult {
  if (opts.suppress) {
    return { point: raw, kind: "none", label: null, guides: [], snapped: false };
  }

  const threshold = SNAP_PX * opts.pixelWorld;

  // 2. Geometry point snap (nearest within threshold; kind rank breaks ties).
  if (opts.enableGuidePoints) {
    let best: PointCandidate | null = null;
    let bestDist = Infinity;
    for (const e of entities) {
      for (const c of entitySnapPoints(e)) {
        const d = dist(raw, c.point);
        if (d > threshold) continue;
        if (d < bestDist - 1e-9 || (Math.abs(d - bestDist) <= 1e-9 && best && KIND_RANK[c.kind] < KIND_RANK[best.kind])) {
          best = c;
          bestDist = d;
        }
      }
    }
    if (best) {
      return { point: best.point, kind: best.kind, label: KIND_LABEL[best.kind], guides: [], snapped: true };
    }
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
