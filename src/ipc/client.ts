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
  BeginGestureResult,
  DocumentChange,
  DocumentProjectionWire,
  DocumentSnapshot,
  DragSolveResult,
  EnterSketchTarget,
  FinishSketchResult,
  Lod,
  NeedsRepairEvent,
  OperationOp,
  PreviewDraft,
  PreviewParams,
  PreviewResult,
  PreviewSession,
  PromotedElement,
  PromotePick,
  RecentProject,
  RecoveryInfo,
  ResolveRefRequest,
  ResolveRefResult,
  SketchConstraint,
  SketchEntity,
  SketchSession,
  SketchUpsertResult,
  Unsubscribe,
  WorkerStatus,
} from "./types";
import type { WireEditCommand } from "./tauriCommandMap";
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
   * Save the open document. `path` `undefined` reuses the last save path; a
   * never-saved document then rejects (io error) and the caller falls back to
   * Save As. Rust owns the filesystem write.
   */
  saveDocument(path?: string): Promise<void>;
  /**
   * Save As: show a native save dialog (`.onecad`), save to the chosen path, and
   * return it — or null if the dialog was cancelled.
   */
  saveDocumentAs(): Promise<string | null>;
  /**
   * Export every body at head to a STEP file. Rust owns the `.step` save dialog +
   * the worker ExportStep verb; resolves to the written path, or null on cancel.
   */
  exportStep(): Promise<string | null>;
  /**
   * Export every body at head to a binary STL file. Rust owns the `.stl` save
   * dialog + the worker ExportStl verb; resolves to the written path, or null on
   * cancel.
   */
  exportStl(): Promise<string | null>;
  /**
   * Export every body at head to an ASCII OBJ file. Rust owns the `.obj` save
   * dialog + the worker ExportObj verb; resolves to the written path, or null on
   * cancel.
   */
  exportObj(): Promise<string | null>;

  /**
   * Subscribe to worker-lifecycle `worker-status` events (starting / ready /
   * restarting / failed). The real event arrives from the C++ sidecar supervisor;
   * the mock never emits (returns a no-op unsubscribe).
   */
  onWorkerStatus(cb: (status: WorkerStatus) => void): Unsubscribe;

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

  // ── Sketch drag gesture (SCHEMA §7.4) ──────────────────────────────────────
  // A point drag: beginGesture → many solveDrag (latest-wins) → endGesture (ONE
  // undo step). The real client routes to the worker's gesture verbs; the mock
  // runs a local identity solve. No frontend caller yet (SketchController point
  // drag lands with M2/M4); wired + tested here so the seam is real.

  /** Open a drag gesture on a sketch point (`dragPointId` = a point entity id). */
  beginGesture(sketchId: string, dragPointId: string): Promise<BeginGestureResult>;

  /**
   * One incremental drag solve to `target` (fire-and-forget, latest-wins). Fire
   * without awaiting serially; each resolves with the fresh preview positions, or
   * `null` when the response was stale/superseded (dropped client-side by `seq`).
   */
  solveDrag(target: [number, number]): Promise<DragSolveResult | null>;

  /** Pointer-up: final exact solve committed as ONE undo command. */
  endGesture(finalTarget?: [number, number]): Promise<SketchUpsertResult>;

  // ── Element identity (SCHEMA §7.5) — pick → promote ────────────────────────

  /**
   * Promote snapshot-scoped TopoKey picks on a body to persistent, Rust-minted
   * `ElementId`s. The real client routes to `AcquireElementIds`; the mock mints
   * deterministic ids. Promoted ids flow back into the selection refs.
   */
  promoteSelection(bodyId: string, picks: PromotePick[]): Promise<PromotedElement[]>;

  // ── Topology repair (SCHEMA §9; M4b) ──────────────────────────────────────

  /**
   * Subscribe to `needs-repair` events (emitted after every published regen —
   * empty `items` means repairs cleared). The real event arrives from Rust; the
   * mock exposes a test seam (`emitMockNeedsRepair`). Returns an unsubscribe.
   */
  onNeedsRepair(cb: (event: NeedsRepairEvent) => void): Unsubscribe;

  /**
   * Dry-run the resolution ladder for repair refs (`resolve_refs`; binds
   * nothing). Returns the full un-lossy resolution per ref (candidates + reason +
   * anchor on `needsRepair`). The mock returns canned candidates.
   */
  resolveRefs(refs: ResolveRefRequest[]): Promise<ResolveRefResult[]>;

  /**
   * Apply one RAW `EditCommand` (repair rebind + history affordances — suppress /
   * rollback / delete). Returns the correlated regen result (same shape as
   * `applyOperation`). The real client routes to `apply_edit_command`; the mock
   * mutates its local document model.
   */
  applyEditCommand(command: WireEditCommand): Promise<ApplyOperationResult>;

  /**
   * Fetch a stored operation's params (the EditCommand `op.params` serde shape),
   * keyed by its record id. A parametric re-edit that changes ONE scalar (revolve
   * angle / shell thickness / fillet radius) fetches these on arm and deep-merges
   * the scalar on commit, so it preserves the non-scalar inputs the projection does
   * not expose (axis / openFaces / edges). The real client routes to
   * `get_operation_params`; the mock returns the op's stored params.
   */
  getOperationParams(recordId: string): Promise<Record<string, unknown>>;

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

  /**
   * Subscribe to authoritative `projection-updated` events (SCHEMA §7.2). The
   * frontend owns the projection stores and re-hydrates them from one payload on
   * open/new/close/edit/regen (F-WP8 flag 2). The real event arrives from Rust;
   * the mock is a no-op (it seeds + mutates its projection stores directly).
   */
  onProjectionUpdated(cb: (projection: DocumentProjectionWire) => void): Unsubscribe;

  /** Undo the last committed op → new revision + changed/removed bodies + timeline. */
  undo(): Promise<ApplyOperationResult>;
  /** Redo the last undone op. */
  redo(): Promise<ApplyOperationResult>;

  // ── Crash recovery (start screen) ─────────────────────────────────────────
  // A crashed session may leave an autosave behind; the start screen offers to
  // restore it. Rust owns the autosave sidecar + the crash detection; the mock
  // exposes a test-seeded seam (`setMockRecovery`).

  /**
   * Check whether a crashed session left an autosave to offer. Resolves the
   * recovery info, or null when there is nothing to recover.
   */
  checkRecovery(): Promise<RecoveryInfo | null>;

  /**
   * Resolve a pending recovery. `accept:true` restores the autosave and opens the
   * recovered document (returns a snapshot); `accept:false` discards it (returns null).
   */
  recoverDocument(accept: boolean): Promise<DocumentSnapshot | null>;
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
