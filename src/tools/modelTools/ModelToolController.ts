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
  ApplyOperationResult,
  AxisRef,
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
import { updateScalarParamsCommand } from "@/ipc/tauriCommandMap";
import { planePointToWorld } from "@/viewport/engine/sketchBasis";
import { parseMeshPayload } from "@/viewport/mesh/parseMeshPayload";
import { buildBodyObjects, getEntry, remove as removeMesh, swap as swapMesh } from "@/viewport/mesh/meshRegistry";
import { toolStore } from "@/stores/toolStore";
import { viewportStore } from "@/stores/viewportStore";
import { documentStore, type FeatureMeta } from "@/stores/documentStore";
import { selectionStore, type EntityRef } from "@/stores/selectionStore";
import { toolChipStore } from "@/stores/toolChipStore";
import { profileFromRegion, profileBounds, type PrismProfile } from "@/tools/preview/prismPreview";
import { axisDepthFromRay, normalize, type Vec3 } from "@/tools/preview/depthProjection";
import { radiusFromDrag, radiusFromValueText } from "@/tools/preview/filletRadius";
import { axisSplitsRegion, type LatheAxis } from "@/tools/preview/lathePreview";
import { angleFromDrag, snapRevolveAngle, clampAngle, angleFromValueText } from "@/tools/preview/revolveAngle";
import { thicknessFromValueText } from "@/tools/preview/shellThickness";
import {
  WORLD_AXIS,
  WORLD_PLANE_NORMAL,
  linearGhostTransforms,
  circularGhostTransforms,
  mirrorGhostTransforms,
  clampPatternCount,
  countFromValueText,
} from "@/tools/preview/patternPreview";
import { PreviewThrottle } from "@/tools/preview/previewThrottle";
import {
  booleanInit,
  booleanStep,
  extrudeInit,
  extrudeStep,
  filletInit,
  filletStep,
  revolveInit,
  revolveStep,
  shellInit,
  shellStep,
  linearPatternInit,
  linearPatternStep,
  circularPatternInit,
  circularPatternStep,
  mirrorInit,
  mirrorStep,
  DEFAULT_EXTRUDE_DEPTH,
  DEFAULT_FILLET_RADIUS,
  DEFAULT_REVOLVE_ANGLE,
  DEFAULT_SHELL_THICKNESS,
  type BooleanFsm,
  type ExtrudeFsm,
  type FilletFsm,
  type RevolveFsm,
  type ShellFsm,
  type LinearPatternFsm,
  type CircularPatternFsm,
  type MirrorFsm,
  type PatternAxis,
  type MirrorPlane,
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

type DragKind = "extrude" | "fillet" | "revolve" | "shell" | null;

/** One pickable revolve axis candidate (a sketch line), plane (u,v) endpoints. */
interface AxisCandidate {
  id: string;
  a: [number, number];
  b: [number, number];
}

export class ModelToolController {
  private extrude: ExtrudeFsm = extrudeInit();
  private fillet: FilletFsm = filletInit();
  private boolean: BooleanFsm = booleanInit();
  private revolve: RevolveFsm = revolveInit();
  private shell: ShellFsm = shellInit();
  private linear: LinearPatternFsm = linearPatternInit();
  private circular: CircularPatternFsm = circularPatternInit();
  private mirror: MirrorFsm = mirrorInit();
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
  private filletEditFeatureId: string | undefined;
  /** Stored params of the fillet being re-edited (radius-only edit preserves edges). */
  private filletStoredParams: Record<string, unknown> | undefined;

  // Shell context (mirrors fillet: face selection + vertical thickness drag).
  private shellFaces: EntityRef[] = [];
  private shellDownY = 0;
  private shellStartThickness = DEFAULT_SHELL_THICKNESS;
  private shellEditFeatureId: string | undefined;
  /** Stored params of the shell being re-edited (thickness-only edit preserves faces). */
  private shellStoredParams: Record<string, unknown> | undefined;

  // Pattern / mirror context (chip-driven; ghost clones of the source body).
  private patternEditFeatureId: string | undefined;

  // Revolve context.
  private revolveProfile: PrismProfile | null = null;
  private revolveSketchId: string | null = null;
  private revolveRegionId: string | null = null;
  private revolveEditFeatureId: string | undefined;
  /** Stored params of the revolve being re-edited (angle-only edit preserves the axis). */
  private revolveStoredParams: Record<string, unknown> | undefined;
  private revolveAxisCandidates: AxisCandidate[] = [];
  private revolveAxis: LatheAxis | null = null;
  private revolveAxisLineId: string | null = null;
  private revolveArmedDown = false; // LMB pressed while armed (maybe an angle drag)
  private revolveDownX = 0;
  private revolveLastX = 0;
  private revolveStartAngle = DEFAULT_REVOLVE_ANGLE;
  private commitRevolveBodyUnsub: (() => void) | null = null;

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
    this.cancelRevolve();
    this.cancelShell();
    this.cancelPattern();
    toolChipStore.getState().clear();
    if (tool === "extrude") this.armExtrudeFromSelection();
    else if (tool === "revolve") this.armRevolveFromSelection();
    else if (tool === "fillet") this.armFilletFromSelection();
    else if (tool === "boolean") this.startBooleanFromSelection();
    else if (tool === "shell") this.armShellFromSelection();
    else if (tool === "linearPattern") this.armLinearFromSelection();
    else if (tool === "circularPattern") this.armCircularFromSelection();
    else if (tool === "mirror") this.armMirrorFromSelection();
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

  // ── revolve ────────────────────────────────────────────────────────────────

  private armRevolveFromSelection(): void {
    const sketch = selectionStore.getState().selected.find((r) => r.kind === "sketch");
    if (sketch) void this.armRevolve(sketch.id);
    else viewportStore.getState().setStatusHint("Select a sketch to revolve");
  }

  /**
   * Arm the revolve tool on a sketch: resolve its profile + candidate axis lines,
   * then enter axis-pick (fresh) or go straight to armed (re-edit, `editFeatureId`).
   */
  private async armRevolve(
    sketchId: string,
    editFeatureId?: string,
    startAngle = DEFAULT_REVOLVE_ANGLE,
  ): Promise<void> {
    const finish = await this.deps.client.finishSketch(sketchId);
    const region = finish.regions[0];
    const profile = region ? profileFromRegion(region) : null;
    if (!region || !profile) {
      viewportStore.getState().setStatusHint("No closed region to revolve");
      toolStore.getState().setTool("select");
      return;
    }
    const session = await this.deps.client.enterSketch(sketchId); // plane + entities
    this.plane = session.plane;
    this.lastArmedSketch = sketchId;
    this.revolveProfile = profile;
    this.revolveSketchId = sketchId;
    this.revolveRegionId = region.regionId;
    this.revolveEditFeatureId = editFeatureId;
    // Re-edit: fetch the stored params so the angle-only commit deep-merges instead of
    // clobbering the user-picked axis (the projection does not expose it).
    this.revolveStoredParams = editFeatureId
      ? await this.deps.client.getOperationParams(editFeatureId).catch(() => undefined)
      : undefined;
    this.revolveAxisCandidates = session.entities
      .filter((e) => e.type === "Line" && e.p0 && e.p1)
      .map((e) => ({ id: e.id, a: e.p0 as [number, number], b: e.p1 as [number, number] }));

    const b = profileBounds(profile);
    const c = planePointToWorld(this.plane, { x: b.centroidU, y: b.centroidV });
    this.centroidWorld = [c.x, c.y, c.z];
    this.normal = normalize(this.plane.normal as Vec3);
    this.revolveArmedDown = false;

    if (editFeatureId) {
      // Re-edit is param-only (angle) — skip axis-pick; use the first candidate (or
      // a fallback) purely so the L1 shell renders. The commit re-targets by id.
      const cand = this.revolveAxisCandidates[0] ?? null;
      this.revolveAxis = cand ? { a: cand.a, b: cand.b } : fallbackAxis(profile.ring);
      this.revolveAxisLineId = cand?.id ?? null;
      this.revolve = revolveStep(revolveInit(), {
        kind: "arm",
        angle: startAngle,
        hasAxis: true,
        axisLineId: this.revolveAxisLineId,
      }).state;
      this.deps.engine.setOrbitSuppressed(true);
      this.deps.engine.showRevolvePreview(this.plane, profile.ring, this.revolveAxis, startAngle);
      toolStore.setState({ phase: "armed" });
      viewportStore.getState().setStatusHint("Drag to set angle, or type a value");
      toolChipStore.getState().showRevolve(
        startAngle,
        this.revolveChipWorld(),
        (v) => this.onRevolveChip(v),
        () => this.resetRevolveAxis(),
      );
    } else {
      this.revolveAxis = null;
      this.revolveAxisLineId = null;
      this.revolve = revolveStep(revolveInit(), { kind: "arm", angle: startAngle }).state; // → axisPick
      this.deps.engine.showRevolveAxisCandidates(
        this.plane,
        this.revolveAxisCandidates.map((k) => ({ a: k.a, b: k.b })),
      );
      toolStore.setState({ phase: "armed" });
      viewportStore.getState().setStatusHint(
        this.revolveAxisCandidates.length
          ? "Pick axis line"
          : "Draw a sketch line to use as the revolve axis",
      );
    }
    this.updateDebug();
  }

  private revolveChipWorld(): Vec3 {
    return this.chipWorld();
  }

  private onRevolveChip(v: number): void {
    if (this.revolve.phase !== "armed" && this.revolve.phase !== "dragging") return;
    const angle = clampAngle(v);
    this.revolve = revolveStep(this.revolve, { kind: "setAngle", angle }).state;
    if (this.revolveAxis && this.revolveProfile) {
      this.deps.engine.setRevolveAngle(this.revolveProfile.ring, this.revolveAxis, angle);
    }
    toolChipStore.getState().setValue(angle);
  }

  /** Chip "Axis" affordance: drop the chosen axis and return to axis-pick. */
  private resetRevolveAxis(): void {
    if (this.revolve.phase !== "armed" && this.revolve.phase !== "dragging") return;
    this.revolve = revolveStep(this.revolve, { kind: "resetAxis" }).state;
    this.revolveAxis = null;
    this.revolveAxisLineId = null;
    this.revolveArmedDown = false;
    if (this.dragging === "revolve") this.dragging = null;
    this.deps.engine.setOrbitSuppressed(false);
    this.deps.engine.hideRevolvePreview();
    if (this.plane) {
      this.deps.engine.showRevolveAxisCandidates(
        this.plane,
        this.revolveAxisCandidates.map((k) => ({ a: k.a, b: k.b })),
      );
    }
    toolChipStore.getState().clear();
    viewportStore.getState().setStatusHint("Pick axis line");
    toolStore.setState({ phase: "armed" });
  }

  /** Axis-pick hover: highlight the nearest candidate line under the pointer. */
  private updateRevolveAxisHover(clientX: number, clientY: number): void {
    if (!this.plane) return;
    const p = this.deps.engine.screenToPlaneOn(this.plane, clientX, clientY);
    const idx = p ? this.nearestAxisCandidate(p.x, p.y) : -1;
    if (idx < 0) {
      this.deps.engine.setRevolveAxisHover(null);
      return;
    }
    const c = this.revolveAxisCandidates[idx];
    this.deps.engine.setRevolveAxisHover({ a: c.a, b: c.b });
  }

  /** Axis-pick click: choose the nearest line, rejecting one that crosses the profile. */
  private tryPickRevolveAxis(clientX: number, clientY: number): void {
    if (!this.plane || !this.revolveProfile) return;
    const p = this.deps.engine.screenToPlaneOn(this.plane, clientX, clientY);
    if (!p) return;
    const idx = this.nearestAxisCandidate(p.x, p.y);
    if (idx < 0) return;
    const cand = this.revolveAxisCandidates[idx];
    const valid = !axisSplitsRegion(cand.a, cand.b, this.revolveProfile.ring);
    this.revolve = revolveStep(this.revolve, { kind: "pickAxis", lineId: cand.id, valid }).state;
    if (!valid) {
      this.deps.engine.setRevolveAxisHover(null);
      viewportStore.getState().setStatusHint("Axis can't cross the profile — pick another line");
      return;
    }
    this.revolveAxis = { a: cand.a, b: cand.b };
    this.revolveAxisLineId = cand.id;
    this.deps.engine.setOrbitSuppressed(true);
    this.deps.engine.showRevolvePreview(this.plane, this.revolveProfile.ring, this.revolveAxis, this.revolve.angle);
    toolChipStore.getState().showRevolve(
      this.revolve.angle,
      this.revolveChipWorld(),
      (v) => this.onRevolveChip(v),
      () => this.resetRevolveAxis(),
    );
    viewportStore.getState().setStatusHint("Drag to set angle, click to revolve 360°, or type a value");
    toolStore.setState({ phase: "armed" });
    this.updateDebug();
  }

  private nearestAxisCandidate(u: number, v: number): number {
    const tol = this.deps.engine.planePixelWorld() * 10;
    let best = -1;
    let bestD = tol;
    this.revolveAxisCandidates.forEach((c, i) => {
      const d = distPointSeg(u, v, c.a, c.b);
      if (d <= bestD) {
        bestD = d;
        best = i;
      }
    });
    return best;
  }

  /** Apply an in-progress angle drag from the current pointer x (Alt suppresses snap). */
  private applyRevolveDrag(clientX: number): void {
    if (!this.revolveAxis || !this.revolveProfile) return;
    this.revolveLastX = clientX;
    const raw = angleFromDrag(this.revolveStartAngle, clientX - this.revolveDownX);
    const angle = snapRevolveAngle(raw, this.altHeld);
    this.revolve = revolveStep(this.revolve, { kind: "drag", angle }).state;
    this.deps.engine.setRevolveAngle(this.revolveProfile.ring, this.revolveAxis, angle);
    toolChipStore.getState().setValue(angle);
  }

  private async commitRevolve(): Promise<void> {
    const angle = this.revolve.angle;
    const sketchId = this.revolveSketchId;
    const regionId = this.revolveRegionId;
    if (!sketchId || !regionId || !this.revolveProfile) {
      this.finishRevolve(null);
      return;
    }
    toolStore.setState({ phase: "committing" });
    const axis: AxisRef | undefined = this.revolveAxisLineId
      ? { kind: "sketchLine", sketchId, lineId: this.revolveAxisLineId }
      : undefined;
    const op: OperationOp = {
      opType: "Revolve",
      sketchId,
      regionId,
      featureId: this.revolveEditFeatureId,
      inputs: [{ primary: { bodyId: "", kind: "face" }, anchor: {} }],
      params: { angleDeg: angle, axis, booleanMode: "NewBody" },
    };
    try {
      // A re-edit changes ONLY the angle: deep-merge into the stored params so the
      // user-picked axis / profile / target survive (a whole-params replace would drop
      // them). A fresh revolve (or a re-edit whose params fetch failed) commits the op.
      const res =
        this.revolveEditFeatureId && this.revolveStoredParams
          ? await this.client.applyEditCommand(
              updateScalarParamsCommand(this.revolveEditFeatureId, "Revolve", this.revolveStoredParams, {
                angleDeg: { value: angle },
              }),
            )
          : await this.client.applyOperation(op);
      this.applyResult(res);
      const bodyId = res.changedBodies[0]?.bodyId ?? null;
      if (bodyId) {
        // Drop L1 only once the exact body enters the scene (mirror the extrude reconcile).
        this.commitRevolveBodyUnsub = this.deps.onBodyLoaded((loaded) => {
          if (loaded !== bodyId) return;
          this.commitRevolveBodyUnsub?.();
          this.commitRevolveBodyUnsub = null;
          this.finishRevolve(bodyId);
        });
      } else {
        this.finishRevolve(null);
      }
    } catch (e) {
      this.finishRevolve(null);
      viewportStore.getState().setStatusHint(`Revolve failed: ${errMessage(e)}`);
    }
  }

  private finishRevolve(bodyId: string | null): void {
    this.deps.engine.hideRevolvePreview();
    this.deps.engine.setOrbitSuppressed(false);
    toolChipStore.getState().clear();
    this.revolve = revolveStep(this.revolve, { kind: "settle" }).state;
    this.revolveProfile = null;
    this.revolveAxis = null;
    this.revolveAxisLineId = null;
    this.revolveAxisCandidates = [];
    this.revolveEditFeatureId = undefined;
    this.revolveStoredParams = undefined;
    this.revolveArmedDown = false;
    if (this.dragging === "revolve") this.dragging = null;
    if (bodyId) {
      selectionStore.getState().set([{ kind: "body", id: bodyId }]);
      viewportStore.getState().setStatusHint("Revolved");
    }
    toolStore.getState().setTool("select");
    this.updateDebug();
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

  // ── shell ──────────────────────────────────────────────────────────────────
  //
  // Shell mirrors fillet: it arms from a FACE selection (selected faces = removed
  // faces), a vertical drag (or the mm chip) sets the wall thickness, and release
  // commits. No cheap L1 mesh (hollowing needs OCCT) — chip + status-hint only.

  private armShellFromSelection(): void {
    const faces = selectionStore.getState().selected.filter((r) => r.kind === "face");
    if (faces.length === 0) {
      viewportStore.getState().setStatusHint("Select faces to remove, then Shell");
      return;
    }
    this.armShell(faces);
  }

  private armShell(faces: EntityRef[], editFeatureId?: string, startThickness = DEFAULT_SHELL_THICKNESS): void {
    this.shellFaces = faces;
    this.shellEditFeatureId = editFeatureId;
    this.shell = shellStep(shellInit(), {
      kind: "arm",
      // A re-edit has no fresh face picks yet — faceCount 1 keeps the FSM out of
      // its bail path (mirrors the fillet re-edit seed).
      faceCount: editFeatureId ? 1 : faces.length,
      thickness: startThickness,
    }).state;
    toolStore.setState({ phase: "armed" });
    this.deps.engine.setOrbitSuppressed(true); // modal: drag adjusts thickness, not orbit
    const n = faces.length;
    viewportStore.getState().setStatusHint(
      editFeatureId
        ? "Edit shell thickness — drag or type, Enter to apply"
        : `Shell ${n} face${n > 1 ? "s" : ""} — drag or type thickness`,
    );
    const anchor = faces[0]?.anchor?.worldPoint ?? [0, 0, 0];
    if (editFeatureId) {
      toolChipStore.getState().showShell(startThickness, anchor, (v) => {
        this.onShellChip(v);
        void this.commitShell(); // chip Enter/blur commits the thickness-only re-edit
      });
    } else {
      toolChipStore.getState().showShell(startThickness, anchor, (v) => this.onShellChip(v));
    }
    this.updateDebug();
  }

  private onShellChip(v: number): void {
    this.shell = shellStep(this.shell, { kind: "setThickness", thickness: v }).state;
    toolChipStore.getState().setValue(v);
  }

  private async commitShell(): Promise<void> {
    const thickness = this.shell.thickness;
    const faces = this.shellFaces;
    const editFeatureId = this.shellEditFeatureId;
    if (faces.length === 0 && !editFeatureId) {
      this.cancelShell();
      return;
    }
    const bodyId = faces[0]?.bodyId;
    const op: OperationOp = {
      opType: "Shell",
      featureId: editFeatureId, // parametric re-edit → UpdateOperationParams
      inputs: faces.map((f) => this.semanticRefFor(f)),
      params: {
        thickness,
        openFaces: faces.map((f) => f.elementId ?? f.topoKey ?? f.id),
        targetBodyId: bodyId,
      },
    };
    this.deps.engine.setOrbitSuppressed(false);
    toolChipStore.getState().clear();
    try {
      // A re-edit changes ONLY the thickness: deep-merge into the stored params so the
      // shell's open faces + target survive (a whole-params replace would wipe them).
      const res =
        editFeatureId && this.shellStoredParams
          ? await this.client.applyEditCommand(
              updateScalarParamsCommand(editFeatureId, "Shell", this.shellStoredParams, {
                thickness: { value: thickness },
              }),
            )
          : await this.client.applyOperation(op);
      this.applyResult(res);
      viewportStore.getState().setStatusHint(editFeatureId ? "Shell thickness updated" : "Shelled");
    } catch (e) {
      viewportStore.getState().setStatusHint(`Shell failed: ${errMessage(e)}`);
    }
    this.shell = shellInit();
    this.shellFaces = [];
    this.shellEditFeatureId = undefined;
    this.shellStoredParams = undefined;
    toolStore.getState().setTool("select");
    this.updateDebug();
  }

  /** Re-arm the shell tool on an existing shell feature (thickness re-edit seed). */
  async editShellFeature(featureId: string): Promise<void> {
    const feat = documentStore.getState().features.find((f) => f.id === featureId);
    if (!feat || feat.kind !== "shell") return;
    const thickness = thicknessFromValueText(feat.valueText);
    // Fetch the stored params so the thickness-only commit deep-merges instead of
    // wiping the shell's open faces + target (the projection does not expose them).
    const stored = await this.deps.client.getOperationParams(featureId).catch(() => undefined);
    toolStore.getState().setTool("shell"); // fires cancelShell (clears shellStoredParams)
    this.shellStoredParams = stored; // set AFTER the tool-change cancel
    this.armShell([], featureId, thickness);
  }

  // ── linear pattern ───────────────────────────────────────────────────────
  //
  // Chip-driven: axis (X/Y/Z) + count (2–12) + spacing (mm) + Apply. A live ghost
  // of translated body clones renders as any chip changes. Orbit stays free so the
  // 3D ghost can be inspected; there is no drag-to-commit (Apply commits).

  private armLinearFromSelection(): void {
    const bodyId = this.firstSelectedBodyId();
    if (!bodyId) {
      viewportStore.getState().setStatusHint("Select a body to pattern");
      return;
    }
    this.armLinear(bodyId);
  }

  private armLinear(bodyId: string, editFeatureId?: string, seedCount?: number): void {
    this.patternEditFeatureId = editFeatureId;
    this.linear = linearPatternStep(linearPatternInit(), { kind: "arm", bodyId, count: seedCount }).state;
    toolStore.setState({ phase: "armed" });
    viewportStore.getState().setStatusHint("Pick axis + count + spacing, then Apply");
    this.rebuildLinearGhost();
    toolChipStore.getState().showLinearPattern(this.linear.axis, this.linear.count, this.linear.spacing, this.bodyCenter(bodyId), {
      onAxis: (a) => this.onLinearAxis(a),
      onCount: (n) => this.onLinearCount(n),
      onSpacing: (v) => this.onLinearSpacing(v),
      onApply: () => void this.commitLinear(),
    });
    this.updateDebug();
  }

  private onLinearAxis(axis: PatternAxis): void {
    this.linear = linearPatternStep(this.linear, { kind: "setAxis", axis }).state;
    toolChipStore.getState().setAxis(this.linear.axis);
    this.rebuildLinearGhost();
  }
  private onLinearCount(count: number): void {
    this.linear = linearPatternStep(this.linear, { kind: "setCount", count: clampPatternCount(count) }).state;
    toolChipStore.getState().setCount(this.linear.count);
    this.rebuildLinearGhost();
  }
  private onLinearSpacing(spacing: number): void {
    this.linear = linearPatternStep(this.linear, { kind: "setSpacing", spacing }).state;
    toolChipStore.getState().setValue(this.linear.spacing);
    this.rebuildLinearGhost();
  }

  private rebuildLinearGhost(): void {
    if (!this.linear.bodyId) return;
    const entry = getEntry(this.linear.bodyId);
    if (!entry) return;
    this.deps.engine.showGhostPreview(
      entry,
      linearGhostTransforms(WORLD_AXIS[this.linear.axis], this.linear.spacing, this.linear.count),
    );
  }

  private async commitLinear(): Promise<void> {
    if (this.linear.phase !== "armed" || !this.linear.bodyId) return;
    const { bodyId, axis, spacing, count } = this.linear;
    const editFeatureId = this.patternEditFeatureId;
    this.linear = linearPatternStep(this.linear, { kind: "apply" }).state;
    const op: OperationOp = {
      opType: "LinearPattern",
      featureId: editFeatureId,
      inputs: [{ primary: { bodyId, kind: "body" } }],
      params: { sourceBodyId: bodyId, direction: WORLD_AXIS[axis], spacing, count, fuseResult: true },
    };
    await this.commitPattern(op, bodyId, `Linear pattern ×${count}`);
  }

  // ── circular pattern ─────────────────────────────────────────────────────

  private armCircularFromSelection(): void {
    const bodyId = this.firstSelectedBodyId();
    if (!bodyId) {
      viewportStore.getState().setStatusHint("Select a body to pattern");
      return;
    }
    this.armCircular(bodyId);
  }

  private armCircular(bodyId: string, editFeatureId?: string, seedCount?: number): void {
    this.patternEditFeatureId = editFeatureId;
    this.circular = circularPatternStep(circularPatternInit(), { kind: "arm", bodyId, count: seedCount }).state;
    toolStore.setState({ phase: "armed" });
    viewportStore.getState().setStatusHint("Pick axis + count + angle, then Apply");
    this.rebuildCircularGhost();
    toolChipStore.getState().showCircularPattern(this.circular.axis, this.circular.count, this.circular.angle, this.bodyCenter(bodyId), {
      onAxis: (a) => this.onCircularAxis(a),
      onCount: (n) => this.onCircularCount(n),
      onAngle: (v) => this.onCircularAngle(v),
      onApply: () => void this.commitCircular(),
    });
    this.updateDebug();
  }

  private onCircularAxis(axis: PatternAxis): void {
    this.circular = circularPatternStep(this.circular, { kind: "setAxis", axis }).state;
    toolChipStore.getState().setAxis(this.circular.axis);
    this.rebuildCircularGhost();
  }
  private onCircularCount(count: number): void {
    this.circular = circularPatternStep(this.circular, { kind: "setCount", count: clampPatternCount(count) }).state;
    toolChipStore.getState().setCount(this.circular.count);
    this.rebuildCircularGhost();
  }
  private onCircularAngle(angle: number): void {
    this.circular = circularPatternStep(this.circular, { kind: "setAngle", angle }).state;
    toolChipStore.getState().setValue(this.circular.angle);
    this.rebuildCircularGhost();
  }

  private rebuildCircularGhost(): void {
    if (!this.circular.bodyId) return;
    const entry = getEntry(this.circular.bodyId);
    if (!entry) return;
    this.deps.engine.showGhostPreview(
      entry,
      circularGhostTransforms([0, 0, 0], WORLD_AXIS[this.circular.axis], this.circular.angle, this.circular.count),
    );
  }

  private async commitCircular(): Promise<void> {
    if (this.circular.phase !== "armed" || !this.circular.bodyId) return;
    const { bodyId, axis, angle, count } = this.circular;
    const editFeatureId = this.patternEditFeatureId;
    this.circular = circularPatternStep(this.circular, { kind: "apply" }).state;
    const op: OperationOp = {
      opType: "CircularPattern",
      featureId: editFeatureId,
      inputs: [{ primary: { bodyId, kind: "body" } }],
      params: {
        sourceBodyId: bodyId,
        axisOrigin: [0, 0, 0],
        axisDirection: WORLD_AXIS[axis],
        angleDeg: angle,
        count,
        fuseResult: true,
      },
    };
    await this.commitPattern(op, bodyId, `Circular pattern ×${count}`);
  }

  // ── mirror body ──────────────────────────────────────────────────────────

  private armMirrorFromSelection(): void {
    const bodyId = this.firstSelectedBodyId();
    if (!bodyId) {
      viewportStore.getState().setStatusHint("Select a body to mirror");
      return;
    }
    this.armMirror(bodyId);
  }

  private armMirror(bodyId: string, editFeatureId?: string, seedPlane?: MirrorPlane): void {
    this.patternEditFeatureId = editFeatureId;
    this.mirror = mirrorStep(mirrorInit(), { kind: "arm", bodyId, plane: seedPlane }).state;
    toolStore.setState({ phase: "armed" });
    viewportStore.getState().setStatusHint("Pick a mirror plane, then Apply");
    this.rebuildMirrorGhost();
    toolChipStore.getState().showMirror(this.mirror.plane, this.bodyCenter(bodyId), {
      onPlane: (p) => this.onMirrorPlane(p),
      onApply: () => void this.commitMirror(),
    });
    this.updateDebug();
  }

  private onMirrorPlane(plane: MirrorPlane): void {
    this.mirror = mirrorStep(this.mirror, { kind: "setPlane", plane }).state;
    toolChipStore.getState().setPlane(this.mirror.plane);
    this.rebuildMirrorGhost();
  }

  private rebuildMirrorGhost(): void {
    if (!this.mirror.bodyId) return;
    const entry = getEntry(this.mirror.bodyId);
    if (!entry) return;
    this.deps.engine.showGhostPreview(entry, mirrorGhostTransforms([0, 0, 0], WORLD_PLANE_NORMAL[this.mirror.plane]));
  }

  private async commitMirror(): Promise<void> {
    if (this.mirror.phase !== "armed" || !this.mirror.bodyId) return;
    const { bodyId, plane } = this.mirror;
    const editFeatureId = this.patternEditFeatureId;
    this.mirror = mirrorStep(this.mirror, { kind: "apply" }).state;
    const op: OperationOp = {
      opType: "MirrorBody",
      featureId: editFeatureId,
      inputs: [{ primary: { bodyId, kind: "body" } }],
      params: {
        sourceBodyId: bodyId,
        planePoint: [0, 0, 0],
        planeNormal: WORLD_PLANE_NORMAL[plane],
        fuseWithOriginal: false,
      },
    };
    await this.commitPattern(op, bodyId, "Mirrored");
  }

  /** Shared commit tail for the pattern/mirror ops (apply → select → teardown). */
  private async commitPattern(op: OperationOp, bodyId: string, doneHint: string): Promise<void> {
    this.deps.engine.hideGhostPreview();
    toolChipStore.getState().clear();
    try {
      const res = await this.client.applyOperation(op);
      this.applyResult(res);
      selectionStore.getState().set([{ kind: "body", id: bodyId }]);
      viewportStore.getState().setStatusHint(doneHint);
    } catch (e) {
      viewportStore.getState().setStatusHint(`Pattern failed: ${errMessage(e)}`);
    }
    this.linear = linearPatternInit();
    this.circular = circularPatternInit();
    this.mirror = mirrorInit();
    this.patternEditFeatureId = undefined;
    toolStore.getState().setTool("select");
    this.updateDebug();
  }

  /** Re-arm a pattern/mirror tool on an existing feature (seeds count/plane). */
  editLinearPatternFeature(featureId: string): void {
    const bodyId = this.reeditSourceBody(featureId, "linearPattern");
    if (!bodyId) return;
    toolStore.getState().setTool("linearPattern");
    this.armLinear(bodyId, featureId, countFromValueText(this.featureValueText(featureId)));
  }
  editCircularPatternFeature(featureId: string): void {
    const bodyId = this.reeditSourceBody(featureId, "circularPattern");
    if (!bodyId) return;
    toolStore.getState().setTool("circularPattern");
    this.armCircular(bodyId, featureId, countFromValueText(this.featureValueText(featureId)));
  }
  editMirrorFeature(featureId: string): void {
    const bodyId = this.reeditSourceBody(featureId, "mirror");
    if (!bodyId) return;
    toolStore.getState().setTool("mirror");
    this.armMirror(bodyId, featureId);
  }

  /**
   * The source body for a pattern/mirror re-edit. The projection does not record
   * which body a pattern cloned, so we fall back to the currently-selected body,
   * else the first document body (a mock-lane re-edit seam — a follow-up threads
   * the real source through the projection).
   */
  private reeditSourceBody(featureId: string, kind: string): string | null {
    const feat = documentStore.getState().features.find((f) => f.id === featureId);
    if (!feat || feat.kind !== kind) return null;
    const bodyId = this.firstSelectedBodyId() ?? Object.keys(documentStore.getState().bodies)[0] ?? null;
    if (!bodyId) viewportStore.getState().setStatusHint("No body to re-pattern");
    return bodyId;
  }

  private featureValueText(featureId: string): string {
    return documentStore.getState().features.find((f) => f.id === featureId)?.valueText ?? "";
  }

  private firstSelectedBodyId(): string | null {
    return selectionStore.getState().selected.find((r) => r.kind === "body")?.id ?? null;
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
    } else if (this.shell.phase === "armed") {
      this.dragging = "shell";
      this.shellDownY = e.clientY;
      this.shellStartThickness = this.shell.thickness;
      this.shell = shellStep(this.shell, { kind: "grab" }).state;
      toolStore.setState({ phase: "dragging" });
    } else if (this.revolve.phase === "armed") {
      // Defer grab to the first move: a plain click (no move) commits 360° instead.
      this.revolveArmedDown = true;
      this.revolveDownX = e.clientX;
      this.revolveLastX = e.clientX;
      this.revolveStartAngle = this.revolve.angle;
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
    } else if (this.dragging === "shell") {
      const dy = this.shellDownY - e.clientY; // up-drag grows the thickness
      const thickness = radiusFromDrag(this.shellStartThickness, dy, { worldPerPx: this.engine.planePixelWorld() });
      this.shell = shellStep(this.shell, { kind: "drag", thickness }).state;
      toolChipStore.getState().setValue(thickness);
    } else if (this.dragging === "revolve") {
      this.applyRevolveDrag(e.clientX);
    } else if (this.revolveArmedDown && this.moved && this.downButton === 0 && this.revolve.phase === "armed") {
      // First movement past the threshold promotes the armed press into an angle drag.
      this.dragging = "revolve";
      this.revolve = revolveStep(this.revolve, { kind: "grab" }).state;
      toolStore.setState({ phase: "dragging" });
      this.applyRevolveDrag(e.clientX);
    } else if (this.revolve.phase === "axisPick") {
      this.updateRevolveAxisHover(e.clientX, e.clientY);
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
    if (this.dragging === "shell") {
      this.dragging = null;
      this.shell = shellStep(this.shell, { kind: "release" }).state;
      void this.commitShell();
      return;
    }
    if (this.dragging === "revolve") {
      this.dragging = null;
      this.revolveArmedDown = false;
      this.revolve = revolveStep(this.revolve, { kind: "release" }).state;
      void this.commitRevolve();
      return;
    }
    // Plain click after an axis is chosen commits the default full 360°.
    if (wasClick && this.revolveArmedDown && this.revolve.phase === "armed") {
      this.revolveArmedDown = false;
      this.revolve = revolveStep(this.revolve, { kind: "quickCommit" }).state;
      void this.commitRevolve();
      return;
    }
    this.revolveArmedDown = false;
    // Revolve axis-pick (a click, not a drag).
    if (wasClick && this.revolve.phase === "axisPick") {
      this.tryPickRevolveAxis(e.clientX, e.clientY);
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

    let res: ApplyOperationResult | null;
    try {
      res = await this.client.endPreview(sessionId, true);
    } catch (e) {
      this.finishExtrude(null);
      viewportStore.getState().setStatusHint(`Extrude failed: ${errMessage(e)}`);
      return;
    }
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
    const editFeatureId = this.filletEditFeatureId;
    if (edges.length === 0 && !editFeatureId) {
      this.cancelFillet();
      return;
    }
    const op: OperationOp = {
      opType: "Fillet",
      featureId: editFeatureId, // parametric re-edit → UpdateOperationParams
      inputs: edges.map((e) => this.semanticRefFor(e)),
      params: { mode: "Fillet", radius, edgeIds: edges.map((e) => e.topoKey ?? e.id), chainTangentEdges: true },
    };
    this.deps.engine.setOrbitSuppressed(false);
    toolChipStore.getState().clear();
    try {
      // A re-edit changes ONLY the radius: deep-merge into the stored params so the
      // fillet's edgeIds + typed edges survive (a whole-params replace would drop
      // them). A fresh fillet commits the op (its typed edges ride via inputs).
      const res =
        editFeatureId && this.filletStoredParams
          ? await this.client.applyEditCommand(
              updateScalarParamsCommand(editFeatureId, "Fillet", this.filletStoredParams, {
                radius: { value: radius },
              }),
            )
          : await this.client.applyOperation(op);
      this.applyResult(res);
      viewportStore.getState().setStatusHint(
        editFeatureId
          ? "Fillet radius updated"
          : `Filleted ${edges.length} edge${edges.length > 1 ? "s" : ""}`,
      );
    } catch (e) {
      viewportStore.getState().setStatusHint(`Fillet failed: ${errMessage(e)}`);
    }
    this.fillet = filletInit();
    this.filletEditFeatureId = undefined;
    this.filletStoredParams = undefined;
    this.filletEdges = [];
    toolStore.getState().setTool("select");
    this.updateDebug();
  }

  /**
   * Re-arm the fillet tool on an existing fillet feature (parametric edit seed;
   * mirrors editRevolveFeature). Seeds the chip with the feature's CURRENT radius;
   * committing routes through `UpdateOperationParams` (edge refs unchanged).
   */
  async editFilletFeature(featureId: string): Promise<void> {
    const feat = documentStore.getState().features.find((f) => f.id === featureId);
    if (!feat || feat.kind !== "fillet") return;
    const radius = radiusFromValueText(feat.valueText);
    // Fetch the stored params so the radius-only commit deep-merges instead of
    // dropping the fillet's edgeIds + typed edges (the projection does not expose them).
    const stored = await this.deps.client.getOperationParams(featureId).catch(() => undefined);
    toolStore.getState().setTool("fillet"); // fires cancelFillet (clears filletStoredParams)
    this.filletStoredParams = stored; // set AFTER the tool-change cancel
    this.filletEdges = [];
    this.filletEditFeatureId = featureId;
    // edgeCount 1 keeps the FSM out of its bail path (a re-edit has no picks yet).
    this.fillet = filletStep(filletInit(), { kind: "arm", edgeCount: 1, radius }).state;
    toolStore.setState({ phase: "armed" });
    this.deps.engine.setOrbitSuppressed(true); // modal: drag adjusts radius, not orbit
    viewportStore.getState().setStatusHint("Edit fillet radius — drag or type, Enter to apply");
    toolChipStore.getState().showFillet(radius, [0, 0, 0], (v) => {
      this.onFilletChip(v);
      void this.commitFillet(); // chip Enter/blur commits the radius-only edit
    });
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
    try {
      const res = await this.client.applyOperation(cmd);
      this.applyResult(res);
      selectionStore.getState().set([{ kind: "body", id: targetBodyId }]);
      viewportStore.getState().setStatusHint(`${operation} applied`);
    } catch (e) {
      viewportStore.getState().setStatusHint(`${operation} failed: ${errMessage(e)}`);
    }
    this.boolean = booleanInit();
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

  /** Re-arm the revolve tool on an existing revolve feature (param-only angle edit). */
  editRevolveFeature(featureId: string): void {
    const feat = documentStore.getState().features.find((f) => f.id === featureId);
    if (!feat || feat.kind !== "revolve") return;
    const sketchId = this.lastArmedSketch ?? Object.keys(documentStore.getState().sketches)[0];
    if (!sketchId) {
      viewportStore.getState().setStatusHint("No sketch to re-edit");
      return;
    }
    const angle = angleFromValueText(feat.valueText);
    toolStore.getState().setTool("revolve");
    void this.armRevolve(sketchId, featureId, angle);
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
      } else if (this.dragging === "revolve") {
        this.applyRevolveDrag(this.revolveLastX); // re-evaluate the snap without Alt
      }
    }
  };

  private onKeyUp = (e: KeyboardEvent): void => {
    if (e.key === "Alt") {
      this.altHeld = false;
      if (this.dragging === "extrude") {
        this.extrude = extrudeStep(this.extrude, { kind: "drag", depth: this.extrude.depth, symmetric: false }).state;
        this.engine.setExtrudeDepth(this.extrude.depth, false);
      } else if (this.dragging === "revolve") {
        this.applyRevolveDrag(this.revolveLastX); // re-apply the 45° snap
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
    this.filletEditFeatureId = undefined;
    this.filletStoredParams = undefined;
    toolChipStore.getState().clear();
  }

  private cancelBoolean(): void {
    this.boolean = booleanInit();
    toolChipStore.getState().clear();
  }

  private cancelShell(): void {
    this.deps.engine.setOrbitSuppressed(false);
    this.shell = shellInit();
    this.shellFaces = [];
    this.shellEditFeatureId = undefined;
    this.shellStoredParams = undefined;
    if (this.dragging === "shell") this.dragging = null;
    toolChipStore.getState().clear();
  }

  private cancelPattern(): void {
    this.deps.engine.hideGhostPreview();
    this.linear = linearPatternInit();
    this.circular = circularPatternInit();
    this.mirror = mirrorInit();
    this.patternEditFeatureId = undefined;
    toolChipStore.getState().clear();
  }

  private cancelRevolve(): void {
    this.commitRevolveBodyUnsub?.();
    this.commitRevolveBodyUnsub = null;
    this.deps.engine.setOrbitSuppressed(false);
    this.engine.hideRevolvePreview();
    this.revolve = revolveInit();
    this.revolveProfile = null;
    this.revolveAxis = null;
    this.revolveAxisLineId = null;
    this.revolveAxisCandidates = [];
    this.revolveEditFeatureId = undefined;
    this.revolveStoredParams = undefined;
    this.revolveArmedDown = false;
    if (this.dragging === "revolve") this.dragging = null;
    toolChipStore.getState().clear();
  }

  private cancelAll(): void {
    this.cancelPreview();
    this.cancelFillet();
    this.cancelBoolean();
    this.cancelRevolve();
    this.cancelShell();
    this.cancelPattern();
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
    this.commitRevolveBodyUnsub?.();
    if (this.session) removeMesh(this.session.previewBodyId);
    for (const u of this.unsubs) u();
    this.unsubs.length = 0;
  }
}

function toFeatureMeta(f: FeatureRecord): FeatureMeta {
  return { id: f.id, kind: f.kind, label: f.label, valueText: f.valueText, status: f.status };
}

/** Human message from a rejected backend call (ApiError → JS Error message). */
function errMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Perpendicular distance from a plane point (u,v) to the segment a→b. */
function distPointSeg(u: number, v: number, a: [number, number], b: [number, number]): number {
  const vx = b[0] - a[0];
  const vy = b[1] - a[1];
  const wx = u - a[0];
  const wy = v - a[1];
  const c2 = vx * vx + vy * vy;
  let t = c2 > 0 ? (vx * wx + vy * wy) / c2 : 0;
  t = Math.max(0, Math.min(1, t));
  return Math.hypot(u - (a[0] + t * vx), v - (a[1] + t * vy));
}

/** A deterministic axis just left of a profile (re-edit fallback when no line exists). */
function fallbackAxis(ring: [number, number][]): LatheAxis {
  let minU = Infinity;
  let minV = Infinity;
  let maxV = -Infinity;
  for (const [u, w] of ring) {
    if (u < minU) minU = u;
    if (w < minV) minV = w;
    if (w > maxV) maxV = w;
  }
  const x = minU - 1;
  return { a: [x, minV], b: [x, maxV] };
}
