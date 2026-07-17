/*
 * BodyObject — a body's scene presence: a face Mesh + an edge LineSegments,
 * both wrapping the registry's zero-copy geometry. The face material carries a
 * polygonOffset so edge lines sit cleanly on top without z-fighting.
 *
 * Materials are SHARED across all bodies (created once by the engine) — a
 * BodyObject owns neither geometry (registry-owned) nor materials, so tearing it
 * down is just removing the group from bodiesRoot. `userData.bodyId`/`kind` let
 * the Picker resolve an intersection back to a body + element.
 */
import * as THREE from "three";
import type { MeshEntry } from "../mesh/meshRegistry";
import { palette } from "./palette";

export interface BodyMaterials {
  face: THREE.MeshStandardMaterial;
  edge: THREE.LineBasicMaterial;
  dispose(): void;
}

/** Create the shared face + edge materials (one set per engine). */
export function createBodyMaterials(): BodyMaterials {
  const face = new THREE.MeshStandardMaterial({
    color: palette.bodyNeutral(),
    metalness: 0.05,
    roughness: 0.75,
    side: THREE.DoubleSide, // closed solids; robust picking regardless of winding
    polygonOffset: true,
    polygonOffsetFactor: 1,
    polygonOffsetUnits: 1,
  });
  const edge = new THREE.LineBasicMaterial({ color: palette.bodyEdge() });
  return {
    face,
    edge,
    dispose() {
      face.dispose();
      edge.dispose();
    },
  };
}

export interface BodyObjectHandle {
  bodyId: string;
  group: THREE.Group;
  setVisible(visible: boolean): void;
}

/** Build the face + edge objects for `entry` under one group. */
export function buildBodyObject(entry: MeshEntry, materials: BodyMaterials): BodyObjectHandle {
  const group = new THREE.Group();
  group.name = `body:${entry.bodyId}`;
  group.userData.bodyId = entry.bodyId;

  const faceMesh = new THREE.Mesh(entry.geometry, materials.face);
  faceMesh.userData.bodyId = entry.bodyId;
  faceMesh.userData.kind = "face";
  group.add(faceMesh);

  if (entry.edgeGeometry) {
    const edges = new THREE.LineSegments(entry.edgeGeometry, materials.edge);
    edges.userData.bodyId = entry.bodyId;
    edges.userData.kind = "edge";
    group.add(edges);
  }

  return {
    bodyId: entry.bodyId,
    group,
    setVisible(visible: boolean) {
      group.visible = visible;
    },
  };
}
