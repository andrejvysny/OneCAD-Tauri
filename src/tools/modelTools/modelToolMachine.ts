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
