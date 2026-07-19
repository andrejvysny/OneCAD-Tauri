/*
 * SketchController — imperative glue between the sketch tool machines, the
 * ViewportEngine, the CadClient (mock solver) and the stores (F-WP6).
 *
 * Lives inside ViewportRoot (created after the engine initializes, so it never
 * runs in jsdom where WebGL is absent — component tests keep the seeded chrome).
 * Responsibilities:
 *   - enter/exit sketch mode (client.enterSketch/finishSketch/cancelSketch,
 *     ortho + plane-normal camera, DOF/status → stores),
 *   - translate container pointer events → tool-machine events (click/move/esc)
 *     against snapped plane coords,
 *   - drive the rubber-band preview, snap indicator + hint, and the H/V ghost,
 *   - on commit: id-assign → auto-constrain → sketchUpsert round-trip → refresh
 *     geometry + DOF badges.
 */
import type { CadClient } from "@/ipc/client";
import type { EnterSketchTarget, SketchConstraint, SketchEntity, SketchSession } from "@/ipc/types";
import type { ViewportEngine } from "@/viewport/engine/ViewportEngine";
import type { Point2 } from "@/viewport/engine/sketchBasis";
import { chooseGridStep } from "@/viewport/engine/GridPlane";
import { toolStore } from "@/stores/toolStore";
import { viewportStore, type Projection } from "@/stores/viewportStore";
import { documentStore, docSketchStatus } from "@/stores/documentStore";
import { settingsStore } from "@/stores/settingsStore";
import { sketchStore } from "@/stores/sketchStore";
import { toolChipStore } from "@/stores/toolChipStore";
import { applySolvedPositions } from "@/ipc/sketchWireMap";
import { planePointToWorld } from "@/viewport/engine/sketchBasis";
import { computeSnap, type SnapResult } from "./snapEngine";
import { inferConstraints } from "./autoConstrain";
import { commitDimensionConstraint } from "./sketchService";
import {
  dimensionInit,
  dimensionStep,
  buildDimensionConstraint,
  dimensionSuffix,
  pickDimensionTarget,
  type DimState,
} from "./dimensionTool";
import {
  TOOL_MACHINES,
  draftToEntityFields,
  type DraftEntity,
  type ToolMachine,
  type ToolState,
} from "./toolMachine";

const DRAG_PX = 4;

export interface SketchControllerDeps {
  engine: ViewportEngine;
  client: CadClient;
  container: HTMLElement;
}

export class SketchController {
  private machine: ToolMachine | null = null;
  private machineState: ToolState | null = null;
  private lastSnap: SnapResult | null = null;
  private altHeld = false;

  // Dimension tool (non-drawing): a pick-accumulator FSM + its open chip.
  private dimensionActive = false;
  private dimState: DimState = dimensionInit();
  private priorProjection: Projection | null = null;
  private entering = false;

  private downX = 0;
  private downY = 0;
  private downButton = -1;
  private moved = false;
  private pendingMove: PointerEvent | null = null;
  private moveScheduled = false;

  private readonly unsubs: Array<() => void> = [];

  constructor(private readonly deps: SketchControllerDeps) {
    const c = deps.container;
    c.addEventListener("pointerdown", this.onPointerDown);
    c.addEventListener("pointermove", this.onPointerMove);
    c.addEventListener("pointerup", this.onPointerUp);
    window.addEventListener("keydown", this.onKeyDown, true);
    window.addEventListener("keyup", this.onKeyUp, true);

    // React to mode + tool changes.
    let lastMode = toolStore.getState().mode;
    let lastTool = toolStore.getState().sketchTool;
    this.unsubs.push(
      toolStore.subscribe((s) => {
        if (s.mode !== lastMode) {
          lastMode = s.mode;
          if (s.mode === "sketch") void this.enter();
          else this.exit();
        }
        if (s.mode === "sketch" && s.sketchTool !== lastTool) {
          lastTool = s.sketchTool;
          this.selectMachine(s.sketchTool);
        }
      }),
    );

    // Enter immediately if we mount already in sketch mode (e.g. ?sketchdemo).
    if (toolStore.getState().mode === "sketch") void this.enter();
  }

  // ── enter / exit ──────────────────────────────────────────────────────────

  private async enter(): Promise<void> {
    if (this.entering) return;
    this.entering = true;
    try {
      // setMode('sketch') fires the mode subscription BEFORE it assigns
      // activeSketchId; yield one microtask so we read the real target.
      await Promise.resolve();
      if (toolStore.getState().mode !== "sketch") return;
      const activeId = viewportStore.getState().activeSketchId ?? "sketch";
      const target: EnterSketchTarget = activeId;
      let session: SketchSession;
      try {
        session = await this.deps.client.enterSketch(target);
      } catch (e) {
        viewportStore.getState().setStatusHint(`Enter sketch failed: ${sketchErr(e)}`);
        return;
      }
      if (toolStore.getState().mode !== "sketch") return; // exited during await

      sketchStore.getState().setSession(session);
      this.pushSolve(session.sketchId, session.dof, session.status);

      this.priorProjection = viewportStore.getState().projection;
      this.deps.engine.enterSketch(session.plane, session.entities, session.status);
      viewportStore.getState().setProjection("ortho");

      this.selectMachine(toolStore.getState().sketchTool);
    } finally {
      this.entering = false;
    }
  }

  private exit(): void {
    this.machine = null;
    this.machineState = null;
    this.lastSnap = null;
    if (this.dimensionActive) this.cancelDimension();
    this.dimensionActive = false;
    this.deps.engine.setSketchDrawingActive(false);
    this.deps.engine.setSketchPreview([]);
    this.deps.engine.setSketchSnap(null, false);
    this.deps.engine.setSketchGhost(null, null);
    this.deps.engine.exitSketch();
    viewportStore.getState().setStatusHint(null);
    if (this.priorProjection) {
      viewportStore.getState().setProjection(this.priorProjection);
      this.priorProjection = null;
    }
    const session = sketchStore.getState().session;
    if (session) void this.deps.client.cancelSketch(session.sketchId);
    sketchStore.getState().setSession(null);
  }

  private selectMachine(tool: string): void {
    // Leaving the dimension tool tears down any in-flight chip/pick.
    if (this.dimensionActive && tool !== "dimension") this.cancelDimension();

    const m = TOOL_MACHINES[tool] ?? null;
    this.machine = m;
    this.machineState = m ? m.init() : null;
    this.dimensionActive = tool === "dimension";
    // The dimension tool owns the pointer (no orbit) so clicks pick entities.
    this.deps.engine.setSketchDrawingActive(!!m || this.dimensionActive);
    this.deps.engine.setSketchPreview([]);
    this.deps.engine.setSketchGhost(null, null);
    if (!m && !this.dimensionActive) this.deps.engine.setSketchSnap(null, false);

    if (this.dimensionActive) {
      this.dimState = dimensionInit();
      viewportStore.getState().setStatusHint("Dimension — click a line, circle, arc, or two points");
      return;
    }
    // trim/mirror remain stubs (buttons exist, no behaviour yet).
    const stub = tool === "trim" || tool === "mirror";
    viewportStore.getState().setStatusHint(stub ? `${cap(tool)} — not yet implemented` : null);
  }

  // ── pointer handling ────────────────────────────────────────────────────

  private snapAt(clientX: number, clientY: number): SnapResult | null {
    const raw = this.deps.engine.screenToPlane(clientX, clientY);
    if (!raw) return null;
    const session = sketchStore.getState().session;
    const settings = settingsStore.getState();
    return computeSnap(raw, session?.entities ?? [], {
      gridStep: chooseGridStep(this.deps.engine.getCameraDistance()).minor,
      pixelWorld: this.deps.engine.planePixelWorld(),
      enableGrid: settings.snapTo.grid,
      enableGuideLines: settings.snapTo.sketchGuideLines,
      enableGuidePoints: settings.snapTo.sketchGuidePoints,
      enableQuadrant: settings.snapTo.quadrant,
      enableIntersection: settings.snapTo.intersection,
      enableOnCurve: settings.snapTo.onCurve,
      suppress: this.altHeld,
      recentPoints: this.machineState?.anchors ?? [],
    });
  }

  private onPointerMove = (e: PointerEvent): void => {
    if (!this.machine && !this.dimensionActive) return;
    this.pendingMove = e;
    if (e.buttons !== 0 && this.downButton === 0) this.moved = true;
    if (this.moveScheduled) return;
    this.moveScheduled = true;
    requestAnimationFrame(() => {
      this.moveScheduled = false;
      const ev = this.pendingMove;
      this.pendingMove = null;
      if (!ev) return;
      const snap = this.snapAt(ev.clientX, ev.clientY);
      if (!snap) return;
      this.lastSnap = snap;
      const showHints = settingsStore.getState().show.snappingHints;
      this.deps.engine.setSketchSnap(snap, showHints);
      // Dimension mode: the indicator aids aiming; there is no rubber-band preview.
      if (!this.machine || !this.machineState) return;
      const stepped = this.machine.step(this.machineState, { kind: "move", pt: snap.point });
      this.machineState = stepped.state;
      this.deps.engine.setSketchPreview(stepped.preview);
      this.updateGhost(stepped.preview, snap.point);
    });
  };

  private onPointerDown = (e: PointerEvent): void => {
    this.downX = e.clientX;
    this.downY = e.clientY;
    this.downButton = e.button;
    this.moved = false;
  };

  private onPointerUp = (e: PointerEvent): void => {
    const wasClick =
      this.downButton === 0 &&
      e.button === 0 &&
      !this.moved &&
      Math.abs(e.clientX - this.downX) <= DRAG_PX &&
      Math.abs(e.clientY - this.downY) <= DRAG_PX;
    this.downButton = -1;
    if (!wasClick) return;
    if (this.dimensionActive) {
      this.handleDimensionClick(e.clientX, e.clientY);
      return;
    }
    if (!this.machine || !this.machineState) return;
    const snap = this.snapAt(e.clientX, e.clientY) ?? this.lastSnap;
    if (!snap) return;
    const stepped = this.machine.step(this.machineState, { kind: "click", pt: snap.point });
    this.machineState = stepped.state;
    this.deps.engine.setSketchPreview(stepped.preview);
    if (stepped.committed && stepped.committed.length > 0) {
      void this.commit(stepped.committed);
    }
    if (stepped.done) this.deps.engine.setSketchGhost(null, null);
  };

  // ── commit round-trip ─────────────────────────────────────────────────────

  private async commit(committed: DraftEntity[]): Promise<void> {
    const store = sketchStore.getState();
    const session = store.session;
    if (!session) return;

    const newEntities: SketchEntity[] = committed.map((d) => ({
      id: store.nextEntityId(),
      ...draftToEntityFields(d),
    }));
    const newConstraints: SketchConstraint[] = inferConstraints(newEntities, session.entities, {
      nextConstraintId: () => sketchStore.getState().nextConstraintId(),
    });

    const entities = [...session.entities, ...newEntities];
    const constraints = [...session.constraints, ...newConstraints];

    let result;
    try {
      result = await this.deps.client.sketchUpsert(session.sketchId, entities, constraints);
    } catch (e) {
      viewportStore.getState().setStatusHint(`Sketch solve failed: ${sketchErr(e)}`);
      return;
    }
    // A late exit could have cleared the session mid-await.
    if (!sketchStore.getState().session) return;

    // F-WP9: the solver may have MOVED points (constraint-driven); write the
    // solved positions back into the geometry (backend point UUIDs were already
    // reverse-mapped to `entityId.Position` keys by the client). No-op when the
    // solve returned no movement (identity upsert) — same array reference.
    const solvedEntities = applySolvedPositions(entities, result.solvedPositions ?? {});

    const next: SketchSession = { ...session, entities: solvedEntities, constraints, dof: result.dof, status: result.status };
    sketchStore.getState().setSession(next);
    this.deps.engine.updateSketchSession(next.plane, solvedEntities, next.status);
    this.pushSolve(session.sketchId, result.dof, result.status);
  }

  private pushSolve(id: string, dof: number, status: SketchSession["status"]): void {
    documentStore.getState().setSketchSolve(id, dof, docSketchStatus(status));
    viewportStore.setState({ dofBadge: dof });
  }

  // ── Dimension tool ─────────────────────────────────────────────────────────

  /** A click in dimension mode: resolve the pick, step the FSM, (re)open the chip. */
  private handleDimensionClick(clientX: number, clientY: number): void {
    const session = sketchStore.getState().session;
    if (!session) return;
    const raw = this.deps.engine.screenToPlane(clientX, clientY);
    if (!raw) return;
    const tol = 8 * this.deps.engine.planePixelWorld(); // same 8px reach as snapping
    const target = pickDimensionTarget(raw, session.entities, tol);
    if (!target) {
      // A click on empty space cancels a half-made pick (but leaves an open chip).
      if (!this.dimState.ready) this.cancelDimension();
      return;
    }
    const step = dimensionStep(this.dimState, { kind: "pick", target });
    this.dimState = step.state;
    if (this.dimState.ready) this.openDimensionChip();
    else {
      toolChipStore.getState().clear();
      viewportStore.getState().setStatusHint("Dimension — pick a second point");
    }
  }

  /** Open (or re-seed) the dimension chip at the armed spec's anchor. */
  private openDimensionChip(): void {
    const spec = this.dimState.ready;
    const session = sketchStore.getState().session;
    if (!spec || !session) return;
    const world = planePointToWorld(session.plane, spec.anchor).toArray() as [number, number, number];
    toolChipStore.getState().showDimension(
      spec.value,
      dimensionSuffix(spec.kind),
      world,
      (v) => void this.commitDimensionValue(v),
      () => this.cancelDimension(),
    );
    viewportStore.getState().setStatusHint(null);
  }

  /** Chip Enter: author the armed dimension through the solver (reject on conflict). */
  private async commitDimensionValue(value: number): Promise<void> {
    const step = dimensionStep(this.dimState, { kind: "commit", value });
    this.dimState = step.state;
    toolChipStore.getState().clear();
    if (!step.emit) return;
    const id = sketchStore.getState().nextConstraintId();
    const constraint = buildDimensionConstraint(step.emit, id);
    try {
      const { rejected } = await commitDimensionConstraint(this.deps.client, constraint);
      viewportStore
        .getState()
        .setStatusHint(rejected ? "Dimension removed — it would over-constrain the sketch" : null);
    } catch (e) {
      viewportStore.getState().setStatusHint(`Dimension failed: ${sketchErr(e)}`);
    }
  }

  /** Esc / tool change / empty click: drop the in-flight dimension + chip. */
  private cancelDimension(): void {
    this.dimState = dimensionInit();
    toolChipStore.getState().clear();
    if (this.dimensionActive) {
      viewportStore.getState().setStatusHint("Dimension — click a line, circle, arc, or two points");
    }
  }

  private updateGhost(preview: DraftEntity[], cursor: Point2): void {
    const line = preview.find((d) => d.type === "Line" && !d.construction && d.p0 && d.p1);
    if (!line || !line.p0 || !line.p1) {
      this.deps.engine.setSketchGhost(null, null);
      return;
    }
    const hv = inferHVDraft(line.p0, line.p1);
    this.deps.engine.setSketchGhost(hv, hv ? cursor : null);
  }

  // ── keyboard (Alt suppress + Esc ends chain) ──────────────────────────────

  private onKeyDown = (e: KeyboardEvent): void => {
    if (e.key === "Alt") this.altHeld = true;
    if (e.key === "Escape" && this.dimensionActive && (this.dimState.ready || this.dimState.pending)) {
      // Cancel the in-flight dimension here; don't let the global Esc ladder run.
      this.cancelDimension();
      e.stopPropagation();
      e.preventDefault();
      return;
    }
    if (e.key === "Escape" && this.machine && this.machineState && this.machineState.anchors.length > 0) {
      // A gesture is in progress: end the chain here, and DON'T let the global
      // Esc ladder also switch tools (capture-phase intercept).
      const stepped = this.machine.step(this.machineState, { kind: "esc" });
      this.machineState = stepped.state;
      this.deps.engine.setSketchPreview([]);
      this.deps.engine.setSketchGhost(null, null);
      e.stopPropagation();
      e.preventDefault();
    }
  };

  private onKeyUp = (e: KeyboardEvent): void => {
    if (e.key === "Alt") this.altHeld = false;
  };

  dispose(): void {
    const c = this.deps.container;
    c.removeEventListener("pointerdown", this.onPointerDown);
    c.removeEventListener("pointermove", this.onPointerMove);
    c.removeEventListener("pointerup", this.onPointerUp);
    window.removeEventListener("keydown", this.onKeyDown, true);
    window.removeEventListener("keyup", this.onKeyUp, true);
    for (const u of this.unsubs) u();
    this.unsubs.length = 0;
  }
}

const cap = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);

/** Human message from a rejected backend sketch call. */
function sketchErr(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Local H/V inference for the ghost glyph (mirrors autoConstrain thresholds). */
function inferHVDraft(p0: Point2, p1: Point2): "H" | "V" | null {
  const HV = (5 * Math.PI) / 180;
  if (p0.x === p1.x && p0.y === p1.y) return null;
  const a = Math.atan2(p1.y - p0.y, p1.x - p0.x);
  const hDev = Math.min(Math.abs(a), Math.abs(Math.abs(a) - Math.PI));
  if (hDev <= HV) return "H";
  const vDev = Math.abs(Math.abs(a) - Math.PI / 2);
  if (vDev <= HV) return "V";
  return null;
}
