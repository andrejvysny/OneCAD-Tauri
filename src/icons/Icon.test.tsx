import { describe, it, expect } from "vitest";
import { render } from "@testing-library/react";
import { Icon } from "@/icons/Icon";
import { ICON_PATHS, type IconName } from "@/icons/paths";

describe("Icon", () => {
  const names = Object.keys(ICON_PATHS) as IconName[];

  it.each(names)("renders the verbatim path for '%s'", (name) => {
    const { container } = render(<Icon name={name} />);
    const path = container.querySelector("path");
    expect(path).not.toBeNull();
    expect(path?.getAttribute("d")).toBe(ICON_PATHS[name]);
  });

  it("applies prototype defaults (size 15, strokeWidth 1.7, viewBox)", () => {
    const { container } = render(<Icon name="select" />);
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("15");
    expect(svg?.getAttribute("height")).toBe("15");
    expect(svg?.getAttribute("stroke-width")).toBe("1.7");
    expect(svg?.getAttribute("viewBox")).toBe("0 0 24 24");
    expect(svg?.getAttribute("fill")).toBe("none");
    expect(svg?.getAttribute("stroke")).toBe("currentColor");
  });

  it("honours size and strokeWidth overrides", () => {
    const { container } = render(
      <Icon name="plus" size={18} strokeWidth={2} />,
    );
    const svg = container.querySelector("svg");
    expect(svg?.getAttribute("width")).toBe("18");
    expect(svg?.getAttribute("stroke-width")).toBe("2");
  });
});
