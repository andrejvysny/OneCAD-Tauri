/*
 * Constraint badge layout (PURE) — maps a solved sketch to the glyphs the
 * ConstraintBadgeLayer renders (SCHEMA §7.3 constraint kinds). Each badge gets a
 * plane-coord anchor (the engine projects it to screen via HtmlOverlayDriver);
 * screen-space nudging (so the glyph floats off the geometry) is CSS, not here.
 */
import type { SketchConstraint, SketchEntity, SketchSession, ConstraintPosition } from "@/ipc/types";
import type { Point2 } from "@/viewport/engine/sketchBasis";

export interface ConstraintBadge {
  id: string;
  glyph: string;
  kind: SketchConstraint["type"];
  at: Point2;
  /** Dimensional constraints render an editable DimensionInput chip. */
  editable: boolean;
  value?: number;
}

const mid = (a: [number, number], b: [number, number]): Point2 => ({ x: (a[0] + b[0]) / 2, y: (a[1] + b[1]) / 2 });

/** The (u,v) coord of an entity's named point (Start/End/Center/Midpoint). */
export function entityPointCoord(e: SketchEntity, position: ConstraintPosition): Point2 | null {
  if (position === "Center" && e.center) return { x: e.center[0], y: e.center[1] };
  if (position === "Start") {
    if (e.type === "Arc" && e.start) return { x: e.start[0], y: e.start[1] };
    if (e.p0) return { x: e.p0[0], y: e.p0[1] };
  }
  if (position === "End") {
    if (e.type === "Arc" && e.end) return { x: e.end[0], y: e.end[1] };
    if (e.p1) return { x: e.p1[0], y: e.p1[1] };
  }
  if (position === "Midpoint" && e.p0 && e.p1) return mid(e.p0, e.p1);
  return null;
}

/** Representative anchor for a badge on an entity (line midpoint / circle center). */
export function entityAnchor(e: SketchEntity): Point2 | null {
  if (e.type === "Line" && e.p0 && e.p1) return mid(e.p0, e.p1);
  if ((e.type === "Circle" || e.type === "Arc") && e.center) return { x: e.center[0], y: e.center[1] };
  if (e.type === "Point" && e.p0) return { x: e.p0[0], y: e.p0[1] };
  return null;
}

function badgeFor(c: SketchConstraint, byId: Map<string, SketchEntity>): ConstraintBadge | null {
  const first = byId.get(c.entities[0]);
  if (!first) return null;

  switch (c.type) {
    case "Horizontal":
    case "Vertical": {
      const at = entityAnchor(first);
      return at ? { id: c.id, glyph: c.type === "Horizontal" ? "H" : "V", kind: c.type, at, editable: false } : null;
    }
    case "Coincident": {
      const pos = c.positions?.[0] ?? "Start";
      const at = entityPointCoord(first, pos);
      return at ? { id: c.id, glyph: "•", kind: c.type, at, editable: false } : null;
    }
    case "Parallel":
    case "Perpendicular":
    case "Tangent":
    case "Concentric":
    case "Equal":
    case "Midpoint":
    case "OnCurve":
    case "Symmetric":
    case "Fixed": {
      const at = entityAnchor(first);
      return at ? { id: c.id, glyph: GLYPH[c.type], kind: c.type, at, editable: false } : null;
    }
    case "Distance":
    case "HorizontalDistance":
    case "VerticalDistance":
    case "Angle":
    case "Radius":
    case "Diameter": {
      const at = entityAnchor(first);
      if (!at) return null;
      const value = c.value ?? 0;
      const glyph = c.type === "Angle" ? `${value.toFixed(1)}°` : value.toFixed(1);
      return { id: c.id, glyph, kind: c.type, at, editable: true, value };
    }
    default:
      return null;
  }
}

const GLYPH: Record<string, string> = {
  Parallel: "∥",
  Perpendicular: "⟂",
  Tangent: "T",
  Concentric: "◎",
  Equal: "=",
  Midpoint: "M",
  OnCurve: "⌒",
  Symmetric: "⋈",
  Fixed: "⚓",
};

export function layoutBadges(session: SketchSession | null): ConstraintBadge[] {
  if (!session) return [];
  const byId = new Map(session.entities.map((e) => [e.id, e]));
  const out: ConstraintBadge[] = [];
  for (const c of session.constraints) {
    const badge = badgeFor(c, byId);
    if (badge) out.push(badge);
  }
  return out;
}
