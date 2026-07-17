import { describe, it, expect, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { RefObject } from "react";
import { SnapPopover } from "./SnapPopover";
import { settingsStore } from "@/stores/settingsStore";
import { resetStores } from "@/test/resetStores";

const anchorRef = { current: null } as RefObject<HTMLButtonElement | null>;

describe("SnapPopover", () => {
  beforeEach(() => {
    localStorage.clear();
    resetStores();
  });

  it("renders SNAP TO + SHOW sections bound to settings", () => {
    render(<SnapPopover open onClose={() => {}} anchorRef={anchorRef} />);
    expect(screen.getByText("Snap to")).toBeInTheDocument();
    expect(screen.getByText("Show")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Grid" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    expect(
      screen.getByRole("switch", { name: "Distant edges" }),
    ).toHaveAttribute("aria-checked", "false");
  });

  it("persists a toggle to the store and localStorage", async () => {
    const user = userEvent.setup();
    render(<SnapPopover open onClose={() => {}} anchorRef={anchorRef} />);

    await user.click(screen.getByRole("switch", { name: "Grid" }));
    expect(settingsStore.getState().snapTo.grid).toBe(false);

    const raw = localStorage.getItem("onecad.settings");
    expect(raw).not.toBeNull();
    expect(JSON.parse(raw!).state.snapTo.grid).toBe(false);
  });

  it("persists a SHOW toggle", async () => {
    const user = userEvent.setup();
    render(<SnapPopover open onClose={() => {}} anchorRef={anchorRef} />);

    await user.click(screen.getByRole("switch", { name: "Snapping hints" }));
    expect(settingsStore.getState().show.snappingHints).toBe(false);
  });
});
