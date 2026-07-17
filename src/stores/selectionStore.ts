/*
 * Selection store (F-WP3).
 *
 * Frontend selection is a list of opaque entity refs plus a transient hover.
 * V1 shell only needs body / sketch / feature refs (the tree + inspector);
 * face / edge / vertex are included in the union for the picking WP that will
 * promote (snapshot, BodyId, TopoKey) tokens into real element refs later.
 */
import { createStore, useStore } from "zustand";

export type EntityKind =
  | "body"
  | "sketch"
  | "feature"
  | "face"
  | "edge"
  | "vertex";

/**
 * Where a face/edge pick landed. Captured on click for a future
 * `AcquireElementIds` promotion (SCHEMA §7.5: frontend sends {topoKey, anchor},
 * Rust mints the ElementId).
 */
export interface SelectionAnchor {
  worldPoint: [number, number, number];
  surfaceUv?: [number, number];
}

export interface EntityRef {
  kind: EntityKind;
  /** Stable id. For face/edge this is the composite `${bodyId}#${topoKey}`. */
  id: string;
  /** Owning body (face/edge picks). */
  bodyId?: string;
  /** Snapshot-scoped TopoKey (`"f:22"` / `"e:5"`) for face/edge picks. */
  topoKey?: string;
  /** Persistent ElementId when already minted (else promoted on demand). */
  elementId?: string;
  /** Pick anchor, for AcquireElementIds promotion. */
  anchor?: SelectionAnchor;
}

export function sameRef(a: EntityRef, b: EntityRef): boolean {
  return a.kind === b.kind && a.id === b.id;
}

/** Composite id for a face/edge ref: `${bodyId}#${topoKey}`. */
export function topoRefId(bodyId: string, topoKey: string): string {
  return `${bodyId}#${topoKey}`;
}

export interface SelectionState {
  hover: EntityRef | null;
  selected: EntityRef[];
  set(refs: EntityRef[]): void;
  toggle(ref: EntityRef): void;
  clear(): void;
  setHover(ref: EntityRef | null): void;
}

export const selectionStore = createStore<SelectionState>()((set) => ({
  // Prototype 1c opens with Sketch 2 selected.
  selected: [{ kind: "sketch", id: "sketch2" }],
  hover: null,

  set(refs) {
    set({ selected: refs });
  },

  toggle(ref) {
    set((s) => {
      const has = s.selected.some((r) => sameRef(r, ref));
      return {
        selected: has
          ? s.selected.filter((r) => !sameRef(r, ref))
          : [...s.selected, ref],
      };
    });
  },

  clear() {
    set({ selected: [] });
  },

  setHover(ref) {
    set({ hover: ref });
  },
}));

/** Typed selector hook over the vanilla store. */
export function useSelectionStore<T>(selector: (s: SelectionState) => T): T {
  return useStore(selectionStore, selector);
}

/** Primary (single) selection — the inspector reads this. */
export function primarySelection(s: SelectionState): EntityRef | null {
  return s.selected[0] ?? null;
}
