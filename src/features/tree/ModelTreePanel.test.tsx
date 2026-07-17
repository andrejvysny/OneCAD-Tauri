import { describe, it, expect, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ModelTreePanel } from "./ModelTreePanel";
import { selectionStore } from "@/stores/selectionStore";
import { documentStore } from "@/stores/documentStore";
import { resetStores } from "@/test/resetStores";

describe("ModelTreePanel", () => {
  beforeEach(() => resetStores());

  it("renders the prototype tree (Body 1 + Sketch 2/4/5) with Sketch 2 selected", () => {
    render(<ModelTreePanel />);
    expect(screen.getByRole("option", { name: /Body 1/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Sketch 4/ })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: /Sketch 2/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("selects a row on click", async () => {
    const user = userEvent.setup();
    render(<ModelTreePanel />);
    await user.click(screen.getByRole("option", { name: /Body 1/ }));
    expect(selectionStore.getState().selected).toEqual([
      { kind: "body", id: "body1" },
    ]);
    expect(screen.getByRole("option", { name: /Body 1/ })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("option", { name: /Sketch 2/ })).toHaveAttribute(
      "aria-selected",
      "false",
    );
  });

  it("toggles visibility in the document store without changing selection", async () => {
    const user = userEvent.setup();
    render(<ModelTreePanel />);
    await user.click(screen.getByRole("option", { name: /Body 1/ }));

    expect(documentStore.getState().sketches.sketch2.visible).toBe(true);
    await user.click(
      screen.getByRole("switch", { name: "Toggle Sketch 2 visibility" }),
    );
    expect(documentStore.getState().sketches.sketch2.visible).toBe(false);
    // Eye click must not steal selection from Body 1.
    expect(selectionStore.getState().selected).toEqual([
      { kind: "body", id: "body1" },
    ]);
  });
});
