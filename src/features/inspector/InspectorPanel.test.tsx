import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import { InspectorPanel } from "./InspectorPanel";
import { selectionStore } from "@/stores/selectionStore";
import { toolStore } from "@/stores/toolStore";
import { resetStores } from "@/test/resetStores";

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

  it("shows the SKETCH state (DOF card + constraints) in sketch mode", () => {
    render(<InspectorPanel />);
    act(() => toolStore.getState().setMode("sketch"));

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
});
