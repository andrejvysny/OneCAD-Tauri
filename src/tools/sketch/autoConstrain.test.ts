import { describe, it, expect } from "vitest";
import {
  inferHV,
  inferConstraints,
  lineAngle,
  angleBetweenLines,
  inferPerpendicularPartner,
  inferParallelPartner,
  inferTangentPartner,
  HV_TOLERANCE_RAD,
  PERPENDICULAR_TOLERANCE_RAD,
  PARALLEL_TOLERANCE_RAD,
  TANGENT_TOLERANCE_RAD,
} from "./autoConstrain";
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

// ── M6c parity: perpendicular / parallel / tangent ────────────────────────────

describe("angleBetweenLines (folded to [0, π/2])", () => {
  it("parallel lines ⇒ 0", () => {
    expect(angleBetweenLines([0, 0], [10, 0], [0, 5], [10, 5])).toBeCloseTo(0);
  });
  it("perpendicular lines ⇒ π/2", () => {
    expect(angleBetweenLines([0, 0], [10, 0], [0, 0], [0, 10])).toBeCloseTo(Math.PI / 2);
  });
  it("45° lines ⇒ π/4", () => {
    expect(angleBetweenLines([0, 0], [10, 0], [0, 0], [10, 10])).toBeCloseTo(Math.PI / 4);
  });
  it("folds obtuse crossings back below π/2", () => {
    // 150° between directions folds to 30°.
    expect(angleBetweenLines([0, 0], [10, 0], [0, 0], [-10, 10 * Math.tan((30 * Math.PI) / 180)])).toBeCloseTo(
      (30 * Math.PI) / 180,
    );
  });
});

describe("tolerances mirror AutoConstrainer.h (all 5°)", () => {
  it("perpendicular / parallel / tangent are all 5°", () => {
    const fiveDeg = (5 * Math.PI) / 180;
    expect(PERPENDICULAR_TOLERANCE_RAD).toBeCloseTo(fiveDeg);
    expect(PARALLEL_TOLERANCE_RAD).toBeCloseTo(fiveDeg);
    expect(TANGENT_TOLERANCE_RAD).toBeCloseTo(fiveDeg);
    expect(HV_TOLERANCE_RAD).toBeCloseTo(fiveDeg);
  });
});

describe("inferPerpendicularPartner", () => {
  const refs = [{ id: "r1", p0: [0, 0] as [number, number], p1: [40, 40] as [number, number] }]; // 45°
  it("matches a line meeting at 90±5°", () => {
    // 135° line is perpendicular to the 45° reference.
    expect(inferPerpendicularPartner([40, 40], [0, 80], refs)).toBe("r1");
  });
  it("rejects a line meeting well off 90°", () => {
    // Parallel (45°) is 0° apart, not perpendicular.
    expect(inferPerpendicularPartner([0, 10], [40, 50], refs)).toBeNull();
  });
  it("accepts within 5° of 90° but not beyond", () => {
    const at = (deg: number): [number, number] => [40 * Math.cos((deg * Math.PI) / 180), 40 * Math.sin((deg * Math.PI) / 180)];
    expect(inferPerpendicularPartner([0, 0], at(45 + 87), refs)).toBe("r1"); // 87° ⇒ within
    expect(inferPerpendicularPartner([0, 0], at(45 + 80), refs)).toBeNull(); // 80° ⇒ beyond
  });
});

describe("inferParallelPartner", () => {
  const refs = [{ id: "r1", p0: [0, 0] as [number, number], p1: [40, 40] as [number, number] }]; // 45°
  it("matches a line within ±5° of the reference direction", () => {
    expect(inferParallelPartner([0, 20], [40, 60], refs)).toBe("r1"); // exactly parallel
  });
  it("rejects a perpendicular line", () => {
    expect(inferParallelPartner([40, 40], [0, 80], refs)).toBeNull();
  });
});

describe("inferTangentPartner", () => {
  // Line along +X; its END is at (10,0).
  const refs = [{ id: "L", p0: [0, 0] as [number, number], p1: [10, 0] as [number, number] }];
  it("matches an arc starting tangent at the line endpoint", () => {
    // center above the endpoint ⇒ radial along −Y ⇒ start tangent along +X (== line).
    expect(inferTangentPartner([10, 10], [10, 0], refs)).toBe("L");
  });
  it("rejects when the arc starts tangent-normal to the line", () => {
    // center to the side ⇒ start tangent along ±Y ⇒ not aligned with the +X line.
    expect(inferTangentPartner([20, 0], [10, 0], refs)).toBeNull();
  });
  it("rejects when the arc start is not on any line endpoint", () => {
    expect(inferTangentPartner([50, 60], [50, 50], refs)).toBeNull();
  });
});

describe("inferConstraints — perpendicular / parallel / tangent + precedence", () => {
  let n = 0;
  const opts = () => {
    n = 0;
    return { nextConstraintId: () => `c${++n}` };
  };

  it("infers Perpendicular for two non-axis lines meeting at 90°", () => {
    const l1: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 40] }; // 45°
    const l2: SketchEntity = { id: "e2", type: "Line", p0: [40, 40], p1: [0, 80] }; // 135°
    const cs = inferConstraints([l2], [l1], opts());
    const perp = cs.find((c) => c.type === "Perpendicular");
    expect(perp).toBeDefined();
    expect(perp!.entities).toEqual(["e2", "e1"]);
  });

  it("infers Parallel for two non-axis parallel lines", () => {
    const l1: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 40] }; // 45°
    const l2: SketchEntity = { id: "e2", type: "Line", p0: [0, 20], p1: [40, 60] }; // 45°, offset
    const cs = inferConstraints([l2], [l1], opts());
    const par = cs.find((c) => c.type === "Parallel");
    expect(par).toBeDefined();
    expect(par!.entities).toEqual(["e2", "e1"]);
  });

  it("H/V wins over Parallel (a horizontal line parallel to a horizontal ref gets only Horizontal)", () => {
    const l1: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] }; // horizontal
    const l2: SketchEntity = { id: "e2", type: "Line", p0: [0, 10], p1: [40, 10] }; // horizontal, parallel
    const cs = inferConstraints([l2], [l1], opts());
    expect(cs.some((c) => c.type === "Horizontal" && c.entities[0] === "e2")).toBe(true);
    expect(cs.some((c) => c.type === "Parallel")).toBe(false);
    expect(cs.some((c) => c.type === "Perpendicular")).toBe(false);
  });

  it("infers Tangent for an arc starting tangent at a line endpoint", () => {
    const line: SketchEntity = { id: "L", type: "Line", p0: [0, 0], p1: [10, 0] };
    const arc: SketchEntity = { id: "A", type: "Arc", center: [10, 10], radius: 10, start: [10, 0], end: [0, 10] };
    const cs = inferConstraints([arc], [line], opts());
    const tan = cs.find((c) => c.type === "Tangent");
    expect(tan).toBeDefined();
    expect(tan!.entities).toEqual(["A", "L"]);
    // The arc START also lands on the line END ⇒ a Coincident is inferred too.
    expect(cs.some((c) => c.type === "Coincident")).toBe(true);
  });
});
