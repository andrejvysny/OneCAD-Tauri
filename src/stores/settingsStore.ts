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
    { name: STORAGE_KEY, version: 1 },
  ),
);

/** Typed selector hook over the vanilla store. */
export function useSettingsStore<T>(selector: (s: SettingsState) => T): T {
  return useStore(settingsStore, selector);
}
