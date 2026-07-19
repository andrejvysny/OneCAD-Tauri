/*
 * Tool-chip store (F-WP7 + M6b) — the small bridge that lets the imperative
 * ModelToolController drive the React overlay chips (extrude depth, fillet
 * radius, revolve angle, boolean op, and the M6b shell / pattern / mirror chips)
 * without owning DOM. The controller sets the descriptor + callbacks; a React
 * layer (ModelToolChips) renders the chip and registers its node with the
 * engine's HTML overlay so it tracks a world anchor each frame.
 *
 * `value`/`count`/`axis`/`plane`/`op` update live during a chip edit (cheap: one
 * tiny input re-renders); the `worldPos` anchor is set once per arm and stays
 * put, so no per-frame churn.
 */
import { createStore, useStore } from "zustand";
import type { BooleanOperation } from "@/ipc/types";
import type { PatternAxis, MirrorPlane } from "@/tools/modelTools/modelToolMachine";

export type ChipKind =
  | "none"
  | "extrudeDepth"
  | "filletRadius"
  | "revolveAngle"
  | "booleanOp"
  | "shellThickness"
  | "linearPattern"
  | "circularPattern"
  | "mirror"
  | "dimension";

export interface ToolChipState {
  kind: ChipKind;
  /** Live dimensional value (depth / radius / thickness / spacing / angle). */
  value: number;
  /** Live instance count (linear / circular pattern chips). */
  count: number;
  /** Selected world axis (pattern chips). */
  axis: PatternAxis;
  /** Selected mirror plane (mirror chip). */
  plane: MirrorPlane;
  /** Selected boolean operation (booleanOp chip). */
  op: BooleanOperation;
  /** Unit suffix for the numeric chip (mm / ° — sketch dimension chip). */
  suffix: string;
  /** World anchor for the overlay driver, or null. */
  worldPos: [number, number, number] | null;
  /** Committed value from the editable chip (Enter/blur). */
  onValue: ((v: number) => void) | null;
  /** Esc / cancel from the dimension chip. */
  onCancel: (() => void) | null;
  /** Boolean op selected. */
  onOp: ((op: BooleanOperation) => void) | null;
  /** Apply pressed (boolean / pattern / mirror chip). */
  onApply: (() => void) | null;
  /** Axis-reset pressed (revolve chip). */
  onResetAxis: (() => void) | null;
  /** World-axis toggled (pattern chips). */
  onAxis: ((axis: PatternAxis) => void) | null;
  /** Mirror plane toggled (mirror chip). */
  onPlane: ((plane: MirrorPlane) => void) | null;
  /** Instance count stepped (pattern chips). */
  onCount: ((count: number) => void) | null;

  showExtrude(value: number, worldPos: [number, number, number], onValue: (v: number) => void): void;
  showFillet(value: number, worldPos: [number, number, number], onValue: (v: number) => void): void;
  showRevolve(
    value: number,
    worldPos: [number, number, number],
    onValue: (v: number) => void,
    onResetAxis: () => void,
  ): void;
  showBoolean(
    op: BooleanOperation,
    worldPos: [number, number, number],
    onOp: (op: BooleanOperation) => void,
    onApply: () => void,
  ): void;
  showShell(value: number, worldPos: [number, number, number], onValue: (v: number) => void): void;
  showLinearPattern(
    axis: PatternAxis,
    count: number,
    spacing: number,
    worldPos: [number, number, number],
    handlers: {
      onAxis: (axis: PatternAxis) => void;
      onCount: (count: number) => void;
      onSpacing: (spacing: number) => void;
      onApply: () => void;
    },
  ): void;
  showCircularPattern(
    axis: PatternAxis,
    count: number,
    angle: number,
    worldPos: [number, number, number],
    handlers: {
      onAxis: (axis: PatternAxis) => void;
      onCount: (count: number) => void;
      onAngle: (angle: number) => void;
      onApply: () => void;
    },
  ): void;
  showMirror(
    plane: MirrorPlane,
    worldPos: [number, number, number],
    handlers: { onPlane: (plane: MirrorPlane) => void; onApply: () => void },
  ): void;
  /** Show the sketch Dimension chip (seeded, auto-focused; Enter commits, Esc cancels). */
  showDimension(
    value: number,
    suffix: string,
    worldPos: [number, number, number],
    onValue: (v: number) => void,
    onCancel: () => void,
  ): void;
  /** Update just the live value during a drag / edit. */
  setValue(value: number): void;
  /** Update just the live instance count. */
  setCount(count: number): void;
  /** Update just the selected axis. */
  setAxis(axis: PatternAxis): void;
  /** Update just the selected mirror plane. */
  setPlane(plane: MirrorPlane): void;
  /** Update just the boolean op. */
  setOp(op: BooleanOperation): void;
  clear(): void;
}

const CLEARED = {
  kind: "none" as ChipKind,
  value: 0,
  count: 3,
  axis: "X" as PatternAxis,
  plane: "XY" as MirrorPlane,
  op: "Union" as BooleanOperation,
  suffix: "",
  worldPos: null,
  onValue: null,
  onCancel: null,
  onOp: null,
  onApply: null,
  onResetAxis: null,
  onAxis: null,
  onPlane: null,
  onCount: null,
};

export const toolChipStore = createStore<ToolChipState>()((set) => ({
  ...CLEARED,

  showExtrude(value, worldPos, onValue) {
    set({ ...CLEARED, kind: "extrudeDepth", value, worldPos, onValue });
  },
  showFillet(value, worldPos, onValue) {
    set({ ...CLEARED, kind: "filletRadius", value, worldPos, onValue });
  },
  showRevolve(value, worldPos, onValue, onResetAxis) {
    set({ ...CLEARED, kind: "revolveAngle", value, worldPos, onValue, onResetAxis });
  },
  showBoolean(op, worldPos, onOp, onApply) {
    set({ ...CLEARED, kind: "booleanOp", op, worldPos, onOp, onApply });
  },
  showShell(value, worldPos, onValue) {
    set({ ...CLEARED, kind: "shellThickness", value, worldPos, onValue });
  },
  showLinearPattern(axis, count, spacing, worldPos, handlers) {
    set({
      ...CLEARED,
      kind: "linearPattern",
      axis,
      count,
      value: spacing,
      worldPos,
      onAxis: handlers.onAxis,
      onCount: handlers.onCount,
      onValue: handlers.onSpacing,
      onApply: handlers.onApply,
    });
  },
  showCircularPattern(axis, count, angle, worldPos, handlers) {
    set({
      ...CLEARED,
      kind: "circularPattern",
      axis,
      count,
      value: angle,
      worldPos,
      onAxis: handlers.onAxis,
      onCount: handlers.onCount,
      onValue: handlers.onAngle,
      onApply: handlers.onApply,
    });
  },
  showMirror(plane, worldPos, handlers) {
    set({ ...CLEARED, kind: "mirror", plane, worldPos, onPlane: handlers.onPlane, onApply: handlers.onApply });
  },
  showDimension(value, suffix, worldPos, onValue, onCancel) {
    set({ ...CLEARED, kind: "dimension", value, suffix, worldPos, onValue, onCancel });
  },
  setValue(value) {
    set({ value });
  },
  setCount(count) {
    set({ count });
  },
  setAxis(axis) {
    set({ axis });
  },
  setPlane(plane) {
    set({ plane });
  },
  setOp(op) {
    set({ op });
  },
  clear() {
    set({ ...CLEARED });
  },
}));

/** Typed selector hook over the vanilla store. */
export function useToolChipStore<T>(selector: (s: ToolChipState) => T): T {
  return useStore(toolChipStore, selector);
}
