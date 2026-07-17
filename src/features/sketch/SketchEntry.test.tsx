import { describe, it, expect, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FloatingToolbar } from "@/features/toolbar/FloatingToolbar";
import { toolStore } from "@/stores/toolStore";
import { viewportStore } from "@/stores/viewportStore";
import { selectionStore } from "@/stores/selectionStore";
import { resetStores } from "@/test/resetStores";

/**
 * F-WP6 sketch-entry-from-toolbar flow: clicking the model "New sketch" tool
 * enters sketch mode, targets the active sketch, selects it (tree/inspector
 * coherence) and swaps the toolbar to the sketch tool set with Line armed. The
 * SketchController then picks this up (browser only) to open the mock session.
 */
describe("sketch entry from the toolbar", () => {
  beforeEach(() => resetStores());

  it("enters sketch mode and targets the active sketch", async () => {
    const user = userEvent.setup();
    render(<FloatingToolbar />);
    expect(toolStore.getState().mode).toBe("model");

    await user.click(screen.getByRole("button", { name: "New sketch" }));

    expect(toolStore.getState().mode).toBe("sketch");
    expect(viewportStore.getState().activeSketchId).toBe("sketch2");
    expect(selectionStore.getState().selected).toEqual([{ kind: "sketch", id: "sketch2" }]);
    // Sketch tool set is shown, Line armed by default.
    expect(screen.getByRole("button", { name: "Line" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("button", { name: "Extrude" })).toBeNull();
  });
});
