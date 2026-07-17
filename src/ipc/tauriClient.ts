/*
 * tauriClient — the REAL CadClient over the Tauri command/event surface (R-WP10/11).
 *
 * Implements the same `CadClient` interface as `mockClient`, mapping each method
 * onto a `#[tauri::command]` (`invoke`) or a backend event (`listen`). It is
 * constructed ONLY inside a Tauri webview (see `createClient` in `client.ts`);
 * dev-in-browser, vitest and Playwright keep the mock.
 *
 * ── What is REAL vs the SEAM ──────────────────────────────────────────────────
 *  REAL commands : lifecycle (new/open/import), dialogs, recents, get_mesh (the
 *                  MESH1 ArrayBuffer path), undo/redo, apply_edit_command; the
 *                  `document-changed` event (pull-model mesh refs).
 *  LOCAL SEAM    : the sketch SOLVER lane + the drag-time L2 PREVIEW lane run in
 *                  the shared `localSolver` module — the real backend does not
 *                  speak them yet. R-WP12 replaces this seam with solver-lane
 *                  verbs (BeginGesture / SolveDrag); a previewed op's COMMIT
 *                  already routes through the real `apply_edit_command`.
 *
 * ── Sync-over-async adapter ───────────────────────────────────────────────────
 * The frontend tool layer consumes a SYNCHRONOUS `ApplyOperationResult` (it drives
 * the stores from method results). The real backend is event-driven: an edit
 * command returns the PRE-regen projection, then regen publishes geometry LATER
 * via `document-changed`. `applyOperation`/`undo`/`redo` therefore correlate the
 * command's returned projection (features/revision) with the next `document-changed`
 * (changed/removed bodies) to synthesize the result the controllers expect.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { CadClient } from "./client";
import type {
  ApplyOperationResult,
  DocumentChange,
  DocumentSnapshot,
  FeatureRecord,
  Lod,
  OperationOp,
  RecentProject,
} from "./types";
import { createLocalSolverLane } from "./localSolver";
import { operationToEditCommand, opLabelFor } from "./tauriCommandMap";

// ── Command + event names (must match src-tauri/src/api + events.rs) ──────────
const CMD = {
  listRecents: "list_recents",
  newDocument: "new_document",
  openDocument: "open_document",
  importStep: "import_step",
  openFileDialog: "open_file_dialog",
  getMesh: "get_mesh",
  applyEditCommand: "apply_edit_command",
  undo: "undo",
  redo: "redo",
} as const;

const EVT = {
  documentChanged: "document-changed",
  projectionUpdated: "projection-updated",
} as const;

/** L2 preview pacing for the local seam (snappy; the real solver lane lands R-WP12). */
const PREVIEW_LATENCY_MS = 16;

/**
 * Max wait for a regen's `document-changed` after an edit command before falling
 * back to the pre-regen projection (empty body delta). Tunable for M2: a
 * `regen-finished` event (reserved in events.rs, not yet emitted) would let this
 * resolve promptly when regen yields no geometry change.
 */
let regenTimeoutMs = 8000;
/** Test seam: shrink the correlation timeout so a no-event path resolves fast. */
export function __setRegenTimeoutForTests(ms: number): void {
  regenTimeoutMs = ms;
}

// ── Wire DTOs (camelCase; mirror src-tauri/src/dto.rs) ────────────────────────
interface DocumentSnapshotDto {
  documentId: string;
  title: string;
}
interface DocumentProjectionDto {
  status: "empty" | "loading" | "ready";
  revision: number;
  title: string;
  dirty: boolean;
  bodies: Record<string, { id: string; name: string; visible: boolean }>;
  sketches: Record<string, unknown>;
  /** `FeatureDto` is field-identical to the frontend `FeatureRecord`. */
  features: FeatureRecord[];
}

/** Normalize a rejected command (Rust `ApiError {kind, message}`) into an Error. */
function toClientError(e: unknown): Error {
  if (e && typeof e === "object" && "kind" in e) {
    const { kind, message } = e as { kind: string; message?: string };
    const err = new Error(message ? `${kind}: ${message}` : kind);
    (err as Error & { kind?: string }).kind = kind;
    return err;
  }
  return e instanceof Error ? e : new Error(String(e));
}

/** Thin invoke wrapper that surfaces backend `ApiError`s as JS Errors. */
async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toClientError(e);
  }
}

export function createTauriClient(): CadClient {
  // Fan-out for app-level document-changed subscribers (MeshIngest).
  const docChangeListeners = new Set<(c: DocumentChange) => void>();
  // Latest authoritative projection (cached from projection-updated).
  let latestRevision = 0;

  // Correlation awaiters: resolved by the next document-changed with a higher rev.
  interface Awaiter {
    baseRev: number;
    resolve(c: DocumentChange | null): void;
    timer: ReturnType<typeof setTimeout>;
  }
  const awaiters = new Set<Awaiter>();

  function onDocumentChangedEvent(change: DocumentChange): void {
    latestRevision = Math.max(latestRevision, change.revision);
    for (const cb of [...docChangeListeners]) cb(change);
    for (const a of [...awaiters]) {
      if (change.revision > a.baseRev) {
        clearTimeout(a.timer);
        awaiters.delete(a);
        a.resolve(change);
      }
    }
  }

  /** Await the next regen publish (or null on timeout). Register BEFORE invoking. */
  function awaitNextChange(baseRev: number): { promise: Promise<DocumentChange | null>; cancel(): void } {
    let awaiter!: Awaiter;
    const promise = new Promise<DocumentChange | null>((resolve) => {
      const timer = setTimeout(() => {
        awaiters.delete(awaiter);
        resolve(null);
      }, regenTimeoutMs);
      awaiter = { baseRev, resolve, timer };
      awaiters.add(awaiter);
    });
    return {
      promise,
      cancel() {
        clearTimeout(awaiter.timer);
        awaiters.delete(awaiter);
      },
    };
  }

  // Persistent event listeners are started lazily (first correlation or first
  // subscribe) so pure command tests don't need the event plugin mocked.
  let eventsStarted = false;
  const unlisteners: UnlistenFn[] = [];
  async function ensureEvents(): Promise<void> {
    if (eventsStarted) return;
    eventsStarted = true;
    try {
      unlisteners.push(
        await listen<DocumentChange>(EVT.documentChanged, (e) => onDocumentChangedEvent(e.payload)),
      );
      unlisteners.push(
        await listen<DocumentProjectionDto>(EVT.projectionUpdated, (e) => {
          latestRevision = Math.max(latestRevision, e.payload.revision);
        }),
      );
    } catch {
      // A missing event bridge must not break command-only flows.
      eventsStarted = false;
    }
  }

  /** Run an edit command and correlate its regen into an ApplyOperationResult. */
  async function applyEdit(
    cmd: string,
    args: Record<string, unknown>,
    opLabel: string | undefined,
  ): Promise<ApplyOperationResult> {
    await ensureEvents();
    const baseRev = latestRevision;
    const awaiter = awaitNextChange(baseRev);
    let projection: DocumentProjectionDto;
    try {
      projection = await call<DocumentProjectionDto>(cmd, args);
    } catch (e) {
      awaiter.cancel();
      throw e;
    }
    const change = await awaiter.promise;
    return {
      revision: change?.revision ?? projection.revision,
      changedBodies: change?.changedBodies ?? [],
      removedBodies: change?.removedBodies ?? [],
      features: projection.features,
      opLabel,
    };
  }

  async function applyOperation(op: OperationOp): Promise<ApplyOperationResult> {
    const command = operationToEditCommand(op);
    return applyEdit(CMD.applyEditCommand, { command }, opLabelFor(op));
  }

  // The sketch + preview seam. Commit routes a previewed op through the REAL
  // apply_edit_command path above (R-WP12 replaces the drag-time lane itself).
  const lane = createLocalSolverLane({
    commit: applyOperation,
    latencyMs: () => PREVIEW_LATENCY_MS,
  });

  return {
    async listRecents(): Promise<RecentProject[]> {
      return call<RecentProject[]>(CMD.listRecents);
    },
    async newDocument(): Promise<DocumentSnapshot> {
      return call<DocumentSnapshotDto>(CMD.newDocument);
    },
    async openDocument(path: string): Promise<DocumentSnapshot> {
      return call<DocumentSnapshotDto>(CMD.openDocument, { path });
    },
    async importStep(path: string): Promise<DocumentSnapshot> {
      return call<DocumentSnapshotDto>(CMD.importStep, { path });
    },
    async openFileDialog(): Promise<string | null> {
      return call<string | null>(CMD.openFileDialog);
    },

    async getBodyMesh(bodyId: string, lod: Lod): Promise<ArrayBuffer> {
      // Rust `get_mesh` returns `tauri::ipc::Response` → invoke resolves an
      // ArrayBuffer (MESH1 bytes verbatim, zero-copy). generation null = latest.
      return call<ArrayBuffer>(CMD.getMesh, { bodyId, lod, generation: null });
    },

    onDocumentChanged(cb: (change: DocumentChange) => void): () => void {
      void ensureEvents();
      docChangeListeners.add(cb);
      return () => docChangeListeners.delete(cb);
    },

    applyOperation,

    async undo(): Promise<ApplyOperationResult> {
      return applyEdit(CMD.undo, {}, undefined);
    },
    async redo(): Promise<ApplyOperationResult> {
      return applyEdit(CMD.redo, {}, undefined);
    },

    // ── Sketch solver lane + two-level preview (shared local seam; R-WP12) ────
    enterSketch: lane.enterSketch,
    sketchUpsert: lane.sketchUpsert,
    finishSketch: lane.finishSketch,
    cancelSketch: lane.cancelSketch,
    beginPreview: lane.beginPreview,
    updatePreview: lane.updatePreview,
    endPreview: lane.endPreview,
    onPreviewResult: lane.onPreviewResult,
  };
}
