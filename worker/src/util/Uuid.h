// Uuid.h — RFC-4122 v4 UUID generation for the OneCAD worker.
//
// Replaces OneCAD-CPP's QUuid::createUuid().toString(QUuid::WithoutBraces)
// (Qt) with a Qt-free generator. Entity/constraint IDs are opaque, random,
// and NOT security-sensitive, so a fast non-cryptographic PRNG
// (std::mt19937_64 seeded from std::random_device) is intentional and
// sufficient. Output format matches Qt's WithoutBraces: lowercase hex,
// 8-4-4-4-12 with the version (4) and variant (10xx) bits set.
#pragma once

#include <array>
#include <cstdint>
#include <random>
#include <string>

namespace onecad::util {

// Generate a random v4 UUID as "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx"
// (lowercase, no braces). Non-cryptographic; see file header.
inline std::string generate_uuid_v4() {
    static thread_local std::mt19937_64 rng(std::random_device{}());
    std::uniform_int_distribution<uint64_t> dist;
    const uint64_t hi = dist(rng);
    const uint64_t lo = dist(rng);

    std::array<uint8_t, 16> b{};
    for (int i = 0; i < 8; ++i) {
        b[i] = static_cast<uint8_t>((hi >> (8 * (7 - i))) & 0xFFu);
        b[8 + i] = static_cast<uint8_t>((lo >> (8 * (7 - i))) & 0xFFu);
    }
    b[6] = static_cast<uint8_t>((b[6] & 0x0Fu) | 0x40u);  // version 4
    b[8] = static_cast<uint8_t>((b[8] & 0x3Fu) | 0x80u);  // variant 10xx

    static const char* kHex = "0123456789abcdef";
    std::string out;
    out.reserve(36);
    for (int i = 0; i < 16; ++i) {
        if (i == 4 || i == 6 || i == 8 || i == 10) {
            out.push_back('-');
        }
        out.push_back(kHex[(b[i] >> 4) & 0x0F]);
        out.push_back(kHex[b[i] & 0x0F]);
    }
    return out;
}

}  // namespace onecad::util
