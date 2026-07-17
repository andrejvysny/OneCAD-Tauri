/*
 * Extrude depth projection (PURE vector math, framework-free).
 *
 * The drag handle sits at the region centroid and points along the plane normal.
 * As the pointer drags, we want the depth = how far along the normal axis the
 * pointer's ray reaches. We take the closest-approach point between the pointer
 * ray and the (infinite) normal axis line, and return the SIGNED distance of that
 * point from the centroid along the normal. Sign gives the direction-flip for
 * free (drag "through zero" ⇒ the prism grows the other way).
 *
 * Kept as plain number-triples so it unit-tests with no THREE / WebGL.
 */
export type Vec3 = [number, number, number];

const dot = (a: Vec3, b: Vec3): number => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const sub = (a: Vec3, b: Vec3): Vec3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const len = (a: Vec3): number => Math.hypot(a[0], a[1], a[2]);
export function normalize(a: Vec3): Vec3 {
  const l = len(a) || 1;
  return [a[0] / l, a[1] / l, a[2] / l];
}

/**
 * Signed depth along `axisDir` (unit) from `axisPoint`, taken at the closest
 * approach between the pointer ray and the axis line. `axisDir` MUST be unit; the
 * ray direction need not be. Parallel rays fall back to projecting the ray origin
 * onto the axis.
 */
export function axisDepthFromRay(
  rayOrigin: Vec3,
  rayDir: Vec3,
  axisPoint: Vec3,
  axisDir: Vec3,
): number {
  const d1 = axisDir; // line 1 (unit)
  const d2 = rayDir; // line 2
  const r = sub(axisPoint, rayOrigin);
  const a = dot(d1, d1); // == 1 for a unit axis
  const e = dot(d2, d2);
  const f = dot(d2, r);
  const c = dot(d1, r);
  const b = dot(d1, d2);
  const denom = a * e - b * b;
  // Parallel (or degenerate ray): project the ray origin onto the axis.
  if (denom <= 1e-9) return -c / (a || 1);
  return (b * f - c * e) / denom;
}

/**
 * Apply the direction / symmetry modifiers to a raw signed depth.
 *  - `symmetric`: the prism grows both ways; the reported (single-side) magnitude
 *    is the half-length, but the extrude spans `2·|depth|`. We keep the signed
 *    drag value and let the op carry `extrudeMode: "Symmetric"`.
 *  - `flip`: negate (an explicit UI flip, distinct from dragging through zero).
 * Returns the depth the op + preview should use (always the drag magnitude with
 * its sign; symmetric handling is a MODE, not a value change).
 */
export function resolveDepth(raw: number, opts: { flip?: boolean } = {}): number {
  return opts.flip ? -raw : raw;
}

/** Snap a depth to the nearest grid step (hold-free coarse snapping aid). */
export function snapDepth(depth: number, step: number): number {
  if (step <= 0) return depth;
  return Math.round(depth / step) * step;
}
