import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

// The menu items dispatch through the shared fileActions bridge — mock it so the
// test asserts wiring (which item calls which action) without touching the client.
vi.mock("./fileActions", () => ({
  openDocumentDialog: vi.fn(),
  saveDocument: vi.fn(),
  saveDocumentAs: vi.fn(),
  exportStep: vi.fn(),
  exportStl: vi.fn(),
  exportObj: vi.fn(),
}));

import { FileMenu } from "./FileMenu";
import * as fileActions from "./fileActions";

describe("FileMenu", () => {
  beforeEach(() => vi.clearAllMocks());

  it("opens on click and shows the file actions with shortcuts", async () => {
    const user = userEvent.setup();
    render(<FileMenu />);

    // Closed by default — items are not mounted.
    expect(screen.queryByRole("menuitem", { name: /Save As/ })).toBeNull();

    await user.click(screen.getByRole("button", { name: /File/ }));
    expect(screen.getByRole("menuitem", { name: /Open/ })).toBeInTheDocument();
    expect(screen.getByText("⌘O")).toBeInTheDocument();
    expect(screen.getByText("⌘S")).toBeInTheDocument();
    expect(screen.getByText("⇧⌘S")).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Export STEP/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Export STL/ })).toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: /Export OBJ/ })).toBeInTheDocument();
  });

  it("dispatches each menu item to its file action", async () => {
    const user = userEvent.setup();
    render(<FileMenu />);

    await user.click(screen.getByRole("button", { name: /File/ }));
    await user.click(screen.getByRole("menuitem", { name: /^Save⌘S/ }));
    expect(fileActions.saveDocument).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /File/ }));
    await user.click(screen.getByRole("menuitem", { name: /Save As/ }));
    expect(fileActions.saveDocumentAs).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /File/ }));
    await user.click(screen.getByRole("menuitem", { name: /Export STEP/ }));
    expect(fileActions.exportStep).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /File/ }));
    await user.click(screen.getByRole("menuitem", { name: /Export STL/ }));
    expect(fileActions.exportStl).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /File/ }));
    await user.click(screen.getByRole("menuitem", { name: /Export OBJ/ }));
    expect(fileActions.exportObj).toHaveBeenCalledTimes(1);

    await user.click(screen.getByRole("button", { name: /File/ }));
    await user.click(screen.getByRole("menuitem", { name: /Open/ }));
    expect(fileActions.openDocumentDialog).toHaveBeenCalledTimes(1);
  });
});
