import { describe, it, expect } from "vitest";
import type { SketchRegion } from "@/ipc/types";
import {
  profileFromRegion,
  profileBounds,
  prismLocal,
  unitPrismGeometry,
} from "./prismPreview";

/** A 40×40 square region centred at (10, 10), fan-triangulated like mockSketch. */
function squareRegion(): SketchRegion {
  const ring: [number, number][] = [
    [-10, -10],
    [30, -10],
    [30, 30],
    [-10, 30],
  ];
  const positions = [10, 10, ...ring.flat()]; // centroid first, then ring
  const indices: number[] = [];
  for (let i = 0; i < ring.length; i++) indices.push(0, 1 + i, 1 + ((i + 1) % ring.length));
  return { regionId: "r_test", outerLoop: ["e1"], holes: [], previewTriangles: { positions, indices } };
}

describe("profileFromRegion", () => {
  it("extracts the boundary ring (excluding the centroid) + the cap fan", () => {
    const p = profileFromRegion(squareRegion())!;
    expect(p).not.toBeNull();
    expect(p.ring).toEqual([
      [-10, -10],
      [30, -10],
      [30, 30],
      [-10, 30],
    ]);
    // The cap keeps the full fan (centroid at index 0).
    expect(p.cap.positions.slice(0, 2)).toEqual([10, 10]);
  });

  it("returns null when there is no usable triangulation", () => {
    expect(profileFromRegion({ regionId: "r", outerLoop: [], holes: [] })).toBeNull();
  });
});

describe("profileBounds", () => {
  it("computes (u,v) bounds + centroid", () => {
    const b = profileBounds(profileFromRegion(squareRegion())!);
    expect(b).toEqual({ minU: -10, maxU: 30, minV: -10, maxV: 30, centroidU: 10, centroidV: 10 });
  });
});

describe("prismLocal", () => {
  it("puts the bottom cap at z=0 and the top cap at z=depth", () => {
    const prism = prismLocal(profileFromRegion(squareRegion())!, 5);
    let minZ = Infinity;
    let maxZ = -Infinity;
    for (let i = 2; i < prism.positions.length; i += 3) {
      minZ = Math.min(minZ, prism.positions[i]);
      maxZ = Math.max(maxZ, prism.positions[i]);
    }
    expect(minZ).toBe(0);
    expect(maxZ).toBe(5);
  });

  it("emits bottom, top and side faces plus boundary edges", () => {
    const prism = prismLocal(profileFromRegion(squareRegion())!, 3);
    expect(prism.faces).toHaveLength(3); // bottom, top, sides
    // A 4-vertex ring ⇒ 4 side quads = 8 triangles.
    expect(prism.faces[2].triangles).toHaveLength(8);
    // Top + bottom loop + verticals (ring ≤ 12 ⇒ one per vertex).
    expect(prism.edges.length).toBe(2 + 4);
  });
});

describe("unitPrismGeometry", () => {
  it("builds a unit-depth (z ∈ [0,1]) merged indexed geometry", () => {
    const g = unitPrismGeometry(profileFromRegion(squareRegion())!);
    expect(g.positions).toBeInstanceOf(Float32Array);
    expect(g.indices).toBeInstanceOf(Uint32Array);
    expect(g.indices.length % 3).toBe(0);
    let maxZ = -Infinity;
    for (let i = 2; i < g.positions.length; i += 3) maxZ = Math.max(maxZ, g.positions[i]);
    expect(maxZ).toBe(1); // caller scales z by the live depth
  });
});
