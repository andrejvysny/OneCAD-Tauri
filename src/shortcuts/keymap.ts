/*
 * Keyboard bindings (F-WP3) — data-driven, mode-scoped.
 *
 * Model:  V select · S new-sketch (enters sketch mode) · E extrude · R revolve
 *         · F fillet · B combine/boolean
 * Sketch: V select · L line · R rectangle · C circle · A arc · D dimension
 *         · T trim · M mirror
 * Global: Esc cancel-ladder · Enter finish-sketch (sketch mode) · H home (stub)
 *         · Shift+F zoom-to-fit
 *
 * `R` intentionally means different tools per mode (revolve vs rectangle) — that
 * is resolved by `mode`, not by a chord. `F` collides between the Fillet tool
 * (model toolbar) and zoom-to-fit (nav pill): the toolbar tool wins plain `F`
 * and zoom-to-fit moves to Shift+F. See the WP report's collision note.
 */
import type { EditorMode, Tool } from "@/stores/toolStore";

export type ShortcutAction =
  | { type: "tool"; tool: Tool }
  | { type: "enterSketch" }
  | { type: "finishSketch" }
  | { type: "cancel" }
  | { type: "zoomFit" }
  | { type: "home" };

export interface KeyBinding {
  /** Single printable key (compared case-insensitively) or a named key. */
  key: string;
  shift?: boolean;
  action: ShortcutAction;
}

export const MODEL_KEYS: KeyBinding[] = [
  { key: "v", action: { type: "tool", tool: "select" } },
  { key: "s", action: { type: "enterSketch" } },
  { key: "e", action: { type: "tool", tool: "extrude" } },
  { key: "r", action: { type: "tool", tool: "revolve" } },
  { key: "f", action: { type: "tool", tool: "fillet" } },
  { key: "b", action: { type: "tool", tool: "boolean" } },
];

export const SKETCH_KEYS: KeyBinding[] = [
  { key: "v", action: { type: "tool", tool: "select" } },
  { key: "l", action: { type: "tool", tool: "line" } },
  { key: "r", action: { type: "tool", tool: "rect" } },
  { key: "c", action: { type: "tool", tool: "circle" } },
  { key: "a", action: { type: "tool", tool: "arc" } },
  { key: "d", action: { type: "tool", tool: "dimension" } },
  { key: "t", action: { type: "tool", tool: "trim" } },
  { key: "m", action: { type: "tool", tool: "mirror" } },
];

export const GLOBAL_KEYS: KeyBinding[] = [
  { key: "Escape", action: { type: "cancel" } },
  { key: "Enter", action: { type: "finishSketch" } },
  { key: "h", action: { type: "home" } },
  { key: "f", shift: true, action: { type: "zoomFit" } },
];

export function modeKeys(mode: EditorMode): KeyBinding[] {
  return mode === "sketch" ? SKETCH_KEYS : MODEL_KEYS;
}

/**
 * Resolve a raw key + shift + mode to an action. Mode bindings win over global
 * ones so tool letters take precedence. Returns null when nothing matches.
 */
export function resolveBinding(
  key: string,
  shift: boolean,
  mode: EditorMode,
): ShortcutAction | null {
  const norm = key.length === 1 ? key.toLowerCase() : key;
  const candidates = [...modeKeys(mode), ...GLOBAL_KEYS];
  const hit = candidates.find(
    (b) => b.key === norm && Boolean(b.shift) === shift,
  );
  return hit ? hit.action : null;
}
