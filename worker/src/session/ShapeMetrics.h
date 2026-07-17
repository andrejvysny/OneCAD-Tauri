// ShapeMetrics.h — deterministic scalar metrics of a TopoDS_Shape (W-WP5).
//
// These feed the geometry SIGNATURE (SCHEMA §12) and are the numeric checks the
// corpus asserts (volume via BRepGProp). Every metric here is a pure function of
// the shape's topology + geometry — independent of OCCT threading/parallel flags —
// so it satisfies Invariant 5 (same plan ⇒ identical quantized signatures) in both
// `determinism` and `fast` session modes.
#pragma once

#include <array>
#include <cstdint>

#include <TopoDS_Shape.hxx>

namespace onecad::session {

// Counts + bounds + volume of one body shape. Counts use TopExp::MapShapes (each
// sub-shape once); volume is BRepGProp::VolumeProperties (0 for non-solids).
struct ShapeMetrics {
    std::uint64_t face_count = 0;
    std::uint64_t edge_count = 0;
    std::uint64_t vertex_count = 0;
    std::array<double, 3> bbox_min{0.0, 0.0, 0.0};
    std::array<double, 3> bbox_max{0.0, 0.0, 0.0};
    double volume = 0.0;  // BRepGProp::VolumeProperties Mass (mm^3); 0 if no solid
};

// Compute the metrics of `shape`. A null shape yields all-zero metrics.
ShapeMetrics compute_shape_metrics(const TopoDS_Shape& shape);

// Volume (mm^3) of a shape via BRepGProp::VolumeProperties — the corpus's
// load-bearing assertion (extrude Blind ⇒ 2000, ThroughAll cut ⇒ 3750, …).
double shape_volume(const TopoDS_Shape& shape);

}  // namespace onecad::session
