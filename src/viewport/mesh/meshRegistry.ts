/*
 * Mesh registry — a plain module Map<bodyId, MeshEntry> living OUTSIDE zustand.
 *
 * GPU geometry is heavy, imperative, and must be disposed deterministically, so
 * it never belongs in a projection store. `buildBodyObjects` turns a parsed
 * BodyMeshView into THREE geometry with ZERO-COPY attributes (positions/normals/
 * index alias the MESH1 blob directly); edges are expanded into LineSegments
 * endpoints (the only non-trivial transform). `swap` double-buffers: the new
 * entry is published immediately and the old one is disposed on the NEXT frame
 * (via flushDisposals) so nothing that referenced it this frame reads freed
 * buffers. `disposeAll` + a dev leak tripwire guarantee the registry is empty
 * after a document closes.
 */
import * as THREE from "three";
import type { BodyMeshView } from "./parseMeshPayload";
import { TopoIndex } from "./faceRangeIndex";

export interface MeshEntry {
  readonly bodyId: string;
  readonly meshRev: number;
  readonly view: BodyMeshView;
  /** Faces: indexed geometry, attributes alias the blob (zero-copy). */
  readonly geometry: THREE.BufferGeometry;
  /** Edges: expanded LineSegments endpoints (null when the mesh has no edges). */
  readonly edgeGeometry: THREE.BufferGeometry | null;
  /** Triangle index → face ordinal → lazy face id. */
  readonly faceIndex: TopoIndex;
  /** Segment ordinal → edge ordinal → lazy edge id (null when no edges). */
  readonly edgeIndex: TopoIndex | null;
  /** Packed {firstSeg, segCount} per edge, for edge-highlight drawRange (null when no edges). */
  readonly edgeSegmentRanges: Uint32Array | null;
  dispose(): void;
}

const registry = new Map<string, MeshEntry>();
const pendingDisposal: MeshEntry[] = [];
let liveGeometryCount = 0;
/** Incremented whenever the leak tripwire catches a non-empty registry on close. */
export let leakTripwireCount = 0;

/**
 * Build THREE geometry from a parsed view. Does NOT insert into the registry —
 * call {@link swap} to publish it. Face attributes are zero-copy views over the
 * MESH1 blob; only the edge segment buffer is materialised.
 */
export function buildBodyObjects(view: BodyMeshView, bodyId: string, meshRev: number): MeshEntry {
  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.BufferAttribute(view.positions, 3));
  if (view.normals) {
    geometry.setAttribute("normal", new THREE.BufferAttribute(view.normals, 3));
  }
  geometry.setIndex(new THREE.BufferAttribute(view.indices, 1));
  geometry.setDrawRange(0, view.indices.length);
  const bmin = view.bboxMin;
  const bmax = view.bboxMax;
  geometry.boundingBox = new THREE.Box3(
    new THREE.Vector3(bmin[0], bmin[1], bmin[2]),
    new THREE.Vector3(bmax[0], bmax[1], bmax[2]),
  );
  geometry.boundingSphere = geometry.boundingBox.getBoundingSphere(new THREE.Sphere());

  const faceIndex = new TopoIndex(view.faceRanges, view.faceCount, view.faceIdOffsets, view.faceIdChars);

  let edgeGeometry: THREE.BufferGeometry | null = null;
  let edgeIndex: TopoIndex | null = null;
  let edgeSegmentRanges: Uint32Array | null = null;

  if (view.hasEdges && view.edgeRanges && view.edgePositions && view.edgeIdOffsets && view.edgeIdChars) {
    const { positions, segRanges, segTotal } = expandEdgeSegments(
      view.edgePositions,
      view.edgeRanges,
      view.edgeCount,
    );
    edgeGeometry = new THREE.BufferGeometry();
    edgeGeometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
    edgeGeometry.setDrawRange(0, segTotal * 2);
    edgeSegmentRanges = segRanges;
    edgeIndex = new TopoIndex(segRanges, view.edgeCount, view.edgeIdOffsets, view.edgeIdChars);
  }

  liveGeometryCount += edgeGeometry ? 2 : 1;
  let disposed = false;
  return {
    bodyId,
    meshRev,
    view,
    geometry,
    edgeGeometry,
    faceIndex,
    edgeIndex,
    edgeSegmentRanges,
    dispose() {
      if (disposed) return;
      disposed = true;
      geometry.dispose();
      edgeGeometry?.dispose();
      liveGeometryCount -= edgeGeometry ? 2 : 1;
    },
  };
}

/**
 * Expand per-edge polylines into GL_LINES segment endpoints. Edge `e` occupies
 * segments `[firstSeg, firstSeg+pointCount-1)`; segment ordinal → edge maps via
 * `segRanges` (packed {firstSeg, segCount}).
 */
export function expandEdgeSegments(
  edgePositions: Float32Array,
  edgeRanges: Uint32Array,
  edgeCount: number,
): { positions: Float32Array; segRanges: Uint32Array; segTotal: number } {
  let segTotal = 0;
  for (let e = 0; e < edgeCount; e++) {
    segTotal += Math.max(0, edgeRanges[e * 2 + 1] - 1);
  }
  const positions = new Float32Array(segTotal * 6); // 2 verts × 3 floats per segment
  const segRanges = new Uint32Array(edgeCount * 2);
  let segCursor = 0;
  let out = 0;
  for (let e = 0; e < edgeCount; e++) {
    const firstPoint = edgeRanges[e * 2];
    const pointCount = edgeRanges[e * 2 + 1];
    const segCount = Math.max(0, pointCount - 1);
    segRanges[e * 2] = segCursor;
    segRanges[e * 2 + 1] = segCount;
    for (let p = 0; p < segCount; p++) {
      const a = (firstPoint + p) * 3;
      const b = (firstPoint + p + 1) * 3;
      positions[out++] = edgePositions[a];
      positions[out++] = edgePositions[a + 1];
      positions[out++] = edgePositions[a + 2];
      positions[out++] = edgePositions[b];
      positions[out++] = edgePositions[b + 1];
      positions[out++] = edgePositions[b + 2];
    }
    segCursor += segCount;
  }
  return { positions, segRanges, segTotal };
}

/** Publish `next` for its body; queue any previous entry for next-frame disposal. */
export function swap(bodyId: string, next: MeshEntry): void {
  const prev = registry.get(bodyId);
  registry.set(bodyId, next);
  if (prev && prev !== next) pendingDisposal.push(prev);
}

/** Remove a body's entry, queuing its geometry for next-frame disposal. */
export function remove(bodyId: string): void {
  const prev = registry.get(bodyId);
  if (prev) {
    registry.delete(bodyId);
    pendingDisposal.push(prev);
  }
}

export function getEntry(bodyId: string): MeshEntry | undefined {
  return registry.get(bodyId);
}

export function registrySize(): number {
  return registry.size;
}

/** Dispose entries queued by a previous swap/remove. Call once per rendered frame. */
export function flushDisposals(): void {
  if (pendingDisposal.length === 0) return;
  for (const e of pendingDisposal) e.dispose();
  pendingDisposal.length = 0;
}

/**
 * Dispose everything and clear the registry (document close). Dev leak tripwire:
 * after this the registry MUST be empty and no live geometry may remain — a
 * violation logs console.error and bumps {@link leakTripwireCount}.
 */
export function disposeAll(): void {
  for (const e of registry.values()) e.dispose();
  registry.clear();
  flushDisposals();
  if (registrySize() !== 0 || liveGeometryCount !== 0) {
    leakTripwireCount++;
    // eslint-disable-next-line no-console
    console.error(
      `[meshRegistry] leak tripwire: size=${registrySize()} liveGeometries=${liveGeometryCount} after disposeAll`,
    );
  }
}

/** Test-only: reset internal counters (does NOT dispose — call disposeAll first). */
export function __resetRegistryForTests(): void {
  registry.clear();
  pendingDisposal.length = 0;
  liveGeometryCount = 0;
  leakTripwireCount = 0;
}
