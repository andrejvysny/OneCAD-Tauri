/*
 * HighlightLayer — hover + selected highlighting via the plan's zero-copy path.
 *
 * Each highlighted face/edge is a tiny THREE.Mesh/LineSegments whose geometry is
 * a SHALLOW clone that shares the body geometry's BufferAttributes (position/
 * normal/index) and just narrows `drawRange` to the face/edge slice. No vertex
 * data is copied. Highlight geometries therefore own NO unique GPU buffers, so
 * they are NEVER disposed (disposing would free the shared attributes the real
 * body still uses) — teardown only removes the objects and disposes the layer's
 * own shared materials. N selected elements ⇒ N tiny meshes in interactionRoot.
 *
 * `setState` (selection change) and `refresh` (after a mesh swap) rebuild against
 * the CURRENT registry, dropping any highlight whose body no longer has a mesh.
 */
import * as THREE from "three";
import type { EntityRef } from "@/stores/selectionStore";
import { getEntry, type MeshEntry } from "../mesh/meshRegistry";
import { palette } from "./palette";

// ── Pure drawRange math (unit-tested) ───────────────────────────────────────

/** Face ordinal → indexed-geometry drawRange (index units: 3 per triangle). */
export function faceDrawRange(
  faceRanges: Uint32Array,
  faceOrdinal: number,
): { start: number; count: number } {
  return { start: faceRanges[faceOrdinal * 2] * 3, count: faceRanges[faceOrdinal * 2 + 1] * 3 };
}

/** Edge ordinal → LineSegments drawRange (vertex units: 2 per segment). */
export function edgeDrawRange(
  segRanges: Uint32Array,
  edgeOrdinal: number,
): { start: number; count: number } {
  return { start: segRanges[edgeOrdinal * 2] * 2, count: segRanges[edgeOrdinal * 2 + 1] * 2 };
}

export interface HighlightDeps {
  root: THREE.Object3D; // interactionRoot
  invalidate: () => void;
}

const HOVER_OPACITY = 0.35;
const SELECT_OPACITY = 0.28;

export class HighlightLayer {
  private hover: EntityRef | null = null;
  private selected: EntityRef[] = [];
  private readonly objects: THREE.Object3D[] = [];

  // Shared materials (layer-owned — the only thing disposed on teardown).
  private readonly hoverFaceMat: THREE.MeshBasicMaterial;
  private readonly selFaceMat: THREE.MeshBasicMaterial;
  private readonly hoverEdgeMat: THREE.LineBasicMaterial;
  private readonly selEdgeMat: THREE.LineBasicMaterial;

  constructor(private readonly deps: HighlightDeps) {
    this.hoverFaceMat = new THREE.MeshBasicMaterial({
      color: palette.hoverAccent(),
      transparent: true,
      opacity: HOVER_OPACITY,
      depthWrite: false,
      polygonOffset: true,
      polygonOffsetFactor: -1,
      polygonOffsetUnits: -1,
      side: THREE.DoubleSide,
    });
    this.selFaceMat = new THREE.MeshBasicMaterial({
      color: palette.selectedTint(),
      transparent: true,
      opacity: SELECT_OPACITY,
      depthWrite: false,
      polygonOffset: true,
      polygonOffsetFactor: -1,
      polygonOffsetUnits: -1,
      side: THREE.DoubleSide,
    });
    this.hoverEdgeMat = new THREE.LineBasicMaterial({
      color: palette.hoverAccent(),
      depthTest: false,
      transparent: true,
    });
    this.selEdgeMat = new THREE.LineBasicMaterial({
      color: palette.selectedEdge(),
      depthTest: false,
      transparent: true,
    });
  }

  setState(hover: EntityRef | null, selected: EntityRef[]): void {
    this.hover = hover;
    this.selected = selected;
    this.rebuild();
  }

  /** Rebuild against the current registry (call after a mesh swap). */
  refresh(): void {
    this.rebuild();
  }

  private rebuild(): void {
    this.clearObjects();
    for (const ref of this.selected) this.addHighlight(ref, false);
    if (this.hover && !this.selected.some((r) => r.id === this.hover!.id && r.kind === this.hover!.kind)) {
      this.addHighlight(this.hover, true);
    }
    this.deps.invalidate();
  }

  private addHighlight(ref: EntityRef, isHover: boolean): void {
    if (ref.kind === "face" || ref.kind === "edge") {
      if (!ref.bodyId || !ref.topoKey) return;
      const entry = getEntry(ref.bodyId);
      if (!entry) return;
      const obj =
        ref.kind === "face"
          ? this.buildFace(entry, ref.topoKey, isHover)
          : this.buildEdge(entry, ref.topoKey, isHover);
      if (obj) this.attach(obj);
    } else if (ref.kind === "body") {
      const entry = getEntry(ref.id);
      if (entry) this.attach(this.buildBody(entry, isHover));
    }
    // sketch / feature refs have no viewport geometry.
  }

  private buildFace(entry: MeshEntry, topoKey: string, isHover: boolean): THREE.Mesh | null {
    const ord = entry.faceIndex.ordinalForId(topoKey);
    if (ord < 0) return null;
    const g = this.shareIndexed(entry.geometry);
    const { start, count } = faceDrawRange(entry.view.faceRanges, ord);
    g.setDrawRange(start, count);
    const mesh = new THREE.Mesh(g, isHover ? this.hoverFaceMat : this.selFaceMat);
    mesh.renderOrder = 2;
    return mesh;
  }

  private buildEdge(entry: MeshEntry, topoKey: string, isHover: boolean): THREE.LineSegments | null {
    if (!entry.edgeGeometry || !entry.edgeIndex || !entry.edgeSegmentRanges) return null;
    const ord = entry.edgeIndex.ordinalForId(topoKey);
    if (ord < 0) return null;
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", entry.edgeGeometry.getAttribute("position")); // shared
    const { start, count } = edgeDrawRange(entry.edgeSegmentRanges, ord);
    g.setDrawRange(start, count);
    const line = new THREE.LineSegments(g, isHover ? this.hoverEdgeMat : this.selEdgeMat);
    line.renderOrder = 3;
    return line;
  }

  private buildBody(entry: MeshEntry, isHover: boolean): THREE.Mesh {
    const g = this.shareIndexed(entry.geometry);
    g.setDrawRange(0, entry.view.indices.length); // all faces
    const mesh = new THREE.Mesh(g, isHover ? this.hoverFaceMat : this.selFaceMat);
    mesh.renderOrder = 2;
    return mesh;
  }

  /** Shallow clone that SHARES the source's attributes + index (zero-copy). */
  private shareIndexed(src: THREE.BufferGeometry): THREE.BufferGeometry {
    const g = new THREE.BufferGeometry();
    g.setAttribute("position", src.getAttribute("position"));
    const normal = src.getAttribute("normal");
    if (normal) g.setAttribute("normal", normal);
    const index = src.getIndex();
    if (index) g.setIndex(index);
    return g;
  }

  private attach(obj: THREE.Object3D): void {
    this.deps.root.add(obj);
    this.objects.push(obj);
  }

  private clearObjects(): void {
    for (const o of this.objects) {
      this.deps.root.remove(o);
      // Do NOT dispose (o as Mesh).geometry — it shares the body's buffers.
    }
    this.objects.length = 0;
  }

  dispose(): void {
    this.clearObjects();
    this.hoverFaceMat.dispose();
    this.selFaceMat.dispose();
    this.hoverEdgeMat.dispose();
    this.selEdgeMat.dispose();
  }
}
