import { describe, it, expect, beforeEach } from "vitest";
import { commitDimensionConstraint } from "./sketchService";
import { mockClient, resetMockSketches } from "@/ipc/mockClient";
import { planeFor } from "@/ipc/mockSketch";
import { sketchStore } from "@/stores/sketchStore";
import type { SketchConstraint, SketchEntity, SketchSession } from "@/ipc/types";

const line: SketchEntity = { id: "e1", type: "Line", p0: [0, 0], p1: [40, 0] };

function seedSession(constraints: SketchConstraint[]): SketchSession {
  const s: SketchSession = {
    sketchId: "sk-dim",
    plane: planeFor("XY"),
    entities: [line],
    constraints,
    dof: 0,
    status: "UnderConstrained",
  };
  sketchStore.getState().setSession(s);
  return s;
}

const distance: SketchConstraint = {
  id: "d1",
  type: "Distance",
  entities: ["e1", "e1"],
  positions: ["Start", "End"],
  value: 40,
};

describe("commitDimensionConstraint — solver round-trip + reject-on-conflict", () => {
  beforeEach(() => {
    resetMockSketches();
    sketchStore.getState().reset();
  });

  it("accepts a dimension that keeps the sketch solvable (under-constrained)", async () => {
    // One Coincident removes 2 of the line's 4 DOF; a Distance removes 1 more ⇒ 1 DOF.
    seedSession([{ id: "c1", type: "Coincident", entities: ["e1", "e1"], positions: ["Start", "End"] }]);
    const { rejected } = await commitDimensionConstraint(mockClient, distance);
    expect(rejected).toBe(false);
    const s = sketchStore.getState().session!;
    expect(s.constraints.some((c) => c.id === "d1")).toBe(true);
    expect(s.dof).toBe(1);
    expect(s.status).toBe("UnderConstrained");
  });

  it("rejects + auto-undoes a dimension that over-constrains the sketch", async () => {
    // Two Coincidents remove all 4 DOF (fully constrained); a Distance ⇒ −1 ⇒ over.
    seedSession([
      { id: "c1", type: "Coincident", entities: ["e1", "e1"], positions: ["Start", "End"] },
      { id: "c2", type: "Coincident", entities: ["e1", "e1"], positions: ["End", "Start"] },
    ]);
    const { rejected } = await commitDimensionConstraint(mockClient, distance);
    expect(rejected).toBe(true);
    const s = sketchStore.getState().session!;
    // The dimension was removed; the sketch reverts to its prior (fully) state.
    expect(s.constraints.some((c) => c.id === "d1")).toBe(false);
    expect(s.constraints).toHaveLength(2);
    expect(s.status).toBe("FullyConstrained");
    expect(s.dof).toBe(0);
  });

  it("is a no-op with no active session", async () => {
    const { rejected } = await commitDimensionConstraint(mockClient, distance);
    expect(rejected).toBe(false);
    expect(sketchStore.getState().session).toBeNull();
  });
});
