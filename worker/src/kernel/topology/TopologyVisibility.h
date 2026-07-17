// Ported from OneCAD-CPP src/kernel/topology/TopologyVisibility.h @ b4ddcccc (2026-07-16)
#ifndef ONECAD_KERNEL_TOPOLOGY_TOPOLOGYVISIBILITY_H
#define ONECAD_KERNEL_TOPOLOGY_TOPOLOGYVISIBILITY_H

#include <BRepAdaptor_Surface.hxx>
#include <BRepLProp_SLProps.hxx>
#include <BRep_Tool.hxx>
#include <Geom2d_Curve.hxx>
#include <GeomAbs_Shape.hxx>
#include <Precision.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <gp_Dir.hxx>
#include <gp_Pnt2d.hxx>
#include <gp_Vec2d.hxx>

#include <cmath>
#include <vector>

namespace onecad::core::modeling {

inline bool sampleFaceNormalAtEdgeMidpoint(const TopoDS_Edge& edge,
                                           const TopoDS_Face& face,
                                           gp_Dir* outNormal) {
    if (!outNormal) {
        return false;
    }

    Standard_Real first = 0.0;
    Standard_Real last = 0.0;
    Handle(Geom2d_Curve) curve2d = BRep_Tool::CurveOnSurface(edge, face, first, last);
    if (curve2d.IsNull()) {
        return false;
    }

    const Standard_Real mid = 0.5 * (first + last);
    gp_Pnt2d uv;
    gp_Vec2d d1;
    curve2d->D1(mid, uv, d1);

    BRepAdaptor_Surface surface(face, true);
    BRepLProp_SLProps props(surface, uv.X(), uv.Y(), 1, Precision::Confusion());
    if (!props.IsNormalDefined()) {
        return false;
    }

    gp_Dir normal = props.Normal();
    if (face.Orientation() == TopAbs_REVERSED) {
        normal.Reverse();
    }
    *outNormal = normal;
    return true;
}

inline bool isSharpEdgeByAngle(const TopoDS_Edge& edge,
                               const TopoDS_Face& firstFace,
                               const TopoDS_Face& secondFace,
                               double smoothEdgeCosine) {
    gp_Dir firstNormal;
    gp_Dir secondNormal;
    if (!sampleFaceNormalAtEdgeMidpoint(edge, firstFace, &firstNormal) ||
        !sampleFaceNormalAtEdgeMidpoint(edge, secondFace, &secondNormal)) {
        return true;
    }

    return firstNormal.Dot(secondNormal) < smoothEdgeCosine;
}

inline bool isVisibleTopologyEdge(const TopoDS_Edge& edge,
                                  const TopTools_ListOfShape& faces,
                                  double smoothEdgeCosine) {
    std::vector<TopoDS_Face> adjacentFaces;
    adjacentFaces.reserve(static_cast<std::size_t>(faces.Extent()));
    for (TopTools_ListIteratorOfListOfShape it(faces); it.More(); it.Next()) {
        TopoDS_Face face = TopoDS::Face(it.Value());
        if (!face.IsNull()) {
            adjacentFaces.push_back(face);
        }
    }

    if (adjacentFaces.empty()) {
        return false;
    }
    if (adjacentFaces.size() == 1) {
        if (BRep_Tool::IsClosed(edge, adjacentFaces.front())) {
            return false;
        }
        return true;
    }

    for (std::size_t i = 0; i + 1 < adjacentFaces.size(); ++i) {
        for (std::size_t j = i + 1; j < adjacentFaces.size(); ++j) {
            const GeomAbs_Shape continuity =
                BRep_Tool::Continuity(edge, adjacentFaces[i], adjacentFaces[j]);
            if (continuity >= GeomAbs_G1) {
                continue;
            }
            if (isSharpEdgeByAngle(edge, adjacentFaces[i], adjacentFaces[j], smoothEdgeCosine)) {
                return true;
            }
        }
    }

    return false;
}

} // namespace onecad::core::modeling

#endif // ONECAD_KERNEL_TOPOLOGY_TOPOLOGYVISIBILITY_H
