/*
 * App-level store: which screen is showing + the recent-projects projection.
 *
 * Pattern (per plan): zustand v5 *vanilla* store + a thin typed hook. Actions
 * call the CadClient and flip `screen` to 'editor' once a document snapshot
 * comes back. The editor itself is a later WP (placeholder for now).
 */
import { createStore, useStore } from "zustand";
import { createClient } from "@/ipc/client";
import type { DocumentSnapshot, RecentProject } from "@/ipc/types";

const client = createClient();

type Screen = "start" | "editor";
type RecentsStatus = "idle" | "loading" | "ready";

export interface AppState {
  screen: Screen;
  recents: RecentProject[];
  recentsStatus: RecentsStatus;
  /** The document opened when transitioning to the editor. */
  document: DocumentSnapshot | null;

  loadRecents(): Promise<void>;
  newProject(): Promise<void>;
  openProject(path: string): Promise<void>;
  openDialogAndOpen(): Promise<void>;
  importStep(): Promise<void>;
}

export const appStore = createStore<AppState>()((set) => {
  const enter = (document: DocumentSnapshot) =>
    set({ screen: "editor", document });

  return {
    screen: "start",
    recents: [],
    recentsStatus: "idle",
    document: null,

    async loadRecents() {
      set({ recentsStatus: "loading" });
      const recents = await client.listRecents();
      set({ recents, recentsStatus: "ready" });
    },

    async newProject() {
      enter(await client.newDocument());
    },

    async openProject(path) {
      enter(await client.openDocument(path));
    },

    async openDialogAndOpen() {
      const path = await client.openFileDialog();
      if (path) enter(await client.openDocument(path));
    },

    async importStep() {
      const path = await client.openFileDialog();
      if (path) enter(await client.importStep(path));
    },
  };
});

/** Typed selector hook over the vanilla store. */
export function useAppStore<T>(selector: (s: AppState) => T): T {
  return useStore(appStore, selector);
}
