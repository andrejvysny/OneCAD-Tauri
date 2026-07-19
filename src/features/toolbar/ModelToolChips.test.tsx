/*
 * ModelToolChips (M6b chips) — render + dispatch. The chip content is portaled
 * into an engine-owned host node; in a test we inject a minimal fake engine whose
 * mountChip attaches that host to document.body so the portaled controls are
 * queryable, then assert each chip's controls dispatch through the chip-store
 * callbacks the ModelToolController registers.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { render, screen, act, fireEvent } from "@testing-library/react";
import { ModelToolChips } from "./ModelToolChips";
import { toolChipStore } from "@/stores/toolChipStore";
import { setViewportEngine } from "@/viewport/engineBridge";
import type { ViewportEngine } from "@/viewport/engine/ViewportEngine";

const WORLD: [number, number, number] = [0, 0, 0];

/** A fake engine that hosts the chip in the document so the portal is queryable. */
function fakeEngine(): ViewportEngine {
  return {
    mountChip: (_id: string, el: HTMLElement) => document.body.appendChild(el),
    unmountChip: (_id: string, el: HTMLElement) => el.remove(),
  } as unknown as ViewportEngine;
}

describe("ModelToolChips (M6b)", () => {
  beforeEach(() => {
    setViewportEngine(fakeEngine());
    toolChipStore.getState().clear();
  });
  afterEach(() => {
    setViewportEngine(null);
    toolChipStore.getState().clear();
  });

  it("renders nothing while cleared", () => {
    render(<ModelToolChips />);
    expect(screen.queryByRole("button", { name: "Apply" })).toBeNull();
  });

  it("shell chip renders a mm dimension input", () => {
    render(<ModelToolChips />);
    act(() => toolChipStore.getState().showShell(2, WORLD, vi.fn()));
    expect(screen.getByLabelText("Dimension value")).toHaveValue("2.0");
    expect(screen.getByText("mm")).toBeInTheDocument();
  });

  it("linear-pattern chip dispatches axis / count / apply", () => {
    const onAxis = vi.fn();
    const onCount = vi.fn();
    const onSpacing = vi.fn();
    const onApply = vi.fn();
    render(<ModelToolChips />);
    act(() =>
      toolChipStore.getState().showLinearPattern("X", 3, 20, WORLD, { onAxis, onCount, onSpacing, onApply }),
    );

    // Axis toggle: X active, click Y.
    expect(screen.getByRole("button", { name: "X" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Y" }));
    expect(onAxis).toHaveBeenCalledWith("Y");

    // Count stepper shows 3; +/− dispatch neighbours.
    expect(screen.getByTestId("pattern-count")).toHaveTextContent("3");
    fireEvent.click(screen.getByRole("button", { name: "More instances" }));
    expect(onCount).toHaveBeenCalledWith(4);
    fireEvent.click(screen.getByRole("button", { name: "Fewer instances" }));
    expect(onCount).toHaveBeenCalledWith(2);

    // Spacing input present + Apply commits.
    expect(screen.getByLabelText("Dimension value")).toHaveValue("20.0");
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onApply).toHaveBeenCalledTimes(1);
  });

  it("circular-pattern chip renders a degree input + axis toggle", () => {
    const handlers = { onAxis: vi.fn(), onCount: vi.fn(), onAngle: vi.fn(), onApply: vi.fn() };
    render(<ModelToolChips />);
    act(() => toolChipStore.getState().showCircularPattern("Z", 4, 360, WORLD, handlers));
    expect(screen.getByRole("button", { name: "Z" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByLabelText("Dimension value")).toHaveValue("360.0");
    expect(screen.getByText("°")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(handlers.onApply).toHaveBeenCalled();
  });

  it("mirror chip dispatches plane pick + apply", () => {
    const onPlane = vi.fn();
    const onApply = vi.fn();
    render(<ModelToolChips />);
    act(() => toolChipStore.getState().showMirror("XY", WORLD, { onPlane, onApply }));
    expect(screen.getByRole("button", { name: "XY" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "YZ" }));
    expect(onPlane).toHaveBeenCalledWith("YZ");
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    expect(onApply).toHaveBeenCalled();
  });
});
