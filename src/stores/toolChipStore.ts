/*
 * Tool-chip store (F-WP7) — the small bridge that lets the imperative
 * ModelToolController drive the React overlay chips (extrude depth, fillet
 * radius, boolean op) without owning DOM. The controller sets the descriptor +
 * callbacks; a React layer (ModelToolChips) renders the chip and registers its
 * node with the engine's HTML overlay so it tracks a world anchor each frame.
 *
 * `value` updates live during a drag (cheap: one tiny input re-renders); the
 * `worldPos` anchor is set once per arm and stays put, so no per-frame churn.
 */
import { createStore, useStore } from "zustand";
import type { BooleanOperation } from "@/ipc/types";

export type ChipKind = "none" | "extrudeDepth" | "filletRadius" | "booleanOp";

export interface ToolChipState {
  kind: ChipKind;
  /** Live dimensional value (depth / radius). */
  value: number;
  /** Selected boolean operation (booleanOp chip). */
  op: BooleanOperation;
  /** World anchor for the overlay driver, or null. */
  worldPos: [number, number, number] | null;
  /** Committed value from the editable chip (Enter/blur). */
  onValue: ((v: number) => void) | null;
  /** Boolean op selected. */
  onOp: ((op: BooleanOperation) => void) | null;
  /** Apply pressed (boolean chip). */
  onApply: (() => void) | null;

  showExtrude(value: number, worldPos: [number, number, number], onValue: (v: number) => void): void;
  showFillet(value: number, worldPos: [number, number, number], onValue: (v: number) => void): void;
  showBoolean(
    op: BooleanOperation,
    worldPos: [number, number, number],
    onOp: (op: BooleanOperation) => void,
    onApply: () => void,
  ): void;
  /** Update just the live value during a drag. */
  setValue(value: number): void;
  /** Update just the boolean op. */
  setOp(op: BooleanOperation): void;
  clear(): void;
}

const CLEARED = {
  kind: "none" as ChipKind,
  value: 0,
  op: "Union" as BooleanOperation,
  worldPos: null,
  onValue: null,
  onOp: null,
  onApply: null,
};

export const toolChipStore = createStore<ToolChipState>()((set) => ({
  ...CLEARED,

  showExtrude(value, worldPos, onValue) {
    set({ kind: "extrudeDepth", value, worldPos, onValue, onOp: null, onApply: null });
  },
  showFillet(value, worldPos, onValue) {
    set({ kind: "filletRadius", value, worldPos, onValue, onOp: null, onApply: null });
  },
  showBoolean(op, worldPos, onOp, onApply) {
    set({ kind: "booleanOp", op, worldPos, onOp, onApply, onValue: null });
  },
  setValue(value) {
    set({ value });
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
