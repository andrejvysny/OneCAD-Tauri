import { describe, it, expect } from "vitest";
import {
  computeSnap,
  entitySnapPoints,
  arcContainsAngle,
  circleQuadrantPoints,
  arcQuadrantPoints,
  nearestOnSegment,
  nearestOnCircle,
  nearestOnCurve,
  segSegIntersection,
  segCircleIntersections,
  circleCircleIntersections,
  entityIntersections,
  type SnapOptions,
} from "./snapEngine";
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
      enableQuadrant: false,
      enableIntersection: false,
      enableOnCurve: false,
    });
    expect(r.snapped).toBe(false);
    expect(r.point).toEqual({ x: 3, y: 4 });
  });
});

// ── new-parity geometry (M6c): quadrant / intersection / onCurve ──────────────

describe("arcContainsAngle (CCW sweep start→end)", () => {
  // Quarter arc from +X (0°) CCW to +Y (90°), centered at origin.
  const c: [number, number] = [0, 0];
  const start: [number, number] = [10, 0];
  const end: [number, number] = [0, 10];
  it("contains an angle inside the sweep", () => {
    expect(arcContainsAngle(c, start, end, Math.PI / 4)).toBe(true);
  });
  it("excludes an angle outside the sweep", () => {
    expect(arcContainsAngle(c, start, end, Math.PI)).toBe(false); // 180°
    expect(arcContainsAngle(c, start, end, -Math.PI / 4)).toBe(false); // 315°
  });
  it("includes the endpoints", () => {
    expect(arcContainsAngle(c, start, end, 0)).toBe(true);
    expect(arcContainsAngle(c, start, end, Math.PI / 2)).toBe(true);
  });
});

describe("quadrant points", () => {
  it("circle yields the four cardinal points", () => {
    const q = circleQuadrantPoints([0, 0], 5);
    expect(q).toHaveLength(4);
    expect(q[0].x).toBeCloseTo(5);
    expect(q[0].y).toBeCloseTo(0);
    expect(q[1].x).toBeCloseTo(0);
    expect(q[1].y).toBeCloseTo(5);
    expect(q[2].x).toBeCloseTo(-5);
    expect(q[3].y).toBeCloseTo(-5);
  });
  it("arc yields only the quadrants inside its extent", () => {
    // Quarter arc 0°→90° contains only the 0° and 90° quadrant points.
    const q = arcQuadrantPoints([0, 0], 10, [10, 0], [0, 10]);
    expect(q).toHaveLength(2);
  });
});

describe("nearest-point-on-curve", () => {
  it("segment: clamps the projection to the endpoints", () => {
    expect(nearestOnSegment({ x: 5, y: 3 }, [0, 0], [10, 0])).toEqual({ x: 5, y: 0 });
    expect(nearestOnSegment({ x: -4, y: 2 }, [0, 0], [10, 0])).toEqual({ x: 0, y: 0 }); // clamp low
    expect(nearestOnSegment({ x: 40, y: 2 }, [0, 0], [10, 0])).toEqual({ x: 10, y: 0 }); // clamp high
  });
  it("circle: projects radially", () => {
    const p = nearestOnCircle({ x: 20, y: 0 }, [0, 0], 5);
    expect(p.x).toBeCloseTo(5);
    expect(p.y).toBeCloseTo(0);
  });
  it("circle: degenerate point-at-center returns a point on the circle", () => {
    const p = nearestOnCircle({ x: 0, y: 0 }, [0, 0], 5);
    expect(Math.hypot(p.x, p.y)).toBeCloseTo(5);
  });
  it("arc: outside the extent snaps to the nearest endpoint", () => {
    const arc: SketchEntity = { id: "a", type: "Arc", center: [0, 0], radius: 10, start: [10, 0], end: [0, 10] };
    // Point off toward −X is outside the 0..90° sweep ⇒ nearest endpoint (0,10) or (10,0).
    const p = nearestOnCurve({ x: -5, y: 8 }, arc)!;
    expect([p.x, p.y]).toEqual([0, 10]);
  });
  it("arc: inside the extent lands on the arc", () => {
    const arc: SketchEntity = { id: "a", type: "Arc", center: [0, 0], radius: 10, start: [10, 0], end: [0, 10] };
    const p = nearestOnCurve({ x: 8, y: 8 }, arc)!;
    expect(Math.hypot(p.x, p.y)).toBeCloseTo(10);
  });
});

describe("segSegIntersection", () => {
  it("crossing segments intersect at the crossing point", () => {
    expect(segSegIntersection([0, 0], [10, 0], [5, -5], [5, 5])).toEqual({ x: 5, y: 0 });
  });
  it("parallel lines return null", () => {
    expect(segSegIntersection([0, 0], [10, 0], [0, 5], [10, 5])).toBeNull();
  });
  it("crossing beyond the segment bounds returns null", () => {
    // Infinite lines cross at (5,0) but the second segment stops short at y=1.
    expect(segSegIntersection([0, 0], [10, 0], [5, 2], [5, 1])).toBeNull();
  });
});

describe("segCircleIntersections", () => {
  it("secant segment yields two points", () => {
    const hits = segCircleIntersections([-10, 0], [10, 0], [0, 0], 5);
    expect(hits).toHaveLength(2);
    expect(hits.map((h) => h.x).sort((a, b) => a - b)).toEqual([-5, 5]);
  });
  it("tangent segment yields one point", () => {
    const hits = segCircleIntersections([-10, 5], [10, 5], [0, 0], 5);
    expect(hits).toHaveLength(1);
    expect(hits[0].y).toBeCloseTo(5);
  });
  it("miss yields none", () => {
    expect(segCircleIntersections([-10, 20], [10, 20], [0, 0], 5)).toHaveLength(0);
  });
});

describe("circleCircleIntersections (degenerate cases)", () => {
  it("two intersecting circles yield two points", () => {
    expect(circleCircleIntersections([0, 0], 5, [6, 0], 5)).toHaveLength(2);
  });
  it("externally tangent circles yield one point", () => {
    const hits = circleCircleIntersections([0, 0], 5, [10, 0], 5);
    expect(hits).toHaveLength(1);
    expect(hits[0].x).toBeCloseTo(5);
    expect(hits[0].y).toBeCloseTo(0);
  });
  it("disjoint circles yield none", () => {
    expect(circleCircleIntersections([0, 0], 5, [20, 0], 5)).toHaveLength(0);
  });
  it("one circle contained in another yields none", () => {
    expect(circleCircleIntersections([0, 0], 10, [1, 0], 2)).toHaveLength(0);
  });
  it("concentric circles yield none", () => {
    expect(circleCircleIntersections([0, 0], 5, [0, 0], 3)).toHaveLength(0);
  });
});

describe("entityIntersections", () => {
  const hLine: SketchEntity = { id: "l1", type: "Line", p0: [0, 0], p1: [10, 0] };
  const vLine: SketchEntity = { id: "l2", type: "Line", p0: [5, -5], p1: [5, 5] };
  const circle: SketchEntity = { id: "c1", type: "Circle", center: [0, 0], radius: 5 };
  it("line-line crossing", () => {
    expect(entityIntersections(hLine, vLine)).toEqual([{ x: 5, y: 0 }]);
  });
  it("line-circle secant", () => {
    const hits = entityIntersections({ id: "l", type: "Line", p0: [-10, 0], p1: [10, 0] }, circle);
    expect(hits).toHaveLength(2);
  });
  it("filters crossings outside an arc's extent", () => {
    // A line through the origin along +X hits the full circle at ±5, but the
    // quarter arc 0..90° only contains the +5 crossing.
    const arc: SketchEntity = { id: "a", type: "Arc", center: [0, 0], radius: 5, start: [5, 0], end: [0, 5] };
    const hits = entityIntersections({ id: "l", type: "Line", p0: [-10, 0.0], p1: [10, 0.0] }, arc);
    expect(hits).toHaveLength(1);
    expect(hits[0].x).toBeCloseTo(5);
  });
});

describe("computeSnap — extended priority ladder", () => {
  const circle: SketchEntity = { id: "c", type: "Circle", center: [0, 0], radius: 20 };
  const hLine: SketchEntity = { id: "l1", type: "Line", p0: [-30, 5], p1: [30, 5] };
  const vLine: SketchEntity = { id: "l2", type: "Line", p0: [5, -30], p1: [5, 30] };

  it("snaps to a quadrant point (beats grid)", () => {
    // Right quadrant of the circle is (20,0); cursor just off it.
    const r = computeSnap({ x: 21, y: 1 }, [circle], base);
    expect(r.kind).toBe("quadrant");
    expect(r.point.x).toBeCloseTo(20);
    expect(r.point.y).toBeCloseTo(0);
    expect(r.label).toBe("Quadrant");
  });

  it("center beats a co-near quadrant (higher tier)", () => {
    const small: SketchEntity = { id: "c", type: "Circle", center: [0, 0], radius: 3 };
    // Cursor near the center; the quadrant at (3,0) is also within 8px.
    const r = computeSnap({ x: 1, y: 0 }, [small], base);
    expect(r.kind).toBe("center");
  });

  it("snaps to a line-line intersection", () => {
    // Long lines that cross at (5,5) but whose endpoints/midpoints are all far
    // from the cursor, so the tier-4 intersection is the only nearby point snap.
    const longH: SketchEntity = { id: "lh", type: "Line", p0: [-100, 5], p1: [50, 5] };
    const longV: SketchEntity = { id: "lv", type: "Line", p0: [5, -100], p1: [5, 50] };
    const r = computeSnap({ x: 6, y: 6 }, [longH, longV], base);
    expect(r.kind).toBe("intersection");
    expect(r.point).toEqual({ x: 5, y: 5 });
  });

  it("onCurve is the lowest point tier (loses to a midpoint)", () => {
    // On the horizontal line near its midpoint: midpoint (0,5) within 8px wins.
    const r = computeSnap({ x: 1, y: 6 }, [hLine], base);
    expect(r.kind).toBe("midpoint");
  });

  it("snaps onto a curve when no better point is near", () => {
    // Far from any endpoint/midpoint/quadrant but on the line body.
    const r = computeSnap({ x: 15, y: 6 }, [hLine], { ...base, enableGrid: false });
    expect(r.kind).toBe("onCurve");
    expect(r.point).toEqual({ x: 15, y: 5 });
  });

  it("respects the quadrant toggle", () => {
    const r = computeSnap({ x: 21, y: 1 }, [circle], { ...base, enableQuadrant: false, enableOnCurve: false });
    expect(r.kind).not.toBe("quadrant");
  });

  it("respects the intersection toggle", () => {
    const r = computeSnap({ x: 6, y: 6 }, [hLine, vLine], {
      ...base,
      enableIntersection: false,
      enableOnCurve: false,
      enableGuidePoints: false,
    });
    expect(r.kind).not.toBe("intersection");
  });

  it("respects the onCurve toggle", () => {
    const r = computeSnap({ x: 15, y: 6 }, [hLine], { ...base, enableOnCurve: false, enableGrid: false });
    expect(r.kind).not.toBe("onCurve");
  });
});
