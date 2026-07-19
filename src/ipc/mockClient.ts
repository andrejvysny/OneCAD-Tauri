/*
 * In-memory CadClient — drives the full start screen + editor UI with no backend.
 *
 * Seeded with a spread of names + dates so name-sort (A→Z), date-sort (newest
 * first) and substring search are all visibly exercised. Doc operations resolve
 * after a short simulated latency so the store's loading states are real.
 *
 * The sketch SOLVER lane + the drag-time PREVIEW lane live in the shared
 * `localSolver` module (F-WP8 seam) so the real `tauriClient` reuses them
 * verbatim. This file owns the mock's DOCUMENT model (synthetic bodies + feature
 * timeline + undo/redo); the tauri client replaces that half with real commands.
 */
import type { CadClient } from "./client";
import type {
  ApplyOperationResult,
  BodyMeshRef,
  DocumentChange,
  DocumentProjectionWire,
  DocumentSnapshot,
  FeatureRecord,
  Lod,
  NeedsRepairEvent,
  OperationOp,
  PromotedElement,
  PromotePick,
  RecentProject,
  ResolveCandidate,
  ResolveRefRequest,
  ResolveRefResult,
  Unsubscribe,
  WorkerStatus,
} from "./types";
import type { WireEditCommand } from "./tauriCommandMap";
import { makeBoxMesh, makeCylinderMesh, makeExtrudeBodyMesh, makeRevolveBodyMesh } from "./mockMeshes";
import type { LatheAxis } from "@/tools/preview/lathePreview";
import { createLocalSolverLane } from "./localSolver";

const LATENCY_MS = 120;
const MESH_LATENCY_MS = 30;

/**
 * Backend latency the mock simulates for document-changed + preview results.
 * Mutable so the 60fps gate can crank it to 300ms and prove the L1 preview holds
 * refresh rate while L2 lags. `wait()` for doc ops reads this live.
 */
let mockLatency = LATENCY_MS;
export function setMockLatency(ms: number): void {
  mockLatency = Math.max(0, ms);
}
export function getMockLatency(): number {
  return mockLatency;
}

const wait = (ms = mockLatency) => new Promise((r) => setTimeout(r, ms));

let nextDocId = 1;
const snapshot = (title: string): DocumentSnapshot => ({
  documentId: `doc-${nextDocId++}`,
  title,
});

// Varied names + dates (unsorted on purpose — the UI sorts).
const RECENTS: RecentProject[] = [
  {
    id: "p-bracket",
    name: "Bracket v2",
    path: "/Users/andrej/CAD/Projects/Bracket v2.onecad",
    modifiedAt: "2026-07-16T14:20:00Z",
  },
  {
    id: "p-enclosure",
    name: "Enclosure rev C",
    path: "/Users/andrej/CAD/Projects/Enclosure rev C.onecad",
    modifiedAt: "2026-07-14T09:05:00Z",
  },
  {
    id: "p-gearbox",
    name: "Gearbox mount",
    path: "/Users/andrej/Client/Gearbox/Gearbox mount.onecad",
    modifiedAt: "2026-07-09T18:42:00Z",
  },
  {
    id: "p-camera",
    name: "Camera rig plate",
    path: "/Users/andrej/CAD/Rigs/Camera rig plate.onecad",
    modifiedAt: "2026-06-30T11:15:00Z",
  },
  {
    id: "p-heatsink",
    name: "Heatsink shroud",
    path: "/Users/andrej/CAD/Projects/Heatsink shroud.onecad",
    modifiedAt: "2026-06-21T16:00:00Z",
  },
  {
    id: "p-adapter",
    name: "Adapter flange",
    path: "/Users/andrej/CAD/Projects/Adapter flange.onecad",
    modifiedAt: "2026-06-10T08:30:00Z",
  },
  {
    id: "p-untitled",
    name: "Untitled",
    path: "/Users/andrej/CAD/Projects/Untitled.onecad",
    modifiedAt: "2026-06-02T13:00:00Z",
  },
];

// ── Mesh + document-changed emitter (mock backend surface) ──────────────────

/** Which synthesized body geometry a given bodyId serves. */
function meshForBody(bodyId: string): ArrayBuffer {
  return bodyId === "body2" ? makeCylinderMesh() : makeBoxMesh();
}

/** MeshCache-style key mirroring Rust's `(BodyId, Lod, generation)`. */
export function mockMeshKey(bodyId: string, lod: Lod, generation = 1): string {
  return `${bodyId}:${lod}:${generation}`;
}

const docChangeListeners = new Set<(c: DocumentChange) => void>();

/**
 * Simulate a worker `document-changed` event (the demo / seed fires this so the
 * viewport ingests through the SAME onDocumentChanged path the real worker uses).
 */
export function emitMockDocumentChanged(change: DocumentChange): void {
  for (const cb of [...docChangeListeners]) cb(change);
}

// ── Topology repair (M4b) — needs-repair emitter + canned resolveRefs ──────────

const needsRepairListeners = new Set<(e: NeedsRepairEvent) => void>();

/** Test seam: push a `needs-repair` event through the mock (drives the banner). */
export function emitMockNeedsRepair(event: NeedsRepairEvent): void {
  for (const cb of [...needsRepairListeners]) cb(event);
}

// ── Mock document model: synthetic bodies + feature timeline + undo/redo ───────
//
// applyOperation / endPreview(commit) append feature entries and synthesize
// bodies; undo/redo restore whole-document snapshots (simple + always correct for
// a mock). Body meshes live here keyed by bodyId (getBodyMesh reads them, falling
// back to the seed box/cylinder). All shapes mirror SCHEMA §7.3 so the F-WP8 swap
// is a no-op for the tool layer.

/** Base timeline — MUST mirror documentStore.seedMockDocument().features. */
const MOCK_BASE_FEATURES: FeatureRecord[] = [
  { id: "f1", kind: "sketch", label: "Sketch 1", valueText: "", status: "ok" },
  { id: "f2", kind: "extrude", label: "Extrude", valueText: "83.3 mm", status: "ok" },
  { id: "f3", kind: "fillet", label: "Fillet", valueText: "2.0 mm", status: "ok" },
  { id: "f4", kind: "sketch", label: "Sketch 2", valueText: "", status: "ok" },
  { id: "f5", kind: "extrude", label: "Extrude", valueText: "12.0 mm", status: "ok" },
];

const cloneFeature = (f: FeatureRecord): FeatureRecord => ({ ...f });

/** Synthetic body meshes by bodyId (seed body1 is a fallback box, not stored). */
const syntheticBodies = new Map<string, ArrayBuffer>();
let mockFeatures: FeatureRecord[] = MOCK_BASE_FEATURES.map(cloneFeature);
let mockRevision = 5; // matches the seed projection revision
let nextBodySeq = 2; // body1 is the seed body
let nextFeatureSeq = 100;

/** featureId → bodyId, so a parametric edit rebuilds the SAME body. */
const featureBodies = new Map<string, string>();

interface DocSnap {
  label: string;
  features: FeatureRecord[];
  bodies: Map<string, ArrayBuffer>;
}
const undoStack: DocSnap[] = [];
const redoStack: DocSnap[] = [];

const bodyRef = (bodyId: string): BodyMeshRef => ({
  bodyId,
  meshKey: mockMeshKey(bodyId, "coarse", mockRevision),
});

function snap(label: string): DocSnap {
  return { label, features: mockFeatures.map(cloneFeature), bodies: new Map(syntheticBodies) };
}

/** Compute changed (new/replaced) + removed bodies between two body maps. */
function diffBodies(
  from: Map<string, ArrayBuffer>,
  to: Map<string, ArrayBuffer>,
): { changed: string[]; removed: string[] } {
  const changed: string[] = [];
  const removed: string[] = [];
  for (const [id, mesh] of to) if (from.get(id) !== mesh) changed.push(id);
  for (const id of from.keys()) if (!to.has(id)) removed.push(id);
  return { changed, removed };
}

/** Restore a snapshot; bumps the revision + returns the resulting body diff. */
function restoreSnap(s: DocSnap): { changed: string[]; removed: string[] } {
  const before = new Map(syntheticBodies);
  mockFeatures = s.features.map(cloneFeature);
  syntheticBodies.clear();
  for (const [k, v] of s.bodies) syntheticBodies.set(k, v);
  mockRevision += 1;
  return diffBodies(before, syntheticBodies);
}

function nextBodyId(): string {
  return `body${nextBodySeq++}`;
}
function nextFeatureId(): string {
  return `mf${nextFeatureSeq++}`;
}

/** A deterministic axis just left of a profile (so a re-edit with no axis still forms a body). */
function fallbackRevolveAxis(ring: [number, number][]): LatheAxis {
  let minU = Infinity;
  let minV = Infinity;
  let maxV = -Infinity;
  for (const [u, v] of ring) {
    if (u < minU) minU = u;
    if (v < minV) minV = v;
    if (v > maxV) maxV = v;
  }
  const x = minU - 1;
  return { a: [x, minV], b: [x, maxV] };
}

/** Apply one op forward (mutates features + bodies); returns the body diff. */
function mutateOp(op: OperationOp): { changed: string[]; removed: string[]; label: string } {
  if (op.opType === "Extrude") {
    const { plane, profile } = lane.resolveExtrudeInput(op.sketchId, op.regionId);
    const distance = op.params.distance ?? 10;
    const editing = op.featureId !== undefined && featureBodies.has(op.featureId);
    const featureId = op.featureId ?? nextFeatureId();
    const bodyId = editing ? featureBodies.get(featureId)! : nextBodyId();
    syntheticBodies.set(bodyId, makeExtrudeBodyMesh(profile, plane, distance));
    featureBodies.set(featureId, bodyId);
    const valueText = `${Math.abs(distance).toFixed(1)} mm`;
    if (editing) {
      mockFeatures = mockFeatures.map((f) => (f.id === featureId ? { ...f, valueText } : f));
    } else {
      mockFeatures = [...mockFeatures, { id: featureId, kind: "extrude", label: "Extrude", valueText, status: "ok" }];
    }
    return { changed: [bodyId], removed: [], label: "Extrude" };
  }
  if (op.opType === "Revolve") {
    const { plane, profile } = lane.resolveExtrudeInput(op.sketchId, op.regionId);
    const angle = op.params.angleDeg ?? 360;
    const axisLine =
      op.params.axis?.kind === "sketchLine"
        ? lane.resolveSketchLine(op.sketchId, op.params.axis.lineId)
        : null;
    // Fall back to a vertical axis just left of the profile so a body still forms
    // (re-edit carries no axis; the mock only needs a deterministic revolve).
    const axis = axisLine ?? fallbackRevolveAxis(profile.ring);
    const editing = op.featureId !== undefined && featureBodies.has(op.featureId);
    const featureId = op.featureId ?? nextFeatureId();
    const bodyId = editing ? featureBodies.get(featureId)! : nextBodyId();
    syntheticBodies.set(bodyId, makeRevolveBodyMesh(profile.ring, axis, plane, angle));
    featureBodies.set(featureId, bodyId);
    const valueText = `${Math.round(Math.abs(angle))}°`;
    if (editing) {
      mockFeatures = mockFeatures.map((f) => (f.id === featureId ? { ...f, valueText } : f));
    } else {
      mockFeatures = [...mockFeatures, { id: featureId, kind: "revolve", label: "Revolve", valueText, status: "ok" }];
    }
    return { changed: [bodyId], removed: [], label: "Revolve" };
  }
  if (op.opType === "Fillet") {
    // MOCK LIMIT: no real rounding — re-emit the target body + add a feature.
    const bodyId = op.inputs?.[0]?.primary.bodyId ?? "body1";
    const featureId = op.featureId ?? nextFeatureId();
    const valueText = `${op.params.radius.toFixed(1)} mm`;
    mockFeatures = [...mockFeatures, { id: featureId, kind: "fillet", label: "Fillet", valueText, status: "ok" }];
    return { changed: [bodyId], removed: [], label: "Fillet" };
  }
  // Boolean: MOCK removes the tool body, keeps the target (no real fusion).
  const { targetBodyId, toolBodyId, operation } = op.params;
  syntheticBodies.delete(toolBodyId);
  const featureId = op.featureId ?? nextFeatureId();
  mockFeatures = [
    ...mockFeatures,
    { id: featureId, kind: "boolean", label: operation, valueText: "", status: "ok" },
  ];
  return { changed: [targetBodyId], removed: [toolBodyId], label: operation };
}

/** Commit an op: push undo, mutate, bump revision, build the result. */
function commitOp(op: OperationOp): ApplyOperationResult {
  undoStack.push(snap(labelForOp(op)));
  redoStack.length = 0;
  const { changed, removed, label } = mutateOp(op);
  mockRevision += 1;
  return {
    revision: mockRevision,
    changedBodies: changed.map(bodyRef),
    removedBodies: removed,
    features: mockFeatures.map(cloneFeature),
    opLabel: label,
  };
}

function labelForOp(op: OperationOp): string {
  if (op.opType === "Boolean") return op.params.operation;
  return op.opType;
}

function noopResult(): ApplyOperationResult {
  return {
    revision: mockRevision,
    changedBodies: [],
    removedBodies: [],
    features: mockFeatures.map(cloneFeature),
  };
}

/** Commit one op through the local model + emit its document-changed (the lane's
 *  `commit` seam + the client's own `applyOperation` share this path). */
function commitAndEmit(op: OperationOp): Promise<ApplyOperationResult> {
  return wait().then(() => {
    const res = commitOp(op);
    emitMockDocumentChanged({
      revision: res.revision,
      changedBodies: res.changedBodies,
      removedBodies: res.removedBodies,
    });
    return res;
  });
}

/** Canned repair candidates for a ref (deterministic; descending score). */
function mockResolveRefs(refs: ResolveRefRequest[]): ResolveRefResult[] {
  return refs.map((r) => {
    const h = mockElementHash(r.refId);
    const candidates: ResolveCandidate[] = [
      {
        topoKey: `e:${(parseInt(h.slice(0, 2), 16) % 40) + 1}`,
        score: 0.91,
        margin: 0.02,
        worldPos: [12, 3.5, 0],
        summary: "linear edge, len≈40mm",
      },
      {
        topoKey: `e:${(parseInt(h.slice(2, 4), 16) % 40) + 1}`,
        score: 0.89,
        margin: 0.02,
        worldPos: [12, -3.5, 0],
        summary: "linear edge, len≈40mm",
      },
    ];
    return {
      refId: r.refId,
      outcome: "needsRepair",
      ladderFailed: "descriptor",
      reason: "ambiguous",
      scoringVersion: 1,
      uiLabel: "Fillet edge",
      candidates,
    };
  });
}

/** Apply one raw EditCommand against the mock document model (M4b). */
async function mockApplyEditCommand(command: WireEditCommand): Promise<ApplyOperationResult> {
  await wait();
  switch (command.cmd) {
    case "removeOperation": {
      undoStack.push(snap("Delete feature"));
      redoStack.length = 0;
      mockFeatures = mockFeatures.filter((f) => f.id !== command.record);
      mockRevision += 1;
      const res: ApplyOperationResult = {
        revision: mockRevision,
        changedBodies: [],
        removedBodies: [],
        features: mockFeatures.map(cloneFeature),
        opLabel: "Delete",
      };
      emitMockDocumentChanged({ revision: res.revision, changedBodies: [], removedBodies: [] });
      return res;
    }
    case "updateOperationParams":
    case "editOperationInput": {
      // Rebind / param edit: no structural change in the lean mock, but bump the
      // revision + emit document-changed so the regen correlation resolves.
      mockRevision += 1;
      const res = noopResult();
      emitMockDocumentChanged({ revision: res.revision, changedBodies: [], removedBodies: [] });
      return res;
    }
    case "setOperationSuppression":
    case "setRollback":
    default:
      // Suppression / rollback carry no distinct projection signal in the lean mock
      // (the real projection maps Suppressed→dirty; the frontend tracks an optimistic
      // overlay for dimming — see historyStore). Return a valid no-op result.
      mockRevision += 1;
      return { ...noopResult(), opLabel: "Edit" };
  }
}

// ── Shared sketch-solver + preview lane (F-WP8 seam; same module the tauri
//    client uses). Commit routes into the local document model above. ──────────
const lane = createLocalSolverLane({ commit: commitAndEmit, latencyMs: () => mockLatency });

/** Test seam: forget all sketch state so a fresh sketch starts empty. */
export function resetMockSketches(): void {
  lane.resetSketches();
}

/** Test seam: forget the whole mock document (bodies, features, undo, sessions). */
export function resetMockDocument(): void {
  syntheticBodies.clear();
  featureBodies.clear();
  lane.resetPreview();
  mockFeatures = MOCK_BASE_FEATURES.map(cloneFeature);
  mockRevision = 5;
  nextBodySeq = 2;
  nextFeatureSeq = 100;
  undoStack.length = 0;
  redoStack.length = 0;
}

export const mockClient: CadClient = {
  async listRecents() {
    await wait();
    return RECENTS.map((p) => ({ ...p }));
  },
  async newDocument() {
    await wait();
    return snapshot("Untitled");
  },
  async openDocument(path) {
    await wait();
    const known = RECENTS.find((p) => p.path === path);
    return snapshot(known?.name ?? basename(path));
  },
  async importStep(path) {
    await wait();
    return snapshot(basename(path));
  },
  async openFileDialog() {
    await wait(40);
    // Rust returns the real chosen path in F-WP8; here we fake a pick.
    return "/Users/andrej/CAD/Projects/Imported.onecad";
  },

  // Save/export are Rust-owned in the real app; the mock keeps them deterministic
  // (no filesystem): saveDocument is a no-op, Save As / Export return fake paths.
  async saveDocument(_path?: string) {
    await wait(40);
  },
  async saveDocumentAs() {
    await wait(40);
    return "/Users/andrej/CAD/Projects/Untitled.onecad";
  },
  async exportStep() {
    await wait(40);
    return "/Users/andrej/CAD/Projects/Untitled.step";
  },

  // The mock has no worker, so it never emits worker-status (no-op unsubscribe).
  onWorkerStatus(_cb: (status: WorkerStatus) => void): Unsubscribe {
    return () => {};
  },

  async getBodyMesh(bodyId, _lod) {
    await wait(MESH_LATENCY_MS);
    // Synthesized bodies (extrude output) win; else the seed box/cylinder.
    return syntheticBodies.get(bodyId) ?? meshForBody(bodyId);
  },

  onDocumentChanged(cb): () => void {
    docChangeListeners.add(cb);
    return () => docChangeListeners.delete(cb);
  },

  // The mock writes its projection stores directly (no backend event stream), so
  // the projection-updated subscription is a no-op that never fires.
  onProjectionUpdated(_cb: (p: DocumentProjectionWire) => void): Unsubscribe {
    return () => {};
  },

  // Deterministic mock promotion (Invariant 1: same pick → same id).
  async promoteSelection(bodyId: string, picks: PromotePick[]): Promise<PromotedElement[]> {
    await wait(MESH_LATENCY_MS);
    return picks.map((p) => ({
      topoKey: p.topoKey,
      elementId: `el_${mockElementHash(`${bodyId}#${p.topoKey}`)}`,
      kind: p.topoKey.startsWith("e:") ? "edge" : "face",
      bodyId,
    }));
  },

  // ── Topology repair (M4b) ──────────────────────────────────────────────────
  onNeedsRepair(cb: (e: NeedsRepairEvent) => void): Unsubscribe {
    needsRepairListeners.add(cb);
    return () => needsRepairListeners.delete(cb);
  },
  async resolveRefs(refs: ResolveRefRequest[]): Promise<ResolveRefResult[]> {
    await wait(MESH_LATENCY_MS);
    return mockResolveRefs(refs);
  },
  applyEditCommand(command: WireEditCommand): Promise<ApplyOperationResult> {
    return mockApplyEditCommand(command);
  },

  // ── Model operations (SCHEMA §7.3) — the mock's local document model ───────

  applyOperation(op: OperationOp): Promise<ApplyOperationResult> {
    return commitAndEmit(op);
  },

  async undo(): Promise<ApplyOperationResult> {
    await wait();
    if (undoStack.length === 0) return noopResult();
    const preOp = undoStack.pop()!;
    redoStack.push(snap(preOp.label));
    const { changed, removed } = restoreSnap(preOp);
    const res: ApplyOperationResult = {
      revision: mockRevision,
      changedBodies: changed.map(bodyRef),
      removedBodies: removed,
      features: mockFeatures.map(cloneFeature),
      opLabel: preOp.label,
    };
    emitMockDocumentChanged({ revision: res.revision, changedBodies: res.changedBodies, removedBodies: res.removedBodies });
    return res;
  },

  async redo(): Promise<ApplyOperationResult> {
    await wait();
    if (redoStack.length === 0) return noopResult();
    const postOp = redoStack.pop()!;
    undoStack.push(snap(postOp.label));
    const { changed, removed } = restoreSnap(postOp);
    const res: ApplyOperationResult = {
      revision: mockRevision,
      changedBodies: changed.map(bodyRef),
      removedBodies: removed,
      features: mockFeatures.map(cloneFeature),
      opLabel: postOp.label,
    };
    emitMockDocumentChanged({ revision: res.revision, changedBodies: res.changedBodies, removedBodies: res.removedBodies });
    return res;
  },

  // ── Sketch solver lane + two-level preview (shared local lane) ─────────────

  enterSketch: lane.enterSketch,
  sketchUpsert: lane.sketchUpsert,
  finishSketch: lane.finishSketch,
  cancelSketch: lane.cancelSketch,
  beginGesture: lane.beginGesture,
  solveDrag: lane.solveDrag,
  endGesture: lane.endGesture,
  beginPreview: lane.beginPreview,
  updatePreview: lane.updatePreview,
  endPreview: lane.endPreview,
  onPreviewResult: lane.onPreviewResult,
};

/** Small deterministic hash for mock ElementIds (FNV-1a-32 hex). */
function mockElementHash(s: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

function basename(path: string): string {
  const file = path.split(/[\\/]/).pop() ?? path;
  return file.replace(/\.[^.]+$/, "");
}
