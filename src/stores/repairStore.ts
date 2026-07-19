/*
 * Repair store (M4b) — the live topology-repair state the banner + repair panel
 * render from.
 *
 * Per the plan, projection stores are "written only by backend events"; the
 * `needs-repair` event (emitted after EVERY published regen — empty `items` means
 * repairs cleared) is relayed here via `client.onNeedsRepair` (wired once at the
 * editor shell, mirroring the worker-status dot). The panel-open + expanded-item
 * UI state is local (not from the backend).
 */
import { createStore, useStore } from "zustand";
import type { NeedsRepairEvent, NeedsRepairItem } from "@/ipc/types";

export interface RepairState {
  /** Revision the current `items` belong to (0 before any event). */
  revision: number;
  /** The refs still needing repair (empty ⇒ nothing to repair). */
  items: NeedsRepairItem[];
  /** Whether the repair panel is open (banner click / NeedsRepair selection). */
  panelOpen: boolean;
  /** The refId of the currently-expanded item (candidates fetched), or null. */
  expandedRefId: string | null;
  /**
   * World position of the candidate currently hovered in the panel, or null.
   * A clean DATA seam for a future engine marker (no engine coupling here —
   * ViewportRoot may subscribe and drop a temporary marker at this point).
   */
  hoveredWorldPos: [number, number, number] | null;

  /** Apply a `needs-repair` event: replace items; auto-close when cleared. */
  applyEvent(event: NeedsRepairEvent): void;
  /** Open the repair panel (banner click). */
  openPanel(): void;
  /** Close the repair panel + collapse any expanded item. */
  closePanel(): void;
  /** Expand one item (toggle: expanding another collapses the previous). */
  setExpanded(refId: string | null): void;
  /** Set (or clear) the hovered candidate world position. */
  setHoveredWorldPos(pos: [number, number, number] | null): void;
  reset(): void;
}

const INITIAL = {
  revision: 0,
  items: [] as NeedsRepairItem[],
  panelOpen: false,
  expandedRefId: null as string | null,
  hoveredWorldPos: null as [number, number, number] | null,
};

export const repairStore = createStore<RepairState>()((set) => ({
  ...INITIAL,

  applyEvent(event) {
    set((s) => {
      const cleared = event.items.length === 0;
      // Keep an expanded ref only if it still needs repair after the new event.
      const stillExpanded =
        s.expandedRefId && event.items.some((i) => i.refId === s.expandedRefId)
          ? s.expandedRefId
          : null;
      return {
        revision: event.revision,
        items: event.items,
        // Cleared repairs auto-dismiss the panel; otherwise leave it as the user left it.
        panelOpen: cleared ? false : s.panelOpen,
        expandedRefId: cleared ? null : stillExpanded,
        hoveredWorldPos: cleared ? null : s.hoveredWorldPos,
      };
    });
  },

  openPanel() {
    set({ panelOpen: true });
  },

  closePanel() {
    set({ panelOpen: false, expandedRefId: null, hoveredWorldPos: null });
  },

  setExpanded(refId) {
    set((s) => ({
      expandedRefId: s.expandedRefId === refId ? null : refId,
      hoveredWorldPos: null,
    }));
  },

  setHoveredWorldPos(pos) {
    set({ hoveredWorldPos: pos });
  },

  reset() {
    set({ ...INITIAL });
  },
}));

/** Typed selector hook over the vanilla store. */
export function useRepairStore<T>(selector: (s: RepairState) => T): T {
  return useStore(repairStore, selector);
}
