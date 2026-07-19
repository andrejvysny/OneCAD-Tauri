/*
 * File actions — the shared bridge the TitleBar File menu + the ⌘S/⇧⌘S/⌘O
 * shortcuts both call. Each routes through the memoized CadClient (Rust owns the
 * dialogs + filesystem), surfaces success/errors through the F-WP9 status-hint
 * pattern (`viewportStore.setStatusHint`), and refreshes the recents projection
 * after an open/save so the persisted store and the start screen stay in sync.
 */
import { createClient } from "@/ipc/client";
import { viewportStore } from "@/stores/viewportStore";
import { documentStore } from "@/stores/documentStore";
import { appStore } from "@/stores/appStore";

const client = createClient();

/** Transient-hint lifetime (ms) before it clears itself (if still showing). */
const TRANSIENT_MS = 2500;

let hintTimer: ReturnType<typeof setTimeout> | null = null;

/** Show a self-clearing success hint (does not stomp a newer hint on clear). */
function transientHint(message: string): void {
  viewportStore.getState().setStatusHint(message);
  if (hintTimer) clearTimeout(hintTimer);
  hintTimer = setTimeout(() => {
    hintTimer = null;
    if (viewportStore.getState().statusHint === message) {
      viewportStore.getState().setStatusHint(null);
    }
  }, TRANSIENT_MS);
}

/** Surface a recoverable failure (stays until the next status change). */
function errorHint(message: string): void {
  if (hintTimer) {
    clearTimeout(hintTimer);
    hintTimer = null;
  }
  viewportStore.getState().setStatusHint(message);
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** The backend reports a missing save path as an `io` ApiError — Save then Save-As. */
function isNoPathError(e: unknown): boolean {
  return message(e).toLowerCase().includes("no save path");
}

/** File stem for a path (drops directories + extension), for the "Saved ⟨name⟩" hint. */
function baseName(path: string): string {
  const file = path.split(/[\\/]/).pop() ?? path;
  return file.replace(/\.[^.]+$/, "");
}

function docName(): string {
  return documentStore.getState().title || "document";
}

function refreshRecents(): void {
  void appStore.getState().loadRecents();
}

/** ⌘S: save to the known path; a never-saved document falls back to Save As. */
export async function saveDocument(): Promise<void> {
  try {
    await client.saveDocument();
    transientHint(`Saved ${docName()}`);
    refreshRecents();
  } catch (e) {
    if (isNoPathError(e)) {
      await saveDocumentAs();
      return;
    }
    errorHint(`Save failed: ${message(e)}`);
  }
}

/** ⇧⌘S: dialog + save. A cancelled dialog is a no-op (no hint). */
export async function saveDocumentAs(): Promise<void> {
  try {
    const path = await client.saveDocumentAs();
    if (!path) return; // cancelled
    transientHint(`Saved ${baseName(path)}`);
    refreshRecents();
  } catch (e) {
    errorHint(`Save failed: ${message(e)}`);
  }
}

/** Export STEP…: dialog (Rust) + worker export. A cancelled dialog is a no-op. */
export async function exportStep(): Promise<void> {
  try {
    const path = await client.exportStep();
    if (!path) return; // cancelled
    transientHint(`Exported ${baseName(path)}`);
  } catch (e) {
    errorHint(`Export failed: ${message(e)}`);
  }
}

/** ⌘O: native open dialog + open (+ recents refresh); stays on the editor shell. */
export async function openDocumentDialog(): Promise<void> {
  try {
    await appStore.getState().openDialogAndOpen();
  } catch (e) {
    errorHint(`Open failed: ${message(e)}`);
  }
}
