/*
 * Sketch plane ↔ world coordinate mapping (PURE math, Z-up right-handed world).
 *
 * A sketch authors geometry in 2D plane coordinates (u, v). The plane carries an
 * origin + orthonormal basis (xAxis, yAxis, normal) in WORLD space. Mapping:
 *
 *   world = origin + u·xAxis + v·yAxis
 *   u = (world − origin)·xAxis,  v = (world − origin)·yAxis
 *
 * The `Matrix4` returned by `planeBasisMatrix` transforms local (u, v, 0) →
 * world, so a scene group carrying it lets entity geometry be authored directly
 * in plane coordinates (see SketchObject). Never rotate content to fake Y-up —
 * the basis lives in the plane definition (README HARD INVARIANT).
 */
import * as THREE from "three";
import type { SketchPlane } from "@/ipc/types";

export interface Point2 {
  x: number;
  y: number;
}

const _o = new THREE.Vector3();
const _x = new THREE.Vector3();
const _y = new THREE.Vector3();
const _n = new THREE.Vector3();
const _d = new THREE.Vector3();

function axes(plane: SketchPlane): {
  origin: THREE.Vector3;
  x: THREE.Vector3;
  y: THREE.Vector3;
  n: THREE.Vector3;
} {
  return {
    origin: _o.fromArray(plane.origin),
    x: _x.fromArray(plane.xAxis),
    y: _y.fromArray(plane.yAxis),
    n: _n.fromArray(plane.normal),
  };
}

/** local (u, v, 0) → world basis matrix (columns xAxis, yAxis, normal; +origin). */
export function planeBasisMatrix(plane: SketchPlane, out = new THREE.Matrix4()): THREE.Matrix4 {
  const { origin, x, y, n } = axes(plane);
  out.makeBasis(x, y, n);
  out.setPosition(origin);
  return out;
}

/** Plane point (u, v) → world position. */
export function planePointToWorld(
  plane: SketchPlane,
  p: Point2,
  out = new THREE.Vector3(),
): THREE.Vector3 {
  const { origin, x, y } = axes(plane);
  return out.copy(origin).addScaledVector(x, p.x).addScaledVector(y, p.y);
}

/** World position → plane point (u, v). Assumes an orthonormal plane basis. */
export function worldToPlanePoint(plane: SketchPlane, world: THREE.Vector3): Point2 {
  const { origin, x, y } = axes(plane);
  _d.copy(world).sub(origin);
  return { x: _d.dot(x), y: _d.dot(y) };
}

/** THREE.Plane through the sketch plane (normal + origin) for raycasting. */
export function planeGeometry(plane: SketchPlane, out = new THREE.Plane()): THREE.Plane {
  const { origin, n } = axes(plane);
  return out.setFromNormalAndCoplanarPoint(n, origin);
}
