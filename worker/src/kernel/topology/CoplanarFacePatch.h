// Ported from OneCAD-CPP src/kernel/topology/CoplanarFacePatch.h @ b4ddcccc (2026-07-16)
/**
 * @file CoplanarFacePatch.h
 * @brief Utilities for connected coplanar face patch extraction.
 */
#ifndef ONECAD_KERNEL_TOPOLOGY_COPLANARFACEPATCH_H
#define ONECAD_KERNEL_TOPOLOGY_COPLANARFACEPATCH_H

#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>

#include <vector>

namespace onecad::core::modeling {

class CoplanarFacePatch {
public:
    struct Options {
        double normalDotTolerance = 0.9999;
        double planeDistanceTolerance = 1e-3;
    };

    /**
     * @brief Collect connected planar faces sharing seed plane orientation and distance.
     */
    static std::vector<TopoDS_Face> collectConnectedFaces(const TopoDS_Shape& body,
                                                          const TopoDS_Face& seedFace,
                                                          const Options& options);

    /**
     * @brief Collect connected planar faces with default tolerances.
     */
    static std::vector<TopoDS_Face> collectConnectedFaces(const TopoDS_Shape& body,
                                                          const TopoDS_Face& seedFace);

    /**
     * @brief Build a shape representing a patch from a set of faces.
     */
    static TopoDS_Shape makeFaceCompound(const std::vector<TopoDS_Face>& faces);
};

} // namespace onecad::core::modeling

#endif // ONECAD_KERNEL_TOPOLOGY_COPLANARFACEPATCH_H
