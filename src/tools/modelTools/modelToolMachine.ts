/*
 * Model-tool state machines (PURE reducers) for the three F-WP7 tools. They own
 * ONLY the discrete phase transitions + parameter bookkeeping; the imperative
 * ModelToolController performs the emitted `effect` (begin/update preview,
 * commit, cancel). Keeping the transitions pure lets the whole interaction be
 * driven by unit-tested "pointer scripts" (mirrors the sketch toolMachine).
 *
 * Phase vocabulary mirrors toolStore.InteractionPhase:
 *   armed      — tool primed on a target (handle/chip shown, base preview live)
 *   dragging   — pointer owns the handle/edge (params track the drag)
 *   committing — pointer released; awaiting the exact L2 result before select
 */
import type { BooleanOperation } from "@/ipc/types";

export type ModelPhase = "idle" | "armed" | "dragging" | "committing";

/** The side-effect the controller runs after a transition. */
export type ToolEffect = "none" | "begin" | "update" | "commit" | "cancel" | "ghost";

// ── Extrude ──────────────────────────────────────────────────────────────────

export const DEFAULT_EXTRUDE_DEPTH = 10;

export interface ExtrudeFsm {
  phase: ModelPhase;
  depth: number;
  symmetric: boolean;
  hasRegion: boolean;
}

export type ExtrudeEvent =
  | { kind: "arm"; depth?: number }
  | { kind: "grab" }
  | { kind: "drag"; depth: number; symmetric?: boolean }
  | { kind: "setDepth"; depth: number }
  | { kind: "release" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface ExtrudeStep {
  state: ExtrudeFsm;
  effect: ToolEffect;
}

export function extrudeInit(): ExtrudeFsm {
  return { phase: "idle", depth: DEFAULT_EXTRUDE_DEPTH, symmetric: false, hasRegion: false };
}

export function extrudeStep(s: ExtrudeFsm, e: ExtrudeEvent): ExtrudeStep {
  switch (e.kind) {
    case "arm":
      return {
        state: { phase: "armed", depth: e.depth ?? DEFAULT_EXTRUDE_DEPTH, symmetric: false, hasRegion: true },
        effect: "begin",
      };
    case "grab":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "dragging" }, effect: "none" };
    case "drag":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return {
        state: { ...s, depth: e.depth, symmetric: e.symmetric ?? s.symmetric },
        effect: "update",
      };
    case "setDepth":
      if (s.phase !== "armed" && s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, depth: e.depth }, effect: "update" };
    case "release":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: extrudeInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: extrudeInit(), effect: "cancel" };
  }
}

// ── Fillet ───────────────────────────────────────────────────────────────────

export const DEFAULT_FILLET_RADIUS = 2;

export interface FilletFsm {
  phase: ModelPhase;
  radius: number;
  edgeCount: number;
}

export type FilletEvent =
  | { kind: "arm"; edgeCount: number; radius?: number }
  | { kind: "grabEdge" }
  | { kind: "drag"; radius: number }
  | { kind: "setRadius"; radius: number }
  | { kind: "release" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface FilletStep {
  state: FilletFsm;
  effect: ToolEffect;
}

export function filletInit(): FilletFsm {
  return { phase: "idle", radius: DEFAULT_FILLET_RADIUS, edgeCount: 0 };
}

export function filletStep(s: FilletFsm, e: FilletEvent): FilletStep {
  switch (e.kind) {
    case "arm":
      if (e.edgeCount <= 0) return { state: filletInit(), effect: "none" };
      return {
        state: { phase: "armed", radius: e.radius ?? DEFAULT_FILLET_RADIUS, edgeCount: e.edgeCount },
        effect: "begin",
      };
    case "grabEdge":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "dragging" }, effect: "none" };
    case "drag":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, radius: e.radius }, effect: "update" };
    case "setRadius":
      if (s.phase !== "armed" && s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, radius: e.radius }, effect: "update" };
    case "release":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: filletInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: filletInit(), effect: "cancel" };
  }
}

// ── Revolve ──────────────────────────────────────────────────────────────────
//
// Revolve adds an `axisPick` phase before the angle drag: the user first clicks a
// sketch LINE to set the axis of revolution, then drags to sweep 0–360°. A plain
// click after the axis is chosen commits the default full 360° (quickCommit).

export const DEFAULT_REVOLVE_ANGLE = 360;

export type RevolvePhase = "idle" | "axisPick" | "armed" | "dragging" | "committing";

export interface RevolveFsm {
  phase: RevolvePhase;
  /** Revolution angle in DEGREES (0–360). */
  angle: number;
  hasRegion: boolean;
  /** The chosen sketch-line axis id (null until an axis is picked). */
  axisLineId: string | null;
}

export type RevolveEvent =
  | { kind: "arm"; angle?: number; hasRegion?: boolean; hasAxis?: boolean; axisLineId?: string | null }
  | { kind: "pickAxis"; lineId: string; valid: boolean }
  | { kind: "resetAxis" }
  | { kind: "grab" }
  | { kind: "drag"; angle: number }
  | { kind: "setAngle"; angle: number }
  | { kind: "quickCommit" }
  | { kind: "release" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface RevolveStep {
  state: RevolveFsm;
  effect: ToolEffect;
}

export function revolveInit(): RevolveFsm {
  return { phase: "idle", angle: DEFAULT_REVOLVE_ANGLE, hasRegion: false, axisLineId: null };
}

export function revolveStep(s: RevolveFsm, e: RevolveEvent): RevolveStep {
  switch (e.kind) {
    case "arm": {
      if (e.hasRegion === false) return { state: revolveInit(), effect: "none" };
      const angle = e.angle ?? DEFAULT_REVOLVE_ANGLE;
      // A re-edit seeds an existing axis (param-only edit) → skip axis-pick.
      if (e.hasAxis) {
        return {
          state: { phase: "armed", angle, hasRegion: true, axisLineId: e.axisLineId ?? null },
          effect: "begin",
        };
      }
      return { state: { phase: "axisPick", angle, hasRegion: true, axisLineId: null }, effect: "none" };
    }
    case "pickAxis":
      if (s.phase !== "axisPick") return { state: s, effect: "none" };
      if (!e.valid) return { state: s, effect: "none" }; // reject: stay in axis-pick
      return { state: { ...s, phase: "armed", axisLineId: e.lineId }, effect: "begin" };
    case "resetAxis":
      if (s.phase !== "armed" && s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, phase: "axisPick", axisLineId: null }, effect: "none" };
    case "grab":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "dragging" }, effect: "none" };
    case "drag":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, angle: e.angle }, effect: "update" };
    case "setAngle":
      if (s.phase !== "armed" && s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, angle: e.angle }, effect: "update" };
    case "quickCommit":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "release":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: revolveInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: revolveInit(), effect: "cancel" };
  }
}

// ── Boolean ──────────────────────────────────────────────────────────────────

export type BooleanPhase = "idle" | "pickTool" | "armed" | "committing";

export interface BooleanFsm {
  phase: BooleanPhase;
  op: BooleanOperation;
  targetBodyId: string | null;
  toolBodyId: string | null;
}

export type BooleanEvent =
  | { kind: "start"; targetBodyId: string }
  | { kind: "pickTool"; toolBodyId: string }
  | { kind: "setOp"; op: BooleanOperation }
  | { kind: "apply" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface BooleanStep {
  state: BooleanFsm;
  effect: ToolEffect;
}

export function booleanInit(): BooleanFsm {
  return { phase: "idle", op: "Union", targetBodyId: null, toolBodyId: null };
}

export function booleanStep(s: BooleanFsm, e: BooleanEvent): BooleanStep {
  switch (e.kind) {
    case "start":
      return {
        state: { phase: "pickTool", op: "Union", targetBodyId: e.targetBodyId, toolBodyId: null },
        effect: "none",
      };
    case "pickTool":
      if (s.phase !== "pickTool" || e.toolBodyId === s.targetBodyId) return { state: s, effect: "none" };
      return { state: { ...s, phase: "armed", toolBodyId: e.toolBodyId }, effect: "ghost" };
    case "setOp":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, op: e.op }, effect: "none" };
    case "apply":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: booleanInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: booleanInit(), effect: "cancel" };
  }
}

// ── Shell ────────────────────────────────────────────────────────────────────
//
// Shell mirrors Fillet: it arms from a FACE selection (the selected faces are the
// removed/open faces), then a vertical drag (or the mm chip) sets the wall
// thickness, and release commits. There is no cheap L1 preview (hollowing needs
// OCCT), so it is chip + status-hint driven — the exact body arrives on commit.

export const DEFAULT_SHELL_THICKNESS = 2;

export interface ShellFsm {
  phase: ModelPhase;
  thickness: number;
  faceCount: number;
}

export type ShellEvent =
  | { kind: "arm"; faceCount: number; thickness?: number }
  | { kind: "grab" }
  | { kind: "drag"; thickness: number }
  | { kind: "setThickness"; thickness: number }
  | { kind: "release" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface ShellStep {
  state: ShellFsm;
  effect: ToolEffect;
}

export function shellInit(): ShellFsm {
  return { phase: "idle", thickness: DEFAULT_SHELL_THICKNESS, faceCount: 0 };
}

export function shellStep(s: ShellFsm, e: ShellEvent): ShellStep {
  switch (e.kind) {
    case "arm":
      if (e.faceCount <= 0) return { state: shellInit(), effect: "none" };
      return {
        state: { phase: "armed", thickness: e.thickness ?? DEFAULT_SHELL_THICKNESS, faceCount: e.faceCount },
        effect: "begin",
      };
    case "grab":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "dragging" }, effect: "none" };
    case "drag":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, thickness: e.thickness }, effect: "update" };
    case "setThickness":
      if (s.phase !== "armed" && s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, thickness: e.thickness }, effect: "update" };
    case "release":
      if (s.phase !== "dragging") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: shellInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: shellInit(), effect: "cancel" };
  }
}

// ── Linear pattern ─────────────────────────────────────────────────────────
//
// LinearPattern arms with a BODY selected; the user picks a world axis (X/Y/Z
// chip), an instance count (2–12 stepper) and spacing (mm chip). A live GHOST of
// translated body clones renders as any chip changes; Apply commits. There is no
// drag-to-commit — orbit stays free so the 3D ghost can be inspected (the
// spacing DRAG affordance is deferred; the chip covers it — see the WP report).

export type PatternAxis = "X" | "Y" | "Z";

export type ConfigPhase = "idle" | "armed" | "committing";

export interface LinearPatternFsm {
  phase: ConfigPhase;
  axis: PatternAxis;
  count: number;
  spacing: number;
  bodyId: string | null;
}

export type LinearPatternEvent =
  | { kind: "arm"; bodyId?: string; axis?: PatternAxis; count?: number; spacing?: number }
  | { kind: "setAxis"; axis: PatternAxis }
  | { kind: "setCount"; count: number }
  | { kind: "setSpacing"; spacing: number }
  | { kind: "apply" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface LinearPatternStep {
  state: LinearPatternFsm;
  effect: ToolEffect;
}

export const DEFAULT_PATTERN_COUNT = 3;
export const DEFAULT_LINEAR_SPACING = 20;

function clampCount(n: number): number {
  if (!Number.isFinite(n)) return DEFAULT_PATTERN_COUNT;
  return Math.max(2, Math.min(12, Math.round(n)));
}

export function linearPatternInit(): LinearPatternFsm {
  return { phase: "idle", axis: "X", count: DEFAULT_PATTERN_COUNT, spacing: DEFAULT_LINEAR_SPACING, bodyId: null };
}

export function linearPatternStep(s: LinearPatternFsm, e: LinearPatternEvent): LinearPatternStep {
  switch (e.kind) {
    case "arm":
      if (!e.bodyId) return { state: linearPatternInit(), effect: "none" };
      return {
        state: {
          phase: "armed",
          axis: e.axis ?? "X",
          count: clampCount(e.count ?? DEFAULT_PATTERN_COUNT),
          spacing: e.spacing ?? DEFAULT_LINEAR_SPACING,
          bodyId: e.bodyId,
        },
        effect: "ghost",
      };
    case "setAxis":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, axis: e.axis }, effect: "ghost" };
    case "setCount":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, count: clampCount(e.count) }, effect: "ghost" };
    case "setSpacing":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, spacing: e.spacing }, effect: "ghost" };
    case "apply":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: linearPatternInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: linearPatternInit(), effect: "cancel" };
  }
}

// ── Circular pattern ─────────────────────────────────────────────────────────
//
// Same shape as LinearPattern but the axis defaults to world Z and the numeric
// param is a TOTAL sweep angle (degrees). Ghost clones are rotated about the axis.

export interface CircularPatternFsm {
  phase: ConfigPhase;
  axis: PatternAxis;
  count: number;
  angle: number;
  bodyId: string | null;
}

export type CircularPatternEvent =
  | { kind: "arm"; bodyId?: string; axis?: PatternAxis; count?: number; angle?: number }
  | { kind: "setAxis"; axis: PatternAxis }
  | { kind: "setCount"; count: number }
  | { kind: "setAngle"; angle: number }
  | { kind: "apply" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface CircularPatternStep {
  state: CircularPatternFsm;
  effect: ToolEffect;
}

export const DEFAULT_CIRCULAR_ANGLE = 360;

function clampCircularAngle(a: number): number {
  if (!Number.isFinite(a)) return DEFAULT_CIRCULAR_ANGLE;
  return Math.max(1, Math.min(360, a));
}

export function circularPatternInit(): CircularPatternFsm {
  return { phase: "idle", axis: "Z", count: DEFAULT_PATTERN_COUNT, angle: DEFAULT_CIRCULAR_ANGLE, bodyId: null };
}

export function circularPatternStep(s: CircularPatternFsm, e: CircularPatternEvent): CircularPatternStep {
  switch (e.kind) {
    case "arm":
      if (!e.bodyId) return { state: circularPatternInit(), effect: "none" };
      return {
        state: {
          phase: "armed",
          axis: e.axis ?? "Z",
          count: clampCount(e.count ?? DEFAULT_PATTERN_COUNT),
          angle: clampCircularAngle(e.angle ?? DEFAULT_CIRCULAR_ANGLE),
          bodyId: e.bodyId,
        },
        effect: "ghost",
      };
    case "setAxis":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, axis: e.axis }, effect: "ghost" };
    case "setCount":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, count: clampCount(e.count) }, effect: "ghost" };
    case "setAngle":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, angle: clampCircularAngle(e.angle) }, effect: "ghost" };
    case "apply":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: circularPatternInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: circularPatternInit(), effect: "cancel" };
  }
}

// ── Mirror body ──────────────────────────────────────────────────────────────
//
// MirrorBody arms with a BODY selected; the user picks a world mirror plane
// (XY/XZ/YZ chip). A ghost of the mirrored clone renders; Apply commits.
// (Datum-plane picking is deferred to a later WP — see the WP report.)

export type MirrorPlane = "XY" | "XZ" | "YZ";

export interface MirrorFsm {
  phase: ConfigPhase;
  plane: MirrorPlane;
  bodyId: string | null;
}

export type MirrorEvent =
  | { kind: "arm"; bodyId?: string; plane?: MirrorPlane }
  | { kind: "setPlane"; plane: MirrorPlane }
  | { kind: "apply" }
  | { kind: "settle" }
  | { kind: "cancel" };

export interface MirrorStep {
  state: MirrorFsm;
  effect: ToolEffect;
}

export function mirrorInit(): MirrorFsm {
  return { phase: "idle", plane: "XY", bodyId: null };
}

export function mirrorStep(s: MirrorFsm, e: MirrorEvent): MirrorStep {
  switch (e.kind) {
    case "arm":
      if (!e.bodyId) return { state: mirrorInit(), effect: "none" };
      return { state: { phase: "armed", plane: e.plane ?? "XY", bodyId: e.bodyId }, effect: "ghost" };
    case "setPlane":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, plane: e.plane }, effect: "ghost" };
    case "apply":
      if (s.phase !== "armed") return { state: s, effect: "none" };
      return { state: { ...s, phase: "committing" }, effect: "commit" };
    case "settle":
      return { state: mirrorInit(), effect: "none" };
    case "cancel":
      if (s.phase === "idle") return { state: s, effect: "none" };
      return { state: mirrorInit(), effect: "cancel" };
  }
}
