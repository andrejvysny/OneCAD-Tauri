/*
 * Camera rig: a PerspectiveCamera + OrthographicCamera pair sharing one logical
 * state (target, yaw, pitch, distance). The active camera is applied from that
 * state each change. Switching persp⇄ortho preserves apparent size at the pivot
 * distance via `orthoHalfHeight` (ortho half-height = distance * tan(fov/2)), so
 * geometry at the target keeps the same on-screen size across the switch.
 *
 * HARD INVARIANT: both cameras use up = (0,0,1). The world is Z-up right-handed
 * and mesh buffers are uploaded verbatim (see README.md). Never rotate content
 * to fake a Y-up world.
 */
import * as THREE from "three";

export type ProjectionKind = "persp" | "ortho";

const UP = new THREE.Vector3(0, 0, 1);
// At the poles (top/bottom) world Z is parallel to the view direction, so the
// Z-up lookAt degenerates. There we use world +Y so the top view reads
// conventionally (X right, Y up) instead of an arbitrary rotation.
const POLE_UP = new THREE.Vector3(0, 1, 0);
const POLE_COS = 0.99999;
const NEAR = 0.1;
const FAR = 100_000;

/**
 * Pure: orthographic half-height that matches a perspective camera's apparent
 * size at `distance`. halfH = distance * tan(fovDeg/2).
 */
export function orthoHalfHeight(distance: number, fovDeg: number): number {
  return distance * Math.tan((fovDeg * Math.PI) / 360);
}

export interface CameraState {
  target: THREE.Vector3;
  /** Eye offset from target (target → camera). */
  offset: THREE.Vector3;
  distance: number;
}

export class CameraRig {
  readonly persp: THREE.PerspectiveCamera;
  readonly ortho: THREE.OrthographicCamera;
  private kind: ProjectionKind = "persp";
  private aspect = 1;
  readonly fovDeg: number;

  constructor(fovDeg = 76) {
    this.fovDeg = fovDeg;
    this.persp = new THREE.PerspectiveCamera(fovDeg, 1, NEAR, FAR);
    this.persp.up.copy(UP);
    this.ortho = new THREE.OrthographicCamera(-1, 1, 1, -1, NEAR, FAR);
    this.ortho.up.copy(UP);
  }

  get projection(): ProjectionKind {
    return this.kind;
  }

  getCamera(): THREE.Camera {
    return this.kind === "persp" ? this.persp : this.ortho;
  }

  setAspect(aspect: number): void {
    this.aspect = aspect > 0 ? aspect : 1;
    this.persp.aspect = this.aspect;
    this.persp.updateProjectionMatrix();
  }

  setProjection(kind: ProjectionKind): void {
    this.kind = kind;
  }

  /**
   * Place the active camera. `offset` is the target→camera vector (length =
   * distance). In ortho, the frustum half-height is derived from `distance` so
   * the perspective apparent size is preserved.
   */
  apply(target: THREE.Vector3, offset: THREE.Vector3, distance: number): void {
    const cam = this.getCamera();
    cam.position.copy(target).add(offset);
    const len = offset.length() || 1;
    const nearPole = Math.abs(offset.z) / len > POLE_COS;
    cam.up.copy(nearPole ? POLE_UP : UP);
    cam.lookAt(target);
    if (this.kind === "ortho") {
      const halfH = orthoHalfHeight(distance, this.fovDeg);
      const halfW = halfH * this.aspect;
      this.ortho.left = -halfW;
      this.ortho.right = halfW;
      this.ortho.top = halfH;
      this.ortho.bottom = -halfH;
      this.ortho.updateProjectionMatrix();
    }
    cam.updateMatrixWorld();
  }
}
