/*
 * Engine bridge — the clean seam between the imperative ViewportEngine and the
 * React shell chrome that lives OUTSIDE <ViewportRoot> (NavPill, ViewCube).
 *
 * ViewportRoot registers the live engine here on init and clears it on dispose.
 * Chrome components read it via useViewportEngine(); store actions reach it via
 * getViewportEngine(). A module singleton (not context) is used because the
 * consumers are siblings of ViewportRoot, not descendants.
 */
import { useSyncExternalStore } from "react";
import type { ViewportEngine } from "./engine/ViewportEngine";

let current: ViewportEngine | null = null;
const listeners = new Set<() => void>();

export function setViewportEngine(engine: ViewportEngine | null): void {
  current = engine;
  for (const l of listeners) l();
}

export function getViewportEngine(): ViewportEngine | null {
  return current;
}

function subscribe(cb: () => void): () => void {
  listeners.add(cb);
  return () => {
    listeners.delete(cb);
  };
}

/** React hook: current engine (or null before it initializes). */
export function useViewportEngine(): ViewportEngine | null {
  return useSyncExternalStore(subscribe, getViewportEngine, () => null);
}
