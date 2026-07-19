/*
 * History overlay store (M4b) — OPTIMISTIC, frontend-only history UI state that
 * the projection cannot carry.
 *
 * SEAM: the Rust projection DTO maps `StepState::Suppressed` → `FeatureStatus`
 * `dirty` (dto.rs `feature_status`), so a suppressed feature is INDISTINGUISHABLE
 * from a dirty one in the authoritative projection. Until the projection surfaces
 * a dedicated `suppressed` flag, the history list dims rows from this optimistic
 * overlay, flipped the instant the user clicks Suppress/Unsuppress (the edit still
 * commits through `SetOperationSuppression`).
 */
import { createStore, useStore } from "zustand";

export interface HistoryOverlayState {
  /** opId → optimistic suppressed flag (absent ⇒ not suppressed). */
  suppressed: Record<string, boolean>;
  setSuppressed(opId: string, suppressed: boolean): void;
  reset(): void;
}

export const historyStore = createStore<HistoryOverlayState>()((set) => ({
  suppressed: {},
  setSuppressed(opId, suppressed) {
    set((s) => {
      const next = { ...s.suppressed };
      if (suppressed) next[opId] = true;
      else delete next[opId];
      return { suppressed: next };
    });
  },
  reset() {
    set({ suppressed: {} });
  },
}));

/** Typed selector hook over the vanilla store. */
export function useHistoryStore<T>(selector: (s: HistoryOverlayState) => T): T {
  return useStore(historyStore, selector);
}
