/*
 * Worker-status store — the C++ sidecar lifecycle the status bar surfaces.
 *
 * Fed by the `worker-status` backend event (via `client.onWorkerStatus`, wired
 * once at the editor shell). `state` is null until the first event arrives; the
 * status bar shows an indicator only for the attention states (restarting /
 * failed). The mock never emits, so this stays null under vitest.
 */
import { createStore, useStore } from "zustand";
import type { WorkerStatus } from "@/ipc/types";

export type WorkerLifecycleState = WorkerStatus["state"] | null;

export interface WorkerStoreState {
  state: WorkerLifecycleState;
  epoch: number;
  set(status: WorkerStatus): void;
  reset(): void;
}

export const workerStore = createStore<WorkerStoreState>()((set) => ({
  state: null,
  epoch: 0,
  set(status) {
    set({ state: status.state, epoch: status.epoch });
  },
  reset() {
    set({ state: null, epoch: 0 });
  },
}));

/** Typed selector hook over the vanilla store. */
export function useWorkerStore<T>(selector: (s: WorkerStoreState) => T): T {
  return useStore(workerStore, selector);
}
