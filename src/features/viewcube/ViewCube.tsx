/*
 * ViewCube — a DOM CSS-3D cube (62px, prototype 1c) synced to the camera.
 *
 * The cube's orientation follows the camera via inverse camera quaternion (with
 * a Y-flip: Three is Y-up, CSS is Y-down). Faces are placed in the WORLD frame;
 * the container maps world→screen. Clicking a face snaps the camera to that
 * canonical view (250ms slerp handled by CadOrbitControls). No React re-render
 * on camera move — the transform is written straight to the DOM.
 */
import { useEffect, useRef } from "react";
import * as THREE from "three";
import { cn } from "@/ui/cn";
import { Tooltip } from "@/ui/Tooltip";
import { useViewportStore } from "@/stores/viewportStore";
import { useViewportEngine } from "@/viewport/engineBridge";

export type CubeFace = "TOP" | "BOTTOM" | "FRONT" | "BACK" | "RIGHT" | "LEFT";

interface FaceDef {
  name: CubeFace;
  /** Outward normal = target→camera direction for this view (world, Z-up). */
  normal: THREE.Vector3;
  up: THREE.Vector3;
}

const HALF = 31; // cube is 62px

export const FACES: FaceDef[] = [
  { name: "FRONT", normal: new THREE.Vector3(0, -1, 0), up: new THREE.Vector3(0, 0, 1) },
  { name: "BACK", normal: new THREE.Vector3(0, 1, 0), up: new THREE.Vector3(0, 0, 1) },
  { name: "RIGHT", normal: new THREE.Vector3(1, 0, 0), up: new THREE.Vector3(0, 0, 1) },
  { name: "LEFT", normal: new THREE.Vector3(-1, 0, 0), up: new THREE.Vector3(0, 0, 1) },
  { name: "TOP", normal: new THREE.Vector3(0, 0, 1), up: new THREE.Vector3(0, 1, 0) },
  { name: "BOTTOM", normal: new THREE.Vector3(0, 0, -1), up: new THREE.Vector3(0, -1, 0) },
];

// ---- Pure transform helpers (unit-tested) --------------------------------

/**
 * Container transform mapping the world-frame cube into CSS screen space:
 * S · worldToView, where worldToView = inverse(cameraQuat) and S = diag(1,-1,1)
 * converts Three's Y-up view space to CSS's Y-down space. Returns the 16
 * column-major matrix elements.
 */
export function cubeContainerMatrix(cameraQuat: {
  x: number;
  y: number;
  z: number;
  w: number;
}): number[] {
  const inv = new THREE.Quaternion(
    cameraQuat.x,
    cameraQuat.y,
    cameraQuat.z,
    cameraQuat.w,
  ).invert();
  const m = new THREE.Matrix4().makeRotationFromQuaternion(inv);
  m.premultiply(new THREE.Matrix4().makeScale(1, -1, 1));
  return m.elements.slice();
}

/** World-frame placement of one face: basis [tangent, up, normal] + translate. */
export function faceMatrix(
  normal: THREE.Vector3,
  up: THREE.Vector3,
  half = HALF,
): number[] {
  const n = normal.clone().normalize();
  const u = up.clone().normalize();
  const t = new THREE.Vector3().crossVectors(u, n).normalize();
  const uu = new THREE.Vector3().crossVectors(n, t).normalize();
  const m = new THREE.Matrix4().makeBasis(t, uu, n);
  m.setPosition(n.clone().multiplyScalar(half));
  return m.elements.slice();
}

export function cssMatrix3d(elements: number[]): string {
  return `matrix3d(${elements.join(",")})`;
}

const CANONICAL_DOT = Math.cos((6 * Math.PI) / 180); // within ~6° snaps to a face

/** Label a camera view direction (target→camera): a face name, ISO, or "—". */
export function viewLabelForDirection(dir: {
  x: number;
  y: number;
  z: number;
}): string {
  const d = new THREE.Vector3(dir.x, dir.y, dir.z);
  if (d.lengthSq() < 1e-9) return "—";
  d.normalize();
  for (const f of FACES) {
    if (d.dot(f.normal) >= CANONICAL_DOT) return f.name;
  }
  const iso = 1 / Math.sqrt(3);
  if (
    Math.abs(Math.abs(d.x) - iso) < 0.12 &&
    Math.abs(Math.abs(d.y) - iso) < 0.12 &&
    Math.abs(Math.abs(d.z) - iso) < 0.12
  ) {
    return "ISO";
  }
  return "—";
}

// ---- Component -----------------------------------------------------------

export function ViewCube() {
  const engine = useViewportEngine();
  const label = useViewportStore((s) => s.cameraViewLabel);
  const cubeRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = cubeRef.current;
    if (!engine || !el) return;
    const q = new THREE.Quaternion();
    const apply = () => {
      engine.getCameraQuaternion(q);
      el.style.transform = cssMatrix3d(cubeContainerMatrix(q));
    };
    apply();
    return engine.onCameraChanged(apply);
  }, [engine]);

  return (
    <Tooltip label="ViewCube — click a face to orient">
      <div
        role="group"
        aria-label={`ViewCube (${label})`}
        className="relative h-[62px] w-[62px]"
        style={{ perspective: "260px" }}
      >
        <div
          ref={cubeRef}
          className="absolute inset-0"
          style={{
            transformStyle: "preserve-3d",
            transform: cssMatrix3d(
              cubeContainerMatrix({ x: 0, y: 0, z: 0, w: 1 }),
            ),
          }}
        >
          {FACES.map((f) => (
            <button
              key={f.name}
              type="button"
              aria-label={`${f.name} view`}
              onClick={() =>
                engine?.snapToViewDirection(f.normal.clone())
              }
              className={cn(
                "absolute inset-0 flex cursor-pointer items-center justify-center",
                "rounded-[2px] border border-border bg-white text-[10px] font-semibold",
                "tracking-[0.06em] text-ink-4 shadow-ctrl",
                "hover:bg-sel-bg hover:text-accent",
                "focus-visible:shadow-focus-ring focus-visible:outline-none",
              )}
              style={{
                transform: cssMatrix3d(faceMatrix(f.normal, f.up)),
                backfaceVisibility: "hidden",
                WebkitBackfaceVisibility: "hidden",
              }}
            >
              {f.name}
            </button>
          ))}
        </div>
      </div>
    </Tooltip>
  );
}
