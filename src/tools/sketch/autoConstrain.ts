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
 *
 * A line is never both Horizontal and Vertical. Intra-batch coincidence is
 * detected (e.g. a rectangle's four shared corners) by folding each processed
 * entity's points into the reference set as we go.
 */
import type {
  ConstraintPosition,
  SketchConstraint,
  SketchEntity,
} from "@/ipc/types";

/** ±5° in radians (AutoConstrainer.h default). */
export const HV_TOLERANCE_RAD = (5 * Math.PI) / 180;

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

export function inferConstraints(
  newEntities: SketchEntity[],
  existing: SketchEntity[],
  opts: InferOptions,
): SketchConstraint[] {
  const tol = opts.angleToleranceRad ?? HV_TOLERANCE_RAD;
  const coincTol = opts.coincidenceTol ?? 1e-6;
  const out: SketchConstraint[] = [];

  // Reference points grow as we process each new entity (intra-batch corners).
  const refs: EntPoint[] = existing.flatMap(entityPoints);
  const seenPairs = new Set<string>();

  for (const e of newEntities) {
    // H/V for lines.
    if (e.type === "Line" && e.p0 && e.p1) {
      const hv = inferHV(e.p0, e.p1, tol);
      if (hv) out.push({ id: opts.nextConstraintId(), type: hv, entities: [e.id] });
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
  }

  return out;
}
