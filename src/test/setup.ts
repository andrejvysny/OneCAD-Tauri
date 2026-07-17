// Extends Vitest's `expect` with jest-dom matchers (toBeInTheDocument, ...)
// and registers the corresponding TypeScript augmentation.
import "@testing-library/jest-dom/vitest";

import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// React Testing Library does not auto-clean without global afterEach.
afterEach(() => {
  cleanup();
});
