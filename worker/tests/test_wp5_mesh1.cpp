// test_wp5_mesh1.cpp — MESH1 structural validator + golden over a tessellated unit
// box (protocol/mesh_format.md). A box's triangulation is stable across OCCT builds
// (2 triangles per planar face), so the structural counts are a reliable golden.
// Also carries a small reusable MESH1 validator. No framework: exit == failures.
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include <BRepPrimAPI_MakeBox.hxx>
#include <TopoDS_Shape.hxx>

#include "tess/Tessellate.h"

namespace {
int g_failures = 0;
void check(bool cond, const std::string& msg) {
    if (!cond) {
        std::fprintf(stderr, "FAIL: %s\n", msg.c_str());
        ++g_failures;
    }
}

std::uint16_t u16(const std::vector<std::uint8_t>& b, std::size_t at) {
    return static_cast<std::uint16_t>(b[at] | (b[at + 1] << 8));
}
std::uint32_t u32(const std::vector<std::uint8_t>& b, std::size_t at) {
    std::uint32_t v = 0;
    for (int i = 0; i < 4; ++i) v |= static_cast<std::uint32_t>(b[at + i]) << (i * 8);
    return v;
}

struct Section {
    std::uint32_t type, off, len;
};

// A small MESH1 structural validator (mesh_format.md §2-§4). Fills counts + the
// section table; asserts header magic/version, table sort/alignment/bounds, and
// the required-section length relationships. Returns false on any structural fault.
bool validate_mesh1(const std::vector<std::uint8_t>& b, std::uint32_t& V, std::uint32_t& T,
                    std::uint32_t& F, std::uint32_t& E, std::vector<Section>& secs,
                    std::string& err) {
    if (b.size() < 64) return err = "blob < 64 bytes", false;
    if (u32(b, 0) != 0x4D455348) return err = "bad magic", false;
    if (u16(b, 4) != 1) return err = "bad version", false;
    V = u32(b, 0x08);
    T = u32(b, 0x0C);
    F = u32(b, 0x10);
    E = u32(b, 0x14);
    const std::uint16_t section_count = u16(b, 0x1E);
    const std::size_t table_end = 0x40 + static_cast<std::size_t>(section_count) * 16;
    if (b.size() < table_end) return err = "truncated section table", false;

    std::uint32_t prev_off = 0;
    for (std::uint16_t i = 0; i < section_count; ++i) {
        const std::size_t e = 0x40 + static_cast<std::size_t>(i) * 16;
        Section s{u32(b, e + 0x00), u32(b, e + 0x08), u32(b, e + 0x0C)};
        if (u32(b, e + 0x04) != 0) return err = "section pad != 0", false;
        if (s.off % 4 != 0) return err = "section offset not 4-aligned", false;
        if (s.off < prev_off) return err = "section table not sorted by offset", false;
        if (static_cast<std::size_t>(s.off) + s.len > b.size()) return err = "section out of bounds", false;
        prev_off = s.off;
        secs.push_back(s);
    }
    return true;
}

const Section* find(const std::vector<Section>& secs, std::uint32_t type) {
    for (const auto& s : secs)
        if (s.type == type) return &s;
    return nullptr;
}

void test_box_mesh1_golden() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(2.0, 2.0, 2.0).Shape();
    onecad::tess::BodyMesh bm =
        onecad::tess::tessellate_body(box, "body_1", "coarse", /*include_edges=*/true, nullptr);
    check(bm.ok, "box tessellation succeeded");

    std::uint32_t V, T, F, E;
    std::vector<Section> secs;
    std::string err;
    check(validate_mesh1(bm.blob, V, T, F, E, secs, err), "MESH1 structurally valid: " + err);

    // Golden counts: a box → 6 faces / 12 edges / 8 verts; each planar face
    // triangulates to 2 triangles over its 4 nodes → V=24, T=12.
    check(F == 6, "box: 6 faces");
    check(E == 12, "box: 12 edges");
    check(T == 12, "box: 12 triangles (2 per face)");
    check(V == 24, "box: 24 vertices (4 per face, no cross-face sharing)");
    check(bm.triangle_count == 12, "BodyMesh triangleCount == 12");

    // Required sections present with the right length relationships (§4).
    const Section* pos = find(secs, 1);
    const Section* idx = find(secs, 3);
    const Section* franges = find(secs, 4);
    const Section* foffs = find(secs, 5);
    const Section* fchars = find(secs, 6);
    check(pos && pos->len == 12 * V, "POSITIONS len == 12·V");
    check(find(secs, 2) != nullptr, "NORMALS present (HAS_NORMALS)");
    check(idx && idx->len == 12 * T, "INDICES len == 12·T");
    check(franges && franges->len == 8 * F, "FACE_RANGES len == 8·F");
    check(foffs && foffs->len == 4 * (F + 1), "FACE_ID_OFFS len == 4·(F+1)");
    check(fchars != nullptr, "FACE_ID_CHARS present");

    // Sum of FACE_RANGES triCount == T.
    std::uint32_t tri_sum = 0;
    for (std::uint32_t i = 0; i < F; ++i) tri_sum += u32(bm.blob, franges->off + i * 8 + 4);
    check(tri_sum == T, "sum(FACE_RANGES.triCount) == triangleCount");

    // Face ids decode to the MapShapes TopoKeys "f:1".."f:6".
    bool ids_ok = true;
    for (std::uint32_t i = 0; i < F; ++i) {
        const std::uint32_t o0 = u32(bm.blob, foffs->off + i * 4);
        const std::uint32_t o1 = u32(bm.blob, foffs->off + (i + 1) * 4);
        std::string id(reinterpret_cast<const char*>(bm.blob.data()) + fchars->off + o0, o1 - o0);
        if (id != "f:" + std::to_string(i + 1)) ids_ok = false;
    }
    check(ids_ok, "face ids decode to f:1..f:6 (MapShapes ordinals)");

    // Edge sections present (HAS_EDGES).
    check(find(secs, 7) && find(secs, 8) && find(secs, 9) && find(secs, 10),
          "EDGE_* sections present (HAS_EDGES)");

    // flags: HAS_NORMALS|HAS_EDGES set; IDS_HAVE_ELEMENTIDS clear (no partition).
    const std::uint16_t flags = u16(bm.blob, 6);
    check((flags & 0x0001) && (flags & 0x0002), "flags HAS_NORMALS|HAS_EDGES");
    check((flags & 0x0008) == 0, "flags IDS_HAVE_ELEMENTIDS clear (pure TopoKeys)");
}

}  // namespace

int main() {
    test_box_mesh1_golden();
    if (g_failures == 0) std::fprintf(stderr, "wp5_mesh1: OK\n");
    return g_failures;
}
