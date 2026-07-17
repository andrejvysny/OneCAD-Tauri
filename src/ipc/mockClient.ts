/*
 * In-memory CadClient — drives the full start screen UI with no backend.
 *
 * Seeded with a spread of names + dates so name-sort (A→Z), date-sort (newest
 * first) and substring search are all visibly exercised. Doc operations resolve
 * after a short simulated latency so the store's loading states are real.
 */
import type { CadClient } from "./client";
import type {
  ApplyOperationResult,
  BodyMeshRef,
  DocumentChange,
  DocumentSnapshot,
  EnterSketchTarget,
  ExtrudeParams,
  FeatureRecord,
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
  SketchPlane,
  SketchRegion,
  SketchSession,
  SketchUpsertResult,
  Unsubscribe,
} from "./types";
import { makeBoxMesh, makeCylinderMesh, makeExtrudeBodyMesh } from "./mockMeshes";
import { detectRegions, planeFor, solveDof } from "./mockSketch";
import { profileFromRegion, type PrismProfile } from "@/tools/preview/prismPreview";

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

// ── In-memory sketch sessions (mock solver lane state) ───────────────────────

const sketchSessions = new Map<string, SketchSession>();
const sketchRevisions = new Map<string, number>();
let nextMockSketchSeq = 1;

/** Deep clone so callers can't mutate the mock's stored session. */
function cloneSession(s: SketchSession): SketchSession {
  return JSON.parse(JSON.stringify(s)) as SketchSession;
}

/** Test seam: forget all sketch state so a fresh sketch starts empty. */
export function resetMockSketches(): void {
  sketchSessions.clear();
  sketchRevisions.clear();
  nextMockSketchSeq = 1;
}

/**
 * Simulate a worker `document-changed` event (the demo / seed fires this so the
 * viewport ingests through the SAME onDocumentChanged path the real worker uses).
 */
export function emitMockDocumentChanged(change: DocumentChange): void {
  for (const cb of [...docChangeListeners]) cb(change);
}

// ── Mock document model: synthetic bodies + feature timeline + undo/redo ───────
//
// The mock is now a tiny parametric document: applyOperation / endPreview(commit)
// append feature entries and synthesize bodies; undo/redo restore whole-document
// snapshots (simple + always correct for a mock). Body meshes live here keyed by
// bodyId (getBodyMesh reads them, falling back to the seed box/cylinder). All
// shapes mirror SCHEMA §7.3 so the F-WP8 swap is a no-op for the tool layer.

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
let nextSessionSeq = 1;

/** featureId → bodyId, so a parametric edit rebuilds the SAME body. */
const featureBodies = new Map<string, string>();

/** Finished regions per sketch (cached by finishSketch) for extrude synthesis. */
const finishedRegions = new Map<string, SketchRegion[]>();

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

/** Resolve an extrude's region → {plane, profile}. Falls back to a 40×40 square. */
function resolveExtrudeInput(
  sketchId: string | undefined,
  regionId: string | undefined,
): { plane: SketchPlane; profile: PrismProfile } {
  const regions = sketchId ? finishedRegions.get(sketchId) : undefined;
  const region = regions?.find((r) => r.regionId === regionId) ?? regions?.[0];
  const plane = (sketchId && sketchSessions.get(sketchId)?.plane) || planeFor("XY");
  const profile = region ? profileFromRegion(region) : null;
  return { plane, profile: profile ?? fallbackSquareProfile() };
}

/** A default 40×40 square profile on the plane, so a demo extrude always shows. */
function fallbackSquareProfile(): PrismProfile {
  const s = 20;
  const ring: [number, number][] = [
    [-s, -s],
    [s, -s],
    [s, s],
    [-s, s],
  ];
  const positions = [0, 0, ...ring.flat()];
  const indices: number[] = [];
  for (let i = 0; i < ring.length; i++) indices.push(0, 1 + i, 1 + ((i + 1) % ring.length));
  return { ring, cap: { positions, indices } };
}

/** Apply one op forward (mutates features + bodies); returns the body diff. */
function mutateOp(op: OperationOp): { changed: string[]; removed: string[]; label: string } {
  if (op.opType === "Extrude") {
    const { plane, profile } = resolveExtrudeInput(op.sketchId, op.regionId);
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

// ── Preview sessions (two-level preview) ──────────────────────────────────────

interface PreviewSessionState {
  opType: PreviewDraft["opType"];
  previewBodyId: string;
  sketchId?: string;
  regionId?: string;
  plane: SketchPlane;
  profile: PrismProfile;
  latestParams: PreviewParams;
  lastEpoch: number;
}
const previewSessions = new Map<string, PreviewSessionState>();
const previewListeners = new Set<(r: PreviewResult) => void>();

function emitPreviewResult(r: PreviewResult): void {
  for (const cb of [...previewListeners]) cb(r);
}

/** Build the concrete Extrude op a committed preview session materializes. */
function buildOpFromSession(s: PreviewSessionState): OperationOp {
  const distance = Number(s.latestParams.distance ?? 10);
  const symmetric = s.latestParams.extrudeMode === "Symmetric";
  const featureId = typeof s.latestParams.featureId === "string" ? s.latestParams.featureId : undefined;
  const params: ExtrudeParams = {
    distance,
    extrudeMode: symmetric ? "Symmetric" : "Blind",
    booleanMode: "NewBody",
  };
  return {
    opType: "Extrude",
    sketchId: s.sketchId ?? "",
    regionId: s.regionId ?? "",
    featureId,
    inputs: [{ primary: { bodyId: "", kind: "face" }, anchor: {} }],
    params,
  };
}

/** Test seam: forget the whole mock document (bodies, features, undo, sessions). */
export function resetMockDocument(): void {
  syntheticBodies.clear();
  featureBodies.clear();
  finishedRegions.clear();
  previewSessions.clear();
  mockFeatures = MOCK_BASE_FEATURES.map(cloneFeature);
  mockRevision = 5;
  nextBodySeq = 2;
  nextFeatureSeq = 100;
  nextSessionSeq = 1;
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

  async getBodyMesh(bodyId, _lod) {
    await wait(MESH_LATENCY_MS);
    // Synthesized bodies (extrude output) win; else the seed box/cylinder.
    return syntheticBodies.get(bodyId) ?? meshForBody(bodyId);
  },

  onDocumentChanged(cb): Unsubscribe {
    docChangeListeners.add(cb);
    return () => docChangeListeners.delete(cb);
  },

  // ── Model operations + two-level preview (SCHEMA §7.3 / NEW_SPEC §15) ──────

  async applyOperation(op: OperationOp): Promise<ApplyOperationResult> {
    await wait();
    const res = commitOp(op);
    emitMockDocumentChanged({
      revision: res.revision,
      changedBodies: res.changedBodies,
      removedBodies: res.removedBodies,
    });
    return res;
  },

  async beginPreview(draft: PreviewDraft): Promise<PreviewSession> {
    const sessionId = `pv-${nextSessionSeq++}`;
    const previewBodyId = `preview:${sessionId}`;
    const { plane, profile } = resolveExtrudeInput(draft.sketchId, draft.regionId);
    previewSessions.set(sessionId, {
      opType: draft.opType,
      previewBodyId,
      sketchId: draft.sketchId,
      regionId: draft.regionId,
      plane,
      profile,
      latestParams: { ...draft.params },
      lastEpoch: 0,
    });
    return { sessionId, previewBodyId };
  },

  updatePreview(sessionId: string, params: PreviewParams, epoch: number): void {
    const s = previewSessions.get(sessionId);
    if (!s) return;
    s.latestParams = { ...s.latestParams, ...params };
    s.lastEpoch = epoch;
    // Only Extrude produces a drag-time L2 mesh; fillet L2 is debounced on commit.
    if (s.opType !== "Extrude") return;
    const distance = Number(s.latestParams.distance ?? 0);
    const bodyId = s.previewBodyId;
    setTimeout(() => {
      if (!previewSessions.has(sessionId)) return; // session ended → drop stale
      const mesh = makeExtrudeBodyMesh(s.profile, s.plane, distance);
      emitPreviewResult({ sessionId, epoch, bodyId, mesh });
    }, mockLatency);
  },

  async endPreview(sessionId: string, commit: boolean): Promise<ApplyOperationResult | null> {
    const s = previewSessions.get(sessionId);
    previewSessions.delete(sessionId);
    if (!s || !commit) {
      await wait(0);
      return null;
    }
    await wait();
    const op = buildOpFromSession(s);
    const res = commitOp(op);
    emitMockDocumentChanged({
      revision: res.revision,
      changedBodies: res.changedBodies,
      removedBodies: res.removedBodies,
    });
    // Deliver the committed exact mesh under the FINAL epoch so the tool can
    // reconcile (drop L1 only once the matching-epoch result exists).
    const committedBodyId = res.changedBodies[0]?.bodyId;
    if (committedBodyId) {
      const mesh = syntheticBodies.get(committedBodyId) ?? meshForBody(committedBodyId);
      emitPreviewResult({ sessionId, epoch: s.lastEpoch, bodyId: committedBodyId, mesh, committed: true });
    }
    return res;
  },

  onPreviewResult(cb: (r: PreviewResult) => void): Unsubscribe {
    previewListeners.add(cb);
    return () => previewListeners.delete(cb);
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

  // ── Sketch solver lane (mock) ────────────────────────────────────────────

  async enterSketch(target: EnterSketchTarget): Promise<SketchSession> {
    await wait(MESH_LATENCY_MS);
    let id: string;
    let planeKind: SketchSession["plane"]["kind"] = "XY";
    if (typeof target === "string") {
      id = target;
    } else {
      planeKind = target.newOnPlane;
      id = target.sketchId ?? `sk-${nextMockSketchSeq++}`;
    }
    let session = sketchSessions.get(id);
    if (!session) {
      session = {
        sketchId: id,
        plane: planeFor(planeKind),
        entities: [],
        constraints: [],
        dof: 0,
        status: "FullyConstrained",
      };
      sketchSessions.set(id, session);
      sketchRevisions.set(id, 0);
    }
    return cloneSession(session);
  },

  async sketchUpsert(
    sketchId: string,
    entities: SketchEntity[],
    constraints: SketchConstraint[],
  ): Promise<SketchUpsertResult> {
    // Near-synchronous: drawing must feel instant; the DOF badge refreshes live.
    await wait(0);
    const prev = sketchSessions.get(sketchId);
    const { dof, status } = solveDof(entities, constraints);
    const session: SketchSession = {
      sketchId,
      plane: prev?.plane ?? planeFor("XY"),
      entities,
      constraints,
      dof,
      status,
    };
    sketchSessions.set(sketchId, session);
    const rev = (sketchRevisions.get(sketchId) ?? 0) + 1;
    sketchRevisions.set(sketchId, rev);
    // The mock is an identity solve (echoes positions) — nothing moved.
    return { sketchId, sketchRevision: rev, dof, status, solvedPositions: {} };
  },

  async finishSketch(sketchId: string): Promise<FinishSketchResult> {
    await wait(MESH_LATENCY_MS);
    const session = sketchSessions.get(sketchId);
    const regions = session ? detectRegions(session.entities) : [];
    finishedRegions.set(sketchId, regions); // cache for extrude synthesis
    return { regions };
  },

  async cancelSketch(_sketchId: string): Promise<void> {
    await wait(0);
    // Real backend discards scratch; the mock keeps the last committed session.
  },
};

function basename(path: string): string {
  const file = path.split(/[\\/]/).pop() ?? path;
  return file.replace(/\.[^.]+$/, "");
}
