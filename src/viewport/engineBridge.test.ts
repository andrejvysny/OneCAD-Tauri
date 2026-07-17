import { describe, it, expect, afterEach } from "vitest";
import { renderHook, act } from "@testing-library/react";
import {
  setViewportEngine,
  getViewportEngine,
  useViewportEngine,
} from "./engineBridge";
import type { ViewportEngine } from "./engine/ViewportEngine";

// A minimal stand-in — the bridge only stores/returns the reference.
const fakeEngine = { id: "fake" } as unknown as ViewportEngine;

afterEach(() => setViewportEngine(null));

describe("engineBridge registry", () => {
  it("get reflects set, and null clears", () => {
    expect(getViewportEngine()).toBeNull();
    setViewportEngine(fakeEngine);
    expect(getViewportEngine()).toBe(fakeEngine);
    setViewportEngine(null);
    expect(getViewportEngine()).toBeNull();
  });

  it("useViewportEngine subscribes and re-renders on change", () => {
    const { result } = renderHook(() => useViewportEngine());
    expect(result.current).toBeNull();
    act(() => setViewportEngine(fakeEngine));
    expect(result.current).toBe(fakeEngine);
    act(() => setViewportEngine(null));
    expect(result.current).toBeNull();
  });
});
