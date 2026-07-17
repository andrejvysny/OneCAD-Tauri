import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, act } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SketchChromeBar } from "./SketchChromeBar";
import { toolStore } from "@/stores/toolStore";
import { resetStores } from "@/test/resetStores";

describe("SketchChromeBar", () => {
  beforeEach(() => resetStores());

  it("is hidden in model mode", () => {
    render(<SketchChromeBar />);
    expect(screen.queryByText(/Editing/)).toBeNull();
  });

  it("shows the editing pill in sketch mode and Finish exits to model", async () => {
    const user = userEvent.setup();
    render(<SketchChromeBar />);
    act(() => toolStore.getState().setMode("sketch"));

    expect(screen.getByText("Editing Sketch 2")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /Finish sketch/ }));

    expect(toolStore.getState().mode).toBe("model");
    expect(screen.queryByText(/Editing/)).toBeNull();
  });

  it("Cancel exits to model", async () => {
    const user = userEvent.setup();
    render(<SketchChromeBar />);
    act(() => toolStore.getState().setMode("sketch"));

    await user.click(screen.getByRole("button", { name: /Cancel/ }));
    expect(toolStore.getState().mode).toBe("model");
  });
});
