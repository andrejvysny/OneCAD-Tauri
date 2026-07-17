/*
 * Binary-search range lookup (triangle→face / segment→edge) edge cases + lazy
 * memoised id decode.
 */
import { describe, it, expect } from "vitest";
import { rangeLookup, TopoIndex } from "./faceRangeIndex";

// 3 faces: face0 tris [0,3), face1 [3,5), face2 [5,8).
const ranges = new Uint32Array([0, 3, 3, 2, 5, 3]);
const count = 3;

describe("rangeLookup binary search", () => {
  it("maps every triangle to the right face across boundaries", () => {
    const expected = [0, 0, 0, 1, 1, 2, 2, 2];
    expected.forEach((face, tri) => expect(rangeLookup(ranges, count, tri)).toBe(face));
  });

  it("hits exact range starts and ends", () => {
    expect(rangeLookup(ranges, count, 0)).toBe(0); // first of face0
    expect(rangeLookup(ranges, count, 3)).toBe(1); // first of face1
    expect(rangeLookup(ranges, count, 4)).toBe(1); // last of face1
    expect(rangeLookup(ranges, count, 5)).toBe(2); // first of face2
    expect(rangeLookup(ranges, count, 7)).toBe(2); // last of face2
  });

  it("returns -1 out of range", () => {
    expect(rangeLookup(ranges, count, 8)).toBe(-1);
    expect(rangeLookup(ranges, count, 100)).toBe(-1);
  });

  it("handles a single-range table", () => {
    const one = new Uint32Array([0, 4]);
    expect(rangeLookup(one, 1, 0)).toBe(0);
    expect(rangeLookup(one, 1, 3)).toBe(0);
    expect(rangeLookup(one, 1, 4)).toBe(-1);
  });

  it("handles an empty table", () => {
    expect(rangeLookup(new Uint32Array([]), 0, 0)).toBe(-1);
  });
});

describe("TopoIndex lazy id decode", () => {
  // ids: "f:0","f:11","f:222" → offs [0,3,7,12].
  const idChars = new TextEncoder().encode("f:0f:11f:222");
  const idOffsets = new Uint32Array([0, 3, 7, 12]);
  const idx = new TopoIndex(ranges, count, idOffsets, idChars);

  it("decodes ordinal → id string", () => {
    expect(idx.idOf(0)).toBe("f:0");
    expect(idx.idOf(1)).toBe("f:11");
    expect(idx.idOf(2)).toBe("f:222");
  });

  it("maps a needle straight to its id", () => {
    expect(idx.idAt(0)).toBe("f:0");
    expect(idx.idAt(4)).toBe("f:11");
    expect(idx.idAt(6)).toBe("f:222");
    expect(idx.idAt(99)).toBeNull();
  });

  it("memoises decoded ids (same string instance on re-decode)", () => {
    const a = idx.idOf(2);
    const b = idx.idOf(2);
    expect(a).toBe(b);
  });

  it("throws on an out-of-range ordinal", () => {
    expect(() => idx.idOf(3)).toThrow(RangeError);
  });
});
