/*
 * SketchObject — one sketch's presence in `sketchRoot` (F-WP6).
 *
 * Everything is authored in plane (u,v) coordinates inside a group carrying the
 * plane basis matrix (see sketchBasis), so local (u,v,0) → world (Z-up). Under
 * that group:
 *   - a low-alpha plane TINT quad (canvas-sketch token) + an adaptive plane grid
 *     (reused GridPlane, re-centered in plane-local coords),
 *   - committed entities as `Line2` (px line width, addons `lines/` — ships with
 *     three 0.185.1 for the WebGLRenderer), colored by constraint state,
 *   - endpoint/center markers as constant-size `THREE.Points`,
 *   - an in-progress PREVIEW channel (rubber-band) the tool machines drive.
 *
 * Line2 needs the material `resolution` in device pixels for correct width, so
 * the engine calls `update(...)` every frame with the viewport size.
 *
 * Line2 import note: `three/addons/lines/Line2.js` is WebGLRenderer-only (the
 * WebGPU build lives under `lines/webgpu/`). We are WebGL-default (F-WP4), so the
 * WebGL Line2 is correct here; a WebGPU sketch path is a later concern.
 */
import * as THREE from "three";
import { Line2 } from "three/examples/jsm/lines/Line2.js";
import { LineGeometry } from "three/examples/jsm/lines/LineGeometry.js";
import { LineMaterial } from "three/examples/jsm/lines/LineMaterial.js";
import type { SketchEntity, SketchPlane, SketchSolveStatus } from "@/ipc/types";
import { GridPlane } from "./GridPlane";
import { palette } from "./palette";
import { planeBasisMatrix, worldToPlanePoint } from "./sketchBasis";
import type { DraftEntity } from "@/tools/sketch/toolMachine";

const ARC_SEGMENTS = 64;
const LINE_WIDTH = 2;
const PREVIEW_WIDTH = 1.5;

/** Flat local xyz (z=0) polyline for an entity, in plane coords. */
export function entityPolyline(e: {
  type: SketchEntity["type"];
  p0?: [number, number];
  p1?: [number, number];
  center?: [number, number];
  radius?: number;
  start?: [number, number];
  end?: [number, number];
}): number[] {
  if (e.type === "Line" && e.p0 && e.p1) return [e.p0[0], e.p0[1], 0, e.p1[0], e.p1[1], 0];
  if (e.type === "Circle" && e.center && e.radius !== undefined) {
    const out: number[] = [];
    for (let i = 0; i <= ARC_SEGMENTS; i++) {
      const a = (i / ARC_SEGMENTS) * Math.PI * 2;
      out.push(e.center[0] + e.radius * Math.cos(a), e.center[1] + e.radius * Math.sin(a), 0);
    }
    return out;
  }
  if (e.type === "Arc" && e.center && e.radius !== undefined && e.start && e.end) {
    const a0 = Math.atan2(e.start[1] - e.center[1], e.start[0] - e.center[0]);
    let a1 = Math.atan2(e.end[1] - e.center[1], e.end[0] - e.center[0]);
    while (a1 <= a0) a1 += Math.PI * 2; // CCW sweep
    const out: number[] = [];
    for (let i = 0; i <= ARC_SEGMENTS; i++) {
      const a = a0 + ((a1 - a0) * i) / ARC_SEGMENTS;
      out.push(e.center[0] + e.radius * Math.cos(a), e.center[1] + e.radius * Math.sin(a), 0);
    }
    return out;
  }
  return [];
}

/** Endpoint/center markers of an entity, flat local xyz. */
function entityMarkers(e: SketchEntity): number[] {
  const out: number[] = [];
  const add = (p?: [number, number]) => p && out.push(p[0], p[1], 0);
  if (e.type === "Line") {
    add(e.p0);
    add(e.p1);
  } else if (e.type === "Circle") {
    add(e.center);
  } else if (e.type === "Arc") {
    add(e.center);
    add(e.start);
    add(e.end);
  } else if (e.type === "Point") {
    add(e.p0);
  }
  return out;
}

interface SketchObjectDeps {
  sketchRoot: THREE.Object3D;
  invalidate: () => void;
}

export class SketchObject {
  private readonly planeGroup = new THREE.Group();
  private readonly entityGroup = new THREE.Group();
  private readonly previewGroup = new THREE.Group();
  private readonly grid: GridPlane;
  private readonly tint: THREE.Mesh;
  private readonly points: THREE.Points;
  private readonly pointsMat: THREE.PointsMaterial;

  private plane: SketchPlane | null = null;
  private entities: SketchEntity[] = [];
  private status: SketchSolveStatus = "UnderConstrained";
  private selected = new Set<string>();

  // Shared line materials, by state.
  private readonly matUnder: LineMaterial;
  private readonly matFull: LineMaterial;
  private readonly matConflict: LineMaterial;
  private readonly matSelected: LineMaterial;
  private readonly matConstruction: LineMaterial;
  private readonly matPreview: LineMaterial;
  private readonly allMaterials: LineMaterial[];

  private readonly _basis = new THREE.Matrix4();
  private readonly _target = new THREE.Vector3();

  constructor(private readonly deps: SketchObjectDeps) {
    this.planeGroup.name = "sketchPlane";
    this.planeGroup.add(this.entityGroup, this.previewGroup);
    deps.sketchRoot.add(this.planeGroup);

    // Plane tint quad (large, low alpha) + adaptive grid, both plane-local.
    this.tint = new THREE.Mesh(
      new THREE.PlaneGeometry(4000, 4000),
      new THREE.MeshBasicMaterial({
        color: palette.sketchPlane(),
        transparent: true,
        opacity: 0.5,
        depthWrite: false,
        side: THREE.DoubleSide,
      }),
    );
    this.tint.renderOrder = -3;
    this.planeGroup.add(this.tint);

    this.grid = new GridPlane({ minor: palette.gridMinor(), major: palette.gridMajor(), clear: palette.sketchPlane() });
    this.grid.setVisible(true);
    this.planeGroup.add(this.grid.object3D);

    this.pointsMat = new THREE.PointsMaterial({
      color: palette.sketchFull(),
      size: 5,
      sizeAttenuation: false,
    });
    this.points = new THREE.Points(new THREE.BufferGeometry(), this.pointsMat);
    this.points.renderOrder = 4;
    this.entityGroup.add(this.points);

    const mk = (color: THREE.Color, opts: Partial<ConstructorParameters<typeof LineMaterial>[0]> = {}) =>
      new LineMaterial({ color: color.getHex(), linewidth: LINE_WIDTH, ...opts });
    this.matUnder = mk(palette.sketchUnder());
    this.matFull = mk(palette.sketchFull());
    this.matConflict = mk(palette.sketchConflict());
    this.matSelected = mk(palette.sketchSelected());
    this.matConstruction = mk(palette.sketchConstruction(), { dashed: true, dashSize: 3, gapSize: 2 });
    this.matPreview = mk(palette.sketchUnder(), { linewidth: PREVIEW_WIDTH, transparent: true, opacity: 0.9 });
    this.allMaterials = [
      this.matUnder,
      this.matFull,
      this.matConflict,
      this.matSelected,
      this.matConstruction,
      this.matPreview,
    ];
  }

  setVisible(visible: boolean): void {
    this.planeGroup.visible = visible;
    this.deps.invalidate();
  }

  /** Rebuild committed geometry from a session. */
  setSession(plane: SketchPlane, entities: SketchEntity[], status: SketchSolveStatus): void {
    this.plane = plane;
    this.entities = entities;
    this.status = status;
    planeBasisMatrix(plane, this._basis);
    this.planeGroup.matrixAutoUpdate = false;
    this.planeGroup.matrix.copy(this._basis);
    this.planeGroup.matrixWorldNeedsUpdate = true;
    this.rebuildEntities();
    this.rebuildPoints();
    this.deps.invalidate();
  }

  /** Replace the rubber-band preview (drafts in the same plane). */
  setPreview(drafts: DraftEntity[]): void {
    for (const c of [...this.previewGroup.children]) this.disposeLine(c);
    this.previewGroup.clear();
    for (const d of drafts) {
      const positions = entityPolyline({
        type: d.type,
        p0: d.p0 ? [d.p0.x, d.p0.y] : undefined,
        p1: d.p1 ? [d.p1.x, d.p1.y] : undefined,
        center: d.center ? [d.center.x, d.center.y] : undefined,
        radius: d.radius,
        start: d.start ? [d.start.x, d.start.y] : undefined,
        end: d.end ? [d.end.x, d.end.y] : undefined,
      });
      if (positions.length < 6) continue;
      const line = this.buildLine(positions, d.construction ? this.matConstruction : this.matPreview);
      this.previewGroup.add(line);
    }
    this.deps.invalidate();
  }

  /** Recolor from the current selection (sketch entity ids). */
  setSelection(selectedIds: Iterable<string>): void {
    this.selected = new Set(selectedIds);
    this.rebuildEntities();
    this.deps.invalidate();
  }

  private statusMaterial(): LineMaterial {
    switch (this.status) {
      case "FullyConstrained":
        return this.matFull;
      case "OverConstrained":
      case "Conflicting":
        return this.matConflict;
      default:
        return this.matUnder;
    }
  }

  private rebuildEntities(): void {
    // Remove all lines (keep the Points object).
    for (const c of [...this.entityGroup.children]) {
      if (c === this.points) continue;
      this.disposeLine(c);
      this.entityGroup.remove(c);
    }
    const statusMat = this.statusMaterial();
    for (const e of this.entities) {
      const positions = entityPolyline(e);
      if (positions.length < 6) continue;
      const mat = this.selected.has(e.id)
        ? this.matSelected
        : e.construction
          ? this.matConstruction
          : statusMat;
      this.entityGroup.add(this.buildLine(positions, mat));
    }
  }

  private rebuildPoints(): void {
    const flat: number[] = [];
    for (const e of this.entities) flat.push(...entityMarkers(e));
    const geo = this.points.geometry;
    geo.setAttribute("position", new THREE.Float32BufferAttribute(flat, 3));
    geo.computeBoundingSphere();
  }

  private buildLine(positions: number[], mat: LineMaterial): Line2 {
    const geo = new LineGeometry();
    geo.setPositions(positions);
    const line = new Line2(geo, mat);
    if (mat.dashed) line.computeLineDistances();
    line.renderOrder = 3;
    return line;
  }

  private disposeLine(obj: THREE.Object3D): void {
    const l = obj as Line2;
    l.geometry?.dispose();
    // Materials are shared — never disposed here.
  }

  /** Per-frame: Line2 resolution + adaptive grid re-center (plane-local). */
  update(width: number, height: number, cameraTarget: THREE.Vector3, cameraDistance: number): void {
    for (const m of this.allMaterials) m.resolution.set(width, height);
    if (this.plane) {
      const local = worldToPlanePoint(this.plane, this._target.copy(cameraTarget));
      this.grid.update(new THREE.Vector3(local.x, local.y, 0), cameraDistance);
    }
  }

  dispose(): void {
    for (const c of [...this.entityGroup.children]) if (c !== this.points) this.disposeLine(c);
    for (const c of [...this.previewGroup.children]) this.disposeLine(c);
    this.points.geometry.dispose();
    this.pointsMat.dispose();
    (this.tint.geometry as THREE.BufferGeometry).dispose();
    (this.tint.material as THREE.Material).dispose();
    this.grid.dispose();
    for (const m of this.allMaterials) m.dispose();
    this.deps.sketchRoot.remove(this.planeGroup);
  }
}
