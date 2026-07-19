import { beforeEach, describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, act, waitFor } from "@testing-library/react";
import { InspectorPanel } from "@/features/inspector/InspectorPanel";
import { repairStore } from "@/stores/repairStore";
import { mockClient } from "@/ipc/mockClient";
import { resetStores } from "@/test/resetStores";
import type { NeedsRepairEvent } from "@/ipc/types";

const oneItem = (opId: string, refId: string): NeedsRepairEvent => ({
  revision: 7,
  items: [{ opId, refId, reason: "ambiguous", scoringVersion: 1, candidateCount: 2 }],
});

function openRepair(opId: string, refId: string): void {
  act(() => {
    repairStore.getState().applyEvent(oneItem(opId, refId));
    repairStore.getState().openPanel();
  });
}

beforeEach(() => resetStores());

describe("RepairPanel (inspector repair state)", () => {
  it("renders the repair panel with the feature label from the projection", () => {
    render(<InspectorPanel />);
    openRepair("f3", "f3.input0"); // f3 = the seeded Fillet feature
    expect(screen.getByText("Repair references")).toBeInTheDocument();
    expect(screen.getByText("Fillet")).toBeInTheDocument();
    expect(screen.getByText(/2 candidates/)).toBeInTheDocument();
  });

  it("falls back to an opId prefix when the feature is not in the projection", () => {
    render(<InspectorPanel />);
    openRepair("op_deadbeef99", "op_deadbeef99.input0");
    expect(screen.getByText(/Feature op_dea/)).toBeInTheDocument();
  });

  it("expanding an item calls resolveRefs and renders candidates sorted by score", async () => {
    const spy = vi.spyOn(mockClient, "resolveRefs");
    render(<InspectorPanel />);
    openRepair("f3", "f3.input0");

    fireEvent.click(screen.getByTestId("repair-item-head-f3.input0"));
    expect(spy).toHaveBeenCalledWith([{ refId: "f3.input0" }]);

    // The canned candidates arrive (mock latency) — highest score first.
    await screen.findByText("91%");
    expect(screen.getByText("89%")).toBeInTheDocument();
    const pcts = screen.getAllByText(/%$/).map((n) => n.textContent);
    expect(pcts).toEqual(["91%", "89%"]);
    spy.mockRestore();
  });

  it("choosing a candidate promotes it then sends an EditOperationInput rebind", async () => {
    const promote = vi.spyOn(mockClient, "promoteSelection");
    const apply = vi.spyOn(mockClient, "applyEditCommand");
    render(<InspectorPanel />);
    openRepair("f3", "f3.input0");

    fireEvent.click(screen.getByTestId("repair-item-head-f3.input0"));
    const candidates = await screen.findAllByTestId(/^repair-candidate-f3\.input0-/);
    fireEvent.click(candidates[0]); // highest-score candidate

    await waitFor(() => expect(promote).toHaveBeenCalled());
    await waitFor(() => expect(apply).toHaveBeenCalled());
    // Promotion carries the candidate topoKey + worldPos anchor, on the seed body.
    expect(promote.mock.calls[0][0]).toBe("body1");
    expect(promote.mock.calls[0][1][0]).toMatchObject({ anchor: { worldPoint: [12, 3.5, 0] } });
    // The rebind targets the fillet op + the slot index parsed from the refId (0).
    expect(apply.mock.calls[0][0]).toMatchObject({
      cmd: "editOperationInput",
      record: "f3",
      path: { path: "filletEdges", index: 0 },
    });
    promote.mockRestore();
    apply.mockRestore();
  });

  it("the close affordance dismisses the panel", () => {
    render(<InspectorPanel />);
    openRepair("f3", "f3.input0");
    fireEvent.click(screen.getByTestId("repair-close"));
    expect(repairStore.getState().panelOpen).toBe(false);
  });
});
