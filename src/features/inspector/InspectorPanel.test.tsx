import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { InspectorPanel } from "./InspectorPanel";
import { selectionStore } from "@/stores/selectionStore";
import { toolStore } from "@/stores/toolStore";
import { sketchStore } from "@/stores/sketchStore";
import { resetStores } from "@/test/resetStores";
import type { SketchConstraint, SketchSession } from "@/ipc/types";

/** A live sketch session carrying the constraints the inspector summarizes. */
function sessionWithConstraints(constraints: SketchConstraint[]): SketchSession {
  return {
    sketchId: "sketch2",
    plane: { kind: "XY", origin: [0, 0, 0], xAxis: [0, 1, 0], yAxis: [-1, 0, 0], normal: [0, 0, 1] },
    entities: [],
    constraints,
    dof: 3,
    status: "UnderConstrained",
  };
}

describe("InspectorPanel", () => {
  beforeEach(() => resetStores());

  it("shows the SELECTION state for the default sketch selection", () => {
    render(<InspectorPanel />);
    expect(screen.getByText("Sketch 2")).toBeInTheDocument();
    expect(screen.getByText("Sketch · 2 profiles")).toBeInTheDocument();
    expect(screen.getByText("Under-constrained · DOF 3")).toBeInTheDocument();
    expect(screen.getByText("History")).toBeInTheDocument();
    expect(screen.getByText("83.3 mm")).toBeInTheDocument();
  });

  it("shows body status + full history when a body is selected", () => {
    render(<InspectorPanel />);
    act(() => selectionStore.getState().set([{ kind: "body", id: "body1" }]));

    expect(screen.getByText("Body 1")).toBeInTheDocument();
    expect(screen.getByText("Solid body · 6 faces")).toBeInTheDocument();
    expect(screen.getByText("Sketch 1")).toBeInTheDocument();
    expect(screen.getByText("Fillet")).toBeInTheDocument();
    expect(screen.getByText("2.0 mm")).toBeInTheDocument();
    // A body is fully defined — no DOF line.
    expect(screen.queryByText("Under-constrained · DOF 3")).toBeNull();
  });

  it("shows the EMPTY state when nothing is selected", () => {
    render(<InspectorPanel />);
    act(() => selectionStore.getState().clear());
    expect(screen.getByText("Nothing selected")).toBeInTheDocument();
  });

  it("shows the SKETCH state (DOF card + live constraints) in sketch mode", () => {
    render(<InspectorPanel />);
    act(() => {
      toolStore.getState().setMode("sketch");
      // Live sketch session drives the CONSTRAINTS panel (no hardcoded demo data).
      sketchStore.getState().setSession(
        sessionWithConstraints([
          { id: "c1", type: "Coincident", entities: ["p1", "p2"] },
          { id: "c2", type: "Coincident", entities: ["p3", "p4"] },
          { id: "c3", type: "Coincident", entities: ["p5", "p6"] },
          { id: "c4", type: "Coincident", entities: ["p7", "p8"] },
          { id: "c5", type: "Horizontal", entities: ["l1"] },
          { id: "c6", type: "Distance", entities: ["p1", "p2"], value: 90 },
        ]),
      );
    });

    expect(screen.getByText("Sketch 2")).toBeInTheDocument();
    expect(screen.getByText("Under-constrained · DOF 3")).toBeInTheDocument();
    expect(screen.getByText("Constraints")).toBeInTheDocument();
    expect(screen.getByText("Coincident")).toBeInTheDocument();
    expect(screen.getByText("×4")).toBeInTheDocument();
    expect(screen.getByText("Distance 90.00")).toBeInTheDocument();
    expect(
      screen.getByText(/degrees of freedom remain/),
    ).toBeInTheDocument();
  });

  it("shows the empty-constraints hint when the sketch has none", () => {
    render(<InspectorPanel />);
    act(() => {
      toolStore.getState().setMode("sketch");
      sketchStore.getState().setSession(sessionWithConstraints([]));
    });
    expect(screen.getByText("No constraints yet.")).toBeInTheDocument();
    expect(screen.queryByText("Coincident")).toBeNull();
  });
});
