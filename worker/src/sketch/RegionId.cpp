// RegionId.cpp — see RegionId.h. Mirrors onecad-core sketch/mod.rs::derive_region_id.
#include "sketch/RegionId.h"

#include <algorithm>
#include <array>
#include <cstdio>

namespace onecad::region {

namespace {

constexpr std::uint64_t kFnvOffset = 0xcbf2'9ce4'8422'2325ULL;
constexpr std::uint64_t kFnvPrime = 0x0000'0100'0000'01b3ULL;

int hex_nibble(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    if (c >= 'A' && c <= 'F') return c - 'A' + 10;
    return -1;
}

}  // namespace

bool parse_uuid_bytes(const std::string& s, std::uint8_t out[16]) {
    // Collect the 32 hex digits, tolerating (but not requiring) dashes only in
    // the canonical 8-4-4-4-12 positions. Any other layout is rejected.
    std::array<int, 32> nibbles{};
    std::size_t n = 0;
    for (std::size_t i = 0; i < s.size(); ++i) {
        const char c = s[i];
        if (c == '-') continue;
        const int v = hex_nibble(c);
        if (v < 0) return false;      // non-hex, non-dash char
        if (n >= 32) return false;    // too many hex digits
        nibbles[n++] = v;
    }
    if (n != 32) return false;        // not 32 hex digits

    for (std::size_t i = 0; i < 16; ++i) {
        out[i] = static_cast<std::uint8_t>((nibbles[2 * i] << 4) | nibbles[2 * i + 1]);
    }
    return true;
}

std::string derive_region_id(const std::vector<std::string>& member_ids, Winding winding) {
    // 1–2. Build per-member byte keys and sort them ascending.
    std::vector<std::vector<std::uint8_t>> keys;
    keys.reserve(member_ids.size());
    for (const auto& id : member_ids) {
        std::uint8_t uuid[16];
        if (parse_uuid_bytes(id, uuid)) {
            keys.emplace_back(uuid, uuid + 16);
        } else {
            // Documented fallback (see header): raw UTF-8 bytes of the id.
            keys.emplace_back(id.begin(), id.end());
        }
    }
    std::sort(keys.begin(), keys.end());

    // 3. FNV-1a over sorted member bytes, then the winding byte.
    std::uint64_t hash = kFnvOffset;
    auto mix = [&hash](std::uint8_t byte) {
        hash ^= static_cast<std::uint64_t>(byte);
        hash *= kFnvPrime;  // wrapping u64 multiply
    };
    for (const auto& key : keys) {
        for (std::uint8_t byte : key) mix(byte);
    }
    mix(static_cast<std::uint8_t>(winding));

    // 4. Render "r_%016x".
    char buf[3 + 16 + 1];
    std::snprintf(buf, sizeof(buf), "r_%016llx", static_cast<unsigned long long>(hash));
    return std::string(buf);
}

}  // namespace onecad::region
