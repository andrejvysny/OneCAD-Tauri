import { describe, it, expect, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { FloatingToolbar } from "./FloatingToolbar";
import { resetStores } from "@/test/resetStores";

describe("FloatingToolbar", () => {
  beforeEach(() => resetStores());

  it("renders the model tool set with Select active by default", () => {
    render(<FloatingToolbar />);
    for (const name of ["Select", "New sketch", "Extrude", "Revolve", "Fillet", "Combine"]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    // Sketch-only tools are absent in model mode.
    expect(screen.queryByRole("button", { name: "Line" })).toBeNull();
    expect(screen.getByRole("button", { name: "Select" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  it("toggles the active tool on click", async () => {
    const user = userEvent.setup();
    render(<FloatingToolbar />);
    await user.click(screen.getByRole("button", { name: "Extrude" }));
    expect(screen.getByRole("button", { name: "Extrude" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Select" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });

  it("swaps to the sketch tool set when entering sketch mode", async () => {
    const user = userEvent.setup();
    render(<FloatingToolbar />);
    // The Model "New sketch" tool enters sketch mode.
    await user.click(screen.getByRole("button", { name: "New sketch" }));

    for (const name of ["Line", "Rectangle", "Circle", "Arc", "Dimension", "Trim", "Mirror"]) {
      expect(screen.getByRole("button", { name })).toBeInTheDocument();
    }
    expect(screen.queryByRole("button", { name: "Extrude" })).toBeNull();
    // Sketch mode defaults to the Line tool.
    expect(screen.getByRole("button", { name: "Line" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );

    // Active tool toggles within sketch mode too.
    await user.click(screen.getByRole("button", { name: "Circle" }));
    expect(screen.getByRole("button", { name: "Circle" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    expect(screen.getByRole("button", { name: "Line" })).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });
});
