/*
 * Fillet radius drag mapping (PURE math).
 *
 * The fillet L1 preview is a documented limitation: we do NOT re-round the mesh
 * on the frontend (that needs OCCT). Instead a vertical drag ON a selected edge
 * adjusts the radius, which drives (a) the live radius chip and (b) a thickened
 * edge highlight, with the exact rounded body arriving from the debounced L2.
 *
 * Mapping (documented): dragging the pointer UP grows the radius. A pixel of
 * upward travel adds `worldPerPx` world units (1:1 with the world scale at the
 * edge's depth), so the feel is consistent across zoom levels. Radius is clamped
 * to a small positive minimum (a zero-radius fillet is a no-op).
 */
export interface RadiusDragOpts {
  /** World units per screen pixel at the edge depth (from the camera). */
  worldPerPx: number;
  /** Minimum radius (world units). Default 0.1. */
  min?: number;
  /** Extra gain on top of the 1:1 world mapping. Default 1. */
  sensitivity?: number;
}

/**
 * Radius after dragging from the grab point. `dyPixels` is `downY - currentY`
 * (screen Y grows downward, so up-drag is positive). Result is clamped ≥ min.
 */
export function radiusFromDrag(
  startRadius: number,
  dyPixels: number,
  opts: RadiusDragOpts,
): number {
  const min = opts.min ?? 0.1;
  const gain = opts.sensitivity ?? 1;
  const delta = dyPixels * opts.worldPerPx * gain;
  return Math.max(min, startRadius + delta);
}

/** Format a radius/depth for the mono chip, matching the history-list style ("2.0 mm"). */
export function formatMm(value: number): string {
  return `${value.toFixed(1)} mm`;
}

/** Default fillet radius (mirrors modelToolMachine.DEFAULT_FILLET_RADIUS). */
export const DEFAULT_FILLET_RADIUS = 2;

/**
 * Parse a fillet feature's display text ("2.0 mm") back to a radius (re-edit
 * seed; mirrors revolve's `angleFromValueText`). A non-numeric / non-positive
 * value falls back to the default radius.
 */
export function radiusFromValueText(text: string, fallback = DEFAULT_FILLET_RADIUS): number {
  const n = Number.parseFloat(text);
  return Number.isFinite(n) && n > 0 ? n : fallback;
}
