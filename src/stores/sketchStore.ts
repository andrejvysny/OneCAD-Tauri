/*
 * Live sketch-edit store (F-WP6).
 *
 * Holds the authoritative sketch SESSION currently being edited (plane +
 * entities + constraints + dof/status) plus the entity/constraint id counters.
 * Written by the SketchController on enter / commit / solve; read by the
 * ConstraintBadgeLayer (badges at entity midpoints) and the engine's
 * SketchObject (via the controller). High-frequency interaction state (preview,
 * snap indicator, ghost badge) is NOT here — it is driven imperatively through
 * the engine so pointer-move does not churn React.
 *
 * DOF/status ALSO flow to documentStore.sketches[id] + viewportStore.dofBadge so
 * the chrome bar / inspector / status bar (bound since F-WP3) stay live.
 */
import { createStore, useStore } from "zustand";
import type { SketchSession } from "@/ipc/types";

export interface SketchState {
  session: SketchSession | null;
  entitySeq: number;
  constraintSeq: number;
  setSession(session: SketchSession | null): void;
  /** Mint the next entity id (`e1`, `e2`, …) and advance the counter. */
  nextEntityId(): string;
  /** Mint the next constraint id (`c1`, `c2`, …) and advance the counter. */
  nextConstraintId(): string;
  reset(): void;
}

export const sketchStore = createStore<SketchState>()((set, get) => ({
  session: null,
  entitySeq: 0,
  constraintSeq: 0,

  setSession(session) {
    set({ session });
  },

  nextEntityId() {
    const n = get().entitySeq + 1;
    set({ entitySeq: n });
    return `e${n}`;
  },

  nextConstraintId() {
    const n = get().constraintSeq + 1;
    set({ constraintSeq: n });
    return `c${n}`;
  },

  reset() {
    set({ session: null, entitySeq: 0, constraintSeq: 0 });
  },
}));

/** Typed selector hook over the vanilla store. */
export function useSketchStore<T>(selector: (s: SketchState) => T): T {
  return useStore(sketchStore, selector);
}
