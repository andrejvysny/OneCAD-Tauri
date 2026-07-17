/*
 * PreviewMesh — the Level-1 extrude preview (NEW_SPEC §15) living in the engine's
 * interactionRoot. A UNIT prism (built once per profile from the region
 * triangulation, plane-local) sits under a group carrying the sketch-plane basis;
 * the live drag only writes `mesh.scale.z` / `mesh.position.z`, so each pointer
 * frame is ZERO-allocation. Translucent accent material (design palette) so the
 * exact L2 body can swap in underneath while L1 stays on top.
 */
import * as THREE from "three";
import type { SketchPlane } from "@/ipc/types";
import { planeBasisMatrix } from "./sketchBasis";
import { unitPrismGeometry, type PrismProfile } from "@/tools/preview/prismPreview";
import { palette } from "./palette";

export interface PreviewMeshDeps {
  root: THREE.Object3D; // interactionRoot
  invalidate: () => void;
}

export class PreviewMesh {
  private readonly group = new THREE.Group();
  private readonly material: THREE.MeshBasicMaterial;
  private mesh: THREE.Mesh | null = null;
  private readonly _basis = new THREE.Matrix4();

  constructor(private readonly deps: PreviewMeshDeps) {
    this.group.name = "extrudePreview";
    this.group.matrixAutoUpdate = false;
    this.material = new THREE.MeshBasicMaterial({
      color: palette.hoverAccent(),
      transparent: true,
      opacity: 0.3,
      depthWrite: false,
      side: THREE.DoubleSide,
    });
    deps.root.add(this.group);
  }

  /** Build the unit prism for a profile on `plane` (rebuild only when the profile changes). */
  setProfile(plane: SketchPlane, profile: PrismProfile): void {
    this.disposeMesh();
    const { positions, indices } = unitPrismGeometry(profile);
    const geo = new THREE.BufferGeometry();
    geo.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    geo.setIndex(new THREE.BufferAttribute(indices, 1));
    this.mesh = new THREE.Mesh(geo, this.material);
    this.mesh.renderOrder = 6; // over bodies + highlights
    this.group.add(this.mesh);

    planeBasisMatrix(plane, this._basis);
    this.group.matrix.copy(this._basis);
    this.group.matrixWorldNeedsUpdate = true;
    this.deps.invalidate();
  }

  /**
   * Set the live depth. Symmetric grows both ways (span 2·|depth|, centred on the
   * plane); a negative depth extrudes the other side (drag-through-zero flip).
   */
  setDepth(depth: number, symmetric: boolean): void {
    if (!this.mesh) return;
    if (symmetric) {
      const h = Math.abs(depth) || 1e-4;
      this.mesh.scale.z = 2 * h;
      this.mesh.position.z = -h;
    } else {
      this.mesh.scale.z = Math.abs(depth) < 1e-4 ? 1e-4 : depth;
      this.mesh.position.z = 0;
    }
    this.deps.invalidate();
  }

  setVisible(visible: boolean): void {
    this.group.visible = visible;
    this.deps.invalidate();
  }

  get visible(): boolean {
    return this.group.visible && this.mesh !== null;
  }

  private disposeMesh(): void {
    if (this.mesh) {
      this.group.remove(this.mesh);
      this.mesh.geometry.dispose();
      this.mesh = null;
    }
  }

  dispose(): void {
    this.disposeMesh();
    this.material.dispose();
    this.deps.root.remove(this.group);
  }
}
