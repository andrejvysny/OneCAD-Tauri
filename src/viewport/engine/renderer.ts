/*
 * Renderer construction — the SOLE place a WebGL/WebGPU renderer is created.
 *
 * WebGL is the default and the only tested path. A WebGPU path exists behind the
 * `experimentalWebGpu` preference (capability-detected via navigator.gpu), using
 * a dynamic `import("three/webgpu")` so the WebGPU build is code-split out of the
 * default bundle. Any WebGPU failure falls back to WebGL.
 */
import * as THREE from "three";
import { palette } from "./palette";

/** The subset of renderer API the engine relies on (WebGL and WebGPU both satisfy it). */
export interface CadRenderer {
  domElement: HTMLCanvasElement;
  setSize(width: number, height: number, updateStyle?: boolean): void;
  setPixelRatio(ratio: number): void;
  setClearColor(color: THREE.ColorRepresentation, alpha?: number): void;
  render(scene: THREE.Scene, camera: THREE.Camera): void | Promise<void>;
  dispose(): void;
}

export interface RendererPrefs {
  /** Flag-gated: attempt WebGPU when available. Default false → WebGL. */
  experimentalWebGpu?: boolean;
}

export interface RendererHandle {
  renderer: CadRenderer;
  isWebGPU: boolean;
  dispose(): void;
}

async function webGpuAvailable(): Promise<boolean> {
  try {
    // @webgpu/types is not installed; probe structurally to avoid the dep.
    const gpu = (navigator as unknown as {
      gpu?: { requestAdapter(): Promise<unknown> };
    }).gpu;
    if (!gpu) return false;
    const adapter = await gpu.requestAdapter();
    return adapter != null;
  } catch {
    return false;
  }
}

function createWebGl(canvas: HTMLCanvasElement): RendererHandle {
  const renderer = new THREE.WebGLRenderer({
    canvas,
    antialias: true,
    powerPreference: "high-performance",
    // On-demand rendering: retain the drawing buffer so the last frame keeps
    // displaying while idle (and is captured by screenshots/thumbnails). Without
    // this a demand-driven canvas composites blank between renders.
    preserveDrawingBuffer: true,
  });
  renderer.setClearColor(palette.clear(), 1);
  return {
    renderer,
    isWebGPU: false,
    dispose() {
      // forceContextLoss frees the GL context promptly (StrictMode re-inits).
      renderer.forceContextLoss();
      renderer.dispose();
    },
  };
}

async function createWebGpu(canvas: HTMLCanvasElement): Promise<RendererHandle> {
  // Dynamic import keeps the WebGPU build out of the default chunk.
  const { WebGPURenderer } = await import("three/webgpu");
  const renderer = new WebGPURenderer({ canvas, antialias: true });
  await renderer.init();
  renderer.setClearColor(palette.clear(), 1);
  return {
    renderer: renderer as unknown as CadRenderer,
    isWebGPU: true,
    dispose() {
      renderer.dispose();
    },
  };
}

/**
 * Construct a renderer for `canvas`. Returns a handle whose `dispose()` fully
 * releases GPU resources. WebGPU is attempted only when the flag is set AND the
 * capability check passes; otherwise (or on any error) WebGL is used.
 */
export async function createRenderer(
  canvas: HTMLCanvasElement,
  prefs: RendererPrefs = {},
): Promise<RendererHandle> {
  if (prefs.experimentalWebGpu && (await webGpuAvailable())) {
    try {
      return await createWebGpu(canvas);
    } catch (err) {
      // Fall through to WebGL — WebGL is the tested path.
      console.warn("[viewport] WebGPU init failed, falling back to WebGL:", err);
    }
  }
  return createWebGl(canvas);
}
