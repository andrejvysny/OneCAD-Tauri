import { describe, it, expect } from "vitest";
import {
  angleFromDrag,
  snapRevolveAngle,
  clampAngle,
  angleFromValueText,
  DEFAULT_DEG_PER_PX,
  DEFAULT_REVOLVE_ANGLE,
} from "./revolveAngle";

describe("clampAngle", () => {
  it("clamps to [0, 360] and maps non-finite to 0", () => {
    expect(clampAngle(-5)).toBe(0);
    expect(clampAngle(400)).toBe(360);
    expect(clampAngle(123)).toBe(123);
    expect(clampAngle(Number.NaN)).toBe(0);
  });
});

describe("angleFromDrag", () => {
  it("maps horizontal pixels to degrees from a start angle, clamped", () => {
    expect(angleFromDrag(0, 0)).toBe(0);
    // 0.75°/px default: +120px → +90°.
    expect(angleFromDrag(0, 120)).toBeCloseTo(120 * DEFAULT_DEG_PER_PX, 6);
    // Clamps at the 360 ceiling and the 0 floor.
    expect(angleFromDrag(300, 1000)).toBe(360);
    expect(angleFromDrag(30, -1000)).toBe(0);
  });

  it("honours a custom degPerPx", () => {
    expect(angleFromDrag(0, 100, { degPerPx: 1 })).toBe(100);
  });
});

describe("snapRevolveAngle", () => {
  it("snaps to the nearest 45° detent when within 3°", () => {
    expect(snapRevolveAngle(44)).toBe(45); // 1° away → snap
    expect(snapRevolveAngle(43)).toBe(45); // 2° away → snap
    expect(snapRevolveAngle(2)).toBe(0); // near the 0 detent
    expect(snapRevolveAngle(358)).toBe(360); // near the 360 detent
    expect(snapRevolveAngle(91)).toBe(90);
  });

  it("leaves the raw value when farther than 3° from a detent", () => {
    expect(snapRevolveAngle(40)).toBe(40); // 5° from 45 → no snap
    expect(snapRevolveAngle(100)).toBe(100);
  });

  it("suppresses the snap (Alt held) but still clamps", () => {
    expect(snapRevolveAngle(44, true)).toBe(44);
    expect(snapRevolveAngle(400, true)).toBe(360);
  });
});

describe("angleFromValueText", () => {
  it("parses a degree label back to a clamped angle (re-edit seed)", () => {
    expect(angleFromValueText("90°")).toBe(90);
    expect(angleFromValueText("360°")).toBe(360);
    expect(angleFromValueText("500")).toBe(360);
    expect(angleFromValueText("")).toBe(DEFAULT_REVOLVE_ANGLE);
  });
});
