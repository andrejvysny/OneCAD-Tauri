// test_region_id.cpp — NORMATIVE cross-check of the C++ region-id derivation
// against the Rust reference (onecad-core sketch/mod.rs::derive_region_id).
//
// The load-bearing assertion is the byte-match against the Rust lock-test value:
//   Rust  tests::derive_region_id_is_order_independent_and_winding_sensitive
//         derive_region_id(&[eid(1), eid(2)], Winding::Ccw) == "r_fbf1e34acfb51ba4"
//   where eid(n) = EntityId(Uuid::from_u128(n)), whose canonical string form is
//   "00000000-0000-0000-0000-00000000000{n}". Feeding those exact member ids to
//   the C++ derivation MUST produce the same id, proving byte-parity of the FNV
//   input (sorted 16-byte UUIDs + winding byte) across the two implementations.
//
// No test framework (matches the prototype style): exit code == failure count.
#include <cstdint>
#include <cstdio>
#include <string>
#include <vector>

#include "sketch/RegionId.h"

using onecad::region::derive_region_id;
using onecad::region::parse_uuid_bytes;
using onecad::region::Winding;

namespace {
int g_failures = 0;
#define CHECK(cond)                                                              \
    do {                                                                         \
        if (!(cond)) {                                                           \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            ++g_failures;                                                        \
        }                                                                        \
    } while (0)
}  // namespace

int main() {
    // --- 1. Byte-match against the Rust locked value (THE cross-check) ---
    const std::vector<std::string> members = {
        "00000000-0000-0000-0000-000000000001",  // == Uuid::from_u128(1)
        "00000000-0000-0000-0000-000000000002",  // == Uuid::from_u128(2)
    };
    const std::string id = derive_region_id(members, Winding::Ccw);
    std::fprintf(stderr, "region id (ccw) = %s (rust lock: r_fbf1e34acfb51ba4)\n", id.c_str());
    CHECK(id == "r_fbf1e34acfb51ba4");

    // --- 2. Order independence (Rust: sorts the 16-byte arrays) ---
    const std::vector<std::string> reversed = {
        "00000000-0000-0000-0000-000000000002",
        "00000000-0000-0000-0000-000000000001",
    };
    CHECK(derive_region_id(reversed, Winding::Ccw) == id);

    // --- 3. Winding sensitivity ---
    CHECK(derive_region_id(members, Winding::Cw) != id);

    // --- 4. Wire form: "r_" + 16 lowercase hex digits ---
    CHECK(id.size() == 2 + 16);
    CHECK(id.rfind("r_", 0) == 0);

    // --- 5. UUID byte parsing (big-endian, == Rust Uuid::as_bytes()) ---
    std::uint8_t bytes[16];
    CHECK(parse_uuid_bytes("00000000-0000-0000-0000-000000000001", bytes));
    for (int i = 0; i < 15; ++i) CHECK(bytes[i] == 0);
    CHECK(bytes[15] == 1);
    // Dashes are optional.
    std::uint8_t bytes2[16];
    CHECK(parse_uuid_bytes("000000000000000000000000000000ff", bytes2));
    CHECK(bytes2[15] == 0xff);
    // Rejects non-UUID strings (fallback path handled by derive_region_id).
    std::uint8_t junk[16];
    CHECK(!parse_uuid_bytes("e1", junk));
    CHECK(!parse_uuid_bytes("not-a-uuid", junk));

    if (g_failures == 0) {
        std::fprintf(stderr, "region-id cross-check: OK\n");
    }
    return g_failures;
}
