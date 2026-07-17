/*
 * Viewport color palette.
 *
 * The engine never hard-codes colors: it reads the design tokens (CSS custom
 * properties emitted by Tailwind `@theme`) ONCE via getComputedStyle and caches
 * THREE.Color instances. tokens.css stays the single source of truth.
 *
 * In non-browser contexts (vitest/jsdom, where Tailwind's `@theme` is not
 * processed) the custom properties resolve to empty strings, so each token
 * falls back to an rgb() mirror of the token value. These fallbacks are rgb(),
 * never `#` hex literals, so the tokens-only hex gate still passes; the browser
 * always overrides them with the real token via getComputedStyle.
 */
import * as THREE from "three";

export type TokenName =
  | "--color-border"
  | "--color-border-strong"
  | "--color-canvas-model"
  | "--color-canvas-sketch"
  | "--color-ink"
  | "--color-ink-5"
  | "--color-accent"
  | "--color-sel-bg"
  | "--color-sel-text"
  | "--color-warn";

// rgb() mirrors of the token values in tokens.css (non-browser fallback only).
const FALLBACK: Record<TokenName, string> = {
  "--color-border": "rgb(226, 228, 232)",
  "--color-border-strong": "rgb(216, 219, 224)",
  "--color-canvas-model": "rgb(238, 240, 243)",
  "--color-canvas-sketch": "rgb(244, 247, 252)",
  "--color-ink": "rgb(27, 29, 33)",
  "--color-ink-5": "rgb(138, 145, 156)",
  "--color-accent": "rgb(46, 111, 224)",
  "--color-sel-bg": "rgb(225, 235, 251)",
  "--color-sel-text": "rgb(29, 79, 168)",
  "--color-warn": "rgb(178, 107, 16)",
};

let cache: Map<TokenName, THREE.Color> | null = null;

function readToken(name: TokenName): string {
  if (typeof document !== "undefined" && typeof getComputedStyle === "function") {
    const value = getComputedStyle(document.documentElement)
      .getPropertyValue(name)
      .trim();
    if (value) return value;
  }
  return FALLBACK[name];
}

function tokenColor(name: TokenName): THREE.Color {
  if (!cache) cache = new Map();
  let color = cache.get(name);
  if (!color) {
    color = new THREE.Color(readToken(name));
    cache.set(name, color);
  }
  return color;
}

/** Named viewport colors, resolved from design tokens on first access. */
export const palette = {
  /** Grid minor lines. */
  gridMinor: () => tokenColor("--color-border"),
  /** Grid major lines. */
  gridMajor: () => tokenColor("--color-border-strong"),
  /** Renderer clear color = model-canvas background. */
  clear: () => tokenColor("--color-canvas-model"),
  /** Neutral body face material. */
  bodyNeutral: () => tokenColor("--color-ink-5"),
  /** Body edge lines. */
  bodyEdge: () => tokenColor("--color-border-strong"),
  /** Hover accent (face + edge highlight). */
  hoverAccent: () => tokenColor("--color-accent"),
  /** Selected face tint. */
  selectedTint: () => tokenColor("--color-sel-bg"),
  /** Selected edge / outline color. */
  selectedEdge: () => tokenColor("--color-sel-text"),

  // ── Sketch entity colors, by constraint state (F-WP6) ──
  /** Under-constrained sketch geometry (the working accent). */
  sketchUnder: () => tokenColor("--color-accent"),
  /** Fully-constrained sketch geometry. */
  sketchFull: () => tokenColor("--color-ink"),
  /** Selected sketch geometry. */
  sketchSelected: () => tokenColor("--color-sel-text"),
  /** Construction (dashed) geometry. */
  sketchConstruction: () => tokenColor("--color-ink-5"),
  /** Conflicting / over-constrained geometry. */
  sketchConflict: () => tokenColor("--color-warn"),
  /** Sketch plane tint quad + sketch canvas background. */
  sketchPlane: () => tokenColor("--color-canvas-sketch"),
};

/** Test / theme-change seam: drop the cache so colors re-read from the DOM. */
export function resetPaletteCache(): void {
  cache = null;
}
