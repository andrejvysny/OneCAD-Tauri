import { describe, it, expect } from "vitest";
import {
  WORLD_AXIS,
  WORLD_PLANE_NORMAL,
  translatePoint,
  rotatePointAboutAxis,
  reflectPoint,
  linearOffsets,
  circularAnglesDeg,
  linearGhostTransforms,
  circularGhostTransforms,
  mirrorGhostTransforms,
  clampPatternCount,
  countFromValueText,
} from "./patternPreview";
describe("point transforms", () => {
  it("translatePoint adds the offset", () => {
    expect(translatePoint([1, 2, 3], [10, 0, -1])).toEqual([11, 2, 2]);
  });

  it("rotatePointAboutAxis rotates about the world Z axis (90° maps +X → +Y)", () => {
    const r = rotatePointAboutAxis([1, 0, 0], [0, 0, 0], [0, 0, 1], Math.PI / 2);
    expect(r[0]).toBeCloseTo(0, 9);
    expect(r[1]).toBeCloseTo(1, 9);
    expect(r[2]).toBeCloseTo(0, 9);
  });

  it("rotatePointAboutAxis honors a non-origin axis", () => {
    // Rotate the point (2,1,0) 180° about the vertical line through (1,1,0) → (0,1,0).
    const r = rotatePointAboutAxis([2, 1, 0], [1, 1, 0], [0, 0, 1], Math.PI);
    expect(r[0]).toBeCloseTo(0, 9);
    expect(r[1]).toBeCloseTo(1, 9);
    expect(r[2]).toBeCloseTo(0, 9);
  });

  it("reflectPoint mirrors across a plane through the origin", () => {
    // Mirror across YZ (normal +X): x flips.
    const r = reflectPoint([3, 2, 1], [0, 0, 0], WORLD_PLANE_NORMAL.YZ);
    expect(r).toEqual([-3, 2, 1]);
    // Mirror across XY (normal +Z): z flips.
    expect(reflectPoint([3, 2, 1], [0, 0, 0], WORLD_PLANE_NORMAL.XY)).toEqual([3, 2, -1]);
  });

  it("reflectPoint mirrors across an offset plane", () => {
    // Plane x = 5 (normal +X, point (5,0,0)): x → 10 − x.
    expect(reflectPoint([3, 0, 0], [5, 0, 0], [1, 0, 0])).toEqual([7, 0, 0]);
  });
});

describe("linear pattern placement", () => {
  it("linearOffsets spaces instances along the unit direction (index 0 = origin)", () => {
    const off = linearOffsets(WORLD_AXIS.X, 20, 3);
    expect(off).toEqual([
      [0, 0, 0],
      [20, 0, 0],
      [40, 0, 0],
    ]);
  });

  it("linearOffsets normalizes a non-unit direction", () => {
    const off = linearOffsets([0, 3, 0], 10, 2); // dir length 3 → normalized to +Y
    expect(off[1][1]).toBeCloseTo(10, 9);
    expect(off[1][0]).toBeCloseTo(0, 9);
  });

  it("linearGhostTransforms yields count−1 translate clones (excludes the original)", () => {
    const t = linearGhostTransforms(WORLD_AXIS.Y, 15, 4);
    expect(t).toHaveLength(3);
    expect(t[0]).toEqual({ kind: "translate", offset: [0, 15, 0] });
    expect(t[2]).toEqual({ kind: "translate", offset: [0, 45, 0] });
  });
});

describe("circular pattern placement", () => {
  it("circularAnglesDeg divides a full 360° by count (no overlap of last + first)", () => {
    expect(circularAnglesDeg(360, 4)).toEqual([0, 90, 180, 270]);
  });

  it("circularAnglesDeg spreads a partial sweep across count−1 gaps", () => {
    expect(circularAnglesDeg(180, 3)).toEqual([0, 90, 180]);
  });

  it("circularGhostTransforms yields count−1 rotate clones about the axis", () => {
    const t = circularGhostTransforms([0, 0, 0], WORLD_AXIS.Z, 360, 4);
    expect(t).toHaveLength(3);
    expect(t[0]).toMatchObject({ kind: "rotate", origin: [0, 0, 0], axis: WORLD_AXIS.Z });
    if (t[0].kind === "rotate") expect(t[0].angleRad).toBeCloseTo(Math.PI / 2, 9);
  });
});

describe("mirror placement", () => {
  it("mirrorGhostTransforms yields a single mirror clone", () => {
    const t = mirrorGhostTransforms([0, 0, 0], WORLD_PLANE_NORMAL.XZ);
    expect(t).toEqual([{ kind: "mirror", point: [0, 0, 0], normal: [0, 1, 0] }]);
  });
});

describe("count clamp + parse", () => {
  it("clampPatternCount clamps to [2, 12] and rounds", () => {
    expect(clampPatternCount(1)).toBe(2);
    expect(clampPatternCount(20)).toBe(12);
    expect(clampPatternCount(3.4)).toBe(3);
    expect(clampPatternCount(Number.NaN)).toBe(3);
  });

  it("countFromValueText parses '×N' back to a count", () => {
    expect(countFromValueText("×4")).toBe(4);
    expect(countFromValueText("×20")).toBe(12); // clamped
    expect(countFromValueText("nope")).toBe(3); // fallback
  });
});
