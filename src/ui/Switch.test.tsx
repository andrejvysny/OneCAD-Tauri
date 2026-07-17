import { describe, it, expect } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Switch } from "@/ui/Switch";

function Harness({ initial = false }: { initial?: boolean }) {
  const [on, setOn] = useState(initial);
  return <Switch checked={on} onChange={setOn} ariaLabel="Snap to grid" />;
}

describe("Switch", () => {
  it("exposes role=switch with aria-checked reflecting state", () => {
    render(<Harness initial />);
    const sw = screen.getByRole("switch", { name: "Snap to grid" });
    expect(sw).toHaveAttribute("aria-checked", "true");
  });

  it("toggles on click", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const sw = screen.getByRole("switch", { name: "Snap to grid" });
    expect(sw).toHaveAttribute("aria-checked", "false");
    await user.click(sw);
    expect(sw).toHaveAttribute("aria-checked", "true");
    await user.click(sw);
    expect(sw).toHaveAttribute("aria-checked", "false");
  });

  it("does not toggle when disabled", async () => {
    const user = userEvent.setup();
    let calls = 0;
    render(
      <Switch
        checked={false}
        disabled
        onChange={() => {
          calls += 1;
        }}
        ariaLabel="Disabled"
      />,
    );
    await user.click(screen.getByRole("switch", { name: "Disabled" }));
    expect(calls).toBe(0);
  });
});
