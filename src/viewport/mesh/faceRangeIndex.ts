/*
 * Topology lookup: element index → ordinal (binary search over contiguous
 * ranges) → lazily-decoded id string (TopoKey or ElementId).
 *
 * INDICES is grouped by face and EDGE_POSITIONS grouped by edge, so FACE_RANGES
 * / edge-segment ranges are contiguous & offset-ordered — a plain binary search
 * over each range's first element maps a picked triangle/segment to its owner.
 * The id is then one TextDecoder slice over the *_ID_CHARS view (no per-id
 * allocation up front; decoded on demand and memoised).
 *
 * Ranges are packed pairs: `ranges[2·i] = first`, `ranges[2·i+1] = count`.
 */

/**
 * Binary-search a packed {first,count} range array for the ordinal owning
 * `needle`. Returns -1 when out of range. Pure.
 */
export function rangeLookup(ranges: Uint32Array, count: number, needle: number): number {
  let lo = 0;
  let hi = count - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    const first = ranges[mid * 2];
    if (needle < first) {
      hi = mid - 1;
    } else if (needle < first + ranges[mid * 2 + 1]) {
      return mid;
    } else {
      lo = mid + 1;
    }
  }
  return -1;
}

/**
 * Lazily maps ordinal → id string over an offset+chars prefix-sum table.
 * `idOffsets` has `count+1` entries; id `i` = chars[offs[i], offs[i+1]).
 * Decoded ids are memoised so repeated picks of the same element are free.
 */
export class TopoIndex {
  private readonly cache = new Map<number, string>();
  private static readonly decoder = new TextDecoder();

  constructor(
    /** Packed {first,count} ranges the ordinal is searched over. */
    private readonly ranges: Uint32Array,
    readonly count: number,
    private readonly idOffsets: Uint32Array,
    private readonly idChars: Uint8Array,
  ) {}

  /** Element index (triangle / segment) → owning ordinal, or -1. */
  ordinalOf(needle: number): number {
    return rangeLookup(this.ranges, this.count, needle);
  }

  /** Ordinal → id string (TopoKey or ElementId), decoded lazily + memoised. */
  idOf(ordinal: number): string {
    if (ordinal < 0 || ordinal >= this.count) {
      throw new RangeError(`ordinal ${ordinal} out of range [0,${this.count})`);
    }
    const hit = this.cache.get(ordinal);
    if (hit !== undefined) return hit;
    const start = this.idOffsets[ordinal];
    const end = this.idOffsets[ordinal + 1];
    const id = TopoIndex.decoder.decode(this.idChars.subarray(start, end));
    this.cache.set(ordinal, id);
    return id;
  }

  /** Element index → id in one step (or null when out of range). */
  idAt(needle: number): string | null {
    const ord = this.ordinalOf(needle);
    return ord < 0 ? null : this.idOf(ord);
  }

  /** Reverse: id string → ordinal (or -1). Builds a lazy reverse map once. */
  ordinalForId(id: string): number {
    if (!this.reverse) {
      this.reverse = new Map();
      for (let i = 0; i < this.count; i++) this.reverse.set(this.idOf(i), i);
    }
    return this.reverse.get(id) ?? -1;
  }

  private reverse: Map<string, number> | null = null;
}
