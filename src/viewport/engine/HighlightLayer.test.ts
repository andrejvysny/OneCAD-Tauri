/*
 * HighlightLayer drawRange math: face ranges are triangle units → index units
 * (×3); edge segment ranges are segment units → LineSegments vertex units (×2).
 */
import { describe, it, expect } from "vitest";
import { faceDrawRange, edgeDrawRange } from "./HighlightLayer";

describe("faceDrawRange (indexed geometry, 3 indices/triangle)", () => {
  // 3 faces: face0 tris[0,3), face1[3,2), face2[5,3).
  const faceRanges = new Uint32Array([0, 3, 3, 2, 5, 3]);
  it("maps face ordinal → {start,count} in index units", () => {
    expect(faceDrawRange(faceRanges, 0)).toEqual({ start: 0, count: 9 });
    expect(faceDrawRange(faceRanges, 1)).toEqual({ start: 9, count: 6 });
    expect(faceDrawRange(faceRanges, 2)).toEqual({ start: 15, count: 9 });
  });
});

describe("edgeDrawRange (LineSegments, 2 vertices/segment)", () => {
  // 2 edges: edge0 segs[0,2), edge1 segs[2,1).
  const segRanges = new Uint32Array([0, 2, 2, 1]);
  it("maps edge ordinal → {start,count} in vertex units", () => {
    expect(edgeDrawRange(segRanges, 0)).toEqual({ start: 0, count: 4 });
    expect(edgeDrawRange(segRanges, 1)).toEqual({ start: 4, count: 2 });
  });
});
