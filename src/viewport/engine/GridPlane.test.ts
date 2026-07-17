import { describe, it, expect } from "vitest";
import { snapToDecade, chooseGridStep } from "./GridPlane";

describe("snapToDecade (1/5/10 progression)", () => {
  it("snaps within a decade", () => {
    expect(snapToDecade(1)).toBe(1);
    expect(snapToDecade(1.9)).toBe(1);
    expect(snapToDecade(2)).toBe(5);
    expect(snapToDecade(4.9)).toBe(5);
    expect(snapToDecade(5)).toBe(10);
    expect(snapToDecade(9.9)).toBe(10);
  });
  it("scales across decades", () => {
    expect(snapToDecade(10)).toBe(10);
    expect(snapToDecade(30)).toBe(50);
    expect(snapToDecade(0.3)).toBeCloseTo(0.5, 9);
  });
});

describe("chooseGridStep", () => {
  it("major is 10× minor", () => {
    const s = chooseGridStep(250);
    expect(s.major).toBe(s.minor * 10);
  });
  it("step grows with camera distance", () => {
    const near = chooseGridStep(50);
    const far = chooseGridStep(5000);
    expect(far.minor).toBeGreaterThan(near.minor);
  });
  it("close distance yields a fine step", () => {
    expect(chooseGridStep(50).minor).toBe(1); // 50/50 = 1
  });
});
