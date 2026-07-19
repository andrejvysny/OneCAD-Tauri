import { describe, it, expect } from "vitest";
import { latheLocal, latheSegmentsFor, axisSplitsRegion, type Vec2 } from "./lathePreview";

// Axis = the line u=0 (direction +v); a square profile 5 units to its right.
const AXIS = { a: [0, -10] as Vec2, b: [0, 10] as Vec2 };
const SQUARE: Vec2[] = [
  [5, -5],
  [10, -5],
  [10, 5],
  [5, 5],
];

describe("latheSegmentsFor", () => {
  it("uses ~15° per segment, floored at 2", () => {
    expect(latheSegmentsFor(90)).toBe(6);
    expect(latheSegmentsFor(360)).toBe(24);
    expect(latheSegmentsFor(0)).toBe(2);
  });
});

describe("latheLocal geometry", () => {
  it("θ=0 ring reproduces the profile in the z=0 plane", () => {
    const g = latheLocal(SQUARE, AXIS, 90);
    for (let i = 0; i < 4; i++) {
      expect(g.positions[i * 3]).toBeCloseTo(SQUARE[i][0], 6);
      expect(g.positions[i * 3 + 1]).toBeCloseTo(SQUARE[i][1], 6);
      expect(g.positions[i * 3 + 2]).toBeCloseTo(0, 6);
    }
  });

  it("sweeps a profile point to |z| = its axis distance at θ=90°", () => {
    const g = latheLocal(SQUARE, AXIS, 90);
    const base = g.segments * g.ringCount; // the last (θ=90°) ring
    const x = g.positions[base * 3];
    const y = g.positions[base * 3 + 1];
    const z = g.positions[base * 3 + 2];
    expect(x).toBeCloseTo(0, 6); // collapses onto the axis foot
    expect(y).toBeCloseTo(-5, 6);
    expect(Math.abs(z)).toBeCloseTo(5, 6); // ring[0]=[5,-5] is distance 5 from u=0
  });

  it("counts (segments+1)·ringN verts, plus 2 caps for a partial sweep", () => {
    const g = latheLocal(SQUARE, AXIS, 90);
    expect(g.segments).toBe(6);
    expect(g.ringCount).toBe(4);
    expect(g.positions.length / 3).toBe(7 * 4 + 2);
    expect(g.indices.length % 3).toBe(0);
  });

  it("a full 360° sweep closes on itself with no caps", () => {
    const g = latheLocal(SQUARE, AXIS, 360);
    expect(g.segments).toBe(24);
    expect(g.positions.length / 3).toBe(25 * 4); // no cap verts
    const base = g.segments * g.ringCount;
    for (let i = 0; i < 4; i++) {
      expect(g.positions[(base + i) * 3]).toBeCloseTo(g.positions[i * 3], 4);
      expect(g.positions[(base + i) * 3 + 2]).toBeCloseTo(0, 4);
    }
  });

  it("keeps a profile point lying ON the axis fixed across the sweep", () => {
    const onAxis: Vec2[] = [
      [0, -5],
      [5, -5],
      [5, 5],
      [0, 5],
    ];
    const g = latheLocal(onAxis, AXIS, 90);
    const base = g.segments * g.ringCount;
    expect([g.positions[0], g.positions[1], g.positions[2]]).toEqual([0, -5, 0]);
    expect(g.positions[base * 3]).toBeCloseTo(0, 6);
    expect(g.positions[base * 3 + 1]).toBeCloseTo(-5, 6);
    expect(g.positions[base * 3 + 2]).toBeCloseTo(0, 6);
  });
});

describe("axisSplitsRegion", () => {
  it("is false when the profile is entirely on one side (a valid axis)", () => {
    expect(axisSplitsRegion(AXIS.a, AXIS.b, SQUARE)).toBe(false);
  });

  it("is false when the profile only touches the axis", () => {
    const touching: Vec2[] = [
      [0, -5],
      [5, -5],
      [5, 5],
      [0, 5],
    ];
    expect(axisSplitsRegion(AXIS.a, AXIS.b, touching)).toBe(false);
  });

  it("is true when the profile straddles the axis (invalid revolve axis)", () => {
    const straddling: Vec2[] = [
      [-5, -5],
      [5, -5],
      [5, 5],
      [-5, 5],
    ];
    expect(axisSplitsRegion(AXIS.a, AXIS.b, straddling)).toBe(true);
  });
});
