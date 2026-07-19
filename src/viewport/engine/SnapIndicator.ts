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

/** Marker glyph shapes, one per snap-type family (constant screen size). */
type MarkerGlyph = "dot" | "diamond" | "cross" | "ring";

/** Map a snap kind to its marker glyph. New M6c types get distinct shapes so the
 *  marker (not just the hint chip) tells endpoint/quadrant/intersection/onCurve
 *  apart, matching the legacy per-type snap markers. */
function glyphFor(kind: SnapResult["kind"]): MarkerGlyph {
  switch (kind) {
    case "quadrant":
      return "diamond";
    case "intersection":
      return "cross";
    case "onCurve":
      return "ring";
    default:
      return "dot"; // endpoint / midpoint / center / grid / align*
  }
}

/** Render a 32×32 glyph sprite (white on transparent; tinted by the material). */
function makeGlyphTexture(glyph: MarkerGlyph): THREE.Texture | null {
  const size = 32;
  const canvas = document.createElement("canvas");
  canvas.width = size;
  canvas.height = size;
  const ctx = canvas.getContext("2d");
  if (!ctx) return null; // jsdom / no 2D context — fall back to a plain point
  // White mask — the PointsMaterial tints it with the (token-derived) marker
  // color; the CSS keyword keeps the tokens-only hex gate clean.
  ctx.strokeStyle = "white";
  ctx.fillStyle = "white";
  ctx.lineWidth = 3;
  const c = size / 2;
  const r = 9;
  ctx.beginPath();
  switch (glyph) {
    case "dot":
      ctx.arc(c, c, r * 0.7, 0, Math.PI * 2);
      ctx.fill();
      break;
    case "diamond":
      ctx.moveTo(c, c - r);
      ctx.lineTo(c + r, c);
      ctx.lineTo(c, c + r);
      ctx.lineTo(c - r, c);
      ctx.closePath();
      ctx.stroke();
      break;
    case "cross":
      ctx.moveTo(c - r, c - r);
      ctx.lineTo(c + r, c + r);
      ctx.moveTo(c + r, c - r);
      ctx.lineTo(c - r, c + r);
      ctx.stroke();
      break;
    case "ring":
      ctx.arc(c, c, r, 0, Math.PI * 2);
      ctx.stroke();
      break;
  }
  const tex = new THREE.CanvasTexture(canvas);
  tex.needsUpdate = true;
  return tex;
}

export class SnapIndicator {
  private readonly group = new THREE.Group();
  private readonly marker: THREE.Points;
  private readonly markerMat: THREE.PointsMaterial;
  private readonly guides: THREE.LineSegments;
  private readonly guideMat: THREE.LineDashedMaterial;
  private readonly hintEl: HTMLElement;
  private readonly _basis = new THREE.Matrix4();
  private readonly glyphTextures: Partial<Record<MarkerGlyph, THREE.Texture | null>> = {};
  private currentGlyph: MarkerGlyph | null = null;
  private plane: SketchPlane | null = null;
  private hintRegistered = false;

  constructor(private readonly deps: SnapIndicatorDeps) {
    this.group.name = "snapIndicator";
    this.group.visible = false;
    this.group.matrixAutoUpdate = false;
    deps.interactionRoot.add(this.group);

    this.markerMat = new THREE.PointsMaterial({
      color: palette.sketchUnder(),
      size: 11,
      sizeAttenuation: false,
      depthTest: false,
      transparent: true,
      alphaTest: 0.4,
    });
    this.marker = new THREE.Points(
      new THREE.BufferGeometry().setAttribute("position", new THREE.Float32BufferAttribute([0, 0, 0], 3)),
      this.markerMat,
    );
    this.marker.renderOrder = 6;
    this.group.add(this.marker);
    this.setGlyph("dot");

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

  /** Swap the marker sprite to the given glyph (lazily building its texture). */
  private setGlyph(glyph: MarkerGlyph): void {
    if (this.currentGlyph === glyph) return;
    if (!(glyph in this.glyphTextures)) this.glyphTextures[glyph] = makeGlyphTexture(glyph);
    this.markerMat.map = this.glyphTextures[glyph] ?? null;
    this.markerMat.needsUpdate = true;
    this.currentGlyph = glyph;
  }

  /** Update from a snap result. `showHints` gates the hint chip only. */
  show(snap: SnapResult, showHints: boolean): void {
    if (!this.plane || !snap.snapped) {
      this.hide();
      return;
    }
    this.group.visible = true;
    this.setGlyph(glyphFor(snap.kind));

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
    for (const tex of Object.values(this.glyphTextures)) tex?.dispose();
    this.markerMat.dispose();
    this.guides.geometry.dispose();
    this.guideMat.dispose();
    if (this.hintRegistered) this.deps.overlay.unregister(HINT_ID);
    this.hintEl.remove();
    this.deps.interactionRoot.remove(this.group);
  }
}
