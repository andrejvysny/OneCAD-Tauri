/*
 * Revolve angle interaction math (PURE, framework-free) — the rotation analogue
 * of depthProjection (extrude) / filletRadius (fillet). A horizontal pointer drag
 * maps to a revolution angle in DEGREES clamped to [0, 360], with 45° detents:
 * the raw value snaps to the nearest detent when within `REVOLVE_SNAP_TOL`, and
 * `suppress` (the Alt modifier) keeps the raw value — mirroring the extrude
 * symmetric/Alt + fillet snap conventions. Unit-tested with no THREE / WebGL.
 */

/** ~480px of horizontal drag sweeps a full 360°. */
export const DEFAULT_DEG_PER_PX = 0.75;
/** Detent spacing (degrees). */
export const REVOLVE_SNAP_STEP = 45;
/** Snap when the raw angle is within this many degrees of a detent. */
export const REVOLVE_SNAP_TOL = 3;
/** Default full-revolution angle (the quick-action value). */
export const DEFAULT_REVOLVE_ANGLE = 360;

/** Clamp an angle to the revolve range [0, 360]; non-finite ⇒ 0. */
export function clampAngle(angle: number): number {
  if (!Number.isFinite(angle)) return 0;
  return Math.max(0, Math.min(360, angle));
}

/** Map a horizontal drag (px) from `startAngle` to a clamped angle. */
export function angleFromDrag(
  startAngle: number,
  dxPx: number,
  opts: { degPerPx?: number } = {},
): number {
  const k = opts.degPerPx ?? DEFAULT_DEG_PER_PX;
  return clampAngle(startAngle + dxPx * k);
}

/**
 * Snap to the nearest 45° detent when within `REVOLVE_SNAP_TOL`; `suppress`
 * (Alt held) keeps the raw clamped value. Always returns a value in [0, 360].
 */
export function snapRevolveAngle(angle: number, suppress = false): number {
  const a = clampAngle(angle);
  if (suppress) return a;
  const nearest = Math.round(a / REVOLVE_SNAP_STEP) * REVOLVE_SNAP_STEP;
  return Math.abs(nearest - a) <= REVOLVE_SNAP_TOL ? nearest : a;
}

/** Parse a revolve feature's display text ("90°") back to an angle (re-edit seed). */
export function angleFromValueText(text: string): number {
  const n = Number.parseFloat(text);
  return Number.isFinite(n) ? clampAngle(n) : DEFAULT_REVOLVE_ANGLE;
}
