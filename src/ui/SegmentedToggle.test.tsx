import { describe, it, expect } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { SegmentedToggle } from "@/ui/SegmentedToggle";

function Harness() {
  const [value, setValue] = useState<"model" | "sketch">("model");
  return (
    <SegmentedToggle
      ariaLabel="Editing mode"
      value={value}
      onChange={setValue}
      options={[
        { value: "model", label: "Model" },
        { value: "sketch", label: "Sketch" },
      ]}
    />
  );
}

describe("SegmentedToggle", () => {
  it("exposes tablist semantics with the active tab selected", () => {
    render(<Harness />);
    expect(screen.getByRole("tablist", { name: "Editing mode" })).toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Model" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Sketch" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
  });

  it("selects an option on click", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByRole("tab", { name: "Sketch" }));
    expect(screen.getByRole("tab", { name: "Sketch" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    expect(screen.getByRole("tab", { name: "Model" })).toHaveAttribute(
      "aria-selected",
      "false",
    );
  });

  it("moves selection with arrow keys", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const model = screen.getByRole("tab", { name: "Model" });
    model.focus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("tab", { name: "Sketch" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("tab", { name: "Model" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });

  it("wraps with Home/End", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    screen.getByRole("tab", { name: "Model" }).focus();
    await user.keyboard("{End}");
    expect(screen.getByRole("tab", { name: "Sketch" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
    await user.keyboard("{Home}");
    expect(screen.getByRole("tab", { name: "Model" })).toHaveAttribute(
      "aria-selected",
      "true",
    );
  });
});
