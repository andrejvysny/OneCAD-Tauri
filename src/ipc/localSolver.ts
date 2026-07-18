/*
 * localSolver — the shared, in-memory sketch-solver + two-level-preview lane.
 *
 * ── Why this module exists (the F-WP8 seam) ──────────────────────────────────
 * The real backend (R-WP10/11) does NOT yet speak the sketch SOLVER lane or the
 * drag-time exact PREVIEW lane — those land with the worker's PlaneGCS actor +
 * gesture verbs in R-WP12 (SCHEMA §7.4 BeginGesture/SolveDrag). So BOTH the mock
 * client AND the real `tauriClient` route sketch-solve / snap-echo / drag-preview
 * interactions through this ONE local module. Extract-don't-duplicate: the mock's
 * lane logic lives here so the two clients evolve together and stay identical.
 *
 *   R-WP12 replaces this lane with real solver-lane verbs (enter/upsert/finish
 *   become BeginGesture + SolveDrag round-trips; updatePreview streams to the
 *   worker's scratch preview job). Until then this deterministic stand-in keeps
 *   the whole sketch + extrude-drag UX working with no backend.
 *
 * The ONE thing that differs between mock and tauri is COMMIT: materializing a
 * previewed op. The mock commits into its local document model; the tauri client
 * commits through the real `apply_edit_command`. That difference is injected as
 * the `commit` dependency, so this lane stays commit-agnostic.
 */
import type {
  ApplyOperationResult,
  BeginGestureResult,
  DragSolveResult,
  ExtrudeParams,
  EnterSketchTarget,
  OperationOp,
  PreviewDraft,
  PreviewParams,
  PreviewResult,
  PreviewSession,
  SketchConstraint,
  SketchEntity,
  SketchPlane,
  SketchRegion,
  SketchSession,
  SketchUpsertResult,
  Unsubscribe,
} from "./types";
import { makeExtrudeBodyMesh } from "./mockMeshes";
import { detectRegions, planeFor, solveDof } from "./mockSketch";
import { profileFromRegion, type PrismProfile } from "@/tools/preview/prismPreview";

/** Simulated latency for sketch enter/finish round-trips (independent of the doc latency). */
const SKETCH_LATENCY_MS = 30;

const wait = (ms: number): Promise<void> => new Promise((r) => setTimeout(r, ms));

/** Deep clone so callers can't mutate the lane's stored session. */
function cloneSession(s: SketchSession): SketchSession {
  return JSON.parse(JSON.stringify(s)) as SketchSession;
}

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

export interface LocalSolverDeps {
  /**
   * Materialize a previewed op. The mock wires this to its local document model;
   * the tauri client wires it to the real `apply_edit_command` path. Must emit
   * the client's own `document-changed` (both do) so meshes flow the usual way.
   */
  commit(op: OperationOp): Promise<ApplyOperationResult>;
  /** Live document latency (ms) used to pace the drag-time L2 preview mesh. */
  latencyMs(): number;
}

/**
 * The public lane surface — exactly the CadClient sketch + preview methods, plus
 * `resolveExtrudeInput` (so a caller's own op-commit path can reuse the finished
 * region → {plane, profile}) and split reset seams.
 */
export interface LocalSolverLane {
  enterSketch(target: EnterSketchTarget): Promise<SketchSession>;
  sketchUpsert(
    sketchId: string,
    entities: SketchEntity[],
    constraints: SketchConstraint[],
  ): Promise<SketchUpsertResult>;
  finishSketch(sketchId: string): Promise<{ regions: SketchRegion[] }>;
  cancelSketch(sketchId: string): Promise<void>;
  beginGesture(sketchId: string, dragPointId: string): Promise<BeginGestureResult>;
  solveDrag(target: [number, number]): Promise<DragSolveResult | null>;
  endGesture(finalTarget?: [number, number]): Promise<SketchUpsertResult>;
  beginPreview(draft: PreviewDraft): Promise<PreviewSession>;
  updatePreview(sessionId: string, params: PreviewParams, epoch: number): void;
  endPreview(sessionId: string, commit: boolean): Promise<ApplyOperationResult | null>;
  onPreviewResult(cb: (r: PreviewResult) => void): Unsubscribe;
  /** Resolve a finished sketch region → the {plane, profile} an extrude consumes. */
  resolveExtrudeInput(
    sketchId: string | undefined,
    regionId: string | undefined,
  ): { plane: SketchPlane; profile: PrismProfile };
  /** Seam: feed a real enter_sketch plane so beginPreview resolves it (tauri). */
  cacheSketchPlane(sketchId: string, plane: SketchPlane): void;
  /** Seam: feed real finish_sketch regions so beginPreview builds the profile (tauri). */
  cacheFinishedRegions(sketchId: string, regions: SketchRegion[]): void;
  /** Forget sketch sessions (mirrors the mock's `resetMockSketches`). */
  resetSketches(): void;
  /** Forget preview sessions + finished regions (mirrors `resetMockDocument`). */
  resetPreview(): void;
}

export function createLocalSolverLane(deps: LocalSolverDeps): LocalSolverLane {
  // Sketch-lane state.
  const sketchSessions = new Map<string, SketchSession>();
  const sketchRevisions = new Map<string, number>();
  let nextSketchSeq = 1;
  const finishedRegions = new Map<string, SketchRegion[]>();

  // Drag-gesture state (mock identity drag — echoes the target position).
  let gestureSeq = 0;
  let activeGesture: { sketchId: string; dragPointId: string; nextSeq: number } | null = null;

  // Preview-lane state.
  const previewSessions = new Map<string, PreviewSessionState>();
  const previewListeners = new Set<(r: PreviewResult) => void>();
  let nextSessionSeq = 1;

  function emitPreviewResult(r: PreviewResult): void {
    for (const cb of [...previewListeners]) cb(r);
  }

  /** A default 40×40 square profile so a demo extrude always shows something. */
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

  /** Build the concrete Extrude op a committed preview session materializes. */
  function buildOpFromSession(s: PreviewSessionState): OperationOp {
    const distance = Number(s.latestParams.distance ?? 10);
    const symmetric = s.latestParams.extrudeMode === "Symmetric";
    const featureId =
      typeof s.latestParams.featureId === "string" ? s.latestParams.featureId : undefined;
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

  return {
    async enterSketch(target: EnterSketchTarget): Promise<SketchSession> {
      await wait(SKETCH_LATENCY_MS);
      let id: string;
      let planeKind: SketchSession["plane"]["kind"] = "XY";
      if (typeof target === "string") {
        id = target;
      } else {
        planeKind = target.newOnPlane;
        id = target.sketchId ?? `sk-${nextSketchSeq++}`;
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
      // The local solver is an identity solve (echoes positions) — nothing moved.
      return { sketchId, sketchRevision: rev, dof, status, solvedPositions: {} };
    },

    async finishSketch(sketchId: string): Promise<{ regions: SketchRegion[] }> {
      await wait(SKETCH_LATENCY_MS);
      const session = sketchSessions.get(sketchId);
      const regions = session ? detectRegions(session.entities) : [];
      finishedRegions.set(sketchId, regions); // cache for extrude synthesis
      return { regions };
    },

    async cancelSketch(_sketchId: string): Promise<void> {
      await wait(0);
      activeGesture = null;
      // Real backend discards scratch; the lane keeps the last committed session.
    },

    // ── Drag gesture (mock identity drag: solveDrag echoes the target) ─────────
    async beginGesture(sketchId: string, dragPointId: string): Promise<BeginGestureResult> {
      await wait(0);
      activeGesture = { sketchId, dragPointId, nextSeq: 1 };
      return { gestureId: ++gestureSeq, ready: true };
    },

    async solveDrag(target: [number, number]): Promise<DragSolveResult | null> {
      await wait(0);
      const g = activeGesture;
      if (!g) return null;
      const seq = g.nextSeq++;
      const session = sketchSessions.get(g.sketchId);
      return {
        gestureId: gestureSeq,
        seq,
        status: "success",
        dof: session?.dof ?? 0,
        conflicting: [],
        positions: { [g.dragPointId]: target },
        solveMicros: 0,
        superseded: false,
      };
    },

    async endGesture(finalTarget?: [number, number]): Promise<SketchUpsertResult> {
      await wait(SKETCH_LATENCY_MS);
      const g = activeGesture;
      activeGesture = null;
      const sketchId = g?.sketchId ?? "";
      const session = sketchId ? sketchSessions.get(sketchId) : undefined;
      const rev = (sketchRevisions.get(sketchId) ?? 0) + 1;
      if (sketchId) sketchRevisions.set(sketchId, rev);
      return {
        sketchId,
        sketchRevision: rev,
        dof: session?.dof ?? 0,
        status: session?.status ?? "FullyConstrained",
        solvedPositions: g && finalTarget ? { [g.dragPointId]: finalTarget } : {},
      };
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
      }, deps.latencyMs());
    },

    async endPreview(sessionId: string, commit: boolean): Promise<ApplyOperationResult | null> {
      const s = previewSessions.get(sessionId);
      previewSessions.delete(sessionId);
      if (!s || !commit) {
        await wait(0);
        return null;
      }
      const op = buildOpFromSession(s);
      const res = await deps.commit(op);
      // Deliver a committed signal under the FINAL epoch so the tool reconciles
      // (drops L1 once the matching-epoch result exists). The controller reads
      // only `epoch` for a committed result — the real body mesh flows through the
      // normal document-changed path — so the mesh field is intentionally empty.
      const committedBodyId = res.changedBodies[0]?.bodyId;
      if (committedBodyId) {
        emitPreviewResult({
          sessionId,
          epoch: s.lastEpoch,
          bodyId: committedBodyId,
          mesh: new ArrayBuffer(0),
          committed: true,
        });
      }
      return res;
    },

    onPreviewResult(cb: (r: PreviewResult) => void): Unsubscribe {
      previewListeners.add(cb);
      return () => previewListeners.delete(cb);
    },

    resolveExtrudeInput,

    cacheSketchPlane(sketchId: string, plane: SketchPlane): void {
      const existing = sketchSessions.get(sketchId);
      sketchSessions.set(
        sketchId,
        existing
          ? { ...existing, plane }
          : { sketchId, plane, entities: [], constraints: [], dof: 0, status: "FullyConstrained" },
      );
    },

    cacheFinishedRegions(sketchId: string, regions: SketchRegion[]): void {
      finishedRegions.set(sketchId, regions);
    },

    resetSketches(): void {
      sketchSessions.clear();
      sketchRevisions.clear();
      nextSketchSeq = 1;
    },

    resetPreview(): void {
      previewSessions.clear();
      finishedRegions.clear();
      nextSessionSeq = 1;
    },
  };
}
