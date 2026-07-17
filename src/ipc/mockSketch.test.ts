import { describe, it, expect } from "vitest";
import {
  planeFor,
  solveDof,
  freeDegrees,
  detectRegions,
  orderedClosedLoop,
  mockRegionId,
} from "./mockSketch";
import type { SketchConstraint, SketchEntity } from "./types";

const rect: SketchEntity[] = [
  { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] },
  { id: "e2", type: "Line", p0: [40, 0], p1: [40, 20] },
  { id: "e3", type: "Line", p0: [40, 20], p1: [0, 20] },
  { id: "e4", type: "Line", p0: [0, 20], p1: [0, 0] },
];

describe("planeFor — exact SCHEMA §7.3 bases", () => {
  it("XY is the non-standard basis", () => {
    expect(planeFor("XY")).toMatchObject({ kind: "XY", xAxis: [0, 1, 0], yAxis: [-1, 0, 0], normal: [0, 0, 1] });
  });
  it("XZ / YZ bases", () => {
    expect(planeFor("XZ")).toMatchObject({ xAxis: [0, 1, 0], yAxis: [0, 0, 1], normal: [1, 0, 0] });
    expect(planeFor("YZ")).toMatchObject({ xAxis: [-1, 0, 0], yAxis: [0, 0, 1], normal: [0, 1, 0] });
  });
});

describe("solveDof — naive mock heuristic", () => {
  it("empty sketch ⇒ dof 0, fully constrained", () => {
    expect(solveDof([], [])).toEqual({ dof: 0, status: "FullyConstrained" });
  });
  it("a single line has 4 free dof", () => {
    expect(freeDegrees([rect[0]])).toBe(4);
    expect(solveDof([rect[0]], [])).toEqual({ dof: 4, status: "UnderConstrained" });
  });
  it("a Horizontal constraint removes one dof", () => {
    const cs: SketchConstraint[] = [{ id: "c1", type: "Horizontal", entities: ["e1"] }];
    expect(solveDof([rect[0]], cs)).toEqual({ dof: 3, status: "UnderConstrained" });
  });
  it("more constraints than dof ⇒ over-constrained", () => {
    const cs: SketchConstraint[] = Array.from({ length: 3 }, (_, i) => ({
      id: `c${i}`,
      type: "Fixed",
      entities: ["e1"],
    }));
    expect(solveDof([rect[0]], cs).status).toBe("OverConstrained");
  });
});

describe("detectRegions", () => {
  it("finds one region for a closed rectangle", () => {
    const regions = detectRegions(rect);
    expect(regions).toHaveLength(1);
    expect(regions[0].outerLoop.sort()).toEqual(["e1", "e2", "e3", "e4"]);
    expect(regions[0].previewTriangles!.indices.length).toBeGreaterThan(0);
  });

  it("finds a region for a circle", () => {
    const regions = detectRegions([{ id: "c", type: "Circle", center: [0, 0], radius: 5 }]);
    expect(regions).toHaveLength(1);
    expect(regions[0].outerLoop).toEqual(["c"]);
  });

  it("returns no region for an open chain", () => {
    const open: SketchEntity[] = [
      { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] },
      { id: "e2", type: "Line", p0: [40, 0], p1: [40, 20] },
      { id: "e3", type: "Line", p0: [40, 20], p1: [10, 20] },
    ];
    expect(detectRegions(open)).toHaveLength(0);
  });

  it("ignores construction geometry", () => {
    const withConstruction = rect.map((e, i) => (i === 0 ? { ...e, construction: true } : e));
    expect(detectRegions(withConstruction)).toHaveLength(0);
  });
});

describe("orderedClosedLoop + mockRegionId", () => {
  it("walks the rectangle into a 4-segment cycle", () => {
    const loop = orderedClosedLoop(rect);
    expect(loop).not.toBeNull();
    expect(loop!.ids).toHaveLength(4);
    expect(loop!.points).toHaveLength(4);
  });
  it("region id is deterministic & order-independent", () => {
    expect(mockRegionId(["e1", "e2", "e3"])).toBe(mockRegionId(["e3", "e1", "e2"]));
    expect(mockRegionId(["e1"])).toMatch(/^r_[0-9a-f]{8}$/);
  });
});
