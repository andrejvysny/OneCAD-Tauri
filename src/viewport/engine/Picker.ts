/*
 * Picker — rAF-coalesced raycast over body face meshes + edge LineSegments.
 *
 * Hover follows the pointer (coalesced to one raycast per frame) and fires only
 * when the hit CHANGES, so an idle pointer schedules no frames (render-on-demand
 * preserved). Edges win over faces within a screen-space tolerance: the ray's
 * `Line.threshold` is scaled to ~6px at the focus distance, and an edge hit is
 * preferred when it is no farther than the face hit (+bias) — so a boundary edge
 * lying on a face surface is selectable, while an occluded edge is not.
 *
 * (Plan notes Line2 for edges; current edges are plain LineSegments, so we drive
 * `raycaster.params.Line.threshold` in world units, derived from px + camera.)
 *
 * A hit resolves through the mesh registry: triangle index → face id, or segment
 * index → edge id (both lazy TopoKey/ElementId decode). Click captures the world
 * anchor for a future AcquireElementIds promotion. The engine owns store wiring
 * via the onHover/onPick callbacks (this class stays store-agnostic).
 */
import * as THREE from "three";
import type { MeshEntry } from "../mesh/meshRegistry";
import { getEntry } from "../mesh/meshRegistry";

export interface PickHit {
  bodyId: string;
  kind: "face" | "edge";
  topoKey: string;
  elementId?: string;
  worldPos: THREE.Vector3;
  /** Local hint for previews/promotion (face normal in world space). */
  surfaceHint?: { normal?: [number, number, number] };
}

export interface PickModifiers {
  shift: boolean;
  meta: boolean;
}

const EDGE_PICK_PX = 6;
const DRAG_PX = 4; // pointer travel over this ⇒ a drag, not a click

// ── Pure helpers (unit-tested) ──────────────────────────────────────────────

/** World units spanning `px` screen pixels at `focusDistance`, for both cameras. */
export function linePickThreshold(
  camera: THREE.Camera,
  viewportHeight: number,
  focusDistance: number,
  px = EDGE_PICK_PX,
): number {
  const h = Math.max(viewportHeight, 1);
  if ((camera as THREE.PerspectiveCamera).isPerspectiveCamera) {
    const fov = (camera as THREE.PerspectiveCamera).fov;
    const worldPerPx = (2 * focusDistance * Math.tan((fov * Math.PI) / 360)) / h;
    return px * worldPerPx;
  }
  const oc = camera as THREE.OrthographicCamera;
  const worldPerPx = (oc.top - oc.bottom) / h;
  return px * worldPerPx;
}

/**
 * Prefer the edge hit when it is no farther than the face hit plus `bias`
 * (edges lie on face boundaries, so distances tie; bias covers float wobble and
 * lets a coincident boundary edge win). An occluded edge (much farther) loses.
 */
export function choosePreferredHit(
  faceHit: THREE.Intersection | null,
  edgeHit: THREE.Intersection | null,
  bias: number,
): { hit: THREE.Intersection; kind: "face" | "edge" } | null {
  if (edgeHit && (!faceHit || edgeHit.distance <= faceHit.distance + bias)) {
    return { hit: edgeHit, kind: "edge" };
  }
  if (faceHit) return { hit: faceHit, kind: "face" };
  return null;
}

/** Resolve a chosen intersection into a PickHit via the mesh registry. */
export function resolvePick(
  hit: THREE.Intersection,
  kind: "face" | "edge",
  lookup: (bodyId: string) => MeshEntry | undefined = getEntry,
): PickHit | null {
  const bodyId = hit.object.userData.bodyId as string | undefined;
  if (!bodyId) return null;
  const entry = lookup(bodyId);
  if (!entry) return null;

  let id: string | null = null;
  let surfaceHint: PickHit["surfaceHint"];
  if (kind === "face") {
    if (hit.faceIndex == null) return null;
    id = entry.faceIndex.idAt(hit.faceIndex);
    if (hit.face) {
      const n = hit.face.normal
        .clone()
        .applyNormalMatrix(new THREE.Matrix3().getNormalMatrix(hit.object.matrixWorld))
        .normalize();
      surfaceHint = { normal: [n.x, n.y, n.z] };
    }
  } else {
    if (hit.index == null || !entry.edgeIndex) return null;
    id = entry.edgeIndex.idAt(hit.index >> 1); // 2 verts per segment
  }
  if (id == null) return null;

  const isElementId = entry.view.idsHaveElementIds;
  return {
    bodyId,
    kind,
    topoKey: id,
    elementId: isElementId ? id : undefined,
    worldPos: hit.point.clone(),
    surfaceHint,
  };
}

/** Stable key for hover-change detection (internal; separator avoids id chars). */
export function pickKey(hit: PickHit | null): string | null {
  return hit ? `${hit.bodyId}/${hit.kind}/${hit.topoKey}` : null;
}

// ── Controller ──────────────────────────────────────────────────────────────

export interface PickerDeps {
  canvas: HTMLCanvasElement;
  getCamera: () => THREE.Camera;
  getRoot: () => THREE.Object3D; // bodiesRoot
  getViewportHeight: () => number;
  getFocusDistance: () => number;
  invalidate: () => void;
  /** Picking is only live in this mode (model + select); returns false in sketch mode. */
  isActive: () => boolean;
  onHover: (hit: PickHit | null) => void;
  onPick: (hit: PickHit | null, mods: PickModifiers) => void;
}

export class Picker {
  private readonly raycaster = new THREE.Raycaster();
  private lastHoverKey: string | null = null;

  private pendingMove: PointerEvent | null = null;
  private moveScheduled = false;

  private downX = 0;
  private downY = 0;
  private downButton = -1;
  private moved = false;

  constructor(private readonly deps: PickerDeps) {
    const c = deps.canvas;
    c.addEventListener("pointermove", this.onPointerMove);
    c.addEventListener("pointerdown", this.onPointerDown);
    c.addEventListener("pointerup", this.onPointerUp);
    c.addEventListener("pointerleave", this.onPointerLeave);
  }

  /** Orbit hit-test seam: is there pickable geometry under this client point? */
  hasHitAt(clientX: number, clientY: number): boolean {
    if (!this.deps.isActive()) return false;
    return this.raycast(clientX, clientY) != null;
  }

  /**
   * One-shot pick regardless of `isActive` — for tools that own picking directly
   * (boolean tool-body pick). Returns the resolved hit or null.
   */
  probe(clientX: number, clientY: number): PickHit | null {
    return this.pickAt(clientX, clientY);
  }

  // ── pointer handlers ──

  private onPointerMove = (e: PointerEvent): void => {
    if (!this.deps.isActive()) return;
    if (e.buttons !== 0) {
      this.moved = true;
      return; // no hover while dragging
    }
    this.pendingMove = e;
    if (this.moveScheduled) return;
    this.moveScheduled = true;
    requestAnimationFrame(() => {
      this.moveScheduled = false;
      const ev = this.pendingMove;
      this.pendingMove = null;
      if (!ev || !this.deps.isActive()) return;
      this.updateHover(ev.clientX, ev.clientY);
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
    if (!wasClick || !this.deps.isActive()) return;
    const hit = this.pickAt(e.clientX, e.clientY);
    this.deps.onPick(hit, { shift: e.shiftKey, meta: e.metaKey || e.ctrlKey });
  };

  private onPointerLeave = (): void => {
    if (this.lastHoverKey !== null) {
      this.lastHoverKey = null;
      this.deps.onHover(null);
      this.deps.invalidate();
    }
  };

  // ── raycasting ──

  private updateHover(clientX: number, clientY: number): void {
    const hit = this.pickAt(clientX, clientY);
    const key = pickKey(hit);
    if (key === this.lastHoverKey) return; // unchanged ⇒ no repaint (idle stays quiet)
    this.lastHoverKey = key;
    this.deps.onHover(hit);
    this.deps.invalidate();
  }

  private pickAt(clientX: number, clientY: number): PickHit | null {
    const chosen = this.raycast(clientX, clientY);
    return chosen ? resolvePick(chosen.hit, chosen.kind) : null;
  }

  /** Raycast faces + edges and apply the edge-preference rule. */
  private raycast(
    clientX: number,
    clientY: number,
  ): { hit: THREE.Intersection; kind: "face" | "edge" } | null {
    const rect = this.deps.canvas.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const ndc = new THREE.Vector2(
      ((clientX - rect.left) / rect.width) * 2 - 1,
      -(((clientY - rect.top) / rect.height) * 2 - 1),
    );
    const camera = this.deps.getCamera();
    this.raycaster.setFromCamera(ndc, camera);
    const threshold = linePickThreshold(
      camera,
      this.deps.getViewportHeight(),
      this.deps.getFocusDistance(),
    );
    this.raycaster.params.Line = { threshold };

    const faceObjects: THREE.Object3D[] = [];
    const edgeObjects: THREE.Object3D[] = [];
    this.deps.getRoot().traverse((o) => {
      if (!o.visible) return;
      if (o.userData.kind === "face") faceObjects.push(o);
      else if (o.userData.kind === "edge") edgeObjects.push(o);
    });

    const faceHit = this.raycaster.intersectObjects(faceObjects, false)[0] ?? null;
    const edgeHit = this.raycaster.intersectObjects(edgeObjects, false)[0] ?? null;
    return choosePreferredHit(faceHit, edgeHit, threshold);
  }

  dispose(): void {
    const c = this.deps.canvas;
    c.removeEventListener("pointermove", this.onPointerMove);
    c.removeEventListener("pointerdown", this.onPointerDown);
    c.removeEventListener("pointerup", this.onPointerUp);
    c.removeEventListener("pointerleave", this.onPointerLeave);
    this.lastHoverKey = null;
    this.pendingMove = null;
  }
}
