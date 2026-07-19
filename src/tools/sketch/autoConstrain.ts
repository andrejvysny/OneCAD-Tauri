/*
 * Auto-constraint inference (PURE) — ports OneCAD-CPP `AutoConstrainer` semantics
 * (NEW_SPEC §14 interaction model). On commit the tools infer:
 *
 *   - Horizontal / Vertical: a line within ±5° of an axis (verbatim tolerance
 *     from AutoConstrainer.h `horizontalTolerance = 5° in radians`).
 *   - Coincident: a new endpoint that lands ON an existing endpoint (snap-to-
 *     endpoint). C++ uses a 2mm proximity; our tools SNAP endpoints exactly, so
 *     the default match tolerance is tight (1e-6). The mock solver is an identity
 *     solve — it will not pull non-coincident points together — so coincidence is
 *     only inferred where snapping already made the points equal.
 *   - Perpendicular: a NON-axis line meeting a reference line at 90±5°
 *     (AutoConstrainer.h `perpendicularTolerance = 5°`). Gated behind H/V exactly
 *     like C++ `inferLineConstraints` (`hasOrientationConstraint` blocks it).
 *   - Parallel: a NON-axis line within ±5° of a reference line's direction
 *     (`parallelTolerance = 5°`). Also gated behind H/V — H/V wins over Parallel,
 *     matching the legacy precedence.
 *   - Tangent: an ARC whose START sits on a reference line's endpoint (within the
 *     2mm `coincidenceTolerance`) and whose start tangent aligns with the line to
 *     within ±5° (`tangentTolerance`). This is the exact legacy rule — the C++
 *     `inferTangent` only handles arc-start-tangent-to-line. (Line↔circle and
 *     arc↔arc tangency exist as SNAPS, not as legacy auto-constraints — seam.)
 *
 * A line is never both Horizontal and Vertical, and never gets Perpendicular /
 * Parallel once it is H or V. Intra-batch relationships (e.g. a polyline's chained
 * segments) are detected by folding each processed entity into the reference sets
 * as we go.
 */
import type {
  ConstraintPosition,
  SketchConstraint,
  SketchEntity,
} from "@/ipc/types";

/** ±5° in radians (AutoConstrainer.h default; shared by H/V/perp/parallel/tangent). */
export const HV_TOLERANCE_RAD = (5 * Math.PI) / 180;
/** perpendicularTolerance = 90±5° (AutoConstrainer.h). */
export const PERPENDICULAR_TOLERANCE_RAD = (5 * Math.PI) / 180;
/** parallelTolerance = ±5° (AutoConstrainer.h). */
export const PARALLEL_TOLERANCE_RAD = (5 * Math.PI) / 180;
/** tangentTolerance = ±5° (AutoConstrainer.h). */
export const TANGENT_TOLERANCE_RAD = (5 * Math.PI) / 180;
/** coincidenceTolerance = 2mm — used to detect an arc START on a line endpoint. */
export const TANGENT_COINCIDENCE_TOL = 2.0;
/** MIN_GEOMETRY_SIZE = 0.01mm (SketchTypes.h) — skip degenerate lines. */
export const MIN_GEOMETRY_SIZE = 0.01;

export interface InferOptions {
  angleToleranceRad?: number;
  coincidenceTol?: number;
  /** Id minter for the emitted constraints. */
  nextConstraintId: () => string;
}

interface EntPoint {
  entityId: string;
  position: ConstraintPosition;
  coord: [number, number];
}

/** A reference line for perpendicular / parallel / tangent inference. */
interface RefLine {
  id: string;
  p0: [number, number];
  p1: [number, number];
}

/** Constrainable points of an entity (Start/End/Center), for coincidence. */
export function entityPoints(e: SketchEntity): EntPoint[] {
  const out: EntPoint[] = [];
  if (e.type === "Point" && e.p0) out.push({ entityId: e.id, position: "Start", coord: e.p0 });
  if (e.type === "Line") {
    if (e.p0) out.push({ entityId: e.id, position: "Start", coord: e.p0 });
    if (e.p1) out.push({ entityId: e.id, position: "End", coord: e.p1 });
  }
  if (e.type === "Circle" && e.center) out.push({ entityId: e.id, position: "Center", coord: e.center });
  if (e.type === "Arc") {
    if (e.center) out.push({ entityId: e.id, position: "Center", coord: e.center });
    if (e.start) out.push({ entityId: e.id, position: "Start", coord: e.start });
    if (e.end) out.push({ entityId: e.id, position: "End", coord: e.end });
  }
  return out;
}

/** Angle of a line to +X in [-π, π]. */
export function lineAngle(p0: [number, number], p1: [number, number]): number {
  return Math.atan2(p1[1] - p0[1], p1[0] - p0[0]);
}

const dist2 = (a: [number, number], b: [number, number]): number => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** Normalize an angle to [-π, π]. */
function normalizeAngle(a: number): number {
  while (a > Math.PI) a -= 2 * Math.PI;
  while (a < -Math.PI) a += 2 * Math.PI;
  return a;
}

/**
 * Angle between two lines, folded to [0, π/2] (verbatim from
 * `AutoConstrainer::angleBetweenLines`: `min(|Δ|, π-|Δ|)`). 0 ⇒ parallel,
 * π/2 ⇒ perpendicular.
 */
export function angleBetweenLines(
  a0: [number, number],
  a1: [number, number],
  b0: [number, number],
  b1: [number, number],
): number {
  const diff = Math.abs(normalizeAngle(lineAngle(a0, a1) - lineAngle(b0, b1)));
  return Math.min(diff, Math.PI - diff);
}

/** Horizontal (`H`), Vertical (`V`) or null for a line within the tolerance. */
export function inferHV(
  p0: [number, number],
  p1: [number, number],
  tol = HV_TOLERANCE_RAD,
): "Horizontal" | "Vertical" | null {
  if (p0[0] === p1[0] && p0[1] === p1[1]) return null; // zero-length
  const a = lineAngle(p0, p1);
  const hDev = Math.min(Math.abs(a), Math.abs(Math.abs(a) - Math.PI));
  if (hDev <= tol) return "Horizontal";
  const vDev = Math.abs(Math.abs(a) - Math.PI / 2);
  if (vDev <= tol) return "Vertical";
  return null;
}

/**
 * Best perpendicular reference line for `line`: the one meeting it at 90±tol with
 * the smallest deviation from 90° (mirrors `inferPerpendicular`'s best-deviation
 * scan). Returns the partner id or null.
 */
export function inferPerpendicularPartner(
  p0: [number, number],
  p1: [number, number],
  refs: RefLine[],
  tol = PERPENDICULAR_TOLERANCE_RAD,
): string | null {
  let best: string | null = null;
  let bestDev = Infinity;
  for (const r of refs) {
    if (dist2(r.p0, r.p1) < MIN_GEOMETRY_SIZE) continue;
    const dev = Math.abs(angleBetweenLines(p0, p1, r.p0, r.p1) - Math.PI / 2);
    if (dev <= tol && dev < bestDev) {
      bestDev = dev;
      best = r.id;
    }
  }
  return best;
}

/**
 * Best parallel reference line for `line`: the one whose direction is within tol
 * (folded angle near 0). Returns the partner id or null. (`inferParallel`.)
 */
export function inferParallelPartner(
  p0: [number, number],
  p1: [number, number],
  refs: RefLine[],
  tol = PARALLEL_TOLERANCE_RAD,
): string | null {
  let best: string | null = null;
  let bestDev = Infinity;
  for (const r of refs) {
    if (dist2(r.p0, r.p1) < MIN_GEOMETRY_SIZE) continue;
    // angleBetweenLines is already folded to [0, π/2]; near 0 ⇒ parallel.
    const dev = angleBetweenLines(p0, p1, r.p0, r.p1);
    if (dev <= tol && dev < bestDev) {
      bestDev = dev;
      best = r.id;
    }
  }
  return best;
}

/**
 * Tangent partner line for an arc: a reference line whose endpoint the arc START
 * sits on (within 2mm) and whose direction aligns with the arc's start tangent to
 * within tol (verbatim from `AutoConstrainer::inferTangent`). Returns the line id
 * or null.
 */
export function inferTangentPartner(
  center: [number, number],
  start: [number, number],
  refs: RefLine[],
  tol = TANGENT_TOLERANCE_RAD,
): string | null {
  // Arc start tangent = perpendicular to the radial (CCW rotation), normalized.
  const rad: [number, number] = [start[0] - center[0], start[1] - center[1]];
  const radLen = Math.hypot(rad[0], rad[1]);
  if (radLen < 1e-12) return null;
  const tangent: [number, number] = [-rad[1] / radLen, rad[0] / radLen];

  let best: string | null = null;
  let bestDev = Infinity;
  for (const r of refs) {
    const dStart = dist2(start, r.p0);
    const dEnd = dist2(start, r.p1);
    let lineDir: [number, number];
    if (dStart < TANGENT_COINCIDENCE_TOL) {
      lineDir = [r.p0[0] - r.p1[0], r.p0[1] - r.p1[1]];
    } else if (dEnd < TANGENT_COINCIDENCE_TOL) {
      lineDir = [r.p1[0] - r.p0[0], r.p1[1] - r.p0[1]];
    } else {
      continue; // arc does not start at either endpoint of this line
    }
    const len = Math.hypot(lineDir[0], lineDir[1]);
    if (len < 1e-12) continue;
    const dot = Math.abs((lineDir[0] / len) * tangent[0] + (lineDir[1] / len) * tangent[1]);
    if (dot > Math.cos(tol)) {
      const dev = Math.acos(Math.min(1, dot));
      if (dev < bestDev) {
        bestDev = dev;
        best = r.id;
      }
    }
  }
  return best;
}

export function inferConstraints(
  newEntities: SketchEntity[],
  existing: SketchEntity[],
  opts: InferOptions,
): SketchConstraint[] {
  const tol = opts.angleToleranceRad ?? HV_TOLERANCE_RAD;
  const coincTol = opts.coincidenceTol ?? 1e-6;
  const out: SketchConstraint[] = [];

  // Reference points + lines grow as we process each new entity (intra-batch).
  const refs: EntPoint[] = existing.flatMap(entityPoints);
  const refLines: RefLine[] = existing
    .filter((e) => e.type === "Line" && e.p0 && e.p1)
    .map((e) => ({ id: e.id, p0: e.p0!, p1: e.p1! }));
  const seenPairs = new Set<string>();

  for (const e of newEntities) {
    // H/V for lines; if neither fires, try Perpendicular then Parallel (never both
    // when a line is already H/V — the legacy `hasOrientationConstraint` gate).
    if (e.type === "Line" && e.p0 && e.p1) {
      const hv = inferHV(e.p0, e.p1, tol);
      if (hv) {
        out.push({ id: opts.nextConstraintId(), type: hv, entities: [e.id] });
      } else if (dist2(e.p0, e.p1) >= MIN_GEOMETRY_SIZE) {
        const perp = inferPerpendicularPartner(e.p0, e.p1, refLines);
        if (perp) out.push({ id: opts.nextConstraintId(), type: "Perpendicular", entities: [e.id, perp] });
        const par = inferParallelPartner(e.p0, e.p1, refLines);
        if (par) out.push({ id: opts.nextConstraintId(), type: "Parallel", entities: [e.id, par] });
      }
    }

    // Tangent for an arc starting on a reference line's endpoint.
    if (e.type === "Arc" && e.center && e.start) {
      const tangentLine = inferTangentPartner(e.center, e.start, refLines);
      if (tangentLine) out.push({ id: opts.nextConstraintId(), type: "Tangent", entities: [e.id, tangentLine] });
    }

    // Coincidence for each of this entity's points against the reference set.
    for (const pt of entityPoints(e)) {
      const hit = refs.find(
        (r) => r.entityId !== e.id && Math.hypot(r.coord[0] - pt.coord[0], r.coord[1] - pt.coord[1]) <= coincTol,
      );
      if (hit) {
        const pairKey = [`${e.id}.${pt.position}`, `${hit.entityId}.${hit.position}`].sort().join("|");
        if (!seenPairs.has(pairKey)) {
          seenPairs.add(pairKey);
          out.push({
            id: opts.nextConstraintId(),
            type: "Coincident",
            entities: [e.id, hit.entityId],
            positions: [pt.position, hit.position],
          });
        }
      }
    }

    refs.push(...entityPoints(e));
    if (e.type === "Line" && e.p0 && e.p1) refLines.push({ id: e.id, p0: e.p0, p1: e.p1 });
  }

  return out;
}
