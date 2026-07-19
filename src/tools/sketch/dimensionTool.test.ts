import { describe, it, expect } from "vitest";
import {
  dimensionInit,
  dimensionStep,
  buildDimensionConstraint,
  dimensionSuffix,
  isConflictStatus,
  angleBetweenDeg,
  pickDimensionTarget,
  type DimPick,
  type DimState,
  type DimStep,
} from "./dimensionTool";
import type { SketchEntity } from "@/ipc/types";

const line = (id: string, p0: [number, number], p1: [number, number]): DimPick => ({ on: "line", id, p0, p1 });
const circle = (id: string, center: [number, number], radius: number): DimPick => ({ on: "circle", id, center, radius });
const arc = (id: string, center: [number, number], radius: number): DimPick => ({ on: "arc", id, center, radius });
const point = (id: string, position: "Start" | "End" | "Center", coord: [number, number]): DimPick => ({
  on: "point",
  id,
  position,
  coord,
});

/** Drive a pick script; return the sequence of steps. */
function run(events: Array<DimPick | { commit: number } | "cancel">): DimStep[] {
  let state: DimState = dimensionInit();
  const steps: DimStep[] = [];
  for (const e of events) {
    const ev =
      e === "cancel"
        ? ({ kind: "cancel" } as const)
        : "commit" in e
          ? ({ kind: "commit", value: e.commit } as const)
          : ({ kind: "pick", target: e } as const);
    const step = dimensionStep(state, ev);
    state = step.state;
    steps.push(step);
  }
  return steps;
}

describe("dimensionStep — single-entity dimensions", () => {
  it("circle → Diameter (value = 2·radius) armed on the first pick", () => {
    const [s] = run([circle("c1", [10, 10], 5)]);
    expect(s.state.ready).toMatchObject({ kind: "Diameter", entities: ["c1"], value: 10 });
    expect(s.state.ready!.anchor).toEqual({ x: 10, y: 10 });
  });

  it("arc → Radius (value = radius)", () => {
    const [s] = run([arc("a1", [0, 0], 7)]);
    expect(s.state.ready).toMatchObject({ kind: "Radius", entities: ["a1"], value: 7 });
  });

  it("line → Distance = its length, entities are the line's Start/End", () => {
    const [s] = run([line("l1", [0, 0], [30, 40])]);
    expect(s.state.ready).toMatchObject({
      kind: "Distance",
      entities: ["l1", "l1"],
      positions: ["Start", "End"],
      value: 50,
    });
    expect(s.state.ready!.anchor).toEqual({ x: 15, y: 20 });
  });
});

describe("dimensionStep — two-pick dimensions", () => {
  it("two points → Distance between them", () => {
    const steps = run([point("l1", "Start", [0, 0]), point("l2", "End", [0, 10])]);
    expect(steps[0].state.ready).toBeNull(); // first point waits
    expect(steps[0].state.pending).not.toBeNull();
    expect(steps[1].state.ready).toMatchObject({
      kind: "Distance",
      entities: ["l1", "l2"],
      positions: ["Start", "End"],
      value: 10,
    });
  });

  it("two distinct lines → Angle (upgrades the first line's length)", () => {
    const steps = run([line("l1", [0, 0], [10, 0]), line("l2", [0, 0], [0, 10])]);
    // First line arms a length dimension.
    expect(steps[0].state.ready!.kind).toBe("Distance");
    expect(steps[0].state.pending).not.toBeNull();
    // Second distinct line upgrades to a 90° angle.
    expect(steps[1].state.ready).toMatchObject({ kind: "Angle", entities: ["l1", "l2"], value: 90 });
    expect(steps[1].state.pending).toBeNull();
  });

  it("re-picking the SAME line keeps the length (does not make a 0° angle)", () => {
    const steps = run([line("l1", [0, 0], [10, 0]), line("l1", [0, 0], [10, 0])]);
    expect(steps[1].state.ready!.kind).toBe("Distance");
  });

  it("re-picking the SAME point does not make a zero-distance", () => {
    const steps = run([point("l1", "Start", [0, 0]), point("l1", "Start", [0, 0])]);
    expect(steps[1].state.ready).toBeNull();
  });
});

describe("dimensionStep — commit / cancel", () => {
  it("commit emits the spec with the committed value and resets", () => {
    let state = dimensionInit();
    state = dimensionStep(state, { kind: "pick", target: line("l1", [0, 0], [30, 40]) }).state;
    const step = dimensionStep(state, { kind: "commit", value: 55 });
    expect(step.emit).toMatchObject({ kind: "Distance", entities: ["l1", "l1"], value: 55 });
    expect(step.state).toEqual(dimensionInit()); // reset
  });

  it("commit with nothing armed is a no-op", () => {
    const step = dimensionStep(dimensionInit(), { kind: "commit", value: 10 });
    expect(step.emit).toBeUndefined();
  });

  it("cancel clears any pending / armed state", () => {
    let state = dimensionStep(dimensionInit(), { kind: "pick", target: line("l1", [0, 0], [10, 0]) }).state;
    state = dimensionStep(state, { kind: "cancel" }).state;
    expect(state).toEqual(dimensionInit());
  });
});

describe("buildDimensionConstraint", () => {
  it("Distance keeps entities + positions + value", () => {
    const spec = run([line("l1", [0, 0], [30, 40])])[0].state.ready!;
    const c = buildDimensionConstraint(spec, "k1");
    expect(c).toEqual({ id: "k1", type: "Distance", entities: ["l1", "l1"], value: 50, positions: ["Start", "End"] });
  });
  it("Diameter omits positions", () => {
    const spec = run([circle("c1", [0, 0], 5)])[0].state.ready!;
    const c = buildDimensionConstraint(spec, "k2");
    expect(c).toEqual({ id: "k2", type: "Diameter", entities: ["c1"], value: 10 });
    expect(c.positions).toBeUndefined();
  });
});

describe("angleBetweenDeg", () => {
  it("perpendicular ⇒ 90", () => expect(angleBetweenDeg([0, 0], [10, 0], [0, 0], [0, 10])).toBeCloseTo(90));
  it("parallel ⇒ 0", () => expect(angleBetweenDeg([0, 0], [10, 0], [5, 5], [15, 5])).toBeCloseTo(0));
  it("45° crossing ⇒ 45", () => expect(angleBetweenDeg([0, 0], [10, 0], [0, 0], [10, 10])).toBeCloseTo(45));
});

describe("dimensionSuffix", () => {
  it("mm for lengths, ° for angle", () => {
    expect(dimensionSuffix("Distance")).toBe("mm");
    expect(dimensionSuffix("Radius")).toBe("mm");
    expect(dimensionSuffix("Diameter")).toBe("mm");
    expect(dimensionSuffix("Angle")).toBe("°");
  });
});

describe("isConflictStatus (reject-on-over-constraint)", () => {
  it("rejects OverConstrained + Conflicting", () => {
    expect(isConflictStatus("OverConstrained")).toBe(true);
    expect(isConflictStatus("Conflicting")).toBe(true);
  });
  it("accepts Under/Fully constrained", () => {
    expect(isConflictStatus("UnderConstrained")).toBe(false);
    expect(isConflictStatus("FullyConstrained")).toBe(false);
  });
});

describe("pickDimensionTarget", () => {
  const entities: SketchEntity[] = [
    { id: "l1", type: "Line", p0: [0, 0], p1: [40, 0] },
    { id: "c1", type: "Circle", center: [100, 0], radius: 10 },
  ];

  it("clicks near a vertex → a point pick", () => {
    const p = pickDimensionTarget({ x: 1, y: 1 }, entities, 5)!;
    expect(p.on).toBe("point");
    if (p.on === "point") {
      expect(p.id).toBe("l1");
      expect(p.position).toBe("Start");
    }
  });

  it("clicks on the line body → a line pick", () => {
    const p = pickDimensionTarget({ x: 20, y: 1 }, entities, 5)!;
    expect(p.on).toBe("line");
    if (p.on === "line") expect(p.id).toBe("l1");
  });

  it("clicks on the circle body → a circle pick", () => {
    // Right edge of the circle at (110,0).
    const p = pickDimensionTarget({ x: 109, y: 0 }, entities, 5)!;
    expect(p.on).toBe("circle");
    if (p.on === "circle") expect(p.id).toBe("c1");
  });

  it("returns null when nothing is within tolerance", () => {
    expect(pickDimensionTarget({ x: 500, y: 500 }, entities, 5)).toBeNull();
  });
});
