/*
 * RevolvePreview — the Level-1 revolve preview living in the engine's
 * interactionRoot (the revolve analogue of PreviewMesh + DragHandle). It owns
 * three plane-local objects, all under one group carrying the sketch-plane basis:
 *
 *   - candidate axis lines: the sketch LINE entities the user can pick as the
 *     axis of revolution (faint), drawn while in the axis-pick phase,
 *   - the hovered / chosen axis line (accent), and
 *   - the coarse lathe shell (translucent accent, DoubleSide, unlit) rebuilt from
 *     lathePreview.latheLocal whenever the angle changes during the drag.
 *
 * Geometry is authored in plane-local (u,v,w) coords; the group matrix (plane
 * basis) maps it to world, exactly as PreviewMesh does for the extrude prism.
 */
import * as THREE from "three";
import type { SketchPlane } from "@/ipc/types";
import { planeBasisMatrix } from "./sketchBasis";
import { palette } from "./palette";
import { latheLocal, type LatheAxis } from "@/tools/preview/lathePreview";

export interface RevolvePreviewDeps {
  root: THREE.Object3D; // interactionRoot
  invalidate: () => void;
}

/** One candidate axis line in plane (u,v). */
export interface AxisCandidate {
  a: [number, number];
  b: [number, number];
}

export class RevolvePreview {
  private readonly group = new THREE.Group();
  private readonly meshMat: THREE.MeshBasicMaterial;
  private readonly candMat: THREE.LineBasicMaterial;
  private readonly hoverMat: THREE.LineBasicMaterial;
  private readonly candidates: THREE.LineSegments;
  private readonly hoverLine: THREE.LineSegments;
  private mesh: THREE.Mesh | null = null;
  private readonly _basis = new THREE.Matrix4();

  constructor(private readonly deps: RevolvePreviewDeps) {
    this.group.name = "revolvePreview";
    this.group.matrixAutoUpdate = false;
    this.group.visible = false;

    this.meshMat = new THREE.MeshBasicMaterial({
      color: palette.hoverAccent(),
      transparent: true,
      opacity: 0.3,
      depthWrite: false,
      side: THREE.DoubleSide,
    });
    this.candMat = new THREE.LineBasicMaterial({ color: palette.bodyNeutral(), transparent: true, opacity: 0.85, depthTest: false });
    this.hoverMat = new THREE.LineBasicMaterial({ color: palette.selectedEdge(), depthTest: false });

    this.candidates = new THREE.LineSegments(new THREE.BufferGeometry(), this.candMat);
    this.candidates.renderOrder = 7;
    this.hoverLine = new THREE.LineSegments(new THREE.BufferGeometry(), this.hoverMat);
    this.hoverLine.renderOrder = 8;
    this.hoverLine.visible = false;

    this.group.add(this.candidates, this.hoverLine);
    deps.root.add(this.group);
  }

  /** Set the sketch plane basis (positions authored plane-local). */
  setPlane(plane: SketchPlane): void {
    planeBasisMatrix(plane, this._basis);
    this.group.matrix.copy(this._basis);
    this.group.matrixWorldNeedsUpdate = true;
    this.deps.invalidate();
  }

  /** Draw the pickable candidate axis lines (axis-pick phase). */
  setCandidates(lines: AxisCandidate[]): void {
    this.setSegments(this.candidates, lines);
    this.candidates.visible = lines.length > 0;
    this.deps.invalidate();
  }

  /** Highlight one axis segment (hovered candidate or the chosen axis), or clear. */
  setHover(seg: AxisCandidate | null): void {
    if (!seg) {
      this.hoverLine.visible = false;
      this.deps.invalidate();
      return;
    }
    this.setSegments(this.hoverLine, [seg]);
    this.hoverLine.visible = true;
    this.deps.invalidate();
  }

  /** Hide the candidate lines (an axis was chosen). */
  hideCandidates(): void {
    this.candidates.visible = false;
    this.deps.invalidate();
  }

  /** Rebuild the coarse lathe shell for `ring` swept around `axis` by `angleDeg`. */
  setLathe(ring: [number, number][], axis: LatheAxis, angleDeg: number): void {
    const { positions, indices } = latheLocal(ring, axis, angleDeg);
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(Float32Array.from(positions), 3));
    geo.setIndex(new THREE.BufferAttribute(Uint32Array.from(indices), 1));
    if (this.mesh) {
      this.mesh.geometry.dispose();
      this.mesh.geometry = geo;
    } else {
      this.mesh = new THREE.Mesh(geo, this.meshMat);
      this.mesh.renderOrder = 6;
      this.group.add(this.mesh);
    }
    this.deps.invalidate();
  }

  clearLathe(): void {
    if (this.mesh) {
      this.group.remove(this.mesh);
      this.mesh.geometry.dispose();
      this.mesh = null;
    }
    this.deps.invalidate();
  }

  setVisible(visible: boolean): void {
    this.group.visible = visible;
    this.deps.invalidate();
  }

  get visible(): boolean {
    return this.group.visible;
  }

  private setSegments(target: THREE.LineSegments, lines: AxisCandidate[]): void {
    const pts: number[] = [];
    for (const l of lines) pts.push(l.a[0], l.a[1], 0, l.b[0], l.b[1], 0);
    const geo = target.geometry;
    geo.setAttribute("position", new THREE.BufferAttribute(Float32Array.from(pts), 3));
    geo.setDrawRange(0, lines.length * 2);
    geo.computeBoundingSphere();
  }

  dispose(): void {
    this.clearLathe();
    this.candidates.geometry.dispose();
    this.hoverLine.geometry.dispose();
    this.meshMat.dispose();
    this.candMat.dispose();
    this.hoverMat.dispose();
    this.deps.root.remove(this.group);
  }
}
