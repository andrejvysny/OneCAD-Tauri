import { describe, it, expect } from "vitest";
import { axisDepthFromRay, resolveDepth, snapDepth, normalize, type Vec3 } from "./depthProjection";

const Z: Vec3 = [0, 0, 1];
const ORIGIN: Vec3 = [0, 0, 0];

describe("axisDepthFromRay", () => {
  it("returns the height of a horizontal ray crossing the axis", () => {
    // Axis = +Z at origin; ray at z=3 heading -x passes over the axis at z=3.
    const depth = axisDepthFromRay([5, 0, 3], [-1, 0, 0], ORIGIN, Z);
    expect(depth).toBeCloseTo(3, 9);
  });

  it("is signed: a ray below the plane yields a negative depth (flip)", () => {
    const depth = axisDepthFromRay([5, 0, -4], [-1, 0, 0], ORIGIN, Z);
    expect(depth).toBeCloseTo(-4, 9);
  });

  it("projects the ray origin onto the axis when the ray is parallel", () => {
    // Ray runs down the axis from z=10 ⇒ closest param = the origin projection.
    const depth = axisDepthFromRay([0, 0, 10], [0, 0, -1], ORIGIN, Z);
    expect(depth).toBeCloseTo(10, 9);
  });

  it("works for a non-origin axis point", () => {
    const depth = axisDepthFromRay([5, 0, 7], [-1, 0, 0], [0, 0, 2], Z);
    expect(depth).toBeCloseTo(5, 9); // 7 measured from z=2
  });
});

describe("resolveDepth", () => {
  it("negates when flipped", () => {
    expect(resolveDepth(6)).toBe(6);
    expect(resolveDepth(6, { flip: true })).toBe(-6);
  });
});

describe("snapDepth", () => {
  it("snaps to the nearest step", () => {
    expect(snapDepth(23, 5)).toBe(25);
    expect(snapDepth(22, 5)).toBe(20);
    expect(snapDepth(7, 0)).toBe(7); // no snapping when step ≤ 0
  });
});

describe("normalize", () => {
  it("returns a unit vector", () => {
    const n = normalize([0, 3, 4]);
    expect(Math.hypot(...n)).toBeCloseTo(1, 9);
  });
});
