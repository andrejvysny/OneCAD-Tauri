/*
 * Dimension tool (PURE) — M6c. A pick-accumulator reducer (mirrors the sketch
 * toolMachine / model FSMs) that maps entity + point picks to a dimensional
 * constraint, plus the geometry to measure the seed value and the pure decision
 * for reject-on-over-constraint. The imperative SketchController resolves clicks
 * to picks, opens the seeded chip, and round-trips the commit through
 * sketch_upsert; ALL the classification + measurement math lives here so the
 * whole flow is unit-tested by pick scripts.
 *
 * PICK → CONSTRAINT (legacy Dimension parity):
 *   - a circle  → Diameter (single pick)
 *   - an arc    → Radius   (single pick)
 *   - a line    → Distance = its length (single pick); a SECOND distinct line
 *                 upgrades it to the Angle between the two lines (two picks)
 *   - two points → Distance between them (two picks)
 *
 * All constraints are authored plain (Distance / Radius / Diameter / Angle) — the
 * H-/V-distance modifier variants (HorizontalDistance / VerticalDistance) exist on
 * the wire but are a V2 concern (seam). Angle values are authored in DEGREES to
 * match the badge display + the revolve tool's `angleDeg` convention; confirm the
 * real PlaneGCS Angle unit (rad vs deg) when the mock solver is replaced (seam).
 */
import type { ConstraintPosition, SketchConstraint, SketchEntity, SketchSolveStatus } from "@/ipc/types";
import type { Point2 } from "@/viewport/engine/sketchBasis";
import { nearestOnCurve } from "./snapEngine";
import { entityPoints } from "./autoConstrain";

/** A pick the dimension tool consumes — an entity body or a named snap point. */
export type DimPick =
  | { on: "line"; id: string; p0: [number, number]; p1: [number, number] }
  | { on: "circle"; id: string; center: [number, number]; radius: number }
  | { on: "arc"; id: string; center: [number, number]; radius: number }
  | { on: "point"; id: string; position: ConstraintPosition; coord: [number, number] };

export type DimensionKind = "Distance" | "Radius" | "Diameter" | "Angle";

/** A ready dimension: the constraint to author + the measured value + chip anchor. */
export interface DimensionSpec {
  kind: DimensionKind;
  entities: string[];
  positions?: ConstraintPosition[];
  /** Seed value: mm for Distance/Radius/Diameter, degrees for Angle. */
  value: number;
  /** Plane-coord anchor for the chip. */
  anchor: Point2;
}

export interface DimState {
  /** A first line/point pick awaiting a partner (null once resolved/committed). */
  pending: DimPick | null;
  /** The armed spec (chip open + seeded), or null when nothing is ready. */
  ready: DimensionSpec | null;
}

export type DimEvent =
  | { kind: "pick"; target: DimPick }
  | { kind: "commit"; value: number }
  | { kind: "cancel" };

export interface DimStep {
  state: DimState;
  /** On commit: the constraint spec to author (value = the committed number). */
  emit?: DimensionSpec;
}

export function dimensionInit(): DimState {
  return { pending: null, ready: null };
}

const mid = (a: [number, number], b: [number, number]): Point2 => ({ x: (a[0] + b[0]) / 2, y: (a[1] + b[1]) / 2 });
const len = (a: [number, number], b: [number, number]): number => Math.hypot(a[0] - b[0], a[1] - b[1]);

/** Unsigned angle (degrees, [0,180]) between two line directions as drawn. */
export function angleBetweenDeg(
  a0: [number, number],
  a1: [number, number],
  b0: [number, number],
  b1: [number, number],
): number {
  const a = Math.atan2(a1[1] - a0[1], a1[0] - a0[0]);
  const b = Math.atan2(b1[1] - b0[1], b1[0] - b0[0]);
  let d = Math.abs(a - b);
  while (d > 2 * Math.PI) d -= 2 * Math.PI;
  if (d > Math.PI) d = 2 * Math.PI - d; // fold to [0, π]
  return (d * 180) / Math.PI;
}

// ── spec builders ─────────────────────────────────────────────────────────────

const diameterSpec = (p: Extract<DimPick, { on: "circle" }>): DimensionSpec => ({
  kind: "Diameter",
  entities: [p.id],
  value: 2 * p.radius,
  anchor: { x: p.center[0], y: p.center[1] },
});

const radiusSpec = (p: Extract<DimPick, { on: "arc" }>): DimensionSpec => ({
  kind: "Radius",
  entities: [p.id],
  value: p.radius,
  anchor: { x: p.center[0], y: p.center[1] },
});

/** Distance = the length of a single line (its Start↔End). */
const lineLengthSpec = (p: Extract<DimPick, { on: "line" }>): DimensionSpec => ({
  kind: "Distance",
  entities: [p.id, p.id],
  positions: ["Start", "End"],
  value: len(p.p0, p.p1),
  anchor: mid(p.p0, p.p1),
});

/** Distance between two point picks. */
const pointDistanceSpec = (a: Extract<DimPick, { on: "point" }>, b: Extract<DimPick, { on: "point" }>): DimensionSpec => ({
  kind: "Distance",
  entities: [a.id, b.id],
  positions: [a.position, b.position],
  value: len(a.coord, b.coord),
  anchor: mid(a.coord, b.coord),
});

function anchorBetween(a: Point2, b: Point2): Point2 {
  return { x: (a.x + b.x) / 2, y: (a.y + b.y) / 2 };
}

/** Angle between two line picks (chip anchored between the two line midpoints). */
const angleSpec = (a: Extract<DimPick, { on: "line" }>, b: Extract<DimPick, { on: "line" }>): DimensionSpec => ({
  kind: "Angle",
  entities: [a.id, b.id],
  value: angleBetweenDeg(a.p0, a.p1, b.p0, b.p1),
  anchor: anchorBetween(mid(a.p0, a.p1), mid(b.p0, b.p1)),
});

function reducePick(s: DimState, t: DimPick): DimStep {
  switch (t.on) {
    case "circle":
      return { state: { pending: null, ready: diameterSpec(t) } };
    case "arc":
      return { state: { pending: null, ready: radiusSpec(t) } };
    case "line": {
      if (s.pending && s.pending.on === "line" && s.pending.id !== t.id) {
        return { state: { pending: null, ready: angleSpec(s.pending, t) } };
      }
      // First line: arm its length, but remember it so a next distinct line upgrades.
      return { state: { pending: t, ready: lineLengthSpec(t) } };
    }
    case "point": {
      if (s.pending && s.pending.on === "point" && !samePoint(s.pending, t)) {
        return { state: { pending: null, ready: pointDistanceSpec(s.pending, t) } };
      }
      // First point: wait for a partner (no single-point dimension exists).
      return { state: { pending: t, ready: null } };
    }
  }
}

function samePoint(a: Extract<DimPick, { on: "point" }>, b: Extract<DimPick, { on: "point" }>): boolean {
  return a.id === b.id && a.position === b.position;
}

export function dimensionStep(s: DimState, e: DimEvent): DimStep {
  switch (e.kind) {
    case "cancel":
      return { state: dimensionInit() };
    case "commit":
      if (!s.ready) return { state: s };
      return { state: dimensionInit(), emit: { ...s.ready, value: e.value } };
    case "pick":
      return reducePick(s, e.target);
  }
}

// ── constraint authoring ──────────────────────────────────────────────────────

/** Build the authored SketchConstraint from a committed spec (mint id externally). */
export function buildDimensionConstraint(spec: DimensionSpec, id: string): SketchConstraint {
  const c: SketchConstraint = { id, type: spec.kind, entities: spec.entities, value: spec.value };
  if (spec.positions) c.positions = spec.positions;
  return c;
}

/** Chip suffix for a spec (mm for lengths, ° for angles). */
export function dimensionSuffix(kind: DimensionKind): string {
  return kind === "Angle" ? "°" : "mm";
}

// ── over-constraint decision (pure) ──────────────────────────────────────────
//
// The mock solver's `solveDof` reports OverConstrained when a constraint pushes
// the signed DOF surplus below zero, and never distinguishes a strictly redundant
// constraint from a genuinely conflicting one (it does not detect contradictory
// equations — see mockSketch.ts). So "reject on conflict" keys off the two solver
// states that mean "this dimension broke the sketch": OverConstrained and
// Conflicting. When the real PlaneGCS lane lands it will additionally surface a
// distinct `redundant` signal per constraint; widen this then (seam).

export function isConflictStatus(status: SketchSolveStatus): boolean {
  return status === "OverConstrained" || status === "Conflicting";
}

// ── click → pick resolution (pure) ────────────────────────────────────────────

/**
 * Resolve a plane click to a dimension pick against the session entities:
 *   1. the nearest named point (endpoint / center) within `tolWorld` → a point
 *      pick (so clicking a vertex starts a point-to-point distance), else
 *   2. the entity whose curve the click lands on within `tolWorld` → an entity
 *      pick (line length / circle diameter / arc radius).
 * Returns null when nothing is close enough.
 */
export function pickDimensionTarget(
  raw: Point2,
  entities: SketchEntity[],
  tolWorld: number,
): DimPick | null {
  // 1. Named points (higher priority — vertices win over the body).
  let bestPt: { d: number; pick: DimPick } | null = null;
  for (const e of entities) {
    for (const p of entityPoints(e)) {
      const d = Math.hypot(raw.x - p.coord[0], raw.y - p.coord[1]);
      if (d <= tolWorld && (!bestPt || d < bestPt.d)) {
        bestPt = { d, pick: { on: "point", id: p.entityId, position: p.position, coord: p.coord } };
      }
    }
  }
  if (bestPt) return bestPt.pick;

  // 2. Entity body under the cursor.
  let bestBody: { d: number; pick: DimPick } | null = null;
  for (const e of entities) {
    const near = nearestOnCurve(raw, e);
    if (!near) continue;
    const d = Math.hypot(raw.x - near.x, raw.y - near.y);
    if (d > tolWorld) continue;
    let pick: DimPick | null = null;
    if (e.type === "Line" && e.p0 && e.p1) pick = { on: "line", id: e.id, p0: e.p0, p1: e.p1 };
    else if (e.type === "Circle" && e.center && e.radius !== undefined)
      pick = { on: "circle", id: e.id, center: e.center, radius: e.radius };
    else if (e.type === "Arc" && e.center && e.radius !== undefined)
      pick = { on: "arc", id: e.id, center: e.center, radius: e.radius };
    if (pick && (!bestBody || d < bestBody.d)) bestBody = { d, pick };
  }
  return bestBody?.pick ?? null;
}
