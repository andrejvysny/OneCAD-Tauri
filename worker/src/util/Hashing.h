// Hashing.h — deterministic hash primitives shared across the worker session.
//
// Two families are used, deliberately kept distinct (they are NOT interchangeable
// on the wire):
//
//   * FNV-1a 64-bit (16 lowercase-hex chars) — the topology SIGNATURES (SCHEMA
//     §12: geometry / bodyLifecycle / referencedBinding) and the descriptor/
//     region hashes (§10/§7.4). Cheap, order-sensitive, 64-bit.
//   * SHA-256 (64 lowercase-hex chars) — the `historyPrefixHash` (SCHEMA §7.2
//     `expectedBaseHash` / §7.7). Chosen to MATCH the Rust core reference
//     `onecad-core regen/planner.rs::history_prefix_hash`, which is SHA-256 over
//     canonical record lines. See HistoryHash.h for the cross-track caveat.
//
// All outputs are lowercase hex strings (SCHEMA §2 hash wire form).
#pragma once

#include <array>
#include <cstdint>
#include <string>
#include <vector>

namespace onecad::hashing {

// FNV-1a 64-bit constants (SCHEMA §10 / §12 — offset basis + prime).
inline constexpr std::uint64_t kFnvOffset = 14695981039346656037ULL;
inline constexpr std::uint64_t kFnvPrime = 1099511628211ULL;

// Incremental FNV-1a 64-bit accumulator over an existing hash state.
inline std::uint64_t fnv1a_update(std::uint64_t h, const void* data, std::size_t len) {
    const auto* p = static_cast<const unsigned char*>(data);
    for (std::size_t i = 0; i < len; ++i) {
        h ^= p[i];
        h *= kFnvPrime;
    }
    return h;
}

// FNV-1a 64-bit over a byte range, starting from the offset basis.
inline std::uint64_t fnv1a(const void* data, std::size_t len) {
    return fnv1a_update(kFnvOffset, data, len);
}

// FNV-1a 64-bit over a string.
inline std::uint64_t fnv1a(const std::string& s) {
    return fnv1a(s.data(), s.size());
}

// Render a 64-bit hash as 16 lowercase-hex chars ($hex64, SCHEMA §2).
std::string hex16(std::uint64_t h);

// SHA-256 of a byte buffer, rendered as 64 lowercase-hex chars.
std::string sha256_hex(const std::uint8_t* data, std::size_t len);

// SHA-256 of a string.
std::string sha256_hex(const std::string& s);

}  // namespace onecad::hashing
