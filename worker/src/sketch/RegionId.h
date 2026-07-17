// RegionId.h — NORMATIVE region-id derivation for SketchRegions (W-WP3b).
//
// This is a NEW worker file (not a port). It mirrors the Rust reference
// implementation onecad-core `sketch/mod.rs::derive_region_id` EXACTLY so the
// C++ worker and the Rust core produce byte-identical region ids from loop
// membership alone (SCHEMA §7.4 "regionId derivation is NORMATIVE").
//
// Algorithm (STABLE — changing it remaps every region id; cross-check-gated):
//   1. Take each member entity id. If it is a canonical UUID string
//      ("xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"), use its 16 raw bytes
//      (big-endian, == Rust `Uuid::as_bytes()`); otherwise fall back to the
//      raw UTF-8 bytes of the id (documented worker-local fallback — see below).
//   2. Sort the per-member byte arrays ascending (so the id is independent of
//      loop-member ordering) — matches the Rust `uuids.sort_unstable()` on the
//      `[u8;16]` arrays for the canonical-UUID case.
//   3. FNV-1a 64-bit (offset 0xcbf29ce484222325, prime 0x100000001b3) over
//      every member's bytes in sorted order, then one winding byte
//      (0 = Ccw/outer, 1 = Cw/hole).
//   4. Render "r_" + 16 lowercase hex digits.
//
// Fallback rationale: in the real system Rust owns the sketch and its EntityIds
// ARE UUIDs, so the UUID path is byte-identical across processes. Non-UUID ids
// only appear in hand-authored fixtures that never round-trip through Rust, so
// their region ids never need cross-process agreement — the fallback only needs
// to be deterministic within the worker.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace onecad::region {

// Loop winding — the discriminant byte fed into the region-id hash.
enum class Winding : std::uint8_t { Ccw = 0, Cw = 1 };

// Parse a canonical UUID string into its 16 raw bytes (big-endian / network
// order, matching Rust `Uuid::as_bytes()`). Accepts the hyphenated 8-4-4-4-12
// form (dashes optional). Returns false (out untouched) if not a valid UUID.
bool parse_uuid_bytes(const std::string& s, std::uint8_t out[16]);

// Derive the normative region id from a loop's member entity ids + winding.
// Byte-identical to the Rust reference for canonical-UUID member ids.
std::string derive_region_id(const std::vector<std::string>& member_ids, Winding winding);

}  // namespace onecad::region
