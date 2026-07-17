// test_hashing.cpp — known-answer vectors for the W-WP4 hash primitives.
//
// The SHA-256 empty-input digest is the LOAD-BEARING cross-track anchor: it MUST
// equal the Rust core's `HistoryPrefixHash::empty()`
// (onecad-core regen/planner.rs). FNV-1a underpins the three §12 signatures.
//
// No test framework: exit code == failure count.
#include <cstdio>
#include <string>
#include <vector>

#include "session/HistoryHash.h"
#include "util/Hashing.h"

namespace {
int g_failures = 0;
#define CHECK_EQ(a, b)                                                                \
    do {                                                                              \
        const std::string va = (a), vb = (b);                                        \
        if (va != vb) {                                                              \
            std::fprintf(stderr, "FAIL %s:%d: %s == %s\n  got: %s\n  want:%s\n",     \
                         __FILE__, __LINE__, #a, #b, va.c_str(), vb.c_str());        \
            ++g_failures;                                                            \
        }                                                                            \
    } while (0)
}  // namespace

int main() {
    using namespace onecad::hashing;
    using onecad::session::kEmptyPrefixHash;

    // --- SHA-256 FIPS 180-4 known answers (still used by mesh/artifact hashing) ---
    CHECK_EQ(sha256_hex(std::string("")),
             "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    CHECK_EQ(sha256_hex(std::string("abc")),
             "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    CHECK_EQ(sha256_hex(std::string("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
             "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
    // A > 64-byte input (exercises multi-block + length padding).
    CHECK_EQ(sha256_hex(std::string(200, 'a')),
             "c2a908d98f5df987ade41b5fce213067efbcc21ef2240212a41e54b5e7c28ae5");

    // --- opaque head anchor: kEmptyPrefixHash is the SHA-256("") cross-track anchor
    //     (W-WP5: the worker no longer COMPUTES history hashes; it stores the
    //     Rust-minted opaque token, whose fresh-session value is this constant). ---
    CHECK_EQ(std::string(kEmptyPrefixHash), sha256_hex(std::string("")));

    // --- FNV-1a 64-bit known answers (16 lowercase hex) ---
    CHECK_EQ(hex16(fnv1a(std::string(""))), "cbf29ce484222325");   // offset basis
    CHECK_EQ(hex16(fnv1a(std::string("hello"))), "a430d84680aabd0b");

    if (g_failures == 0) std::fprintf(stderr, "hashing: OK\n");
    return g_failures;
}
