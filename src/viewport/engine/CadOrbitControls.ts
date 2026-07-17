/*
 * CadOrbitControls — a small custom orbit controller (no three examples dep).
 *
 *   LMB / 1-finger drag  = turntable orbit: yaw about WORLD Z, pitch clamped to
 *                          ±(90° − ε). Orbit is SUPPRESSED when the drag starts on
 *                          pickable geometry (that gesture is a selection click);
 *                          dragging from empty space orbits (`hitTest` seam).
 *   MMB / 2-finger drag  = pan in the view plane.
 *   Wheel / pinch        = zoom-to-cursor dolly (persp) / frustum zoom (ortho).
 *   Home                 = animated iso view (250ms).
 *   Fit                  = frame the scene bbox (animated).
 *
 * No damping in v1. Camera state is (target, yaw, pitch, distance); the rig maps
 * it to the active camera. Math helpers are pure and unit-tested.
 */
import * as THREE from "three";
import type { CameraRig } from "./CameraRig";

// ---- Pure math helpers (unit-tested) -------------------------------------

const HALF_PI = Math.PI / 2;

/** Clamp pitch to just inside ±90° so the Z-up lookAt never degenerates. */
export function clampPitch(pitch: number, eps = 1e-3): number {
  const lim = HALF_PI - eps;
  return Math.max(-lim, Math.min(lim, pitch));
}

/** Turntable spherical → target→camera offset (yaw about Z, pitch from XY plane). */
export function sphericalToOffset(
  yaw: number,
  pitch: number,
  radius: number,
): THREE.Vector3 {
  const cp = Math.cos(pitch);
  return new THREE.Vector3(
    radius * cp * Math.cos(yaw),
    radius * cp * Math.sin(yaw),
    radius * Math.sin(pitch),
  );
}

/**
 * Zoom about a world point: scale camera position and target toward `cursor` by
 * `factor`. The camera→target vector keeps its direction (distance *= factor)
 * and the point under the cursor stays fixed on screen.
 */
export function zoomToCursor(
  camPos: THREE.Vector3,
  target: THREE.Vector3,
  cursor: THREE.Vector3,
  factor: number,
): { camPos: THREE.Vector3; target: THREE.Vector3 } {
  const lerpToward = (p: THREE.Vector3) =>
    new THREE.Vector3().copy(cursor).lerp(p, factor);
  return { camPos: lerpToward(camPos), target: lerpToward(target) };
}

/** Shortest-path angular lerp (handles wraparound). */
export function shortestAngleLerp(a: number, b: number, t: number): number {
  let d = (b - a) % (Math.PI * 2);
  if (d > Math.PI) d -= Math.PI * 2;
  if (d < -Math.PI) d += Math.PI * 2;
  return a + d * t;
}

export function easeInOutCubic(t: number): number {
  return t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2;
}

// ---- Controller ----------------------------------------------------------

const ORBIT_SPEED = 0.008;
const PAN_SENS = 1.4;
const ZOOM_SENS = 0.0015;
const TWEEN_MS = 250;
const ISO_YAW = -Math.PI / 4;
const ISO_PITCH = Math.atan(1 / Math.SQRT2); // ~35.26°

interface TweenTarget {
  yaw: number;
  pitch: number;
  distance: number;
  target: THREE.Vector3;
}

interface Tween {
  from: TweenTarget;
  to: TweenTarget;
  start: number;
  dur: number;
}

export interface OrbitOptions {
  rig: CameraRig;
  element: HTMLElement;
  onChange: () => void;
  /** Scene bounds for Fit; null when empty. */
  getBounds: () => THREE.Box3 | null;
  /**
   * Orbit gating: is there pickable geometry under this client point? When it
   * returns true on LMB-down, orbit is suppressed for that gesture (a selection
   * click). Absent ⇒ LMB always orbits (previous behaviour).
   */
  hitTest?: (clientX: number, clientY: number) => boolean;
}

export class CadOrbitControls {
  private readonly rig: CameraRig;
  private readonly el: HTMLElement;
  private readonly onChange: () => void;
  private readonly getBounds: () => THREE.Box3 | null;
  private readonly hitTest: ((x: number, y: number) => boolean) | null;

  target = new THREE.Vector3(0, 0, 0);
  yaw = ISO_YAW;
  pitch = ISO_PITCH;
  distance = 260;

  private readonly pointers = new Map<number, THREE.Vector2>();
  private button = -1;
  private lastPinch = 0;
  private tween: Tween | null = null;
  /** True while an LMB gesture began on geometry (orbit suppressed). */
  private orbitSuppressed = false;
  /** Sticky suppression of LMB orbit (sketch drawing tools own LMB). */
  private lmbOrbitSuppressed = false;

  constructor(opts: OrbitOptions) {
    this.rig = opts.rig;
    this.el = opts.element;
    this.onChange = opts.onChange;
    this.getBounds = opts.getBounds;
    this.hitTest = opts.hitTest ?? null;
    this.el.addEventListener("pointerdown", this.onPointerDown);
    this.el.addEventListener("pointermove", this.onPointerMove);
    this.el.addEventListener("pointerup", this.onPointerUp);
    this.el.addEventListener("pointercancel", this.onPointerUp);
    this.el.addEventListener("wheel", this.onWheel, { passive: false });
    this.el.addEventListener("contextmenu", this.preventContext);
  }

  getDistance(): number {
    return this.distance;
  }

  getTarget(): THREE.Vector3 {
    return this.target;
  }

  /** target→camera direction (unit), for the view label. */
  getViewDirection(): THREE.Vector3 {
    return sphericalToOffset(this.yaw, this.pitch, 1);
  }

  /** Push current state to the rig (no notification). */
  applyToRig(): void {
    const offset = sphericalToOffset(this.yaw, this.pitch, this.distance);
    this.rig.apply(this.target, offset, this.distance);
  }

  private commit(): void {
    this.applyToRig();
    this.onChange();
  }

  // ---- Pointer handling ----

  private onPointerDown = (e: PointerEvent): void => {
    this.el.setPointerCapture(e.pointerId);
    this.pointers.set(e.pointerId, new THREE.Vector2(e.clientX, e.clientY));
    if (this.pointers.size === 1) {
      this.button = e.button;
      // Suppress orbit for an LMB gesture that starts on geometry (selection).
      this.orbitSuppressed =
        e.button === 0 && this.hitTest !== null && this.hitTest(e.clientX, e.clientY);
    }
    if (this.pointers.size === 2) this.lastPinch = this.pinchDistance();
    this.tween = null; // user input cancels any animation
  };

  private onPointerMove = (e: PointerEvent): void => {
    const prev = this.pointers.get(e.pointerId);
    if (!prev) return;
    const dx = e.clientX - prev.x;
    const dy = e.clientY - prev.y;
    prev.set(e.clientX, e.clientY);

    if (this.pointers.size >= 2) {
      this.pan(dx, dy);
      this.applyPinchZoom(this.pinchDistance());
      return;
    }
    // Middle button pans; left button orbits.
    if (this.button === 1) this.pan(dx, dy);
    else this.orbit(dx, dy);
  };

  private onPointerUp = (e: PointerEvent): void => {
    this.pointers.delete(e.pointerId);
    if (this.el.hasPointerCapture(e.pointerId)) {
      this.el.releasePointerCapture(e.pointerId);
    }
    if (this.pointers.size < 2) this.lastPinch = 0;
    if (this.pointers.size === 0) {
      this.button = -1;
      this.orbitSuppressed = false;
    }
  };

  private onWheel = (e: WheelEvent): void => {
    e.preventDefault();
    const factor = clampFactor(Math.exp(e.deltaY * ZOOM_SENS));
    this.zoomAtScreen(e.clientX, e.clientY, factor);
  };

  private preventContext = (e: Event): void => e.preventDefault();

  // ---- Gestures ----

  /** Sticky: while true, an LMB drag never orbits (sketch tools own LMB). */
  setLmbOrbitSuppressed(suppressed: boolean): void {
    this.lmbOrbitSuppressed = suppressed;
  }

  private orbit(dx: number, dy: number): void {
    if (this.orbitSuppressed || this.lmbOrbitSuppressed) return; // gesture started on geometry / sketch tool
    this.yaw -= dx * ORBIT_SPEED;
    this.pitch = clampPitch(this.pitch + dy * ORBIT_SPEED);
    this.commit();
  }

  private pan(dx: number, dy: number): void {
    const cam = this.rig.getCamera();
    const right = new THREE.Vector3().setFromMatrixColumn(cam.matrixWorld, 0);
    const up = new THREE.Vector3().setFromMatrixColumn(cam.matrixWorld, 1);
    const scale = (this.distance * PAN_SENS) / Math.max(this.el.clientHeight, 1);
    this.target.addScaledVector(right, -dx * scale);
    this.target.addScaledVector(up, dy * scale);
    this.commit();
  }

  /** Wheel/pinch zoom at a screen point (zoom-to-cursor). */
  private zoomAtScreen(clientX: number, clientY: number, factor: number): void {
    const cursor = this.worldOnTargetPlane(clientX, clientY);
    const cam = this.rig.getCamera();
    if (cursor) {
      const { target } = zoomToCursor(cam.position.clone(), this.target, cursor, factor);
      this.target.copy(target);
    }
    this.distance = clampDistance(this.distance * factor);
    this.commit();
  }

  private applyPinchZoom(pinch: number): void {
    if (this.lastPinch <= 0) {
      this.lastPinch = pinch;
      return;
    }
    const factor = clampFactor(this.lastPinch / pinch);
    this.lastPinch = pinch;
    this.distance = clampDistance(this.distance * factor);
    this.commit();
  }

  private pinchDistance(): number {
    const pts = [...this.pointers.values()];
    if (pts.length < 2) return 0;
    return pts[0].distanceTo(pts[1]);
  }

  /** Ray from the cursor onto the plane through the target facing the camera. */
  private worldOnTargetPlane(clientX: number, clientY: number): THREE.Vector3 | null {
    const rect = this.el.getBoundingClientRect();
    const ndc = new THREE.Vector2(
      ((clientX - rect.left) / rect.width) * 2 - 1,
      -(((clientY - rect.top) / rect.height) * 2 - 1),
    );
    const cam = this.rig.getCamera();
    const ray = new THREE.Raycaster();
    ray.setFromCamera(ndc, cam);
    const normal = new THREE.Vector3().subVectors(cam.position, this.target).normalize();
    const plane = new THREE.Plane().setFromNormalAndCoplanarPoint(normal, this.target);
    const hit = new THREE.Vector3();
    return ray.ray.intersectPlane(plane, hit) ? hit : null;
  }

  // ---- Named views / framing ----

  homeView(animated = true): void {
    const to: TweenTarget = {
      yaw: ISO_YAW,
      pitch: ISO_PITCH,
      distance: this.defaultDistance(),
      target: new THREE.Vector3(0, 0, 0),
    };
    if (animated) this.animateTo(to);
    else this.setImmediate(to);
  }

  private setImmediate(to: TweenTarget): void {
    this.yaw = to.yaw;
    this.pitch = to.pitch;
    this.distance = to.distance;
    this.target.copy(to.target);
    this.tween = null;
    this.commit();
  }

  fitView(): void {
    const bounds = this.getBounds();
    if (!bounds || bounds.isEmpty()) {
      this.animateTo({
        yaw: this.yaw,
        pitch: this.pitch,
        distance: this.defaultDistance(),
        target: new THREE.Vector3(0, 0, 0),
      });
      return;
    }
    const sphere = bounds.getBoundingSphere(new THREE.Sphere());
    const dist = (sphere.radius / Math.sin((this.rig.fovDeg * Math.PI) / 360)) * 1.15;
    this.animateTo({
      yaw: this.yaw,
      pitch: this.pitch,
      distance: clampDistance(dist),
      target: sphere.center.clone(),
    });
  }

  /** Snap to a canonical view given a target→camera direction (ViewCube). */
  snapToViewDirection(dir: THREE.Vector3): void {
    const d = dir.clone().normalize();
    const yaw = Math.atan2(d.y, d.x);
    const pitch = clampPitch(Math.asin(Math.max(-1, Math.min(1, d.z))));
    this.animateTo({ yaw, pitch, distance: this.distance, target: this.target.clone() });
  }

  /** Snapshot of the full view state (for sketch enter → restore on exit). */
  getViewState(): TweenTarget {
    return { yaw: this.yaw, pitch: this.pitch, distance: this.distance, target: this.target.clone() };
  }

  /** Animate (or jump) to a full view state. */
  setView(view: TweenTarget, animated = true): void {
    const to = { ...view, target: view.target.clone() };
    if (animated) this.animateTo(to);
    else this.setImmediate(to);
  }

  /**
   * Look straight at a sketch plane: camera on the +normal side, target at the
   * plane origin. `normal` is the plane normal (target→camera direction).
   */
  viewAlongNormal(normal: THREE.Vector3, target: THREE.Vector3, distance: number, animated = true): void {
    const d = normal.clone().normalize();
    const yaw = Math.atan2(d.y, d.x);
    const pitch = clampPitch(Math.asin(Math.max(-1, Math.min(1, d.z))));
    this.setView({ yaw, pitch, distance, target: target.clone() }, animated);
  }

  private defaultDistance(): number {
    const bounds = this.getBounds();
    if (bounds && !bounds.isEmpty()) {
      const r = bounds.getBoundingSphere(new THREE.Sphere()).radius;
      return clampDistance((r / Math.sin((this.rig.fovDeg * Math.PI) / 360)) * 1.4);
    }
    return 260;
  }

  private animateTo(to: TweenTarget): void {
    this.tween = {
      from: { yaw: this.yaw, pitch: this.pitch, distance: this.distance, target: this.target.clone() },
      to,
      start: now(),
      dur: TWEEN_MS,
    };
    this.commit(); // kick the rAF loop
  }

  /** Advance an active tween. Returns true while an animation is running. */
  update(nowMs: number): boolean {
    if (!this.tween) return false;
    const { from, to, start, dur } = this.tween;
    const t = Math.min(1, (nowMs - start) / dur);
    const e = easeInOutCubic(t);
    this.yaw = shortestAngleLerp(from.yaw, to.yaw, e);
    this.pitch = from.pitch + (to.pitch - from.pitch) * e;
    this.distance = from.distance + (to.distance - from.distance) * e;
    this.target.copy(from.target).lerp(to.target, e);
    this.commit();
    if (t >= 1) this.tween = null;
    return true;
  }

  dispose(): void {
    this.el.removeEventListener("pointerdown", this.onPointerDown);
    this.el.removeEventListener("pointermove", this.onPointerMove);
    this.el.removeEventListener("pointerup", this.onPointerUp);
    this.el.removeEventListener("pointercancel", this.onPointerUp);
    this.el.removeEventListener("wheel", this.onWheel);
    this.el.removeEventListener("contextmenu", this.preventContext);
    this.pointers.clear();
    this.tween = null;
  }
}

// ---- module-local helpers ----

const MIN_DIST = 0.5;
const MAX_DIST = 50_000;

function clampDistance(d: number): number {
  return Math.max(MIN_DIST, Math.min(MAX_DIST, d));
}

function clampFactor(f: number): number {
  return Math.max(0.2, Math.min(5, f));
}

function now(): number {
  return typeof performance !== "undefined" ? performance.now() : Date.now();
}
