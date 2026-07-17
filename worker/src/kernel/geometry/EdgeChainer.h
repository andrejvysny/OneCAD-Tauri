// Ported from OneCAD-CPP src/kernel/geometry/EdgeChainer.h @ b4ddcccc (2026-07-16)
/**
 * @file EdgeChainer.h
 * @brief Utility for building chains of tangent-continuous edges.
 */
#ifndef ONECAD_KERNEL_GEOMETRY_EDGECHAINER_H
#define ONECAD_KERNEL_GEOMETRY_EDGECHAINER_H

#include <TopoDS_Edge.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Vertex.hxx>
#include <gp_Dir.hxx>
#include <vector>

namespace onecad::core::modeling {

class EdgeChainer {
public:
    struct ChainResult {
        std::vector<TopoDS_Edge> edges;
        bool isClosed = false;
    };

    /**
     * @brief Builds a chain of tangent-continuous edges starting from a seed edge.
     * @param seed The starting edge.
     * @param body The shape containing all edges to consider.
     * @param tangentTolerance Cosine tolerance for tangent continuity (default ~0.5 deg).
     * @return ChainResult containing all connected tangent edges.
     */
    static ChainResult buildChain(
        const TopoDS_Edge& seed,
        const TopoDS_Shape& body,
        double tangentTolerance = 0.9999  // cos(~0.8 deg)
    );

    /**
     * @brief Checks if two edges are tangent continuous at their shared vertex.
     * @param e1 First edge.
     * @param e2 Second edge.
     * @param tolerance Cosine tolerance for tangent comparison.
     * @return true if edges are tangent continuous.
     */
    static bool areTangentContinuous(
        const TopoDS_Edge& e1,
        const TopoDS_Edge& e2,
        double tolerance = 0.9999
    );

    /**
     * @brief Gets all edges adjacent to the given edge (sharing a vertex).
     * @param edge The reference edge.
     * @param body The shape to search in.
     * @return Vector of adjacent edges (excluding the input edge).
     */
    static std::vector<TopoDS_Edge> adjacentEdges(
        const TopoDS_Edge& edge,
        const TopoDS_Shape& body
    );

private:
    /**
     * @brief Computes tangent direction at a vertex on an edge.
     * @param edge The edge.
     * @param vertex The vertex (must be on the edge).
     * @return Tangent direction pointing away from the vertex.
     */
    static gp_Dir tangentAtVertex(
        const TopoDS_Edge& edge,
        const TopoDS_Vertex& vertex
    );

    /**
     * @brief Finds shared vertex between two edges, if any.
     * @param e1 First edge.
     * @param e2 Second edge.
     * @return The shared vertex, or null vertex if none.
     */
    static TopoDS_Vertex findSharedVertex(
        const TopoDS_Edge& e1,
        const TopoDS_Edge& e2
    );
};

} // namespace onecad::core::modeling

#endif // ONECAD_KERNEL_GEOMETRY_EDGECHAINER_H
