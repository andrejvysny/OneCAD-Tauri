// Tessellate.h — BRepMesh triangulation → MESH1 blob for one body (W-WP5).
//
// SCHEMA §7.6 Tessellate. Faces/edges are labelled with snapshot-scoped TopoKeys
// ("f:N"/"e:N", the MapShapes ordinal — consistent with the ElementMap partition)
// and, where the partition already holds a minted ElementId for that TopoKey, the
// persistent ElementId (IDS_HAVE_ELEMENTIDS). Meshing parallelism never affects the
// ids or the ordinal (Invariant 5).
//
// LOD tiers (deflection relative to the body bbox diagonal, per the migration
// plan): coarse/medium/fine. Planar prisms/booleans (the W-WP5 corpus) tessellate
// identically across tiers (2 triangles per planar rectangular face), so the corpus
// meshes are byte-stable.
#pragma once

#include <cstdint>
#include <string>
#include <vector>

#include <TopoDS_Shape.hxx>

#include "elementmap/ElementMapPartition.h"

namespace onecad::tess {

struct BodyMesh {
    std::string body_id;
    std::vector<std::uint8_t> blob;       // MESH1 bytes
    std::uint32_t triangle_count = 0;
    bool ok = false;                      // false if the body produced no triangulation
};

// Raw triangle geometry for one body — the SAME positions/normals/indices
// tessellate_body assembles for the MESH1 blob, minus id labelling + edges. Reused
// by the STL/OBJ mesh exporters (io/MeshExport) so an exported mesh is byte-for-byte
// the geometry the viewport meshes (identical BRepMesh params + winding ⇒ the STL
// triangle count equals the tessellation triangle count; determinism, Invariant 5).
struct RawMesh {
    std::vector<float> positions;         // 3·V (xyz per vertex)
    std::vector<float> normals;           // 3·V (per-vertex, area-weighted)
    std::vector<std::uint32_t> indices;   // 3·T (triangle vertex indices)
    std::uint32_t triangle_count = 0;
};

// Tessellate one body into a MESH1 blob. `lod` ∈ "coarse"|"medium"|"fine".
// `partition` (optional) supplies minted ElementIds by TopoKey for id labelling.
BodyMesh tessellate_body(const TopoDS_Shape& shape, const std::string& body_id,
                         const std::string& lod, bool include_edges,
                         const elementmap::ElementMapPartition* partition);

// Mesh one body into raw triangle arrays (no ids, no edges). `lod` selects the same
// deflection tier as tessellate_body, so the triangles match the viewport mesh.
RawMesh tessellate_raw(const TopoDS_Shape& shape, const std::string& lod);

}  // namespace onecad::tess
