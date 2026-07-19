/*
 * App-level store: which screen is showing + the recent-projects projection.
 *
 * Pattern (per plan): zustand v5 *vanilla* store + a thin typed hook. Actions
 * call the CadClient and flip `screen` to 'editor' once a document snapshot
 * comes back. The editor itself is a later WP (placeholder for now).
 */
import { createStore, useStore } from "zustand";
import { createClient } from "@/ipc/client";
import type { DocumentSnapshot, RecentProject, RecoveryInfo } from "@/ipc/types";

const client = createClient();

type Screen = "start" | "editor";
type RecentsStatus = "idle" | "loading" | "ready";
type RecoveryStatus = "idle" | "loading" | "ready";

export interface AppState {
  screen: Screen;
  recents: RecentProject[];
  recentsStatus: RecentsStatus;
  /** The document opened when transitioning to the editor. */
  document: DocumentSnapshot | null;
  /** A crashed session's autosave offer (null once checked-and-empty or resolved). */
  recovery: RecoveryInfo | null;
  recoveryStatus: RecoveryStatus;

  loadRecents(): Promise<void>;
  newProject(): Promise<void>;
  openProject(path: string): Promise<void>;
  openDialogAndOpen(): Promise<void>;
  importStep(): Promise<void>;
  checkRecovery(): Promise<void>;
  recoverDocument(): Promise<void>;
  discardRecovery(): Promise<void>;
}

export const appStore = createStore<AppState>()((set, get) => {
  const enter = (document: DocumentSnapshot) =>
    set({ screen: "editor", document });

  return {
    screen: "start",
    recents: [],
    recentsStatus: "idle",
    document: null,
    recovery: null,
    recoveryStatus: "idle",

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
      // The backend recorded this open in the recents store; refresh so the
      // start-screen list reflects it (newest first) next time it renders.
      void get().loadRecents();
    },

    async openDialogAndOpen() {
      const path = await client.openFileDialog();
      if (!path) return; // cancelled
      enter(await client.openDocument(path));
      void get().loadRecents();
    },

    async importStep() {
      const path = await client.openFileDialog();
      if (path) enter(await client.importStep(path));
    },

    async checkRecovery() {
      set({ recoveryStatus: "loading" });
      const recovery = await client.checkRecovery();
      set({ recovery, recoveryStatus: "ready" });
    },

    async recoverDocument() {
      const snap = await client.recoverDocument(true);
      set({ recovery: null, recoveryStatus: "ready" });
      if (snap) enter(snap);
      // The recovered open counts as a recent; refresh the start-screen list.
      void get().loadRecents();
    },

    async discardRecovery() {
      await client.recoverDocument(false);
      set({ recovery: null, recoveryStatus: "ready" });
    },
  };
});

/** Typed selector hook over the vanilla store. */
export function useAppStore<T>(selector: (s: AppState) => T): T {
  return useStore(appStore, selector);
}
