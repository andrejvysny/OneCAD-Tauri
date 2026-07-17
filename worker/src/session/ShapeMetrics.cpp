// ShapeMetrics.cpp — see ShapeMetrics.h.
#include "session/ShapeMetrics.h"

#include <BRepBndLib.hxx>
#include <BRepGProp.hxx>
#include <Bnd_Box.hxx>
#include <GProp_GProps.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>

namespace onecad::session {

double shape_volume(const TopoDS_Shape& shape) {
    if (shape.IsNull()) return 0.0;
    GProp_GProps props;
    // VolumeProperties integrates over the solids in the shape; a shape with no
    // solid (bare face/wire) integrates to 0.
    BRepGProp::VolumeProperties(shape, props);
    return props.Mass();
}

ShapeMetrics compute_shape_metrics(const TopoDS_Shape& shape) {
    ShapeMetrics m;
    if (shape.IsNull()) return m;

    TopTools_IndexedMapOfShape faces, edges, verts;
    TopExp::MapShapes(shape, TopAbs_FACE, faces);
    TopExp::MapShapes(shape, TopAbs_EDGE, edges);
    TopExp::MapShapes(shape, TopAbs_VERTEX, verts);
    m.face_count = static_cast<std::uint64_t>(faces.Extent());
    m.edge_count = static_cast<std::uint64_t>(edges.Extent());
    m.vertex_count = static_cast<std::uint64_t>(verts.Extent());

    Bnd_Box box;
    BRepBndLib::Add(shape, box);
    if (!box.IsVoid()) {
        Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
        box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
        m.bbox_min = {xmin, ymin, zmin};
        m.bbox_max = {xmax, ymax, zmax};
    }

    m.volume = shape_volume(shape);
    return m;
}

}  // namespace onecad::session
