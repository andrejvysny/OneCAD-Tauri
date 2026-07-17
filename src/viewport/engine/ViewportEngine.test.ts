/*
 * Engine init/dispose + render-on-demand smoke test.
 *
 * jsdom has no real WebGL, so the renderer is mocked and rAF is driven manually.
 * This verifies lifecycle (StrictMode-safe idempotent init/dispose) and the
 * on-demand contract (a frame renders only when dirty; idle renders nothing).
 * The actual GPU output is only verifiable in-browser (see README.md).
 */
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

const mocks = vi.hoisted(() => {
  const renderer = {
    domElement: null as unknown as HTMLCanvasElement,
    render: vi.fn(),
    setSize: vi.fn(),
    setPixelRatio: vi.fn(),
    setClearColor: vi.fn(),
    dispose: vi.fn(),
  };
  const handleDispose = vi.fn();
  const createRenderer = vi.fn(async () => ({
    renderer,
    isWebGPU: false,
    dispose: handleDispose,
  }));
  return { renderer, handleDispose, createRenderer };
});

vi.mock("./renderer", () => ({ createRenderer: mocks.createRenderer }));

import { ViewportEngine } from "./ViewportEngine";

let rafCbs: FrameRequestCallback[] = [];
function flushFrame(t = 16): void {
  const cbs = rafCbs;
  rafCbs = [];
  for (const cb of cbs) cb(t);
}

beforeEach(() => {
  rafCbs = [];
  mocks.createRenderer.mockClear();
  mocks.handleDispose.mockClear();
  mocks.renderer.render.mockClear();
  mocks.renderer.dispose.mockClear();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
    rafCbs.push(cb);
    return rafCbs.length;
  });
  vi.stubGlobal("cancelAnimationFrame", () => {});
});

afterEach(() => vi.unstubAllGlobals());

function newDom() {
  return {
    canvas: document.createElement("div"), // engine creates its own <canvas>
    overlay: document.createElement("div"),
  };
}

describe("ViewportEngine lifecycle", () => {
  it("initializes with the mocked renderer and renders on demand", async () => {
    const { canvas, overlay } = newDom();
    const engine = new ViewportEngine();
    await engine.init(canvas, overlay, {});

    expect(mocks.createRenderer).toHaveBeenCalledTimes(1);
    expect(engine.frameCount).toBe(0); // nothing rendered until a frame runs
    expect(rafCbs.length).toBe(1); // a frame is scheduled (dirty)

    flushFrame();
    expect(engine.frameCount).toBe(1);
    expect(mocks.renderer.render).toHaveBeenCalled();

    // Idle: nothing scheduled, nothing renders.
    expect(rafCbs.length).toBe(0);
    flushFrame();
    expect(engine.frameCount).toBe(1);

    // invalidate() re-schedules exactly one frame.
    engine.invalidate();
    expect(rafCbs.length).toBe(1);
    flushFrame();
    expect(engine.frameCount).toBe(2);

    engine.dispose();
  });

  it("dispose() releases the renderer and is idempotent", async () => {
    const { canvas, overlay } = newDom();
    const engine = new ViewportEngine();
    await engine.init(canvas, overlay, {});

    engine.dispose();
    expect(mocks.handleDispose).toHaveBeenCalledTimes(1);
    engine.dispose();
    expect(mocks.handleDispose).toHaveBeenCalledTimes(1);
  });

  it("dispose() racing an in-flight init still releases the renderer", async () => {
    const { canvas, overlay } = newDom();
    const engine = new ViewportEngine();
    const pending = engine.init(canvas, overlay, {});
    engine.dispose(); // before init resolves
    await pending;
    expect(mocks.handleDispose).toHaveBeenCalledTimes(1);
    expect(rafCbs.length).toBe(0); // disposed → nothing scheduled
  });

  it("setProjection swaps the camera and re-renders", async () => {
    const { canvas, overlay } = newDom();
    const engine = new ViewportEngine();
    await engine.init(canvas, overlay, {});
    flushFrame();
    const before = engine.frameCount;

    engine.setProjection("ortho");
    expect(rafCbs.length).toBe(1);
    flushFrame();
    expect(engine.frameCount).toBe(before + 1);

    engine.dispose();
  });
});
