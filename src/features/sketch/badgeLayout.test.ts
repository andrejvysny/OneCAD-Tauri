import { describe, it, expect } from "vitest";
import { layoutBadges, entityAnchor, entityPointCoord } from "./badgeLayout";
import { planeFor } from "@/ipc/mockSketch";
import type { SketchSession } from "@/ipc/types";

const session: SketchSession = {
  sketchId: "sk",
  plane: planeFor("XY"),
  entities: [
    { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] },
    { id: "e2", type: "Line", p0: [40, 0], p1: [40, 20] },
    { id: "c1", type: "Circle", center: [10, 10], radius: 5 },
  ],
  constraints: [
    { id: "k1", type: "Horizontal", entities: ["e1"] },
    { id: "k2", type: "Coincident", entities: ["e2", "e1"], positions: ["Start", "End"] },
    { id: "k3", type: "Distance", entities: ["e1"], value: 40 },
    { id: "k4", type: "Radius", entities: ["c1"], value: 5 },
  ],
  dof: 2,
  status: "UnderConstrained",
};

describe("entityAnchor / entityPointCoord", () => {
  it("line anchor is the midpoint", () => {
    expect(entityAnchor(session.entities[0])).toEqual({ x: 20, y: 0 });
  });
  it("circle anchor is the center", () => {
    expect(entityAnchor(session.entities[2])).toEqual({ x: 10, y: 10 });
  });
  it("named point coords resolve", () => {
    expect(entityPointCoord(session.entities[0], "End")).toEqual({ x: 40, y: 0 });
    expect(entityPointCoord(session.entities[1], "Start")).toEqual({ x: 40, y: 0 });
  });
});

describe("layoutBadges", () => {
  const badges = layoutBadges(session);

  it("places a Horizontal glyph at the line midpoint", () => {
    const h = badges.find((b) => b.id === "k1")!;
    expect(h.glyph).toBe("H");
    expect(h.at).toEqual({ x: 20, y: 0 });
    expect(h.editable).toBe(false);
  });

  it("places a Coincident dot at the shared point", () => {
    const c = badges.find((b) => b.id === "k2")!;
    expect(c.glyph).toBe("•");
    expect(c.at).toEqual({ x: 40, y: 0 });
  });

  it("marks dimensional constraints editable with a value glyph", () => {
    const d = badges.find((b) => b.id === "k3")!;
    expect(d.editable).toBe(true);
    expect(d.value).toBe(40);
    expect(d.glyph).toBe("40.0");
    const r = badges.find((b) => b.id === "k4")!;
    expect(r.editable).toBe(true);
    expect(r.at).toEqual({ x: 10, y: 10 });
  });

  it("returns [] for a null session", () => {
    expect(layoutBadges(null)).toEqual([]);
  });
});
