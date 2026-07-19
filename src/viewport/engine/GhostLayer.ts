/*
 * GhostLayer — the Level-1 preview for the "duplicate a body" tools
 * (LinearPattern / CircularPattern / MirrorBody). It instances a SOURCE body's
 * existing registry geometry as translucent clones at a set of transforms
 * (translate / rotate / mirror), living under the engine's interactionRoot.
 *
 * This is the cheap-and-honest preview: no re-modelling, just reused geometry at
 * offsets (the exact fused body arrives from the backend regen on commit). The
 * transform MATH is pure (tools/preview/patternPreview); this layer only builds
 * the THREE.Matrix4 per descriptor and manages the clone meshes' lifetime.
 *
 * Geometry is registry-owned (zero-copy), so a clone Mesh is disposed by removing
 * it — never by disposing the shared geometry. The single translucent material is
 * unlit (MeshBasicMaterial) so a mirror's negative-determinant matrix (flipped
 * winding) still renders correctly with DoubleSide.
 */
import * as THREE from "three";
import type { MeshEntry } from "../mesh/meshRegistry";
import type { GhostTransform } from "@/tools/preview/patternPreview";
import { palette } from "./palette";

export interface GhostLayerDeps {
  root: THREE.Object3D; // interactionRoot
  invalidate: () => void;
}

/** Build a THREE.Matrix4 for one ghost transform descriptor. */
export function ghostMatrix(t: GhostTransform): THREE.Matrix4 {
  const m = new THREE.Matrix4();
  if (t.kind === "translate") {
    m.makeTranslation(t.offset[0], t.offset[1], t.offset[2]);
    return m;
  }
  if (t.kind === "rotate") {
    const axis = new THREE.Vector3(t.axis[0], t.axis[1], t.axis[2]).normalize();
    const origin = new THREE.Vector3(t.origin[0], t.origin[1], t.origin[2]);
    // T(origin) · R(axis, θ) · T(−origin)
    const rot = new THREE.Matrix4().makeRotationAxis(axis, t.angleRad);
    const toOrigin = new THREE.Matrix4().makeTranslation(origin.x, origin.y, origin.z);
    const fromOrigin = new THREE.Matrix4().makeTranslation(-origin.x, -origin.y, -origin.z);
    return toOrigin.multiply(rot).multiply(fromOrigin);
  }
  // mirror: Householder reflection across the plane through `point` with `normal`.
  const n = new THREE.Vector3(t.normal[0], t.normal[1], t.normal[2]).normalize();
  const d = n.x * t.point[0] + n.y * t.point[1] + n.z * t.point[2];
  // Linear part L = I − 2·n·nᵀ ; translation = 2·d·n (plane offset from origin).
  // prettier-ignore
  m.set(
    1 - 2 * n.x * n.x,    -2 * n.x * n.y,    -2 * n.x * n.z, 2 * d * n.x,
       -2 * n.y * n.x, 1 - 2 * n.y * n.y,    -2 * n.y * n.z, 2 * d * n.y,
       -2 * n.z * n.x,    -2 * n.z * n.y, 1 - 2 * n.z * n.z, 2 * d * n.z,
                    0,                 0,                 0,           1,
  );
  return m;
}

export class GhostLayer {
  private readonly group = new THREE.Group();
  private readonly material: THREE.MeshBasicMaterial;
  private meshes: THREE.Mesh[] = [];

  constructor(private readonly deps: GhostLayerDeps) {
    this.group.name = "ghostLayer";
    this.group.visible = false;
    this.material = new THREE.MeshBasicMaterial({
      color: palette.hoverAccent(),
      transparent: true,
      opacity: 0.28,
      depthWrite: false,
      side: THREE.DoubleSide,
    });
    deps.root.add(this.group);
  }

  /** Show translucent clones of `entry`'s geometry at each transform. */
  show(entry: MeshEntry, transforms: GhostTransform[]): void {
    this.clearMeshes();
    for (const t of transforms) {
      const mesh = new THREE.Mesh(entry.geometry, this.material);
      mesh.matrixAutoUpdate = false;
      mesh.matrix.copy(ghostMatrix(t));
      mesh.renderOrder = 6;
      this.group.add(mesh);
      this.meshes.push(mesh);
    }
    this.group.visible = this.meshes.length > 0;
    this.deps.invalidate();
  }

  hide(): void {
    this.clearMeshes();
    this.group.visible = false;
    this.deps.invalidate();
  }

  get visible(): boolean {
    return this.group.visible;
  }

  get instanceCount(): number {
    return this.meshes.length;
  }

  private clearMeshes(): void {
    for (const m of this.meshes) this.group.remove(m);
    this.meshes = [];
  }

  dispose(): void {
    this.clearMeshes();
    this.material.dispose();
    this.deps.root.remove(this.group);
  }
}
