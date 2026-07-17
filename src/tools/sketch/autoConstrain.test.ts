import { describe, it, expect } from "vitest";
import { inferHV, inferConstraints, lineAngle, HV_TOLERANCE_RAD } from "./autoConstrain";
import type { SketchEntity } from "@/ipc/types";

const deg = (d: number): [number, number] => [Math.cos((d * Math.PI) / 180) * 40, Math.sin((d * Math.PI) / 180) * 40];

describe("inferHV — ±5° horizontal / vertical", () => {
  it("exact horizontal", () => expect(inferHV([0, 0], [40, 0])).toBe("Horizontal"));
  it("exact vertical", () => expect(inferHV([0, 0], [0, 40])).toBe("Vertical"));
  it("3° from horizontal ⇒ Horizontal (within tolerance)", () =>
    expect(inferHV([0, 0], deg(3))).toBe("Horizontal"));
  it("87° ⇒ Vertical (within tolerance of 90°)", () => expect(inferHV([0, 0], deg(87))).toBe("Vertical"));
  it("10° ⇒ neither", () => expect(inferHV([0, 0], deg(10))).toBeNull());
  it("horizontal pointing −X still Horizontal", () => expect(inferHV([0, 0], [-40, 0])).toBe("Horizontal"));
  it("zero-length ⇒ null", () => expect(inferHV([1, 1], [1, 1])).toBeNull());
  it("HV_TOLERANCE_RAD is 5°", () => expect(HV_TOLERANCE_RAD).toBeCloseTo((5 * Math.PI) / 180));
  it("lineAngle basics", () => expect(lineAngle([0, 0], [0, 5])).toBeCloseTo(Math.PI / 2));
});

describe("inferConstraints", () => {
  let n = 0;
  const opts = () => {
    n = 0;
    return { nextConstraintId: () => `c${++n}` };
  };

  it("adds Horizontal for a horizontal new line", () => {
    const line: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] };
    const cs = inferConstraints([line], [], opts());
    expect(cs).toContainEqual({ id: "c1", type: "Horizontal", entities: ["e1"] });
  });

  it("adds Coincident when a new endpoint lands on an existing endpoint", () => {
    const existing: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] };
    const next: SketchEntity = { id: "e2", type: "Line", p0: [40, 0], p1: [40, 20] };
    const cs = inferConstraints([next], [existing], opts());
    const coincident = cs.find((c) => c.type === "Coincident");
    expect(coincident).toBeDefined();
    expect(coincident!.entities).toEqual(["e2", "e1"]);
    expect(coincident!.positions).toEqual(["Start", "End"]);
    // Also Vertical for the vertical new line.
    expect(cs.some((c) => c.type === "Vertical")).toBe(true);
  });

  it("a rectangle's four lines infer 4 coincident + 2 H + 2 V", () => {
    const rect: SketchEntity[] = [
      { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] },
      { id: "e2", type: "Line", p0: [40, 0], p1: [40, 20] },
      { id: "e3", type: "Line", p0: [40, 20], p1: [0, 20] },
      { id: "e4", type: "Line", p0: [0, 20], p1: [0, 0] },
    ];
    const cs = inferConstraints(rect, [], opts());
    expect(cs.filter((c) => c.type === "Coincident")).toHaveLength(4);
    expect(cs.filter((c) => c.type === "Horizontal")).toHaveLength(2);
    expect(cs.filter((c) => c.type === "Vertical")).toHaveLength(2);
  });

  it("does not infer coincidence for a far endpoint (tight tolerance)", () => {
    const existing: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] };
    const next: SketchEntity = { id: "e2", type: "Line", p0: [41, 1], p1: [41, 20] };
    const cs = inferConstraints([next], [existing], opts());
    expect(cs.some((c) => c.type === "Coincident")).toBe(false);
  });
});
