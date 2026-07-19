/*
 * Settings store (F-WP3) — snap / show preferences behind the corner-cluster
 * snap popover. Persisted to localStorage under a versioned key so choices
 * survive reloads (prototype 1c snap popover; defaults from its winInit()).
 */
import { createStore, useStore } from "zustand";
import { persist } from "zustand/middleware";

export interface SnapSettings {
  grid: boolean;
  sketchGuideLines: boolean;
  sketchGuidePoints: boolean;
  /** Circle/arc 0/90/180/270° quadrant snaps (M6c parity, default on). */
  quadrant: boolean;
  /** Entity-entity intersection snaps (M6c parity, default on). */
  intersection: boolean;
  /** Nearest-point-on-curve snaps (M6c parity, default on). */
  onCurve: boolean;
  guidePoints3d: boolean;
  distantEdges: boolean;
}

export interface ShowSettings {
  guidePoints: boolean;
  snappingHints: boolean;
}

export type SnapKey = keyof SnapSettings;
export type ShowKey = keyof ShowSettings;

export interface SettingsState {
  snapTo: SnapSettings;
  show: ShowSettings;
  /**
   * Experimental: use the WebGPU renderer when the platform supports it. Default
   * false — WebGL is the tested path. Gates the WebGPU code path in renderer.ts.
   */
  experimentalWebGpu: boolean;
  setSnap(key: SnapKey, value: boolean): void;
  setShow(key: ShowKey, value: boolean): void;
  setExperimentalWebGpu(value: boolean): void;
}

/** Versioned localStorage key (bump `version` on a breaking shape change). */
const STORAGE_KEY = "onecad.settings";

export const settingsStore = createStore<SettingsState>()(
  persist(
    (set) => ({
      snapTo: {
        grid: true,
        sketchGuideLines: true,
        sketchGuidePoints: true,
        quadrant: true,
        intersection: true,
        onCurve: true,
        guidePoints3d: true,
        distantEdges: false,
      },
      show: {
        guidePoints: true,
        snappingHints: true,
      },
      experimentalWebGpu: false,
      setSnap(key, value) {
        set((s) => ({ snapTo: { ...s.snapTo, [key]: value } }));
      },
      setShow(key, value) {
        set((s) => ({ show: { ...s.show, [key]: value } }));
      },
      setExperimentalWebGpu(value) {
        set({ experimentalWebGpu: value });
      },
    }),
    {
      name: STORAGE_KEY,
      version: 2,
      // v1 → v2 added the M6c snap types (quadrant / intersection / onCurve).
      // A v1 blob has no keys for them; backfill the on-by-default values so an
      // existing user's popover shows them enabled (parity with a fresh install).
      migrate: (persisted, version) => {
        const s = persisted as Partial<SettingsState>;
        if (s && version < 2) {
          s.snapTo = {
            quadrant: true,
            intersection: true,
            onCurve: true,
            ...(s.snapTo as Partial<SnapSettings>),
          } as SnapSettings;
        }
        return s as unknown as SettingsState;
      },
    },
  ),
);

/** Typed selector hook over the vanilla store. */
export function useSettingsStore<T>(selector: (s: SettingsState) => T): T {
  return useStore(settingsStore, selector);
}
