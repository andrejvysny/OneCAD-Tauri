/*
 * DragHandle — the extrude depth gizmo: a cylinder shaft + cone arrowhead at the
 * region centroid, pointing along the sketch-plane normal, kept at a CONSTANT
 * screen size (scaled by world-per-pixel each frame). Hover state brightens it.
 * The engine raycasts it on pointerdown to decide whether a drag grabs the handle
 * (starts an extrude) or orbits.
 */
import * as THREE from "three";
import { palette } from "./palette";

const SHAFT_PX = 46; // handle length in screen pixels
const SHAFT_RADIUS_PX = 2.2;
const CONE_PX = 14;
const CONE_RADIUS_PX = 6;
const HIT_PAD = 1.8; // enlarge the pickable envelope for an easy grab

export interface DragHandleDeps {
  root: THREE.Object3D; // interactionRoot
  invalidate: () => void;
}

export class DragHandle {
  private readonly group = new THREE.Group();
  private readonly shaft: THREE.Mesh;
  private readonly cone: THREE.Mesh;
  private readonly hitCyl: THREE.Mesh; // invisible fat pick target
  private readonly matNormal: THREE.MeshBasicMaterial;
  private readonly matHover: THREE.MeshBasicMaterial;
  private readonly _q = new THREE.Quaternion();
  private readonly _up = new THREE.Vector3(0, 1, 0);
  private hovered = false;

  constructor(private readonly deps: DragHandleDeps) {
    this.group.name = "extrudeHandle";
    this.group.visible = false;
    this.matNormal = new THREE.MeshBasicMaterial({ color: palette.hoverAccent(), depthTest: false, transparent: true, opacity: 0.9 });
    this.matHover = new THREE.MeshBasicMaterial({ color: palette.selectedEdge(), depthTest: false });

    // Geometry authored pointing +Y, sized in "px units" (scaled per frame).
    this.shaft = new THREE.Mesh(
      new THREE.CylinderGeometry(SHAFT_RADIUS_PX, SHAFT_RADIUS_PX, SHAFT_PX, 12),
      this.matNormal,
    );
    this.shaft.position.y = SHAFT_PX / 2;
    this.cone = new THREE.Mesh(new THREE.ConeGeometry(CONE_RADIUS_PX, CONE_PX, 16), this.matNormal);
    this.cone.position.y = SHAFT_PX + CONE_PX / 2;
    this.hitCyl = new THREE.Mesh(
      new THREE.CylinderGeometry(CONE_RADIUS_PX * HIT_PAD, CONE_RADIUS_PX * HIT_PAD, SHAFT_PX + CONE_PX, 8),
      new THREE.MeshBasicMaterial({ visible: false }),
    );
    this.hitCyl.position.y = (SHAFT_PX + CONE_PX) / 2;
    this.hitCyl.userData.extrudeHandle = true;

    this.group.add(this.shaft, this.cone, this.hitCyl);
    this.group.renderOrder = 7;
    this.shaft.renderOrder = 7;
    this.cone.renderOrder = 7;
    deps.root.add(this.group);
  }

  /** Position the handle at `origin`, pointing along unit `dir`. */
  setAnchor(origin: THREE.Vector3, dir: THREE.Vector3): void {
    this.group.position.copy(origin);
    this.group.quaternion.copy(this._q.setFromUnitVectors(this._up, dir.clone().normalize()));
    this.deps.invalidate();
  }

  /** Keep the handle a constant screen size: `worldPerPx` world units per pixel. */
  setScale(worldPerPx: number): void {
    this.group.scale.setScalar(Math.max(worldPerPx, 1e-6));
    this.deps.invalidate();
  }

  setHover(hovered: boolean): void {
    if (hovered === this.hovered) return;
    this.hovered = hovered;
    const mat = hovered ? this.matHover : this.matNormal;
    this.shaft.material = mat;
    this.cone.material = mat;
    this.deps.invalidate();
  }

  setVisible(visible: boolean): void {
    this.group.visible = visible;
    this.deps.invalidate();
  }

  /** True when `raycaster` hits the handle's (fat) pick envelope. */
  raycast(raycaster: THREE.Raycaster): boolean {
    if (!this.group.visible) return false;
    return raycaster.intersectObject(this.hitCyl, false).length > 0;
  }

  dispose(): void {
    this.shaft.geometry.dispose();
    this.cone.geometry.dispose();
    this.hitCyl.geometry.dispose();
    (this.hitCyl.material as THREE.Material).dispose();
    this.matNormal.dispose();
    this.matHover.dispose();
    this.deps.root.remove(this.group);
  }
}
