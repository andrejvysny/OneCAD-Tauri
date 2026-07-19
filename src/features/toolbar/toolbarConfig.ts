/*
 * Floating-toolbar tool sets (F-WP3), one per mode, extracted from prototype 1c
 * (Component.MD / Component.SK, lines 503-523). Separators reproduce the
 * prototype grouping exactly.
 */
import type { IconName } from "@/icons/paths";
import type { EditorMode, Tool } from "@/stores/toolStore";

export interface ToolItem {
  id: Tool;
  icon: IconName;
  label: string;
  shortcut: string;
}

export interface ToolSeparator {
  sep: true;
}

export type ToolEntry = ToolItem | ToolSeparator;

export function isSeparator(e: ToolEntry): e is ToolSeparator {
  return "sep" in e;
}

export const MODEL_TOOLS: ToolEntry[] = [
  { id: "select", icon: "select", label: "Select", shortcut: "V" },
  { id: "sketch", icon: "sketch", label: "New sketch", shortcut: "S" },
  { sep: true },
  { id: "extrude", icon: "extrude", label: "Extrude", shortcut: "E" },
  { id: "revolve", icon: "revolve", label: "Revolve", shortcut: "R" },
  { id: "fillet", icon: "fillet", label: "Fillet", shortcut: "F" },
  { id: "boolean", icon: "boolean", label: "Combine", shortcut: "B" },
  { sep: true },
  { id: "shell", icon: "shell", label: "Shell", shortcut: "K" },
  { id: "linearPattern", icon: "linearPattern", label: "Linear pattern", shortcut: "P" },
  { id: "circularPattern", icon: "circularPattern", label: "Circular pattern", shortcut: "C" },
  { id: "mirror", icon: "mirrorBody", label: "Mirror", shortcut: "M" },
];

export const SKETCH_TOOLS: ToolEntry[] = [
  { id: "select", icon: "select", label: "Select", shortcut: "V" },
  { sep: true },
  { id: "line", icon: "line", label: "Line", shortcut: "L" },
  { id: "rect", icon: "rect", label: "Rectangle", shortcut: "R" },
  { id: "circle", icon: "circle", label: "Circle", shortcut: "C" },
  { id: "arc", icon: "arc", label: "Arc", shortcut: "A" },
  { sep: true },
  { id: "dimension", icon: "dimension", label: "Dimension", shortcut: "D" },
  { id: "trim", icon: "trim", label: "Trim", shortcut: "T" },
  { id: "mirror", icon: "mirror", label: "Mirror", shortcut: "M" },
];

export function toolsForMode(mode: EditorMode): ToolEntry[] {
  return mode === "sketch" ? SKETCH_TOOLS : MODEL_TOOLS;
}
