// LittleEndian.h — the wire framing (magic, jsonLen, binLen) is little-endian.
//
// The frame codec reads/writes u32 length fields as raw little-endian bytes, so
// it is byte-order-correct on any host. This header additionally asserts the
// host itself is little-endian: OneCAD targets Apple Silicon + x86-64, both LE,
// and the MESH1 binary path (W-*, later) relies on native LE layout. A big-
// endian host would be a silent correctness hazard, so we fail loudly instead.
#pragma once

#include <bit>
#include <cstdint>

namespace onecad::endian {

// Compile-time guard: refuse to build on a non-little-endian host.
static_assert(std::endian::native == std::endian::little,
              "OneCAD worker requires a little-endian host (Apple Silicon / x86-64). "
              "The wire framing and MESH1 binary layout assume little-endian.");

// Runtime cross-check — call once at startup (belt-and-suspenders vs. exotic
// toolchains where std::endian could be misreported). Returns true iff LE.
inline bool host_is_little_endian() {
    const std::uint32_t probe = 0x01020304u;
    std::uint8_t bytes[4];
    __builtin_memcpy(bytes, &probe, sizeof(probe));
    return bytes[0] == 0x04 && bytes[1] == 0x03 && bytes[2] == 0x02 && bytes[3] == 0x01;
}

}  // namespace onecad::endian
