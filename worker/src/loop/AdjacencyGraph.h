// Ported from OneCAD-CPP src/core/loop/AdjacencyGraph.h @ b4ddcccc (2026-07-16)
#ifndef ONECAD_CORE_LOOP_ADJACENCY_GRAPH_H
#define ONECAD_CORE_LOOP_ADJACENCY_GRAPH_H

#include "../sketch/SketchTypes.h"

#include <optional>
#include <unordered_map>
#include <vector>

namespace onecad::core::loop {

namespace sk = onecad::core::sketch;

struct GraphNode {
    sk::EntityID id;
    sk::Vec2d position;
    std::vector<int> edges;
    std::vector<sk::EntityID> pointIds;
};

struct GraphEdge {
    sk::EntityID entityId;
    int startNode = -1;
    int endNode = -1;
    bool isArc = false;
    bool isCircle = false;
    sk::Vec2d startPos{};
    sk::Vec2d endPos{};
    sk::Vec2d centerPos{};
    double radius = 0.0;
    double startAngle = 0.0;
    double endAngle = 0.0;
};

struct AdjacencyGraph {
    std::vector<GraphNode> nodes;
    std::vector<GraphEdge> edges;
    std::unordered_map<sk::EntityID, int> nodeByPointId;
    std::unordered_map<sk::EntityID, int> edgeByEntity;

    int findOrCreateNode(const sk::Vec2d& pos,
                         const std::optional<sk::EntityID>& pointId,
                         double tolerance);
};

} // namespace onecad::core::loop

#endif  // ONECAD_CORE_LOOP_ADJACENCY_GRAPH_H
