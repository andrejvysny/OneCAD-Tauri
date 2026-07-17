/*
 * CadClient — the frontend's single seam to the backend.
 *
 * Only the surface the START SCREEN needs today is declared. Later WPs EXTEND
 * this interface (document mutations, mesh fetch, picking, solver gestures, …);
 * keep additions append-only so the mock + real clients evolve together.
 *
 * Two implementations satisfy CadClient:
 *   - mockClient  (this WP) — in-memory, drives the whole UI with no backend.
 *   - tauri client (F-WP8) — real IPC via tauri commands + the C++ worker.
 */
import type {
  ApplyOperationResult,
  DocumentChange,
  DocumentSnapshot,
  EnterSketchTarget,
  FinishSketchResult,
  Lod,
  OperationOp,
  PreviewDraft,
  PreviewParams,
  PreviewResult,
  PreviewSession,
  RecentProject,
  SketchConstraint,
  SketchEntity,
  SketchSession,
  SketchUpsertResult,
  Unsubscribe,
} from "./types";
import { mockClient } from "./mockClient";
import { createTauriClient } from "./tauriClient";

export interface CadClient {
  /** Recent projects for the start screen list. */
  listRecents(): Promise<RecentProject[]>;
  /** Create a blank document and open it. */
  newDocument(): Promise<DocumentSnapshot>;
  /** Open an existing .onecad project at `path`. */
  openDocument(path: string): Promise<DocumentSnapshot>;
  /** Import a STEP file at `path` into a new document. */
  importStep(path: string): Promise<DocumentSnapshot>;
  /**
   * Show a native file-open dialog and resolve to the chosen path (or null if
   * cancelled). Rust owns the real dialog (tauri-plugin-dialog Rust API — the
   * webview gets zero fs/dialog capability); the mock returns a fake path.
   */
  openFileDialog(): Promise<string | null>;

  /**
   * Fetch a body's MESH1 blob at `lod` (pull model). The real client returns a
   * single-body blob verbatim from the Rust MeshCache via `tauri::ipc::Response`
   * (zero-copy ArrayBuffer). The mock synthesizes the exact bytes locally.
   */
  getBodyMesh(bodyId: string, lod: Lod): Promise<ArrayBuffer>;

  /**
   * Subscribe to backend `document-changed` events. Fires with the changed +
   * removed bodies so the viewport can fetch/swap/drop meshes. Returns an
   * unsubscribe. The real event arrives from the worker (F-WP8); the mock is an
   * in-process emitter.
   */
  onDocumentChanged(cb: (change: DocumentChange) => void): Unsubscribe;

  // ── Sketch solver lane (SCHEMA §7.4) ──────────────────────────────────────
  // The real client routes these to the worker's PlaneGCS actor; the mock runs
  // an in-memory naive "solver" so the full sketch UI works with no backend.

  /**
   * Open a sketch for editing. `target` is an existing sketch id or a request
   * for a fresh sketch on a named plane. Returns the authoritative sketch
   * (plane basis + entities + constraints + dof/status).
   */
  enterSketch(target: EnterSketchTarget): Promise<SketchSession>;

  /**
   * Upsert the authoritative sketch (append/replace entities + constraints) and
   * re-solve. Returns the new dof/status and any CHANGED point coordinates.
   */
  sketchUpsert(
    sketchId: string,
    entities: SketchEntity[],
    constraints: SketchConstraint[],
  ): Promise<SketchUpsertResult>;

  /** Compute the closed profile regions for a sketch (extrude/revolve input). */
  finishSketch(sketchId: string): Promise<FinishSketchResult>;

  /** Discard the in-flight sketch edit session (no geometry change). */
  cancelSketch(sketchId: string): Promise<void>;

  // ── Model operations + two-level preview (SCHEMA §7.3 / NEW_SPEC §15) ──────
  // The real client routes these to the worker's ExecutePlan (op) + solver-style
  // preview lane; the mock synthesizes bodies + a feature timeline locally.

  /**
   * Apply one operation (Extrude / Fillet / Boolean) and commit it, returning the
   * new revision, the changed/removed bodies (pull-model mesh refs) and the full
   * feature timeline. A `featureId` on the op re-targets an existing feature
   * (parametric edit). Also emits a `document-changed` event.
   */
  applyOperation(op: OperationOp): Promise<ApplyOperationResult>;

  /**
   * Open a Level-2 preview session for a drafted op (NEW_SPEC §15). Returns the
   * session id + the body id the exact preview mesh is published under. The
   * frontend runs the Level-1 preview locally while streaming param updates here.
   */
  beginPreview(draft: PreviewDraft): Promise<PreviewSession>;

  /**
   * Push the latest drag params for a preview session (fire-and-forget,
   * latest-wins). `epoch` stamps the request so the frontend can discard stale
   * exact results. Exact meshes arrive asynchronously via `onPreviewResult`.
   */
  updatePreview(sessionId: string, params: PreviewParams, epoch: number): void;

  /**
   * End a preview session. `commit` materializes the op (same result shape as
   * `applyOperation`, plus a committed exact mesh on `onPreviewResult`); a cancel
   * discards the scratch state and resolves null.
   */
  endPreview(sessionId: string, commit: boolean): Promise<ApplyOperationResult | null>;

  /** Subscribe to exact Level-2 preview results (carry their epoch for reconcile). */
  onPreviewResult(cb: (r: PreviewResult) => void): Unsubscribe;

  /** Undo the last committed op → new revision + changed/removed bodies + timeline. */
  undo(): Promise<ApplyOperationResult>;
  /** Redo the last undone op. */
  redo(): Promise<ApplyOperationResult>;
}

/**
 * Pick the client for the current runtime — the single construction point.
 *
 * Inside a Tauri webview `window.__TAURI_INTERNALS__` is injected (the property
 * `invoke` bridge lives on it); we build the real `tauriClient`. In a plain
 * browser, vitest jsdom, or Playwright-over-vite there is no bridge, so the mock
 * drives the whole UI unchanged. `mockIPC` (test) also sets `__TAURI_INTERNALS__`,
 * so a test that mocks the bridge exercises the real client — as intended.
 *
 * The tauri client is memoized: it owns persistent event listeners + one solver
 * lane, so all call sites (appStore, ViewportRoot, badge layer) share one.
 */
let tauriClientSingleton: CadClient | null = null;

export function createClient(): CadClient {
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    return (tauriClientSingleton ??= createTauriClient());
  }
  return mockClient;
}
