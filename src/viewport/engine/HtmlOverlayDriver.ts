/*
 * HTML overlay driver.
 *
 * A registry of {id, worldPos, el}. Every rendered frame, world positions are
 * projected to screen space and written straight to each element's transform —
 * no React re-render. Elements behind the camera or outside the frustum are
 * hidden. Consumers (dimension inputs, constraint glyphs) land in a later WP;
 * a dev demo label is registered behind ?vpdebug.
 */
import * as THREE from "three";

export interface ScreenPos {
  x: number;
  y: number;
  /** False when behind the camera or outside the clip volume. */
  visible: boolean;
}

/**
 * Pure world→screen projection. `viewProj` is projectionMatrix * viewMatrix
 * (camera.matrixWorldInverse). Uses a Vector4 so the sign of w distinguishes
 * points behind the camera from points in front.
 */
export function projectToScreen(
  world: { x: number; y: number; z: number },
  viewProj: THREE.Matrix4,
  width: number,
  height: number,
): ScreenPos {
  const v = new THREE.Vector4(world.x, world.y, world.z, 1).applyMatrix4(viewProj);
  const behind = v.w <= 1e-9;
  const ndcX = v.x / v.w;
  const ndcY = v.y / v.w;
  const x = (ndcX * 0.5 + 0.5) * width;
  const y = (-ndcY * 0.5 + 0.5) * height;
  const inClip = ndcX >= -1 && ndcX <= 1 && ndcY >= -1 && ndcY <= 1;
  return { x, y, visible: !behind && inClip };
}

interface OverlayItem {
  worldPos: THREE.Vector3;
  el: HTMLElement;
}

export class HtmlOverlayDriver {
  private readonly items = new Map<string, OverlayItem>();
  private readonly viewProj = new THREE.Matrix4();

  register(id: string, el: HTMLElement, worldPos: THREE.Vector3): void {
    el.style.position = "absolute";
    el.style.left = "0";
    el.style.top = "0";
    el.style.willChange = "transform";
    this.items.set(id, { el, worldPos: worldPos.clone() });
  }

  setWorldPos(id: string, worldPos: THREE.Vector3): void {
    const item = this.items.get(id);
    if (item) item.worldPos.copy(worldPos);
  }

  unregister(id: string): void {
    this.items.delete(id);
  }

  get size(): number {
    return this.items.size;
  }

  /** Project all items and write their transforms. Called once per render. */
  update(camera: THREE.Camera, width: number, height: number): void {
    if (this.items.size === 0) return;
    this.viewProj.multiplyMatrices(
      camera.projectionMatrix,
      camera.matrixWorldInverse,
    );
    for (const { el, worldPos } of this.items.values()) {
      const p = projectToScreen(worldPos, this.viewProj, width, height);
      if (!p.visible) {
        el.style.display = "none";
        continue;
      }
      el.style.display = "";
      el.style.transform = `translate(-50%, -50%) translate(${p.x}px, ${p.y}px)`;
    }
  }

  clear(): void {
    this.items.clear();
  }
}
