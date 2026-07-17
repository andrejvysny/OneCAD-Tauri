import { describe, it, expect } from "vitest";
import { lineTool, rectTool, circleTool, arcTool, draftToEntityFields, type ToolMachine, type ToolStep } from "./toolMachine";
import type { Point2 } from "@/viewport/engine/sketchBasis";

function run(m: ToolMachine, events: Array<["click" | "move", Point2] | ["esc"]>): ToolStep[] {
  let state = m.init();
  const steps: ToolStep[] = [];
  for (const e of events) {
    const ev = e[0] === "esc" ? { kind: "esc" as const } : { kind: e[0], pt: e[1] as Point2 };
    const step = m.step(state, ev);
    state = step.state;
    steps.push(step);
  }
  return steps;
}

describe("lineTool — click-click chaining, Esc ends chain", () => {
  it("commits a segment on the second click and keeps chaining", () => {
    const steps = run(lineTool, [
      ["click", { x: 0, y: 0 }],
      ["move", { x: 40, y: 0 }],
      ["click", { x: 40, y: 0 }],
      ["click", { x: 40, y: 20 }],
    ]);
    expect(steps[0].committed).toBeUndefined();
    expect(steps[1].preview).toEqual([{ type: "Line", p0: { x: 0, y: 0 }, p1: { x: 40, y: 0 } }]);
    expect(steps[2].committed).toEqual([{ type: "Line", p0: { x: 0, y: 0 }, p1: { x: 40, y: 0 } }]);
    expect(steps[3].committed).toEqual([{ type: "Line", p0: { x: 40, y: 0 }, p1: { x: 40, y: 20 } }]);
  });

  it("Esc ends the chain (done, state reset)", () => {
    const steps = run(lineTool, [["click", { x: 0, y: 0 }], ["esc"]]);
    expect(steps[1].done).toBe(true);
    expect(steps[1].state.anchors).toEqual([]);
    expect(steps[1].preview).toEqual([]);
  });
});

describe("rectTool — 2 corner clicks → 4 lines", () => {
  it("commits four lines forming the rectangle", () => {
    const steps = run(rectTool, [["click", { x: 0, y: 0 }], ["click", { x: 40, y: 20 }]]);
    expect(steps[1].done).toBe(true);
    const c = steps[1].committed!;
    expect(c).toHaveLength(4);
    expect(c).toEqual([
      { type: "Line", p0: { x: 0, y: 0 }, p1: { x: 40, y: 0 } },
      { type: "Line", p0: { x: 40, y: 0 }, p1: { x: 40, y: 20 } },
      { type: "Line", p0: { x: 40, y: 20 }, p1: { x: 0, y: 20 } },
      { type: "Line", p0: { x: 0, y: 20 }, p1: { x: 0, y: 0 } },
    ]);
  });

  it("ignores a degenerate second corner (shared axis)", () => {
    const steps = run(rectTool, [["click", { x: 0, y: 0 }], ["click", { x: 40, y: 0 }]]);
    expect(steps[1].committed).toBeUndefined();
  });

  it("previews the rectangle while moving", () => {
    const steps = run(rectTool, [["click", { x: 0, y: 0 }], ["move", { x: 10, y: 5 }]]);
    expect(steps[1].preview).toHaveLength(4);
  });
});

describe("circleTool — center → radius", () => {
  it("commits a circle with the dragged radius", () => {
    const steps = run(circleTool, [["click", { x: 0, y: 0 }], ["click", { x: 3, y: 4 }]]);
    expect(steps[1].committed).toEqual([{ type: "Circle", center: { x: 0, y: 0 }, radius: 5 }]);
  });
});

describe("arcTool — center → start → end (center-start-end)", () => {
  it("locks the radius from center→start and projects end onto the circle", () => {
    const steps = run(arcTool, [
      ["click", { x: 0, y: 0 }],
      ["click", { x: 10, y: 0 }], // radius 10, start at angle 0
      ["click", { x: 0, y: 20 }], // end direction +Y ⇒ projected to (0,10)
    ]);
    const arc = steps[2].committed![0];
    expect(arc.type).toBe("Arc");
    expect(arc.radius).toBeCloseTo(10);
    expect(arc.start).toEqual({ x: 10, y: 0 });
    expect(arc.end!.x).toBeCloseTo(0);
    expect(arc.end!.y).toBeCloseTo(10);
  });
});

describe("draftToEntityFields", () => {
  it("flattens Point2 coords into [u,v] pairs", () => {
    const f = draftToEntityFields({ type: "Line", p0: { x: 1, y: 2 }, p1: { x: 3, y: 4 } });
    expect(f).toEqual({ type: "Line", p0: [1, 2], p1: [3, 4] });
  });
  it("keeps the construction flag", () => {
    const f = draftToEntityFields({ type: "Line", construction: true, p0: { x: 0, y: 0 }, p1: { x: 1, y: 1 } });
    expect(f.construction).toBe(true);
  });
});
