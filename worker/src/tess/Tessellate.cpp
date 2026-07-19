// Tessellate.cpp — see Tessellate.h.
#include "tess/Tessellate.h"

#include <algorithm>
#include <cmath>
#include <map>

#include <BRepAdaptor_Curve.hxx>
#include <BRepBndLib.hxx>
#include <BRepMesh_IncrementalMesh.hxx>
#include <BRep_Tool.hxx>
#include <Bnd_Box.hxx>
#include <GeomAbs_CurveType.hxx>
#include <Poly_Triangle.hxx>
#include <Poly_Triangulation.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopExp.hxx>
#include <TopLoc_Location.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <gp_Pnt.hxx>
#include <gp_Vec.hxx>

#include "tess/Mesh1.h"

namespace onecad::tess {

namespace km = onecad::kernel::elementmap;

namespace {

std::uint16_t lod_code(const std::string& lod) {
    if (lod == "fine") return 2;
    if (lod == "medium") return 1;
    return 0;  // coarse
}

// Deflection tiers relative to the bbox diagonal (migration plan).
void deflections(const std::string& lod, double diag, double& lin, double& ang) {
    double rel = 0.05, a = 0.8;  // coarse
    if (lod == "medium") {
        rel = 0.02;
        a = 0.5;
    } else if (lod == "fine") {
        rel = 0.005;
        a = 0.2;
    }
    lin = std::max(diag * rel, 1e-3);
    ang = a;
}

// TopoKey → minted ElementId lookup for one body (empty map when no partition).
std::map<std::string, std::string> minted_ids(const elementmap::ElementMapPartition* partition,
                                              const std::string& body_id) {
    std::map<std::string, std::string> out;
    if (!partition) return out;
    for (const elementmap::PartitionEntry* e : partition->entries_for_body(body_id)) {
        if (!e->topo_key.empty()) out[e->topo_key] = e->element_id;
    }
    return out;
}

}  // namespace

BodyMesh tessellate_body(const TopoDS_Shape& shape, const std::string& body_id,
                         const std::string& lod, bool include_edges,
                         const elementmap::ElementMapPartition* partition) {
    BodyMesh out;
    out.body_id = body_id;
    if (shape.IsNull()) return out;

    Bnd_Box box;
    BRepBndLib::Add(shape, box);
    double diag = 1.0;
    if (!box.IsVoid()) {
        Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
        box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
        diag = gp_Pnt(xmin, ymin, zmin).Distance(gp_Pnt(xmax, ymax, zmax));
        out.body_id = body_id;
    }
    double lin = 0.1, ang = 0.5;
    deflections(lod, diag, lin, ang);

    // Mesh (single-threaded for determinism; the ids/ordinal are threading-
    // independent regardless — Invariant 5).
    BRepMesh_IncrementalMesh mesher(shape, lin, Standard_False, ang, Standard_False);
    mesher.Perform();

    const std::map<std::string, std::string> ids = minted_ids(partition, body_id);
    auto label = [&](char prefix, int index) {
        const std::string topo = std::string(1, prefix) + ":" + std::to_string(index);
        auto it = ids.find(topo);
        if (it != ids.end()) return std::make_pair(it->second, true);
        return std::make_pair(topo, false);
    };

    Mesh1Input mi;
    mi.lod = lod_code(lod);
    mi.has_normals = true;

    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(shape, TopAbs_FACE, faces);

    std::uint32_t tri_cursor = 0;
    bool any_elementid = false;

    for (int fi = 1; fi <= faces.Extent(); ++fi) {
        const TopoDS_Face face = TopoDS::Face(faces(fi));
        TopLoc_Location loc;
        Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(face, loc);
        const std::uint32_t first_tri = tri_cursor;
        if (tri.IsNull() || tri->NbNodes() < 3 || tri->NbTriangles() < 1) {
            mi.face_ranges.emplace_back(first_tri, 0);  // face present, no triangles
            const auto lbl = label('f', fi);
            any_elementid = any_elementid || lbl.second;
            mi.face_ids.push_back(lbl.first);
            continue;
        }
        const gp_Trsf trsf = loc.Transformation();
        const bool reversed = (face.Orientation() == TopAbs_REVERSED);

        const std::uint32_t base = static_cast<std::uint32_t>(mi.positions.size() / 3);
        const int nb_nodes = tri->NbNodes();
        std::vector<gp_Vec> accum(static_cast<std::size_t>(nb_nodes), gp_Vec(0, 0, 0));
        std::vector<gp_Pnt> pts(static_cast<std::size_t>(nb_nodes));
        for (int i = 1; i <= nb_nodes; ++i) {
            gp_Pnt p = tri->Node(i).Transformed(trsf);
            pts[static_cast<std::size_t>(i - 1)] = p;
        }

        std::uint32_t face_tris = 0;
        for (int t = 1; t <= tri->NbTriangles(); ++t) {
            Standard_Integer n1, n2, n3;
            tri->Triangle(t).Get(n1, n2, n3);
            if (reversed) std::swap(n2, n3);  // outward winding
            const gp_Pnt& a = pts[static_cast<std::size_t>(n1 - 1)];
            const gp_Pnt& b = pts[static_cast<std::size_t>(n2 - 1)];
            const gp_Pnt& c = pts[static_cast<std::size_t>(n3 - 1)];
            gp_Vec normal = gp_Vec(a, b).Crossed(gp_Vec(a, c));  // area-weighted
            accum[static_cast<std::size_t>(n1 - 1)] += normal;
            accum[static_cast<std::size_t>(n2 - 1)] += normal;
            accum[static_cast<std::size_t>(n3 - 1)] += normal;
            mi.indices.push_back(base + static_cast<std::uint32_t>(n1 - 1));
            mi.indices.push_back(base + static_cast<std::uint32_t>(n2 - 1));
            mi.indices.push_back(base + static_cast<std::uint32_t>(n3 - 1));
            ++face_tris;
        }

        for (int i = 0; i < nb_nodes; ++i) {
            mi.positions.push_back(static_cast<float>(pts[static_cast<std::size_t>(i)].X()));
            mi.positions.push_back(static_cast<float>(pts[static_cast<std::size_t>(i)].Y()));
            mi.positions.push_back(static_cast<float>(pts[static_cast<std::size_t>(i)].Z()));
            gp_Vec n = accum[static_cast<std::size_t>(i)];
            if (n.Magnitude() > 1e-12) {
                n.Normalize();
            } else {
                n = gp_Vec(0, 0, 1);  // degenerate fallback
            }
            mi.normals.push_back(static_cast<float>(n.X()));
            mi.normals.push_back(static_cast<float>(n.Y()));
            mi.normals.push_back(static_cast<float>(n.Z()));
        }

        // Vertices are NOT shared across faces (each face owns its node set), so
        // normals are smoothed WITHIN a face and hard-split at every face boundary
        // (crease-split — mesh_format.md). Correct flat shading for planar prisms.
        mi.face_ranges.emplace_back(first_tri, face_tris);
        tri_cursor += face_tris;
        const auto lbl = label('f', fi);
        any_elementid = any_elementid || lbl.second;
        mi.face_ids.push_back(lbl.first);
    }

    // --- edges (polylines) ---
    if (include_edges) {
        mi.has_edges = true;
        TopTools_IndexedMapOfShape edges;
        TopExp::MapShapes(shape, TopAbs_EDGE, edges);
        for (int ei = 1; ei <= edges.Extent(); ++ei) {
            const TopoDS_Edge edge = TopoDS::Edge(edges(ei));
            const std::uint32_t first_point = static_cast<std::uint32_t>(mi.edge_positions.size() / 3);
            std::uint32_t point_count = 0;
            try {
                BRepAdaptor_Curve curve(edge);
                const double f = curve.FirstParameter();
                const double l = curve.LastParameter();
                // Deterministic sampling: straight edges → 2 points; else 16 spans.
                const int spans = (curve.GetType() == GeomAbs_Line) ? 1 : 16;
                for (int s = 0; s <= spans; ++s) {
                    const double u = f + (l - f) * (static_cast<double>(s) / spans);
                    const gp_Pnt p = curve.Value(u);
                    mi.edge_positions.push_back(static_cast<float>(p.X()));
                    mi.edge_positions.push_back(static_cast<float>(p.Y()));
                    mi.edge_positions.push_back(static_cast<float>(p.Z()));
                    ++point_count;
                }
            } catch (...) {
                // Non-samplable edge (degenerate) → zero-length polyline.
            }
            mi.edge_ranges.emplace_back(first_point, point_count);
            const auto lbl = label('e', ei);
            any_elementid = any_elementid || lbl.second;
            mi.edge_ids.push_back(lbl.first);
        }
    }

    mi.ids_have_elementids = any_elementid;

    // bbox
    if (!box.IsVoid()) {
        Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
        box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
        mi.bbox_min[0] = static_cast<float>(xmin);
        mi.bbox_min[1] = static_cast<float>(ymin);
        mi.bbox_min[2] = static_cast<float>(zmin);
        mi.bbox_max[0] = static_cast<float>(xmax);
        mi.bbox_max[1] = static_cast<float>(ymax);
        mi.bbox_max[2] = static_cast<float>(zmax);
    }

    out.triangle_count = static_cast<std::uint32_t>(mi.indices.size() / 3);
    out.blob = encode_mesh1(mi);
    out.ok = true;
    return out;
}

RawMesh tessellate_raw(const TopoDS_Shape& shape, const std::string& lod) {
    RawMesh out;
    if (shape.IsNull()) return out;

    Bnd_Box box;
    BRepBndLib::Add(shape, box);
    double diag = 1.0;
    if (!box.IsVoid()) {
        Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
        box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
        diag = gp_Pnt(xmin, ymin, zmin).Distance(gp_Pnt(xmax, ymax, zmax));
    }
    double lin = 0.1, ang = 0.5;
    deflections(lod, diag, lin, ang);

    // Single-threaded meshing for determinism (Invariant 5). Same params as
    // tessellate_body, so the produced triangle set is identical.
    BRepMesh_IncrementalMesh mesher(shape, lin, Standard_False, ang, Standard_False);
    mesher.Perform();

    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(shape, TopAbs_FACE, faces);

    for (int fi = 1; fi <= faces.Extent(); ++fi) {
        const TopoDS_Face face = TopoDS::Face(faces(fi));
        TopLoc_Location loc;
        Handle(Poly_Triangulation) tri = BRep_Tool::Triangulation(face, loc);
        if (tri.IsNull() || tri->NbNodes() < 3 || tri->NbTriangles() < 1) continue;
        const gp_Trsf trsf = loc.Transformation();
        const bool reversed = (face.Orientation() == TopAbs_REVERSED);

        const std::uint32_t base = static_cast<std::uint32_t>(out.positions.size() / 3);
        const int nb_nodes = tri->NbNodes();
        std::vector<gp_Vec> accum(static_cast<std::size_t>(nb_nodes), gp_Vec(0, 0, 0));
        std::vector<gp_Pnt> pts(static_cast<std::size_t>(nb_nodes));
        for (int i = 1; i <= nb_nodes; ++i) pts[static_cast<std::size_t>(i - 1)] = tri->Node(i).Transformed(trsf);

        for (int t = 1; t <= tri->NbTriangles(); ++t) {
            Standard_Integer n1, n2, n3;
            tri->Triangle(t).Get(n1, n2, n3);
            if (reversed) std::swap(n2, n3);  // outward winding
            const gp_Pnt& a = pts[static_cast<std::size_t>(n1 - 1)];
            const gp_Pnt& b = pts[static_cast<std::size_t>(n2 - 1)];
            const gp_Pnt& c = pts[static_cast<std::size_t>(n3 - 1)];
            gp_Vec normal = gp_Vec(a, b).Crossed(gp_Vec(a, c));  // area-weighted
            accum[static_cast<std::size_t>(n1 - 1)] += normal;
            accum[static_cast<std::size_t>(n2 - 1)] += normal;
            accum[static_cast<std::size_t>(n3 - 1)] += normal;
            out.indices.push_back(base + static_cast<std::uint32_t>(n1 - 1));
            out.indices.push_back(base + static_cast<std::uint32_t>(n2 - 1));
            out.indices.push_back(base + static_cast<std::uint32_t>(n3 - 1));
        }

        for (int i = 0; i < nb_nodes; ++i) {
            const gp_Pnt& p = pts[static_cast<std::size_t>(i)];
            out.positions.push_back(static_cast<float>(p.X()));
            out.positions.push_back(static_cast<float>(p.Y()));
            out.positions.push_back(static_cast<float>(p.Z()));
            gp_Vec n = accum[static_cast<std::size_t>(i)];
            if (n.Magnitude() > 1e-12) {
                n.Normalize();
            } else {
                n = gp_Vec(0, 0, 1);
            }
            out.normals.push_back(static_cast<float>(n.X()));
            out.normals.push_back(static_cast<float>(n.Y()));
            out.normals.push_back(static_cast<float>(n.Z()));
        }
    }

    out.triangle_count = static_cast<std::uint32_t>(out.indices.size() / 3);
    return out;
}

}  // namespace onecad::tess
