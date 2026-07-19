import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { HistoryList, type HistoryRowActions } from "./HistoryList";
import type { FeatureMeta } from "@/stores/documentStore";

const items: FeatureMeta[] = [
  { id: "f1", kind: "sketch", label: "Sketch 1", valueText: "", status: "ok" },
  { id: "f2", kind: "extrude", label: "Extrude", valueText: "25.0 mm", status: "ok" },
  { id: "f3", kind: "fillet", label: "Fillet", valueText: "2.0 mm", status: "ok" },
];

describe("HistoryList", () => {
  it("renders one interactive chip per feature with its value", () => {
    render(<HistoryList items={items} onSelect={() => {}} />);
    expect(screen.getByTestId("history-row-f1")).toBeInTheDocument();
    expect(screen.getByText("25.0 mm")).toBeInTheDocument();
    expect(screen.getByTestId("history-row-f2")).toHaveAttribute("role", "button");
  });

  it("calls onSelect with the feature id on click", () => {
    const onSelect = vi.fn();
    render(<HistoryList items={items} onSelect={onSelect} />);
    fireEvent.click(screen.getByTestId("history-row-f2"));
    expect(onSelect).toHaveBeenCalledWith("f2");
  });

  it("calls onEdit with the feature on double-click", () => {
    const onEdit = vi.fn();
    render(<HistoryList items={items} onEdit={onEdit} />);
    fireEvent.doubleClick(screen.getByTestId("history-row-f2"));
    expect(onEdit).toHaveBeenCalledWith(items[1]);
  });

  it("highlights the selected feature", () => {
    render(<HistoryList items={items} selectedId="f2" />);
    expect(screen.getByTestId("history-row-f2").className).toContain("bg-sel-bg");
    expect(screen.getByTestId("history-row-f1").className).not.toContain("bg-sel-bg");
  });

  it("is non-interactive (no button role) without handlers", () => {
    render(<HistoryList items={items} />);
    expect(screen.getByTestId("history-row-f1")).not.toHaveAttribute("role");
  });
});

describe("HistoryList row affordances (M4b)", () => {
  const actions = (over: Partial<HistoryRowActions> = {}): ((i: FeatureMeta) => HistoryRowActions) => {
    const base: HistoryRowActions = {
      suppressed: false,
      onToggleSuppress: vi.fn(),
      onRoll: vi.fn(),
      onDelete: vi.fn(),
      ...over,
    };
    return () => base;
  };

  it("shows suppress / roll / delete affordances only with rowActions", () => {
    const { rerender } = render(<HistoryList items={items} onSelect={() => {}} />);
    expect(screen.queryByTestId("history-suppress-f2")).toBeNull();
    rerender(<HistoryList items={items} onSelect={() => {}} rowActions={actions()} />);
    expect(screen.getByTestId("history-suppress-f2")).toBeInTheDocument();
    expect(screen.getByTestId("history-roll-f2")).toBeInTheDocument();
    expect(screen.getByTestId("history-delete-f2")).toBeInTheDocument();
  });

  it("calls onToggleSuppress / onRoll with the item", () => {
    const onToggleSuppress = vi.fn();
    const onRoll = vi.fn();
    render(<HistoryList items={items} rowActions={actions({ onToggleSuppress, onRoll })} />);
    fireEvent.click(screen.getByTestId("history-suppress-f2"));
    expect(onToggleSuppress).toHaveBeenCalledWith(items[1]);
    fireEvent.click(screen.getByTestId("history-roll-f2"));
    expect(onRoll).toHaveBeenCalledWith(items[1]);
  });

  it("delete requires a second confirm click (no browser confirm)", () => {
    const onDelete = vi.fn();
    render(<HistoryList items={items} rowActions={actions({ onDelete })} />);
    fireEvent.click(screen.getByTestId("history-delete-f2")); // arms confirm
    expect(onDelete).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("history-delete-confirm-f2")); // confirms
    expect(onDelete).toHaveBeenCalledWith(items[1]);
  });

  it("a suppressed row dims (line-through) and keeps its icon visible", () => {
    render(<HistoryList items={items} rowActions={actions({ suppressed: true })} />);
    expect(screen.getByTestId("history-row-f2").className).toContain("opacity-60");
    // The label carries a strike-through as the suppressed signal.
    expect(screen.getByText("Extrude").className).toContain("line-through");
  });

  it("affordance clicks do not also select the row", () => {
    const onSelect = vi.fn();
    render(<HistoryList items={items} onSelect={onSelect} rowActions={actions()} />);
    fireEvent.click(screen.getByTestId("history-roll-f2"));
    expect(onSelect).not.toHaveBeenCalled();
  });
});
