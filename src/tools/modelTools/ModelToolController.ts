/*
 * ModelToolController — imperative glue for the three model tools (F-WP7),
 * mirroring the sketch-mode SketchController. Lives inside ViewportRoot (created
 * after the engine initializes, so it never runs in jsdom). It:
 *   - arms/cancels the extrude / fillet / boolean tools on tool + selection
 *     changes (and the finish-sketch → auto-arm extrude flow),
 *   - drives the two-level extrude preview: L1 unit prism + drag handle track the
 *     pointer at refresh rate (depth = pointer-ray ∩ plane normal), while the
 *     previewThrottle paces exact L2 meshes that swap in underneath,
 *   - reconciles epochs on commit: L1 is dropped only once the exact body for the
 *     final epoch is present in the scene (via MeshIngest.onBodyLoaded),
 *   - runs the fillet radius drag + chip and the boolean tool-body pick + ghost.
 *
 * The pure transition logic lives in modelToolMachine; the pure math in
 * tools/preview/*. This file is the imperative wiring only.
 */
import type { CadClient } from "@/ipc/client";
import type {
  BooleanOperation,
  FeatureRecord,
  OperationOp,
  PreviewDraft,
  PreviewParams,
  PreviewResult,
  PreviewSession,
  SemanticRef,
  SketchPlane,
} from "@/ipc/types";
import type { ViewportEngine } from "@/viewport/engine/ViewportEngine";
import { planePointToWorld } from "@/viewport/engine/sketchBasis";
import { parseMeshPayload } from "@/viewport/mesh/parseMeshPayload";
import { buildBodyObjects, getEntry, remove as removeMesh, swap as swapMesh } from "@/viewport/mesh/meshRegistry";
import { toolStore } from "@/stores/toolStore";
import { viewportStore } from "@/stores/viewportStore";
import { documentStore, type FeatureMeta } from "@/stores/documentStore";
import { selectionStore, type EntityRef } from "@/stores/selectionStore";
import { toolChipStore } from "@/stores/toolChipStore";
import { profileFromRegion, profileBounds } from "@/tools/preview/prismPreview";
import { axisDepthFromRay, normalize, type Vec3 } from "@/tools/preview/depthProjection";
import { radiusFromDrag } from "@/tools/preview/filletRadius";
import { PreviewThrottle } from "@/tools/preview/previewThrottle";
import {
  booleanInit,
  booleanStep,
  extrudeInit,
  extrudeStep,
  filletInit,
  filletStep,
  DEFAULT_EXTRUDE_DEPTH,
  DEFAULT_FILLET_RADIUS,
  type BooleanFsm,
  type ExtrudeFsm,
  type FilletFsm,
} from "./modelToolMachine";

const DRAG_PX = 4;

export interface ModelToolDeps {
  engine: ViewportEngine;
  client: CadClient;
  container: HTMLElement;
  /** MeshIngest.onBodyLoaded — fires when a committed body enters the scene. */
  onBodyLoaded: (cb: (bodyId: string) => void) => () => void;
  debug?: boolean;
}

type DragKind = "extrude" | "fillet" | null;

export class ModelToolController {
  private extrude: ExtrudeFsm = extrudeInit();
  private fillet: FilletFsm = filletInit();
  private boolean: BooleanFsm = booleanInit();
  private readonly throttle = new PreviewThrottle<{ distance: number }>({ trailingMs: 80 });

  // Extrude preview context.
  private session: PreviewSession | null = null;
  private plane: SketchPlane | null = null;
  private centroidWorld: Vec3 = [0, 0, 0];
  private normal: Vec3 = [0, 0, 1];
  private lastArmedSketch: string | null = null;
  private previewMeshRev = 0;

  // Fillet context.
  private filletEdges: EntityRef[] = [];
  private filletDownY = 0;
  private filletStartRadius = DEFAULT_FILLET_RADIUS;

  // Commit reconciliation.
  private commitBodyId: string | null = null;
  private commitFinalEpoch = 0;
  private committedEpoch = 0;
  private lastL2Epoch = 0;
  private commitBodyUnsub: (() => void) | null = null;

  // Pointer bookkeeping.
  private downX = 0;
  private downY = 0;
  private downButton = -1;
  private moved = false;
  private dragging: DragKind = null;
  private altHeld = false;

  private readonly unsubs: Array<() => void> = [];

  constructor(private readonly deps: ModelToolDeps) {
    const c = deps.container;
    c.addEventListener("pointerdown", this.onPointerDown);
    c.addEventListener("pointermove", this.onPointerMove);
    c.addEventListener("pointerup", this.onPointerUp);
    window.addEventListener("keydown", this.onKeyDown, true);
    window.addEventListener("keyup", this.onKeyUp, true);

    this.unsubs.push(deps.client.onPreviewResult((r) => this.onPreviewResult(r)));

    // React to model-mode tool changes + the finish-sketch handoff.
    let lastMode = toolStore.getState().mode;
    let lastTool = toolStore.getState().modelTool;
    this.unsubs.push(
      toolStore.subscribe((s) => {
        if (s.mode !== lastMode) {
          lastMode = s.mode;
          if (s.mode !== "model") this.cancelAll();
        }
        if (s.mode === "model" && s.modelTool !== lastTool) {
          lastTool = s.modelTool;
          this.onToolChange(s.modelTool);
        }
      }),
    );

    let lastPending = viewportStore.getState().pendingExtrudeSketch;
    this.unsubs.push(
      viewportStore.subscribe((s) => {
        if (s.pendingExtrudeSketch !== lastPending) {
          lastPending = s.pendingExtrudeSketch;
          if (s.pendingExtrudeSketch && toolStore.getState().mode === "model") {
            const id = s.pendingExtrudeSketch;
            viewportStore.getState().setPendingExtrude(null);
            toolStore.getState().setTool("extrude");
            void this.armExtrude(id);
          }
        }
      }),
    );
  }

  // ── tool arming ────────────────────────────────────────────────────────────

  private onToolChange(tool: string): void {
    // Any tool switch cleans up the previous tool's transient state first.
    this.cancelPreview();
    this.cancelFillet();
    this.cancelBoolean();
    toolChipStore.getState().clear();
    if (tool === "extrude") this.armExtrudeFromSelection();
    else if (tool === "fillet") this.armFilletFromSelection();
    else if (tool === "boolean") this.startBooleanFromSelection();
    else if (tool === "revolve") viewportStore.getState().setStatusHint("Revolve — not yet implemented");
    else viewportStore.getState().setStatusHint(null);
  }

  private armExtrudeFromSelection(): void {
    const sketch = selectionStore.getState().selected.find((r) => r.kind === "sketch");
    if (sketch) void this.armExtrude(sketch.id);
    else viewportStore.getState().setStatusHint("Select a sketch to extrude");
  }

  private async armExtrude(sketchId: string, editFeatureId?: string, startDepth = DEFAULT_EXTRUDE_DEPTH): Promise<void> {
    const finish = await this.deps.client.finishSketch(sketchId);
    const region = finish.regions[0];
    const profile = region ? profileFromRegion(region) : null;
    if (!region || !profile) {
      viewportStore.getState().setStatusHint("No closed region to extrude");
      toolStore.getState().setTool("select");
      return;
    }
    const session = await this.deps.client.enterSketch(sketchId); // plane only
    this.plane = session.plane;
    this.lastArmedSketch = sketchId;

    const b = profileBounds(profile);
    const c = planePointToWorld(this.plane, { x: b.centroidU, y: b.centroidV });
    this.centroidWorld = [c.x, c.y, c.z];
    this.normal = normalize(this.plane.normal as Vec3);

    this.engine.showExtrudePreview(this.plane, profile, this.centroidWorld, this.normal);
    this.engine.setExtrudeDepth(startDepth, false);

    const params: PreviewParams = {
      distance: startDepth,
      extrudeMode: "Blind",
      booleanMode: "NewBody",
    };
    if (editFeatureId) params.featureId = editFeatureId;
    const draft: PreviewDraft = { opType: "Extrude", sketchId, regionId: region.regionId, params };
    this.session = await this.deps.client.beginPreview(draft);

    this.throttle.reset();
    this.extrude = extrudeStep(extrudeInit(), { kind: "arm", depth: startDepth }).state;
    toolStore.setState({ phase: "armed" });
    viewportStore.getState().setStatusHint("Drag the arrow to set depth, or type a value");

    const chipWorld = this.chipWorld();
    toolChipStore.getState().showExtrude(startDepth, chipWorld, (v) => this.onExtrudeChip(v));

    this.sendPreview(startDepth); // initial exact L2
    this.updateDebug();
  }

  private chipWorld(): Vec3 {
    return [
      this.centroidWorld[0] + this.normal[0] * 8,
      this.centroidWorld[1] + this.normal[1] * 8,
      this.centroidWorld[2] + this.normal[2] * 8,
    ];
  }

  private onExtrudeChip(v: number): void {
    if (this.extrude.phase !== "armed" && this.extrude.phase !== "dragging") return;
    this.extrude = extrudeStep(this.extrude, { kind: "setDepth", depth: v }).state;
    this.engine.setExtrudeDepth(v, this.extrude.symmetric);
    toolChipStore.getState().setValue(v);
    this.sendPreview(v);
  }

  private armFilletFromSelection(): void {
    const edges = selectionStore.getState().selected.filter((r) => r.kind === "edge");
    if (edges.length === 0) {
      viewportStore.getState().setStatusHint("Select edges, then Fillet");
      return;
    }
    this.filletEdges = edges;
    this.fillet = filletStep(filletInit(), { kind: "arm", edgeCount: edges.length, radius: DEFAULT_FILLET_RADIUS }).state;
    toolStore.setState({ phase: "armed" });
    this.deps.engine.setOrbitSuppressed(true); // modal: drag adjusts radius, not orbit
    viewportStore.getState().setStatusHint(`Fillet ${edges.length} edge${edges.length > 1 ? "s" : ""} — drag or type radius`);
    const anchor = edges[0].anchor?.worldPoint ?? [0, 0, 0];
    toolChipStore.getState().showFillet(DEFAULT_FILLET_RADIUS, anchor, (v) => this.onFilletChip(v));
    this.updateDebug();
  }

  private onFilletChip(v: number): void {
    this.fillet = filletStep(this.fillet, { kind: "setRadius", radius: v }).state;
    toolChipStore.getState().setValue(v);
  }

  private startBooleanFromSelection(): void {
    const body = selectionStore.getState().selected.find((r) => r.kind === "body");
    if (!body) {
      viewportStore.getState().setStatusHint("Select the target body, then pick the tool body");
      return;
    }
    this.boolean = booleanStep(booleanInit(), { kind: "start", targetBodyId: body.id }).state;
    toolStore.setState({ phase: "armed" });
    viewportStore.getState().setStatusHint("Pick the tool body to combine");
    this.updateDebug();
  }

  // ── pointer handling ─────────────────────────────────────────────────────────

  private onPointerDown = (e: PointerEvent): void => {
    if (toolStore.getState().mode !== "model") return;
    this.downX = e.clientX;
    this.downY = e.clientY;
    this.downButton = e.button;
    this.moved = false;
    if (e.button !== 0) return;

    if (this.extrude.phase === "armed" && this.engine.hitExtrudeHandle(e.clientX, e.clientY)) {
      this.dragging = "extrude";
      this.extrude = extrudeStep(this.extrude, { kind: "grab" }).state;
      this.engine.setExtrudeHandleHover(true);
      toolStore.setState({ phase: "dragging" });
    } else if (this.fillet.phase === "armed") {
      this.dragging = "fillet";
      this.filletDownY = e.clientY;
      this.filletStartRadius = this.fillet.radius;
      this.fillet = filletStep(this.fillet, { kind: "grabEdge" }).state;
      toolStore.setState({ phase: "dragging" });
    }
  };

  private onPointerMove = (e: PointerEvent): void => {
    if (Math.abs(e.clientX - this.downX) > DRAG_PX || Math.abs(e.clientY - this.downY) > DRAG_PX) {
      this.moved = true;
    }
    if (this.dragging === "extrude") {
      const ray = this.engine.screenRay(e.clientX, e.clientY);
      if (!ray) return;
      const depth = axisDepthFromRay(ray.origin, ray.dir, this.centroidWorld, this.normal);
      this.extrude = extrudeStep(this.extrude, { kind: "drag", depth, symmetric: this.altHeld }).state;
      this.engine.setExtrudeDepth(this.extrude.depth, this.extrude.symmetric);
      toolChipStore.getState().setValue(this.extrude.depth);
      this.sendPreview(this.extrude.depth);
    } else if (this.dragging === "fillet") {
      const dy = this.filletDownY - e.clientY; // up-drag grows the radius
      const radius = radiusFromDrag(this.filletStartRadius, dy, { worldPerPx: this.engine.planePixelWorld() });
      this.fillet = filletStep(this.fillet, { kind: "drag", radius }).state;
      toolChipStore.getState().setValue(radius);
    } else if (this.extrude.phase === "armed") {
      this.engine.setExtrudeHandleHover(this.engine.hitExtrudeHandle(e.clientX, e.clientY));
    }
  };

  private onPointerUp = (e: PointerEvent): void => {
    const wasClick =
      this.downButton === 0 && e.button === 0 && !this.moved &&
      Math.abs(e.clientX - this.downX) <= DRAG_PX && Math.abs(e.clientY - this.downY) <= DRAG_PX;
    this.downButton = -1;

    if (this.dragging === "extrude") {
      this.dragging = null;
      this.engine.setExtrudeHandleHover(false);
      this.extrude = extrudeStep(this.extrude, { kind: "release" }).state;
      void this.commitExtrude();
      return;
    }
    if (this.dragging === "fillet") {
      this.dragging = null;
      this.fillet = filletStep(this.fillet, { kind: "release" }).state;
      void this.commitFillet();
      return;
    }
    // Boolean tool-body pick (a click, not a drag).
    if (wasClick && this.boolean.phase === "pickTool") {
      const hit = this.engine.probePick(e.clientX, e.clientY);
      if (hit && hit.bodyId !== this.boolean.targetBodyId) this.pickBooleanTool(hit.bodyId);
    }
  };

  private get engine(): ViewportEngine {
    return this.deps.engine;
  }

  /** Gate/test hook: grab the extrude handle without a real pointerdown. */
  forceExtrudeGrab(): void {
    if (this.extrude.phase !== "armed") return;
    this.dragging = "extrude";
    this.extrude = extrudeStep(this.extrude, { kind: "grab" }).state;
    toolStore.setState({ phase: "dragging" });
  }

  /** True while an extrude preview is armed or dragging (gate readiness probe). */
  get extrudeActive(): boolean {
    return this.extrude.phase === "armed" || this.extrude.phase === "dragging";
  }

  /** Gate/test hook: pick a boolean tool body without a real click. */
  forceBooleanPick(toolBodyId: string): void {
    if (this.boolean.phase === "pickTool") this.pickBooleanTool(toolBodyId);
  }

  // ── extrude preview send / receive ───────────────────────────────────────────

  private get client(): CadClient {
    return this.deps.client;
  }

  private sendPreview(depth: number): void {
    if (!this.session) return;
    const send = this.throttle.request({ distance: depth }, performance.now());
    if (send) this.client.updatePreview(this.session.sessionId, { distance: send.params.distance }, send.epoch);
    this.scheduleTrailing();
  }

  private trailingTimer: ReturnType<typeof setTimeout> | null = null;
  private scheduleTrailing(): void {
    if (this.trailingTimer) return;
    this.trailingTimer = setTimeout(() => {
      this.trailingTimer = null;
      if (!this.session) return;
      const send = this.throttle.tick(performance.now());
      if (send) {
        this.client.updatePreview(this.session.sessionId, { distance: send.params.distance }, send.epoch);
        this.scheduleTrailing();
      }
    }, 90);
  }

  private onPreviewResult(r: PreviewResult): void {
    if (!this.session || r.sessionId !== this.session.sessionId) return;
    const now = performance.now();
    if (r.committed) {
      this.committedEpoch = r.epoch;
      this.updateDebug();
      return;
    }
    const fresh = this.throttle.onResponse(r.epoch, now);
    if (!fresh) return; // stale drag result — discard
    this.applyPreviewBody(r);
    this.lastL2Epoch = r.epoch;
    const send = this.throttle.tick(now);
    if (send) this.client.updatePreview(this.session.sessionId, { distance: send.params.distance }, send.epoch);
    this.updateDebug();
  }

  /** Swap the exact L2 body underneath L1 (reuses the meshSync double-buffer). */
  private applyPreviewBody(r: PreviewResult): void {
    const view = parseMeshPayload(r.mesh);
    const entry = buildBodyObjects(view, r.bodyId, ++this.previewMeshRev);
    swapMesh(r.bodyId, entry);
    this.engine.setPreviewBody(entry);
  }

  private async commitExtrude(): Promise<void> {
    if (!this.session) return;
    const sessionId = this.session.sessionId;
    toolStore.setState({ phase: "committing" });
    const now = performance.now();
    const finalDepth = this.extrude.depth;
    const symmetric = this.extrude.symmetric;

    // Force the final params out as the newest epoch, then commit.
    this.throttle.request({ distance: finalDepth }, now);
    const send = this.throttle.flush(now);
    this.commitFinalEpoch = send ? send.epoch : this.throttle.epoch;
    this.client.updatePreview(sessionId, { distance: finalDepth, extrudeMode: symmetric ? "Symmetric" : "Blind" }, this.commitFinalEpoch);

    const res = await this.client.endPreview(sessionId, true);
    if (!res || !res.changedBodies[0]) {
      this.finishExtrude(null);
      return;
    }
    const bodyId = res.changedBodies[0].bodyId;
    this.commitBodyId = bodyId;
    this.applyResult(res);
    // Drop L1 only once the exact body is present in the scene (matching epoch).
    this.commitBodyUnsub = this.deps.onBodyLoaded((loaded) => {
      if (loaded !== bodyId) return;
      this.commitBodyUnsub?.();
      this.commitBodyUnsub = null;
      this.finishExtrude(bodyId);
    });
  }

  private finishExtrude(bodyId: string | null): void {
    this.engine.hideExtrudePreview();
    if (this.session) removeMesh(this.session.previewBodyId);
    this.engine.clearPreviewBody();
    toolChipStore.getState().clear();
    this.session = null;
    this.extrude = extrudeStep(this.extrude, { kind: "settle" }).state;
    this.throttle.reset();
    if (bodyId) {
      selectionStore.getState().set([{ kind: "body", id: bodyId }]);
      viewportStore.getState().setStatusHint("Extruded");
    }
    toolStore.getState().setTool("select");
    this.commitBodyId = null;
    this.updateDebug();
  }

  // ── fillet commit ────────────────────────────────────────────────────────────

  private async commitFillet(): Promise<void> {
    const radius = this.fillet.radius;
    const edges = this.filletEdges;
    if (edges.length === 0) {
      this.cancelFillet();
      return;
    }
    const op: OperationOp = {
      opType: "Fillet",
      inputs: edges.map((e) => this.semanticRefFor(e)),
      params: { mode: "Fillet", radius, edgeIds: edges.map((e) => e.topoKey ?? e.id), chainTangentEdges: true },
    };
    this.deps.engine.setOrbitSuppressed(false);
    toolChipStore.getState().clear();
    const res = await this.client.applyOperation(op);
    this.applyResult(res);
    this.fillet = filletInit();
    viewportStore.getState().setStatusHint(`Filleted ${edges.length} edge${edges.length > 1 ? "s" : ""}`);
    toolStore.getState().setTool("select");
    this.updateDebug();
  }

  // ── boolean ──────────────────────────────────────────────────────────────────

  private pickBooleanTool(toolBodyId: string): void {
    this.boolean = booleanStep(this.boolean, { kind: "pickTool", toolBodyId }).state;
    if (this.boolean.phase !== "armed") return;
    // Ghost: highlight both bodies translucently (documented; a dedicated ghost
    // material lands with the design's combine dialog).
    selectionStore.getState().set([
      { kind: "body", id: this.boolean.targetBodyId! },
      { kind: "body", id: toolBodyId },
    ]);
    const world = this.bodyCenter(toolBodyId);
    toolChipStore.getState().showBoolean(
      this.boolean.op,
      world,
      (op) => this.setBooleanOp(op),
      () => void this.commitBoolean(),
    );
    viewportStore.getState().setStatusHint("Choose Union / Cut / Intersect, then Apply");
    this.updateDebug();
  }

  private setBooleanOp(op: BooleanOperation): void {
    this.boolean = booleanStep(this.boolean, { kind: "setOp", op }).state;
    toolChipStore.getState().setOp(op);
  }

  private async commitBoolean(): Promise<void> {
    if (this.boolean.phase !== "armed" || !this.boolean.targetBodyId || !this.boolean.toolBodyId) return;
    const { targetBodyId, toolBodyId, op } = this.boolean;
    this.boolean = booleanStep(this.boolean, { kind: "apply" }).state;
    toolChipStore.getState().clear();
    const operation = op;
    const cmd: OperationOp = {
      opType: "Boolean",
      inputs: [
        { primary: { bodyId: targetBodyId, kind: "body" } },
        { primary: { bodyId: toolBodyId, kind: "body" } },
      ],
      params: { operation, targetBodyId, toolBodyId },
    };
    const res = await this.client.applyOperation(cmd);
    this.applyResult(res);
    this.boolean = booleanInit();
    selectionStore.getState().set([{ kind: "body", id: targetBodyId }]);
    viewportStore.getState().setStatusHint(`${operation} applied`);
    toolStore.getState().setTool("select");
    this.updateDebug();
  }

  private bodyCenter(bodyId: string): Vec3 {
    const entry = getEntry(bodyId);
    if (!entry) return [0, 0, 0];
    const mn = entry.view.bboxMin;
    const mx = entry.view.bboxMax;
    return [(mn[0] + mx[0]) / 2, (mn[1] + mx[1]) / 2, (mn[2] + mx[2]) / 2];
  }

  // ── undo / redo ──────────────────────────────────────────────────────────────

  async undo(): Promise<void> {
    const res = await this.client.undo();
    this.applyResult(res);
    if (res.opLabel) viewportStore.getState().setStatusHint(`Undid ${res.opLabel}`);
  }

  async redo(): Promise<void> {
    const res = await this.client.redo();
    this.applyResult(res);
    if (res.opLabel) viewportStore.getState().setStatusHint(`Redid ${res.opLabel}`);
  }

  // ── parametric edit (double-click extrude feature) ───────────────────────────

  /** Re-arm the extrude tool on an existing extrude feature (parametric edit seed). */
  editExtrudeFeature(featureId: string): void {
    const feat = documentStore.getState().features.find((f) => f.id === featureId);
    if (!feat || feat.kind !== "extrude") return;
    const sketchId = this.lastArmedSketch ?? Object.keys(documentStore.getState().sketches)[0];
    if (!sketchId) {
      viewportStore.getState().setStatusHint("No sketch to re-edit");
      return;
    }
    const depth = parseFloat(feat.valueText) || DEFAULT_EXTRUDE_DEPTH;
    toolStore.getState().setTool("extrude");
    void this.armExtrude(sketchId, featureId, depth);
  }

  // ── shared helpers ────────────────────────────────────────────────────────────

  private semanticRefFor(e: EntityRef): SemanticRef {
    return {
      primary: { bodyId: e.bodyId ?? "", elementId: e.elementId, kind: e.kind === "edge" ? "edge" : "face" },
      anchor: e.anchor ? { worldPoint: e.anchor.worldPoint, surfaceUv: e.anchor.surfaceUv } : undefined,
    };
  }

  private applyResult(res: {
    revision: number;
    features: FeatureRecord[];
    changedBodies?: { bodyId: string }[];
    removedBodies?: string[];
  }): void {
    const doc = documentStore.getState();
    // Register any freshly-created body + drop removed ones (tree + visibility).
    const bodies = { ...doc.bodies };
    let n = Object.keys(bodies).length;
    for (const ref of res.changedBodies ?? []) {
      if (!bodies[ref.bodyId]) bodies[ref.bodyId] = { id: ref.bodyId, name: `Body ${++n}`, visible: true };
    }
    for (const id of res.removedBodies ?? []) delete bodies[id];
    doc.applyChange({
      revision: res.revision,
      features: res.features.map(toFeatureMeta),
      bodies,
      dirty: true,
    });
  }

  private updateDebug(): void {
    if (!this.deps.debug) return;
    (window as unknown as { __extrudePreview?: unknown }).__extrudePreview = {
      l1Present: this.engine.isExtrudePreviewVisible(),
      phase: this.extrude.phase,
      depth: this.extrude.depth,
      sessionId: this.session?.sessionId ?? null,
      commitBodyId: this.commitBodyId,
      lastL2Epoch: this.lastL2Epoch,
      finalEpoch: this.commitFinalEpoch,
      committedEpoch: this.committedEpoch,
      throttleEpoch: this.throttle.epoch,
      inFlight: this.throttle.inFlight,
    };
  }

  // ── keyboard ──────────────────────────────────────────────────────────────────

  private onKeyDown = (e: KeyboardEvent): void => {
    if (e.key === "Alt") {
      this.altHeld = true;
      if (this.dragging === "extrude") {
        this.extrude = extrudeStep(this.extrude, { kind: "drag", depth: this.extrude.depth, symmetric: true }).state;
        this.engine.setExtrudeDepth(this.extrude.depth, true);
      }
    }
  };

  private onKeyUp = (e: KeyboardEvent): void => {
    if (e.key === "Alt") {
      this.altHeld = false;
      if (this.dragging === "extrude") {
        this.extrude = extrudeStep(this.extrude, { kind: "drag", depth: this.extrude.depth, symmetric: false }).state;
        this.engine.setExtrudeDepth(this.extrude.depth, false);
      }
    }
  };

  // ── cancel / teardown ──────────────────────────────────────────────────────────

  private cancelPreview(): void {
    // Cancel an in-flight extrude preview (tool switched away).
    if (this.session) {
      void this.client.endPreview(this.session.sessionId, false);
      removeMesh(this.session.previewBodyId);
      this.session = null;
    }
    this.commitBodyUnsub?.();
    this.commitBodyUnsub = null;
    this.engine.hideExtrudePreview();
    this.engine.clearPreviewBody();
    this.throttle.reset();
    this.extrude = extrudeInit();
    this.dragging = null;
    if (this.trailingTimer) {
      clearTimeout(this.trailingTimer);
      this.trailingTimer = null;
    }
  }

  private cancelFillet(): void {
    this.deps.engine.setOrbitSuppressed(false);
    this.fillet = filletInit();
    this.filletEdges = [];
    toolChipStore.getState().clear();
  }

  private cancelBoolean(): void {
    this.boolean = booleanInit();
    toolChipStore.getState().clear();
  }

  private cancelAll(): void {
    this.cancelPreview();
    this.cancelFillet();
    this.cancelBoolean();
    toolChipStore.getState().clear();
    toolStore.setState({ phase: toolStore.getState().modelTool === "select" ? "idle" : "armed" });
    this.updateDebug();
  }

  dispose(): void {
    const c = this.deps.container;
    c.removeEventListener("pointerdown", this.onPointerDown);
    c.removeEventListener("pointermove", this.onPointerMove);
    c.removeEventListener("pointerup", this.onPointerUp);
    window.removeEventListener("keydown", this.onKeyDown, true);
    window.removeEventListener("keyup", this.onKeyUp, true);
    if (this.trailingTimer) clearTimeout(this.trailingTimer);
    this.commitBodyUnsub?.();
    if (this.session) removeMesh(this.session.previewBodyId);
    for (const u of this.unsubs) u();
    this.unsubs.length = 0;
  }
}

function toFeatureMeta(f: FeatureRecord): FeatureMeta {
  return { id: f.id, kind: f.kind, label: f.label, valueText: f.valueText, status: f.status };
}
