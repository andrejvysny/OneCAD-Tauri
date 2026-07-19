import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import App from "@/App";
import { StartScreen } from "./StartScreen";
import { appStore } from "@/stores/appStore";

beforeEach(() => {
  appStore.setState({
    screen: "start",
    recents: [],
    recentsStatus: "idle",
    document: null,
    recovery: null,
    recoveryStatus: "ready",
  });
});

/** The name shown on the first (top-left) project card in DOM order. */
function firstCard(): HTMLElement {
  return screen.getAllByTitle(/\.onecad$/)[0];
}

describe("StartScreen", () => {
  it("loads and renders recent projects", async () => {
    render(<StartScreen />);
    expect(await screen.findByText("Bracket v2")).toBeInTheDocument();
    expect(screen.getByText("Gearbox mount")).toBeInTheDocument();
  });

  it("filters recents by case-insensitive name substring", async () => {
    const user = userEvent.setup();
    render(<StartScreen />);
    await screen.findByText("Bracket v2");

    await user.type(screen.getByLabelText("Search projects"), "brack");

    expect(screen.getByText("Bracket v2")).toBeInTheDocument();
    expect(screen.queryByText("Enclosure rev C")).not.toBeInTheDocument();
    expect(screen.queryByText("Gearbox mount")).not.toBeInTheDocument();
  });

  it("toggles sort order between date (default) and name", async () => {
    const user = userEvent.setup();
    render(<StartScreen />);
    await screen.findByText("Bracket v2");

    // Default sort = date desc → newest first.
    expect(within(firstCard()).getByText("Bracket v2")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Date modified/ }));
    await user.click(screen.getByRole("menuitem", { name: "Name" }));

    // Sort = name asc → "Adapter flange" first.
    expect(within(firstCard()).getByText("Adapter flange")).toBeInTheDocument();
  });

  it("shows the search empty-state when nothing matches", async () => {
    const user = userEvent.setup();
    render(<StartScreen />);
    await screen.findByText("Bracket v2");

    await user.type(screen.getByLabelText("Search projects"), "zzz-no-match");

    expect(
      screen.getByText("No projects match your search."),
    ).toBeInTheDocument();
  });

  it("New project transitions to the editor screen", async () => {
    const user = userEvent.setup();
    render(<App />);

    await user.click(screen.getByRole("button", { name: /New project/ }));

    // Editor shell (F-WP3) mounts — its Model⇄Sketch toggle is a stable marker.
    expect(await screen.findByRole("tab", { name: "Model" })).toBeInTheDocument();
  });

  it("shows the recovery card and Restore enters the editor", async () => {
    const user = userEvent.setup();
    appStore.setState({
      screen: "start",
      recents: [],
      recentsStatus: "ready",
      document: null,
      recovery: {
        autosavePath: "/x/autosave/foo.onecad",
        originalPath: "/docs/Bracket.onecad",
        modifiedMs: 1_700_000_000_000,
      },
      recoveryStatus: "ready",
    });
    render(<StartScreen />);

    expect(screen.getByText("Unsaved changes recovered")).toBeInTheDocument();
    expect(screen.getByText("Bracket")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Restore/ }));

    await waitFor(() => expect(appStore.getState().screen).toBe("editor"));
    expect(appStore.getState().recovery).toBeNull();
  });

  it("Discard clears the recovery card without leaving the start screen", async () => {
    const user = userEvent.setup();
    appStore.setState({
      screen: "start",
      recents: [],
      recentsStatus: "ready",
      document: null,
      recovery: {
        autosavePath: "/x/autosave/foo.onecad",
        originalPath: "/docs/Bracket.onecad",
        modifiedMs: 1_700_000_000_000,
      },
      recoveryStatus: "ready",
    });
    render(<StartScreen />);
    expect(screen.getByText("Unsaved changes recovered")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /Discard/ }));

    await waitFor(() => expect(appStore.getState().recovery).toBeNull());
    expect(appStore.getState().screen).toBe("start");
    expect(
      screen.queryByText("Unsaved changes recovered"),
    ).not.toBeInTheDocument();
  });
});
