import { describe, it, expect } from "vitest";
import { radiusFromDrag, formatMm, radiusFromValueText, DEFAULT_FILLET_RADIUS } from "./filletRadius";

describe("radiusFromDrag", () => {
  it("grows the radius 1:1 with world units when dragging up", () => {
    // dy = downY - currentY, so an upward drag is positive.
    expect(radiusFromDrag(2, 10, { worldPerPx: 0.5 })).toBeCloseTo(7, 9);
  });

  it("shrinks the radius when dragging down", () => {
    expect(radiusFromDrag(5, -6, { worldPerPx: 0.5 })).toBeCloseTo(2, 9);
  });

  it("clamps to the minimum radius", () => {
    expect(radiusFromDrag(2, -100, { worldPerPx: 0.5, min: 0.1 })).toBe(0.1);
  });

  it("applies the sensitivity gain", () => {
    expect(radiusFromDrag(0, 10, { worldPerPx: 1, sensitivity: 2 })).toBeCloseTo(20, 9);
  });
});

describe("formatMm", () => {
  it("formats a value like the history list", () => {
    expect(formatMm(2)).toBe("2.0 mm");
    expect(formatMm(83.25)).toBe("83.3 mm");
  });
});

describe("radiusFromValueText (fillet re-edit seed)", () => {
  it("parses a fillet feature's display text back to a radius", () => {
    expect(radiusFromValueText("2.0 mm")).toBe(2);
    expect(radiusFromValueText("12.5 mm")).toBe(12.5);
  });

  it("falls back to the default for non-numeric / non-positive text", () => {
    expect(radiusFromValueText("")).toBe(DEFAULT_FILLET_RADIUS);
    expect(radiusFromValueText("—")).toBe(DEFAULT_FILLET_RADIUS);
    expect(radiusFromValueText("0 mm")).toBe(DEFAULT_FILLET_RADIUS);
    expect(radiusFromValueText("bad", 7)).toBe(7);
  });
});
