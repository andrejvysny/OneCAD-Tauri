import { describe, it, expect } from "vitest";
import { useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { EyeToggle } from "@/ui/EyeToggle";

function Harness({ initial = true }: { initial?: boolean }) {
  const [on, setOn] = useState(initial);
  return <EyeToggle on={on} onChange={setOn} ariaLabel="Body 1 visibility" />;
}

describe("EyeToggle", () => {
  it("reflects visible state via aria-checked and opacity", () => {
    render(<Harness initial />);
    const btn = screen.getByRole("switch", { name: "Body 1 visibility" });
    expect(btn).toHaveAttribute("aria-checked", "true");
    expect(btn.className).toContain("opacity-[0.85]");
  });

  it("toggles to hidden on click", async () => {
    const user = userEvent.setup();
    render(<Harness initial />);
    const btn = screen.getByRole("switch", { name: "Body 1 visibility" });
    await user.click(btn);
    expect(btn).toHaveAttribute("aria-checked", "false");
    expect(btn.className).toContain("opacity-30");
  });

  it("renders the eye glyph", () => {
    const { container } = render(<Harness initial />);
    expect(container.querySelector("path")).not.toBeNull();
  });
});
