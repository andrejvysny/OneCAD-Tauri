import { describe, it, expect } from "vitest";
import { computeSnap, entitySnapPoints, type SnapOptions } from "./snapEngine";
import type { SketchEntity } from "@/ipc/types";

const base: SnapOptions = {
  gridStep: 10,
  pixelWorld: 1, // ⇒ 8px snap threshold = 8 world units
  enableGrid: true,
  enableGuideLines: true,
  enableGuidePoints: true,
  suppress: false,
};

const hLine: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] };
const circle: SketchEntity = { id: "e2", type: "Circle", center: [100, 100], radius: 5 };

describe("entitySnapPoints", () => {
  it("yields endpoints + midpoint for a line", () => {
    const pts = entitySnapPoints(hLine);
    expect(pts).toContainEqual({ point: { x: 0, y: 0 }, kind: "endpoint" });
    expect(pts).toContainEqual({ point: { x: 40, y: 0 }, kind: "endpoint" });
    expect(pts).toContainEqual({ point: { x: 20, y: 0 }, kind: "midpoint" });
  });
  it("yields the center for a circle", () => {
    expect(entitySnapPoints(circle)).toContainEqual({ point: { x: 100, y: 100 }, kind: "center" });
  });
});

describe("computeSnap priority", () => {
  it("snaps to a nearby endpoint (beats grid)", () => {
    const r = computeSnap({ x: 41, y: 1 }, [hLine], base);
    expect(r.kind).toBe("endpoint");
    expect(r.point).toEqual({ x: 40, y: 0 });
    expect(r.label).toBe("Endpoint");
  });

  it("snaps to a midpoint", () => {
    const r = computeSnap({ x: 21, y: 1 }, [hLine], base);
    expect(r.kind).toBe("midpoint");
    expect(r.point).toEqual({ x: 20, y: 0 });
  });

  it("prefers endpoint over midpoint on a tie", () => {
    // A degenerate zero-length line: endpoint and midpoint coincide at (5,5).
    const dot: SketchEntity = { id: "d", type: "Line", p0: [5, 5], p1: [5, 5] };
    const r = computeSnap({ x: 5, y: 5 }, [dot], base);
    expect(r.kind).toBe("endpoint");
  });

  it("falls to grid when no geometry is near", () => {
    const r = computeSnap({ x: 12, y: 7 }, [circle], base);
    expect(r.kind).toBe("grid");
    expect(r.point).toEqual({ x: 10, y: 10 });
  });

  it("emits an H/V alignment guide from a recent point", () => {
    const r = computeSnap({ x: 10.2, y: 60 }, [], { ...base, recentPoints: [{ x: 10, y: 0 }] });
    expect(r.kind).toBe("alignV");
    expect(r.point.x).toBe(10);
    expect(r.guides).toEqual([{ orientation: "vertical", value: 10 }]);
  });

  it("emits both guides (aligned) when x and y both line up", () => {
    const r = computeSnap({ x: 10.1, y: 20.1 }, [], {
      ...base,
      enableGrid: false,
      recentPoints: [{ x: 10, y: 0 }, { x: 0, y: 20 }],
    });
    expect(r.kind).toBe("alignHV");
    expect(r.point).toEqual({ x: 10, y: 20 });
    expect(r.guides).toHaveLength(2);
  });

  it("Alt suppresses all snapping (raw point)", () => {
    const r = computeSnap({ x: 41, y: 1 }, [hLine], { ...base, suppress: true });
    expect(r.snapped).toBe(false);
    expect(r.kind).toBe("none");
    expect(r.point).toEqual({ x: 41, y: 1 });
  });

  it("returns raw when everything is disabled and nothing is near", () => {
    const r = computeSnap({ x: 3, y: 4 }, [], {
      ...base,
      enableGrid: false,
      enableGuideLines: false,
      enableGuidePoints: false,
    });
    expect(r.snapped).toBe(false);
    expect(r.point).toEqual({ x: 3, y: 4 });
  });
});
