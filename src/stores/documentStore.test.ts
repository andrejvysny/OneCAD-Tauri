/*
 * documentStore seeding gate: under a real Tauri webview the store starts EMPTY
 * (hydrated by backend `projection-updated`); in a plain browser / vitest it keeps
 * the `seedMockDocument()` demo so the mock-driven UI + suites render.
 *
 * The initial projection is chosen at module-init from `window.__TAURI_INTERNALS__`,
 * so each case resets modules and re-imports with the flag toggled.
 */
import { describe, it, expect, afterEach, vi } from "vitest";

const TAURI = "__TAURI_INTERNALS__";

afterEach(() => {
  delete (window as unknown as Record<string, unknown>)[TAURI];
  vi.resetModules();
});

describe("documentStore seeding gate", () => {
  it("seeds the mock document when NOT under Tauri (browser / vitest)", async () => {
    delete (window as unknown as Record<string, unknown>)[TAURI];
    vi.resetModules();
    const { documentStore } = await import("./documentStore");
    const s = documentStore.getState();
    expect(s.status).toBe("ready");
    expect(s.title).toBe("Bracket v2");
    expect(Object.keys(s.bodies)).toContain("body1");
  });

  it("starts EMPTY under a Tauri webview (awaits backend hydration)", async () => {
    (window as unknown as Record<string, unknown>)[TAURI] = {};
    vi.resetModules();
    const { documentStore } = await import("./documentStore");
    const s = documentStore.getState();
    expect(s.status).toBe("empty");
    expect(s.title).toBe("");
    expect(Object.keys(s.bodies)).toHaveLength(0);
    expect(s.features).toHaveLength(0);
  });

  it("emptyDocument() is the no-document projection", async () => {
    const { emptyDocument } = await import("./documentStore");
    expect(emptyDocument()).toEqual({
      status: "empty",
      revision: 0,
      title: "",
      dirty: false,
      bodies: {},
      sketches: {},
      features: [],
    });
  });
});
