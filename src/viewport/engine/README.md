# Viewport engine (F-WP4)

Imperative Three.js core for the OneCAD viewport. **No react-three-fiber** — a
`ViewportEngine` class owns everything and a thin `ViewportRoot` React component
bridges it to stores and DOM. Rendering is on-demand.

## HARD INVARIANT — Z-UP, RIGHT-HANDED, verbatim buffers

The world is **Z-up, right-handed**. `camera.up = (0, 0, 1)` for both the
perspective and orthographic cameras (`CameraRig`).

- The ground/grid plane is world **XY at Z = 0**.
- Mesh vertex buffers from the worker (MESH1) are uploaded **verbatim**. The
  kernel already produces Z-up geometry.
- **Never** rotate `scene`, `bodiesRoot`, or any root group to "fix" a Y-up
  look, and never bake an axis swap into ingestion. If something appears
  rotated, the bug is upstream (camera/orientation), not the buffers. Rotating
  content to compensate corrupts picking, normals, and saved coordinates.

Turntable orbit yaws about **world Z**; pitch is clamped to ±(90° − ε) so the
Z-up `lookAt` never degenerates.

## Rendering model — on-demand, single rAF

`invalidate()` marks the frame dirty and schedules **one** `requestAnimationFrame`.
While idle, **no frame is scheduled and nothing renders** (verify under
`?vpdebug`: `window.__vpFrames` stops incrementing). Camera tweens (Home / Fit /
ViewCube snap) keep scheduling frames until they finish, then the loop goes
quiet. There is no continuous render loop.

Inputs that must repaint call `invalidate()` (or go through the controls'
`onChange`, which does). `ResizeObserver` is devicePixelRatio-aware (capped at
2×) for crisp output on HiDPI displays.

## Lifecycle — StrictMode-safe

`init()` and `dispose()` are idempotent. React 19 StrictMode double-invokes mount
effects (mount → unmount → mount); a `dispose()` that races an in-flight async
`init()` still releases the GPU context (the renderer is disposed the moment the
awaited construction resolves after disposal).

## Files

| File                 | Role                                                            |
| -------------------- | --------------------------------------------------------------- |
| `renderer.ts`        | SOLE renderer construction; WebGL default, flag-gated WebGPU.   |
| `ViewportEngine.ts`  | Orchestrator: scene graph, render loop, resize, actions.        |
| `CameraRig.ts`       | Persp+ortho pair; switch preserves apparent size at the pivot.  |
| `CadOrbitControls.ts`| Turntable orbit / pan / zoom-to-cursor; Home/Fit/snap tweens.   |
| `GridPlane.ts`       | Adaptive XY grid (1/5/10 decade step), re-centered on target.   |
| `HtmlOverlayDriver.ts`| Projects world→screen and writes DOM transforms per frame.     |
| `palette.ts`         | Reads design tokens (tokens.css) via getComputedStyle once.     |
| `BodyObject.ts`      | Per-body face Mesh + edge LineSegments; shared body materials.  |
| `Picker.ts`          | rAF-coalesced raycast → face/edge PickHit; edge screen-bias.    |
| `HighlightLayer.ts`  | Hover/selected highlight via shared-attribute drawRange clones. |

Colors come from `palette.ts` (design tokens) — the engine never hard-codes hex.

## Mesh ingestion + picking (F-WP5)

MESH1 blobs are parsed zero-copy (`../mesh/parseMeshPayload.ts`) into typed-array
views, built into GPU geometry in `../mesh/meshRegistry.ts` (a module Map OUTSIDE
zustand — double-buffered swap, old geometry disposed one frame later via
`flushDisposals()` in the render loop; a leak tripwire asserts the registry is
empty on document close). `../mesh/meshSync.ts` (`MeshIngest`) is the app glue:
`document-changed` → fetch visible bodies → swap → BodyObject in `bodiesRoot`.

Picking raycasts `bodiesRoot`; a triangle/segment index is mapped to a face/edge
id by binary search over the MESH1 ranges (`../mesh/faceRangeIndex.ts`), decoding
the id string LAZILY on pick. Highlights are shallow-cloned geometries that SHARE
the body's BufferAttributes and only narrow `drawRange` — so they own no GPU
buffers and are never disposed (that would free the shared buffers). Orbit is
suppressed when an LMB drag starts on geometry (`CadOrbitControls` `hitTest` seam).

## Scene graph

```
scene
├── HemisphereLight + headlight DirectionalLight (follows camera)
├── GridPlane           (world XY, Z=0)
├── bodiesRoot          (body face Mesh + edge LineSegments — F-WP5)
├── sketchRoot          (sketch entities — later WP)
└── interactionRoot     (hover/selected highlight meshes — F-WP5)
```

## Testing note

jsdom has no real WebGL, so the engine's GPU path is only fully verifiable
in-browser (Playwright vs vite). Unit tests cover the **pure** math (camera
apparent-size, orbit yaw/pitch/zoom, grid step, overlay projection, ViewCube
transform) and a mocked-renderer init/dispose smoke test.
