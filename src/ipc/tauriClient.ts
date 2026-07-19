/*
 * tauriClient — the REAL CadClient over the Tauri command/event surface
 * (R-WP10/11/12).
 *
 * Implements the same `CadClient` interface as `mockClient`, mapping each method
 * onto a `#[tauri::command]` (`invoke`) or a backend event (`listen`). Constructed
 * ONLY inside a Tauri webview (see `createClient` in `client.ts`); dev-in-browser,
 * vitest and Playwright keep the mock.
 *
 * ── What is REAL vs the SEAM (F-WP9) ──────────────────────────────────────────
 *  REAL commands : lifecycle, dialogs, recents, get_mesh (MESH1 ArrayBuffer),
 *                  undo/redo, apply_edit_command; the SKETCH SOLVER lane
 *                  (enter/upsert/finish/cancel) + the DRAG gesture lane
 *                  (begin/solve/end, latest-wins) + promote_selection.
 *  REAL events   : document-changed, projection-updated (→ store hydration),
 *                  regen-finished (prompt correlation), sketch-solved.
 *  LOCAL SEAM    : the drag-time L2 PREVIEW lane. The real backend has NO cheap
 *                  preview verb (only apply/regen commits geometry), so L2 stays
 *                  on the local previewer — seam-marked "(backend preview verb
 *                  TBD)". A previewed op's COMMIT already routes through the real
 *                  `apply_edit_command`; only the drag-time exact mesh is local.
 *
 * ── Sync-over-async adapter ───────────────────────────────────────────────────
 * `applyOperation`/`undo`/`redo` return a SYNCHRONOUS `ApplyOperationResult`, but
 * the backend is event-driven: an edit returns the PRE-regen projection, then
 * regen publishes geometry LATER. Each edit correlates the command's projection
 * with the next `document-changed` (bodies) OR `regen-finished` (`{revision,
 * outcome}` — resolves the no-geometry / noop case promptly, replacing the old 8 s
 * wait; F-WP8 flag 3).
 *
 * ── Sketch marshalling ────────────────────────────────────────────────────────
 * The frontend authors inlined-coordinate entities with string ids; `sketch_upsert`
 * wants `SketchEditOp[]` (Rust typed doc form, UUID ids, point-referenced). The
 * pure `sketchWireMap` module bridges them, synthesizing points + keeping an
 * id-map per sketch (see its header). See the M2-gate notes for the round-trip
 * items this WP marshals but cannot end-to-end validate without the real worker.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { CadClient } from "./client";
import type {
  ApplyOperationResult,
  BeginGestureResult,
  DocumentChange,
  DocumentProjectionWire,
  DocumentSnapshot,
  DragSolveResult,
  EnterSketchTarget,
  FeatureRecord,
  FinishSketchResult,
  Lod,
  NeedsRepairEvent,
  OperationOp,
  PromotedElement,
  PromotePick,
  RecentProject,
  RecoveryInfo,
  RegenFinished,
  ResolveRefRequest,
  ResolveRefResult,
  SketchConstraint,
  SketchEntity,
  SketchPlane,
  SketchPlaneKind,
  SketchRegion,
  SketchSession,
  SketchSolveStatus,
  SketchUpsertResult,
  WorkerStatus,
} from "./types";
import { createLocalSolverLane } from "./localSolver";
import { operationToEditCommand, opLabelFor, editCommandLabel, type WireEditCommand } from "./tauriCommandMap";
import {
  buildAddSketch,
  createIdMap,
  frontendConstraintsFromDto,
  frontendEntitiesFromDto,
  frontendSolvedPositions,
  marshalUpsert,
  mintUuid,
  type SketchIdMap,
} from "./sketchWireMap";
import { applyProjectionToStore } from "./projectionHydration";

// ── Command + event names (must match src-tauri/src/api + events.rs) ──────────
const CMD = {
  listRecents: "list_recents",
  newDocument: "new_document",
  openDocument: "open_document",
  importStep: "import_step",
  checkRecovery: "check_recovery",
  recoverDocument: "recover_document",
  saveDocument: "save_document",
  exportStepFile: "export_step_file",
  exportStlFile: "export_stl_file",
  exportObjFile: "export_obj_file",
  openFileDialog: "open_file_dialog",
  saveFileDialog: "save_file_dialog",
  getMesh: "get_mesh",
  applyEditCommand: "apply_edit_command",
  undo: "undo",
  redo: "redo",
  enterSketch: "enter_sketch",
  sketchUpsert: "sketch_upsert",
  finishSketch: "finish_sketch",
  cancelSketch: "cancel_sketch",
  beginGesture: "begin_gesture",
  solveDrag: "solve_drag",
  endGesture: "end_gesture",
  promoteSelection: "promote_selection",
  resolveRefs: "resolve_refs",
} as const;

const EVT = {
  documentChanged: "document-changed",
  projectionUpdated: "projection-updated",
  regenFinished: "regen-finished",
  sketchSolved: "sketch-solved",
  workerStatus: "worker-status",
  needsRepair: "needs-repair",
} as const;

/** L2 preview pacing for the local seam (snappy; there is no backend preview verb). */
const PREVIEW_LATENCY_MS = 16;

/**
 * Ultimate fallback for a regen's correlation if NEITHER `document-changed` nor
 * `regen-finished` arrives (should not happen — regen-finished fires on every
 * regen). Kept as a safety net; the prompt path is regen-finished.
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
/** `RecoveryInfoDto` is field-identical to the frontend `RecoveryInfo`. */
interface RecoveryInfoDto {
  originalPath?: string;
  autosavePath: string;
  modifiedMs: number;
}
interface DocumentProjectionDto extends DocumentProjectionWire {
  /** `FeatureDto` is field-identical to the frontend `FeatureRecord`. */
  features: FeatureRecord[];
}
interface SketchSessionDto {
  sketchId: string;
  plane: SketchPlane;
  entities: unknown;
  constraints: unknown;
  dof: number;
  status: SketchSolveStatus;
}
interface SketchUpsertDto {
  sketchId: string;
  sketchRevision: number;
  dof: number;
  status: SketchSolveStatus;
  solvedPositions: Record<string, [number, number]>;
}
interface BeginGestureDto {
  gestureId: number;
  ready: boolean;
}
interface DragSolveDto {
  gestureId: number;
  seq: number;
  status: string;
  dof: number;
  conflicting: string[];
  positions: Record<string, [number, number]>;
  solveMicros: number;
  superseded: boolean;
}
interface SketchRegionDto {
  regionId: string;
  outerLoop: string[];
  holes: string[][];
  previewTriangles?: { positions: number[]; indices: number[] };
}
interface FinishSketchDto {
  regions: SketchRegionDto[];
}
interface PromotedElementDto {
  topoKey: string;
  elementId: string;
  kind: string;
  bodyId: string;
}

/** Last `sketch-solved` payload seen (test/debug seam; the command return is the
 *  authoritative sync result — the event mirrors it for out-of-band subscribers). */
let lastSketchSolved: SketchUpsertDto | null = null;
/** Test seam: read the most recent `sketch-solved` event payload. */
export function __lastSketchSolvedForTests(): SketchUpsertDto | null {
  return lastSketchSolved;
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
  // Fan-out for app-level subscribers.
  const docChangeListeners = new Set<(c: DocumentChange) => void>();
  const projectionListeners = new Set<(p: DocumentProjectionWire) => void>();
  const workerStatusListeners = new Set<(s: WorkerStatus) => void>();
  const needsRepairListeners = new Set<(e: NeedsRepairEvent) => void>();
  // Latest authoritative revision (cached from any event).
  let latestRevision = 0;
  // Latest published snapshot id for promote_selection — carried by every
  // `document-changed` event (SCHEMA §7.5). Picks resolve against the snapshot the
  // fetched mesh was tessellated at (Invariant 4). Starts at 0 (nothing published).
  let currentSnapshotId = 0;

  // Correlation awaiters: resolved by the next document-changed / regen-finished
  // with a higher revision than the edit's base.
  interface Resolved {
    change: DocumentChange | null;
    revision: number;
  }
  interface Awaiter {
    baseRev: number;
    resolve(r: Resolved | null): void;
    timer: ReturnType<typeof setTimeout>;
  }
  const awaiters = new Set<Awaiter>();

  /** Resolve every awaiter whose edit base is below `resolved.revision`. */
  function resolveAwaiters(resolved: Resolved): void {
    for (const a of [...awaiters]) {
      if (resolved.revision > a.baseRev) {
        clearTimeout(a.timer);
        awaiters.delete(a);
        a.resolve(resolved);
      }
    }
  }

  function onDocumentChangedEvent(change: DocumentChange): void {
    latestRevision = Math.max(latestRevision, change.revision);
    // Adopt the published snapshot id so promoteSelection scopes picks correctly.
    if (change.snapshotId && change.snapshotId > 0) currentSnapshotId = change.snapshotId;
    for (const cb of [...docChangeListeners]) cb(change);
    resolveAwaiters({ change, revision: change.revision });
  }

  function onRegenFinishedEvent(rf: RegenFinished): void {
    latestRevision = Math.max(latestRevision, rf.revision);
    // Resolve any edit awaiting this regen that no document-changed already
    // resolved (the noop / no-geometry-change case → prompt, no 8 s wait).
    resolveAwaiters({ change: null, revision: rf.revision });
  }

  function onProjectionUpdatedEvent(p: DocumentProjectionWire): void {
    latestRevision = Math.max(latestRevision, p.revision);
    applyProjectionToStore(p); // hydrate documentStore (revision-reconciled)
    for (const cb of [...projectionListeners]) cb(p);
  }

  function onWorkerStatusEvent(s: WorkerStatus): void {
    for (const cb of [...workerStatusListeners]) cb(s);
  }

  function onNeedsRepairEvent(e: NeedsRepairEvent): void {
    latestRevision = Math.max(latestRevision, e.revision);
    for (const cb of [...needsRepairListeners]) cb(e);
  }

  /** Await the next regen publish (or null on the safety timeout). Register BEFORE invoking. */
  function awaitNextChange(baseRev: number): { promise: Promise<Resolved | null>; cancel(): void } {
    let awaiter!: Awaiter;
    const promise = new Promise<Resolved | null>((resolve) => {
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

  // Persistent event listeners started lazily (first correlation / subscribe) so
  // pure command tests don't need the event plugin mocked.
  let eventsStarted = false;
  const unlisteners: UnlistenFn[] = [];
  async function ensureEvents(): Promise<void> {
    if (eventsStarted) return;
    eventsStarted = true;
    try {
      unlisteners.push(
        await listen<DocumentChange>(EVT.documentChanged, (e) => onDocumentChangedEvent(e.payload)),
        await listen<DocumentProjectionDto>(EVT.projectionUpdated, (e) => onProjectionUpdatedEvent(e.payload)),
        await listen<RegenFinished>(EVT.regenFinished, (e) => onRegenFinishedEvent(e.payload)),
        await listen<SketchUpsertDto>(EVT.sketchSolved, (e) => {
          lastSketchSolved = e.payload;
        }),
        await listen<WorkerStatus>(EVT.workerStatus, (e) => onWorkerStatusEvent(e.payload)),
        await listen<NeedsRepairEvent>(EVT.needsRepair, (e) => onNeedsRepairEvent(e.payload)),
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
    const resolved = await awaiter.promise;
    return {
      revision: resolved?.revision ?? projection.revision,
      changedBodies: resolved?.change?.changedBodies ?? [],
      removedBodies: resolved?.change?.removedBodies ?? [],
      features: projection.features,
      opLabel,
    };
  }

  async function applyOperation(op: OperationOp): Promise<ApplyOperationResult> {
    const command = operationToEditCommand(op);
    return applyEdit(CMD.applyEditCommand, { command }, opLabelFor(op));
  }

  // ── Sketch solver lane state (frontend id ↔ backend UUID via sketchWireMap) ──
  const sketchMaps = new Map<string, SketchIdMap>();
  let sketchNameSeq = 1;

  /** The frontend-facing id for an enter target (kept as the session id). */
  function frontendIdFor(target: EnterSketchTarget): string {
    if (typeof target === "string") return target;
    return target.sketchId ?? `sk-${sketchNameSeq}`;
  }

  /** Resolve (or lazily create) the backend sketch for a frontend id + plane. */
  async function ensureBackendSketch(frontendId: string, planeKind: SketchPlaneKind): Promise<SketchIdMap> {
    const existing = sketchMaps.get(frontendId);
    if (existing) return existing;
    // New sketch: mint a real SketchId, register it via AddSketch, then enter.
    const backendSketchId = mintUuid();
    const map = createIdMap(backendSketchId, planeKind);
    sketchMaps.set(frontendId, map);
    // Create the backend sketch (a fresh world-plane sketch; SketchData defaults).
    // NOTE: this fires an edit + regen; the sketch appears in the tree via
    // projection-updated hydration. (custom/host-face planes → M2+; see M2 notes.)
    await call<DocumentProjectionDto>(CMD.applyEditCommand, {
      command: buildAddSketch(backendSketchId, `Sketch ${sketchNameSeq++}`, planeKind),
    });
    return map;
  }

  // The two-level PREVIEW lane stays LOCAL (no backend preview verb — see header).
  // Commit routes a previewed op through the REAL apply_edit_command path above.
  const lane = createLocalSolverLane({
    commit: applyOperation,
    latencyMs: () => PREVIEW_LATENCY_MS,
  });

  async function enterSketch(target: EnterSketchTarget): Promise<SketchSession> {
    await ensureEvents();
    const frontendId = frontendIdFor(target);
    const planeKind: SketchPlaneKind = typeof target === "string" ? "XY" : target.newOnPlane;
    const map = await ensureBackendSketch(frontendId, planeKind);
    const dto = await call<SketchSessionDto>(CMD.enterSketch, { sketchId: map.backendSketchId });
    const entities = frontendEntitiesFromDto(dto.entities);
    // Re-entry hydration: the backend returns the sketch's real constraints in the
    // worker-wire form (Rust `wire_constraint`, field-identical to SketchConstraint);
    // reverse-map them so the inspector shows live constraints (kills the []-seam).
    const constraints = frontendConstraintsFromDto(dto.constraints);
    // Feed the plane into the local preview lane so beginPreview resolves it.
    lane.cacheSketchPlane(frontendId, dto.plane);
    return {
      sketchId: frontendId, // keep the frontend id; the map holds the backend UUID
      plane: dto.plane,
      entities,
      constraints,
      dof: dto.dof,
      status: dto.status,
    };
  }

  async function sketchUpsert(
    sketchId: string,
    entities: SketchEntity[],
    constraints: SketchConstraint[],
  ): Promise<SketchUpsertResult> {
    const map = sketchMaps.get(sketchId);
    if (!map) throw new Error(`sketchUpsert: unknown sketch ${sketchId} (enter first)`);
    const ops = marshalUpsert(map, { entities, constraints });
    const dto = await call<SketchUpsertDto>(CMD.sketchUpsert, { sketchId: map.backendSketchId, ops });
    return {
      sketchId, // frontend id
      sketchRevision: dto.sketchRevision,
      dof: dto.dof,
      status: dto.status,
      // F-WP9 fix: the worker keys solvedPositions by backend POINT-entity UUID;
      // reverse-map them to the frontend `entityId.point` keys the SketchController
      // applies (via sketchWireMap's id-map). Unknown keys are dropped.
      solvedPositions: frontendSolvedPositions(map, dto.solvedPositions),
    };
  }

  async function finishSketch(sketchId: string): Promise<FinishSketchResult> {
    const map = sketchMaps.get(sketchId);
    if (!map) throw new Error(`finishSketch: unknown sketch ${sketchId} (enter first)`);
    const dto = await call<FinishSketchDto>(CMD.finishSketch, { sketchId: map.backendSketchId });
    const regions: SketchRegion[] = dto.regions.map((r) => ({
      regionId: r.regionId,
      outerLoop: r.outerLoop,
      holes: r.holes,
      previewTriangles: r.previewTriangles,
    }));
    lane.cacheFinishedRegions(sketchId, regions); // feed the local L2 preview
    return { regions };
  }

  async function cancelSketch(sketchId: string): Promise<void> {
    const map = sketchMaps.get(sketchId);
    if (!map) return; // never entered — nothing to cancel
    await call<void>(CMD.cancelSketch, { sketchId: map.backendSketchId });
  }

  // ── Sketch drag gesture (latest-wins) ─────────────────────────────────────
  let dragSketchId: string | null = null;
  let dragMaxSeq = 0;

  async function beginGesture(sketchId: string, dragPointId: string): Promise<BeginGestureResult> {
    await ensureEvents();
    const map = sketchMaps.get(sketchId);
    if (!map) throw new Error(`beginGesture: unknown sketch ${sketchId} (enter first)`);
    // Translate the frontend point ref → backend point UUID (a synthesized point,
    // a Point entity, or an already-real uuid pass-through).
    const dragPoint = map.point.get(dragPointId) ?? map.entity.get(dragPointId) ?? dragPointId;
    dragSketchId = sketchId;
    dragMaxSeq = 0;
    const dto = await call<BeginGestureDto>(CMD.beginGesture, {
      sketchId: map.backendSketchId,
      dragPoint,
    });
    return { gestureId: dto.gestureId, ready: dto.ready };
  }

  async function solveDrag(target: [number, number]): Promise<DragSolveResult | null> {
    // Fire-and-forget: the caller does NOT await serially. Responses reconcile
    // latest-wins by seq — a stale/superseded seq is dropped (returns null).
    const dto = await call<DragSolveDto>(CMD.solveDrag, { target });
    if (dto.superseded || dto.seq <= dragMaxSeq) return null; // stale — drop
    dragMaxSeq = dto.seq;
    return {
      gestureId: dto.gestureId,
      seq: dto.seq,
      status: dto.status,
      dof: dto.dof,
      conflicting: dto.conflicting,
      positions: dto.positions,
      solveMicros: dto.solveMicros,
      superseded: dto.superseded,
    };
  }

  async function endGesture(finalTarget?: [number, number]): Promise<SketchUpsertResult> {
    const sketchId = dragSketchId;
    dragSketchId = null;
    dragMaxSeq = 0;
    const dto = await call<SketchUpsertDto>(CMD.endGesture, { finalTarget: finalTarget ?? null });
    const map = sketchId ? sketchMaps.get(sketchId) : undefined;
    return {
      sketchId: sketchId ?? dto.sketchId,
      sketchRevision: dto.sketchRevision,
      dof: dto.dof,
      status: dto.status,
      // Reverse-map backend point UUIDs → frontend `entityId.point` keys (F-WP9).
      solvedPositions: map ? frontendSolvedPositions(map, dto.solvedPositions) : dto.solvedPositions,
    };
  }

  // ── Topology repair (dry-run resolve + raw edit commands; M4b) ─────────────
  async function resolveRefs(refs: ResolveRefRequest[]): Promise<ResolveRefResult[]> {
    // `resolve_refs` wants `{snapshotId, refs: [{refId, ...ElementRef}]}`; each ref
    // flattens the (optional) primary/anchor. The lean `needs-repair` event carries
    // no ElementRef, so callers usually pass `refId` only and the backend resolves
    // the stored ref by id. (SEAM: if the backend requires a full ElementRef the
    // needs-repair event must surface it — reported.)
    const out = await call<ResolveRefResult[]>(CMD.resolveRefs, {
      snapshotId: currentSnapshotId,
      refs: refs.map((r) => ({ refId: r.refId, primary: r.primary, anchor: r.anchor })),
    });
    return out.map((r) => ({ ...r, candidates: r.candidates ?? [] }));
  }

  async function applyEditCommand(command: WireEditCommand): Promise<ApplyOperationResult> {
    return applyEdit(CMD.applyEditCommand, { command }, editCommandLabel(command));
  }

  // ── Promotion (pick → ElementId) ──────────────────────────────────────────
  async function promoteSelection(bodyId: string, picks: PromotePick[]): Promise<PromotedElement[]> {
    // promote_selection wants the `body_<uuid>` wire form; document-changed hands
    // the frontend a bare uuid, so prefix it here (get_mesh keeps the bare form).
    const wireBodyId = bodyId.startsWith("body_") ? bodyId : `body_${bodyId}`;
    const out = await call<PromotedElementDto[]>(CMD.promoteSelection, {
      snapshotId: currentSnapshotId,
      bodyId: wireBodyId,
      picks: picks.map((p) => ({ topoKey: p.topoKey, anchor: p.anchor })),
    });
    return out.map((e) => ({ topoKey: e.topoKey, elementId: e.elementId, kind: e.kind, bodyId: e.bodyId }));
  }

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
    async checkRecovery(): Promise<RecoveryInfo | null> {
      return call<RecoveryInfoDto | null>(CMD.checkRecovery);
    },
    async recoverDocument(accept: boolean): Promise<DocumentSnapshot | null> {
      return call<DocumentSnapshotDto | null>(CMD.recoverDocument, { accept });
    },
    async openFileDialog(): Promise<string | null> {
      return call<string | null>(CMD.openFileDialog);
    },

    async saveDocument(path?: string): Promise<void> {
      // `path` null ⇒ the backend reuses the last save path (an unsaved document
      // with no path rejects with an io error; the caller falls back to Save As).
      await call<void>(CMD.saveDocument, { path: path ?? null });
    },
    async saveDocumentAs(): Promise<string | null> {
      const path = await call<string | null>(CMD.saveFileDialog);
      if (!path) return null; // cancelled
      await call<void>(CMD.saveDocument, { path });
      return path;
    },
    async exportStep(): Promise<string | null> {
      // Rust owns the `.step` save dialog + the worker ExportStep verb; a cancel
      // resolves to null (no path written).
      return call<string | null>(CMD.exportStepFile, { path: null });
    },
    async exportStl(): Promise<string | null> {
      // Rust owns the `.stl` save dialog + the worker ExportStl verb; a cancel
      // resolves to null (no path written).
      return call<string | null>(CMD.exportStlFile, { path: null });
    },
    async exportObj(): Promise<string | null> {
      // Rust owns the `.obj` save dialog + the worker ExportObj verb; a cancel
      // resolves to null (no path written).
      return call<string | null>(CMD.exportObjFile, { path: null });
    },

    onWorkerStatus(cb: (s: WorkerStatus) => void): () => void {
      void ensureEvents();
      workerStatusListeners.add(cb);
      return () => workerStatusListeners.delete(cb);
    },

    onNeedsRepair(cb: (e: NeedsRepairEvent) => void): () => void {
      void ensureEvents();
      needsRepairListeners.add(cb);
      return () => needsRepairListeners.delete(cb);
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

    onProjectionUpdated(cb: (p: DocumentProjectionWire) => void): () => void {
      void ensureEvents();
      projectionListeners.add(cb);
      return () => projectionListeners.delete(cb);
    },

    applyOperation,

    async undo(): Promise<ApplyOperationResult> {
      return applyEdit(CMD.undo, {}, undefined);
    },
    async redo(): Promise<ApplyOperationResult> {
      return applyEdit(CMD.redo, {}, undefined);
    },

    // ── Sketch solver lane + drag gesture + promotion (REAL commands) ─────────
    enterSketch,
    sketchUpsert,
    finishSketch,
    cancelSketch,
    beginGesture,
    solveDrag,
    endGesture,
    promoteSelection,
    resolveRefs,
    applyEditCommand,

    // ── Two-level preview (local seam; backend preview verb TBD) ──────────────
    beginPreview: lane.beginPreview,
    updatePreview: lane.updatePreview,
    endPreview: lane.endPreview,
    onPreviewResult: lane.onPreviewResult,
  };
}
