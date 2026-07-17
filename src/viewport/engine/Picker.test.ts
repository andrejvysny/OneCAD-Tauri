/*
 * Picker pure helpers: screen→world line threshold, edge-vs-face preference,
 * and intersection → PickHit resolution through the registry (fake intersections).
 */
import { describe, it, expect } from "vitest";
import * as THREE from "three";
import {
  linePickThreshold,
  choosePreferredHit,
  resolvePick,
  pickKey,
} from "./Picker";
import { buildBodyObjects, type MeshEntry } from "../mesh/meshRegistry";
import { parseMeshPayload } from "../mesh/parseMeshPayload";
import { makeBoxMesh } from "@/ipc/mockMeshes";

function boxEntry(): MeshEntry {
  return buildBodyObjects(parseMeshPayload(makeBoxMesh()), "body1", 1);
}

function fakeFaceHit(bodyId: string, faceIndex: number): THREE.Intersection {
  return {
    distance: 10,
    point: new THREE.Vector3(40, 0, 0),
    object: Object.assign(new THREE.Object3D(), { userData: { bodyId, kind: "face" } }),
    faceIndex,
    face: { normal: new THREE.Vector3(1, 0, 0) } as unknown as THREE.Face,
  } as unknown as THREE.Intersection;
}

function fakeEdgeHit(bodyId: string, vertexIndex: number, distance = 10): THREE.Intersection {
  return {
    distance,
    point: new THREE.Vector3(40, 30, 15),
    object: Object.assign(new THREE.Object3D(), { userData: { bodyId, kind: "edge" } }),
    index: vertexIndex,
  } as unknown as THREE.Intersection;
}

describe("linePickThreshold", () => {
  it("scales linearly with focus distance (perspective)", () => {
    const cam = new THREE.PerspectiveCamera(76, 1, 0.1, 1000);
    const near = linePickThreshold(cam, 800, 100, 6);
    const far = linePickThreshold(cam, 800, 200, 6);
    expect(near).toBeGreaterThan(0);
    expect(far).toBeCloseTo(near * 2, 5);
  });

  it("uses the frustum height for an orthographic camera", () => {
    const cam = new THREE.OrthographicCamera(-100, 100, 50, -50, 0.1, 1000); // height 100
    // 6px of 600px viewport over a 100-unit frustum = 1 world unit.
    expect(linePickThreshold(cam, 600, 260, 6)).toBeCloseTo(1, 5);
  });
});

describe("choosePreferredHit — edge wins within tolerance, loses when occluded", () => {
  const face = fakeFaceHit("body1", 0); // distance 10
  it("prefers an edge at (or within bias of) the face distance", () => {
    const edge = fakeEdgeHit("body1", 0, 10.1);
    expect(choosePreferredHit(face, edge, 0.5)?.kind).toBe("edge");
  });
  it("keeps the face when the edge is much farther (occluded)", () => {
    const edge = fakeEdgeHit("body1", 0, 40);
    expect(choosePreferredHit(face, edge, 0.5)?.kind).toBe("face");
  });
  it("edge-only and face-only cases", () => {
    expect(choosePreferredHit(null, fakeEdgeHit("body1", 0), 0.5)?.kind).toBe("edge");
    expect(choosePreferredHit(face, null, 0.5)?.kind).toBe("face");
    expect(choosePreferredHit(null, null, 0.5)).toBeNull();
  });
});

describe("resolvePick — intersection → PickHit via the registry", () => {
  const entry = boxEntry();
  const lookup = (id: string) => (id === "body1" ? entry : undefined);

  it("maps a face triangle index to its TopoKey + world anchor", () => {
    const hit = resolvePick(fakeFaceHit("body1", 0), "face", lookup);
    expect(hit).not.toBeNull();
    expect(hit!.kind).toBe("face");
    expect(hit!.topoKey).toBe("f:0");
    expect(hit!.elementId).toBeUndefined(); // pure TopoKeys (no IDS_HAVE_ELEMENTIDS)
    expect(hit!.worldPos.x).toBe(40);
    expect(hit!.surfaceHint?.normal).toEqual([1, 0, 0]);
  });

  it("maps triangle 11 (last) to face f:5", () => {
    expect(resolvePick(fakeFaceHit("body1", 11), "face", lookup)!.topoKey).toBe("f:5");
  });

  it("maps an edge segment (vertexIndex>>1) to its edge TopoKey", () => {
    expect(resolvePick(fakeEdgeHit("body1", 0), "edge", lookup)!.topoKey).toBe("e:0");
    expect(resolvePick(fakeEdgeHit("body1", 10), "edge", lookup)!.topoKey).toBe("e:5");
  });

  it("returns null for an unknown body or missing index", () => {
    expect(resolvePick(fakeFaceHit("ghost", 0), "face", lookup)).toBeNull();
  });
});

describe("pickKey", () => {
  it("is stable for the same element and null-safe", () => {
    const entry = boxEntry();
    const hit = resolvePick(fakeFaceHit("body1", 0), "face", () => entry)!;
    expect(pickKey(hit)).toBe("body1/face/f:0");
    expect(pickKey(null)).toBeNull();
  });
});
