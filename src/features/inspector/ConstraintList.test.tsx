import { describe, it, expect } from "vitest";
import { summarizeConstraints } from "./ConstraintList";
import type { SketchConstraint } from "@/ipc/types";

describe("summarizeConstraints", () => {
  it("groups by kind, counts, and folds the value into dimensional labels", () => {
    const constraints: SketchConstraint[] = [
      { id: "c1", type: "Coincident", entities: ["p1", "p2"] },
      { id: "c2", type: "Coincident", entities: ["p3", "p4"] },
      { id: "c3", type: "Horizontal", entities: ["l1"] },
      { id: "c4", type: "Distance", entities: ["p1", "p2"], value: 90 },
    ];
    expect(summarizeConstraints(constraints)).toEqual([
      { label: "Coincident", count: "×2" },
      { label: "Horizontal", count: "×1" },
      { label: "Distance 90.00", count: "×1" },
    ]);
  });

  it("keeps distinct dimensional values as separate rows", () => {
    const rows = summarizeConstraints([
      { id: "a", type: "Distance", entities: ["p1", "p2"], value: 10 },
      { id: "b", type: "Distance", entities: ["p3", "p4"], value: 20 },
    ]);
    expect(rows).toEqual([
      { label: "Distance 10.00", count: "×1" },
      { label: "Distance 20.00", count: "×1" },
    ]);
  });

  it("returns no rows for an empty constraint set", () => {
    expect(summarizeConstraints([])).toEqual([]);
  });
});
