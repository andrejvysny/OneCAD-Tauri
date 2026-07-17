/*
 * Viewport chrome store.
 *
 * Holds the camera / display state the status bar + corner cluster render. The
 * viewport engine (F-WP4) writes `cursor` (pointer raycast onto Z=0) and
 * `cameraViewLabel` (on camera change) through ViewportRoot; `zoomFit`/`homeView`
 * dispatch to the live engine via the bridge. DOF is still a solver mock.
 */
import { createStore, useStore } from "zustand";
import { getViewportEngine } from "@/viewport/engineBridge";

export type Projection = "persp" | "ortho";
export type DisplayMode = "shaded" | "shadedEdges" | "wireframe";

export interface CursorCoords {
  x: number;
  y: number;
  z: number;
}

const DISPLAY_CYCLE: DisplayMode[] = ["shaded", "shadedEdges", "wireframe"];

export interface ViewportState {
  projection: Projection;
  displayMode: DisplayMode;
  gridVisible: boolean;
  activeSketchId: string | null;
  cameraViewLabel: string;
  fov: number;
  cursor: CursorCoords;
  /** Current DOF count the shell displays (mirrors the active sketch solver). */
  dofBadge: number | null;
  /** Transient status-bar hint (e.g. an unimplemented tool notice). */
  statusHint: string | null;
  /** Finish-sketch → auto-arm extrude handoff: the sketch just finished (F-WP7). */
  pendingExtrudeSketch: string | null;
  setPendingExtrude(sketchId: string | null): void;
  setProjection(p: Projection): void;
  cycleDisplayMode(): void;
  toggleGrid(): void;
  setActiveSketch(id: string | null): void;
  /** Engine → store: live pointer read-out (raycast onto Z=0). */
  setCursor(c: CursorCoords): void;
  /** Engine → store: canonical view name (TOP/FRONT/…/ISO/—). */
  setCameraViewLabel(label: string): void;
  /** Set/clear a transient status-bar hint. */
  setStatusHint(hint: string | null): void;
  /** Dispatch to the live viewport engine (no-op until it mounts). */
  zoomFit(): void;
  homeView(): void;
}

export const viewportStore = createStore<ViewportState>()((set) => ({
  projection: "persp",
  displayMode: "shaded",
  // Off by default so the grid button matches the prototype's neutral state; the
  // pressed (accent) treatment appears when toggled on. The viewport WP may flip
  // this default once a real grid renders.
  gridVisible: false,
  activeSketchId: null,
  cameraViewLabel: "TOP",
  fov: 76,
  cursor: { x: 273, y: 210, z: 0 },
  dofBadge: 3,
  statusHint: null,
  pendingExtrudeSketch: null,

  setPendingExtrude(sketchId) {
    set({ pendingExtrudeSketch: sketchId });
  },

  setProjection(p) {
    set({ projection: p });
  },

  cycleDisplayMode() {
    set((s) => {
      const next = DISPLAY_CYCLE[(DISPLAY_CYCLE.indexOf(s.displayMode) + 1) % DISPLAY_CYCLE.length];
      return { displayMode: next };
    });
  },

  toggleGrid() {
    set((s) => ({ gridVisible: !s.gridVisible }));
  },

  setActiveSketch(id) {
    set({ activeSketchId: id });
  },

  setCursor(c) {
    set({ cursor: c });
  },

  setCameraViewLabel(label) {
    set({ cameraViewLabel: label });
  },

  setStatusHint(hint) {
    set({ statusHint: hint });
  },

  // Dispatch to the live engine via the bridge; no-op before it mounts.
  zoomFit() {
    getViewportEngine()?.fitView();
  },
  homeView() {
    getViewportEngine()?.homeView();
  },
}));

/** Typed selector hook over the vanilla store. */
export function useViewportStore<T>(selector: (s: ViewportState) => T): T {
  return useStore(viewportStore, selector);
}

/**
 * Format the mono X/Y/Z read-out exactly like prototype 1c (white-space:pre):
 *   "X  273.00   Y  210.00   Z    0.00"
 * (axis + 2 spaces + value right-padded to width 6, columns joined by 3 spaces).
 */
export function formatCursor(c: CursorCoords): string {
  const cols: [string, number][] = [
    ["X", c.x],
    ["Y", c.y],
    ["Z", c.z],
  ];
  return cols.map(([ax, v]) => `${ax}  ${v.toFixed(2).padStart(6)}`).join("   ");
}
