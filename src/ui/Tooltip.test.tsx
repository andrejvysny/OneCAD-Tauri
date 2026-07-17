import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Tooltip } from "@/ui/Tooltip";

describe("Tooltip", () => {
  it("is hidden until hovered", () => {
    render(
      <Tooltip label="Extrude (E)">
        <button type="button">anchor</button>
      </Tooltip>,
    );
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("appears on hover and disappears on unhover", async () => {
    const user = userEvent.setup();
    render(
      <Tooltip label="Extrude (E)">
        <button type="button">anchor</button>
      </Tooltip>,
    );
    await user.hover(screen.getByText("anchor"));
    expect(screen.getByRole("tooltip")).toHaveTextContent("Extrude (E)");
    await user.unhover(screen.getByText("anchor"));
    expect(screen.queryByRole("tooltip")).not.toBeInTheDocument();
  });

  it("renders when forced open", () => {
    render(
      <Tooltip label="Always" open>
        <span>anchor</span>
      </Tooltip>,
    );
    expect(screen.getByRole("tooltip")).toHaveTextContent("Always");
  });
});
