// Mesh1.h — MESH1 binary encoder (protocol/mesh_format.md, W-WP5).
//
// Produces a self-contained MESH1 blob: 64-byte header + 16-byte section table
// (sorted by offset) + 4-byte-aligned section data. All fields little-endian
// (the host is asserted LE at startup). Face/edge id tables carry snapshot-scoped
// TopoKeys ("f:22"/"e:5") or, where already minted, persistent ElementIds
// (IDS_HAVE_ELEMENTIDS flag). See mesh_format.md §2-§5 for the exact layout.
#pragma once

#include <cstdint>
#include <string>
#include <utility>
#include <vector>

namespace onecad::tess {

// Assembled mesh arrays for one body, ready to serialize into MESH1.
struct Mesh1Input {
    // Data sections (grouped by face for INDICES; per-vertex for POSITIONS/NORMALS).
    std::vector<float> positions;                        // 3·V (xyz per vertex)
    std::vector<float> normals;                          // 3·V (per-vertex, or empty)
    std::vector<std::uint32_t> indices;                  // 3·T (grouped by face)
    std::vector<std::pair<std::uint32_t, std::uint32_t>> face_ranges;  // {firstTri, triCount}
    std::vector<std::string> face_ids;                   // one per face (TopoKey/ElementId)

    // Edge sections (present iff has_edges).
    std::vector<float> edge_positions;                   // 3·P (polyline points, grouped by edge)
    std::vector<std::pair<std::uint32_t, std::uint32_t>> edge_ranges;  // {firstPoint, pointCount}
    std::vector<std::string> edge_ids;                   // one per edge

    float bbox_min[3] = {0, 0, 0};
    float bbox_max[3] = {0, 0, 0};
    std::uint16_t lod = 0;                               // 0 coarse, 1 medium, 2 fine
    bool has_normals = false;
    bool has_edges = false;
    bool ids_have_elementids = false;
};

// Serialize `in` to a MESH1 blob (mesh_format.md). Deterministic: identical input
// bytes ⇒ identical output bytes (Invariant 5).
std::vector<std::uint8_t> encode_mesh1(const Mesh1Input& in);

}  // namespace onecad::tess
