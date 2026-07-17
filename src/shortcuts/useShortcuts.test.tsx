import { describe, it, expect, beforeEach } from "vitest";
import { render, act } from "@testing-library/react";
import { resolveBinding } from "./keymap";
import { useShortcuts } from "./useShortcuts";
import { toolStore } from "@/stores/toolStore";
import { selectionStore } from "@/stores/selectionStore";
import { resetStores } from "@/test/resetStores";

function press(key: string, opts: { shift?: boolean } = {}) {
  act(() => {
    window.dispatchEvent(
      new KeyboardEvent("keydown", {
        key,
        shiftKey: opts.shift ?? false,
        bubbles: true,
        cancelable: true,
      }),
    );
  });
}

function Harness() {
  useShortcuts();
  return null;
}

describe("keymap resolveBinding", () => {
  it("resolves the same letter to different tools per mode", () => {
    expect(resolveBinding("r", false, "model")).toEqual({
      type: "tool",
      tool: "revolve",
    });
    expect(resolveBinding("r", false, "sketch")).toEqual({
      type: "tool",
      tool: "rect",
    });
  });

  it("routes S to enter-sketch and Enter to finish-sketch", () => {
    expect(resolveBinding("s", false, "model")).toEqual({ type: "enterSketch" });
    expect(resolveBinding("Enter", false, "sketch")).toEqual({
      type: "finishSketch",
    });
  });

  it("keeps F as the Fillet tool and moves zoom-fit to Shift+F", () => {
    expect(resolveBinding("f", false, "model")).toEqual({
      type: "tool",
      tool: "fillet",
    });
    expect(resolveBinding("f", true, "model")).toEqual({ type: "zoomFit" });
  });
});

describe("useShortcuts", () => {
  beforeEach(() => resetStores());

  it("switches tools mode-scoped (R = revolve in model, rect in sketch)", () => {
    render(<Harness />);

    press("r");
    expect(toolStore.getState().modelTool).toBe("revolve");

    act(() => toolStore.getState().setMode("sketch"));
    press("r");
    expect(toolStore.getState().sketchTool).toBe("rect");
  });

  it("enters sketch mode on S and finishes on Enter", () => {
    render(<Harness />);
    press("s");
    expect(toolStore.getState().mode).toBe("sketch");
    press("Enter");
    expect(toolStore.getState().mode).toBe("model");
  });

  it("runs the Esc ladder: cancel tool → deselect → exit sketch", () => {
    render(<Harness />);
    // Model: arm a tool, then Esc reverts to select before deselecting.
    press("e");
    expect(toolStore.getState().modelTool).toBe("extrude");
    press("Escape");
    expect(toolStore.getState().modelTool).toBe("select");
    // Selection still present (Sketch 2) — next Esc clears it.
    expect(selectionStore.getState().selected.length).toBe(1);
    press("Escape");
    expect(selectionStore.getState().selected.length).toBe(0);
  });

  it("bails when a text input is focused", () => {
    render(
      <>
        <Harness />
        <input data-testid="field" />
      </>,
    );
    const input = document.querySelector("input")!;
    input.focus();
    act(() => {
      input.dispatchEvent(
        new KeyboardEvent("keydown", { key: "e", bubbles: true }),
      );
    });
    expect(toolStore.getState().modelTool).toBe("select");
  });
});
