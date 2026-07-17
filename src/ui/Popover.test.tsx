import { describe, it, expect } from "vitest";
import { useRef, useState } from "react";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Popover } from "@/ui/Popover";

function Harness() {
  const anchor = useRef<HTMLButtonElement | null>(null);
  const [open, setOpen] = useState(true);
  return (
    <div>
      <button ref={anchor} type="button" onClick={() => setOpen((v) => !v)}>
        anchor
      </button>
      <button type="button">outside</button>
      <Popover open={open} onClose={() => setOpen(false)} anchorRef={anchor} caret>
        <div>popover-body</div>
      </Popover>
    </div>
  );
}

describe("Popover", () => {
  it("renders its children when open", () => {
    render(<Harness />);
    expect(screen.getByText("popover-body")).toBeInTheDocument();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });

  it("closes on Escape", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByText("popover-body")).toBeInTheDocument();
    await user.keyboard("{Escape}");
    expect(screen.queryByText("popover-body")).not.toBeInTheDocument();
  });

  it("closes on outside click", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    expect(screen.getByText("popover-body")).toBeInTheDocument();
    await user.click(screen.getByText("outside"));
    expect(screen.queryByText("popover-body")).not.toBeInTheDocument();
  });

  it("keeps open when clicking inside the panel", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    await user.click(screen.getByText("popover-body"));
    expect(screen.getByText("popover-body")).toBeInTheDocument();
  });
});
