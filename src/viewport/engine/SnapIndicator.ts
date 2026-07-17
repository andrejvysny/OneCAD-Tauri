/*
 * SnapIndicator — the in-canvas snap feedback (F-WP6, NEW_SPEC §14).
 *
 * Renders, in `interactionRoot` (plane-local under the plane basis):
 *   - a constant-size marker at the snapped point (THREE.Points, sizeAttenuation
 *     off ⇒ crisp regardless of zoom),
 *   - dashed H/V alignment guide lines spanning the plane,
 * plus a hint chip ("Endpoint" / "Vertical" / …) via the HtmlOverlayDriver,
 * honoring `settingsStore.show.snappingHints`.
 *
 * The chip is styled from design tokens through `var(--color-*)` (no raw hex —
 * the tokens-only gate), positioned each frame by the overlay driver.
 */
import * as THREE from "three";
import type { SketchPlane } from "@/ipc/types";
import type { HtmlOverlayDriver } from "./HtmlOverlayDriver";
import type { SnapResult } from "@/tools/sketch/snapEngine";
import { palette } from "./palette";
import { planeBasisMatrix, planePointToWorld } from "./sketchBasis";

const GUIDE_EXTENT = 5000;
const HINT_ID = "__sketch_snap_hint";

interface SnapIndicatorDeps {
  interactionRoot: THREE.Object3D;
  overlay: HtmlOverlayDriver;
  overlayEl: HTMLElement;
  invalidate: () => void;
}

export class SnapIndicator {
  private readonly group = new THREE.Group();
  private readonly marker: THREE.Points;
  private readonly markerMat: THREE.PointsMaterial;
  private readonly guides: THREE.LineSegments;
  private readonly guideMat: THREE.LineDashedMaterial;
  private readonly hintEl: HTMLElement;
  private readonly _basis = new THREE.Matrix4();
  private plane: SketchPlane | null = null;
  private hintRegistered = false;

  constructor(private readonly deps: SnapIndicatorDeps) {
    this.group.name = "snapIndicator";
    this.group.visible = false;
    this.group.matrixAutoUpdate = false;
    deps.interactionRoot.add(this.group);

    this.markerMat = new THREE.PointsMaterial({
      color: palette.sketchUnder(),
      size: 10,
      sizeAttenuation: false,
      depthTest: false,
      transparent: true,
    });
    this.marker = new THREE.Points(
      new THREE.BufferGeometry().setAttribute("position", new THREE.Float32BufferAttribute([0, 0, 0], 3)),
      this.markerMat,
    );
    this.marker.renderOrder = 6;
    this.group.add(this.marker);

    this.guideMat = new THREE.LineDashedMaterial({
      color: palette.sketchUnder(),
      dashSize: 6,
      gapSize: 4,
      depthTest: false,
      transparent: true,
      opacity: 0.75,
    });
    this.guides = new THREE.LineSegments(new THREE.BufferGeometry(), this.guideMat);
    this.guides.renderOrder = 5;
    this.group.add(this.guides);

    this.hintEl = document.createElement("div");
    this.hintEl.dataset.sketchSnapHint = "1";
    this.hintEl.style.font = "500 11px var(--font-ui)";
    this.hintEl.style.padding = "3px 7px";
    this.hintEl.style.borderRadius = "5px";
    this.hintEl.style.whiteSpace = "nowrap";
    this.hintEl.style.pointerEvents = "none";
    this.hintEl.style.background = "var(--color-tooltip)";
    this.hintEl.style.color = "var(--color-tooltip-text)";
    this.hintEl.style.display = "none";
    deps.overlayEl.appendChild(this.hintEl);
  }

  setPlane(plane: SketchPlane): void {
    this.plane = plane;
    planeBasisMatrix(plane, this._basis);
    this.group.matrix.copy(this._basis);
    this.group.matrixWorldNeedsUpdate = true;
  }

  /** Update from a snap result. `showHints` gates the hint chip only. */
  show(snap: SnapResult, showHints: boolean): void {
    if (!this.plane || !snap.snapped) {
      this.hide();
      return;
    }
    this.group.visible = true;

    // Marker (plane-local).
    const pos = this.marker.geometry.getAttribute("position") as THREE.BufferAttribute;
    pos.setXYZ(0, snap.point.x, snap.point.y, 0);
    pos.needsUpdate = true;

    // Guides (plane-local dashed segments across the plane).
    const seg: number[] = [];
    for (const g of snap.guides) {
      if (g.orientation === "vertical") seg.push(g.value, -GUIDE_EXTENT, 0, g.value, GUIDE_EXTENT, 0);
      else seg.push(-GUIDE_EXTENT, g.value, 0, GUIDE_EXTENT, g.value, 0);
    }
    this.guides.geometry.setAttribute("position", new THREE.Float32BufferAttribute(seg, 3));
    this.guides.computeLineDistances();

    // Hint chip.
    if (showHints && snap.label) {
      const world = planePointToWorld(this.plane, snap.point);
      if (!this.hintRegistered) {
        this.deps.overlay.register(HINT_ID, this.hintEl, world);
        this.hintRegistered = true;
      } else {
        this.deps.overlay.setWorldPos(HINT_ID, world);
      }
      this.hintEl.textContent = snap.label;
      this.hintEl.style.display = "";
      // Float the chip up-right of the marker.
      this.hintEl.style.marginLeft = "12px";
      this.hintEl.style.marginTop = "-18px";
    } else {
      this.hintEl.style.display = "none";
    }
    this.deps.invalidate();
  }

  hide(): void {
    if (this.group.visible) {
      this.group.visible = false;
      this.deps.invalidate();
    }
    this.hintEl.style.display = "none";
  }

  dispose(): void {
    this.marker.geometry.dispose();
    this.markerMat.dispose();
    this.guides.geometry.dispose();
    this.guideMat.dispose();
    if (this.hintRegistered) this.deps.overlay.unregister(HINT_ID);
    this.hintEl.remove();
    this.deps.interactionRoot.remove(this.group);
  }
}
