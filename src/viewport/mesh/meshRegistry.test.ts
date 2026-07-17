/*
 * Registry: zero-copy geometry build, edge-segment expansion, double-buffer
 * swap (old disposed only on flush), and the document-close leak tripwire.
 */
import { describe, it, expect, beforeEach, vi } from "vitest";
import * as reg from "./meshRegistry";
import { parseMeshPayload } from "./parseMeshPayload";
import { makeBoxMesh } from "@/ipc/mockMeshes";

function box(rev: number): reg.MeshEntry {
  return reg.buildBodyObjects(parseMeshPayload(makeBoxMesh()), "body1", rev);
}

beforeEach(() => {
  reg.disposeAll();
  reg.__resetRegistryForTests();
});

describe("buildBodyObjects", () => {
  it("builds indexed face geometry with zero-copy position/normal attributes", () => {
    const view = parseMeshPayload(makeBoxMesh());
    const entry = reg.buildBodyObjects(view, "body1", 1);
    const pos = entry.geometry.getAttribute("position");
    expect(pos.count).toBe(24);
    // Attribute array is the SAME Float32Array the parser viewed (no copy).
    expect(pos.array).toBe(view.positions);
    expect(entry.geometry.getIndex()!.count).toBe(36); // 12 tris × 3
    expect(entry.edgeGeometry).not.toBeNull();
    expect(entry.faceIndex.count).toBe(6);
    expect(entry.edgeIndex!.count).toBe(12);
    entry.dispose();
  });

  it("maps a picked triangle to its face id via faceIndex", () => {
    const view = parseMeshPayload(makeBoxMesh());
    const entry = reg.buildBodyObjects(view, "body1", 1);
    // Triangle 0 belongs to face 0; triangle 11 to face 5.
    expect(entry.faceIndex.idAt(0)).toBe("f:0");
    expect(entry.faceIndex.idAt(11)).toBe("f:5");
    entry.dispose();
  });
});

describe("expandEdgeSegments", () => {
  it("expands polylines into GL_LINES endpoints with correct segment ranges", () => {
    // 2 edges: edge0 = 3-point polyline (2 segs), edge1 = 2-point (1 seg).
    const edgePositions = new Float32Array([
      0, 0, 0, 1, 0, 0, 2, 0, 0, // edge0 points p0,p1,p2
      0, 1, 0, 0, 2, 0, // edge1 points p3,p4
    ]);
    const edgeRanges = new Uint32Array([0, 3, 3, 2]);
    const { positions, segRanges, segTotal } = reg.expandEdgeSegments(edgePositions, edgeRanges, 2);
    expect(segTotal).toBe(3); // (3-1) + (2-1)
    expect([...segRanges]).toEqual([0, 2, 2, 1]); // edge0 segs[0,2), edge1 segs[2,3)
    // First segment endpoints = p0..p1.
    expect([...positions.slice(0, 6)]).toEqual([0, 0, 0, 1, 0, 0]);
    // Third (edge1's only) segment endpoints = p3..p4.
    expect([...positions.slice(12, 18)]).toEqual([0, 1, 0, 0, 2, 0]);
  });
});

describe("double-buffer swap", () => {
  it("publishes the new entry but disposes the old only on flush", () => {
    const first = box(1);
    reg.swap("body1", first);
    expect(reg.getEntry("body1")).toBe(first);

    const second = box(2);
    const disposeSpy = vi.spyOn(first, "dispose");
    reg.swap("body1", second);
    expect(reg.getEntry("body1")).toBe(second); // new is live immediately
    expect(disposeSpy).not.toHaveBeenCalled(); // old survives this frame

    reg.flushDisposals();
    expect(disposeSpy).toHaveBeenCalledTimes(1); // disposed next frame
  });

  it("swapping the same entry is a no-op for disposal", () => {
    const only = box(1);
    reg.swap("body1", only);
    const spy = vi.spyOn(only, "dispose");
    reg.swap("body1", only);
    reg.flushDisposals();
    expect(spy).not.toHaveBeenCalled();
    only.dispose();
  });
});

describe("disposeAll leak tripwire", () => {
  it("empties the registry with no console.error when clean", () => {
    const err = vi.spyOn(console, "error").mockImplementation(() => {});
    reg.swap("body1", box(1));
    reg.swap("body2", box(1));
    expect(reg.registrySize()).toBe(2);
    const before = reg.leakTripwireCount;
    reg.disposeAll();
    expect(reg.registrySize()).toBe(0);
    expect(reg.leakTripwireCount).toBe(before); // no leak detected
    expect(err).not.toHaveBeenCalled();
    err.mockRestore();
  });
});
