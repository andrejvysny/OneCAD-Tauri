import { describe, it, expect, beforeEach } from "vitest";
import { render, screen, within } from "@testing-library/react";
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
});
