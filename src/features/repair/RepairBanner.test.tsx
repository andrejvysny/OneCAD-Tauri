import { beforeEach, describe, it, expect } from "vitest";
import { render, screen, fireEvent, act } from "@testing-library/react";
import { RepairBanner } from "./RepairBanner";
import { repairStore } from "@/stores/repairStore";
import type { NeedsRepairEvent } from "@/ipc/types";

const evt = (revision: number, n: number): NeedsRepairEvent => ({
  revision,
  items: Array.from({ length: n }, (_, i) => ({
    opId: "op_5",
    refId: `op_5.input${i}`,
    reason: "ambiguous",
    candidateCount: 2,
  })),
});

beforeEach(() => repairStore.getState().reset());

describe("RepairBanner", () => {
  it("renders nothing when there are no repair items", () => {
    render(<RepairBanner />);
    expect(screen.queryByTestId("repair-banner")).toBeNull();
  });

  it("shows a pluralized count when items arrive", () => {
    render(<RepairBanner />);
    act(() => repairStore.getState().applyEvent(evt(5, 2)));
    expect(screen.getByTestId("repair-banner")).toHaveTextContent("2 references need repair");
  });

  it("shows the singular form for one item", () => {
    render(<RepairBanner />);
    act(() => repairStore.getState().applyEvent(evt(5, 1)));
    expect(screen.getByTestId("repair-banner")).toHaveTextContent("1 reference needs repair");
  });

  it("clicking opens the repair panel", () => {
    render(<RepairBanner />);
    act(() => repairStore.getState().applyEvent(evt(5, 1)));
    fireEvent.click(screen.getByTestId("repair-banner"));
    expect(repairStore.getState().panelOpen).toBe(true);
  });

  it("auto-dismisses when a later event carries empty items", () => {
    render(<RepairBanner />);
    act(() => repairStore.getState().applyEvent(evt(5, 2)));
    expect(screen.getByTestId("repair-banner")).toBeInTheDocument();
    act(() => repairStore.getState().applyEvent({ revision: 6, items: [] }));
    expect(screen.queryByTestId("repair-banner")).toBeNull();
  });
});
