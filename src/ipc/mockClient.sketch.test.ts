import { describe, it, expect, beforeEach } from "vitest";
import { mockClient, resetMockSketches } from "./mockClient";
import type { SketchConstraint, SketchEntity } from "./types";

describe("mockClient — sketch solver lane flows", () => {
  beforeEach(() => resetMockSketches());

  it("enterSketch(existing id) opens an empty XY session", async () => {
    const s = await mockClient.enterSketch("sketch2");
    expect(s.sketchId).toBe("sketch2");
    expect(s.plane.kind).toBe("XY");
    expect(s.entities).toEqual([]);
    expect(s.dof).toBe(0);
    expect(s.status).toBe("FullyConstrained");
  });

  it("enterSketch({newOnPlane}) mints an id on the requested plane", async () => {
    const s = await mockClient.enterSketch({ newOnPlane: "XZ" });
    expect(s.sketchId).toMatch(/^sk-\d+$/);
    expect(s.plane.kind).toBe("XZ");
  });

  it("sketchUpsert re-solves and bumps the revision", async () => {
    await mockClient.enterSketch("sk");
    const entities: SketchEntity[] = [{ id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] }];
    const constraints: SketchConstraint[] = [{ id: "c1", type: "Horizontal", entities: ["e1"] }];
    const r1 = await mockClient.sketchUpsert("sk", entities, constraints);
    expect(r1.sketchRevision).toBe(1);
    expect(r1.dof).toBe(3); // line (4) − 1
    expect(r1.status).toBe("UnderConstrained");
    const r2 = await mockClient.sketchUpsert("sk", entities, constraints);
    expect(r2.sketchRevision).toBe(2);
  });

  it("finishSketch returns regions for a committed rectangle", async () => {
    await mockClient.enterSketch("sk");
    const rect: SketchEntity[] = [
      { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] },
      { id: "e2", type: "Line", p0: [40, 0], p1: [40, 20] },
      { id: "e3", type: "Line", p0: [40, 20], p1: [0, 20] },
      { id: "e4", type: "Line", p0: [0, 20], p1: [0, 0] },
    ];
    await mockClient.sketchUpsert("sk", rect, []);
    const res = await mockClient.finishSketch("sk");
    expect(res.regions).toHaveLength(1);
    expect(res.regions[0].outerLoop).toHaveLength(4);
  });

  it("finishSketch on an unknown sketch yields no regions", async () => {
    const res = await mockClient.finishSketch("nope");
    expect(res.regions).toEqual([]);
  });

  it("cancelSketch resolves without throwing", async () => {
    await mockClient.enterSketch("sk");
    await expect(mockClient.cancelSketch("sk")).resolves.toBeUndefined();
  });
});
