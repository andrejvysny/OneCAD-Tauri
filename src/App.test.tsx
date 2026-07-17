import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
// Imported via the "@" alias to prove path resolution works in build + test.
import App from "@/App";

describe("App", () => {
  it("renders the OneCAD wordmark", () => {
    render(<App />);
    expect(screen.getByText("OneCAD")).toBeInTheDocument();
  });
});
