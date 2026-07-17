import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { HistoryList } from "./HistoryList";
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
