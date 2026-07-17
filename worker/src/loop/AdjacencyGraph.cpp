// Ported from OneCAD-CPP src/core/loop/AdjacencyGraph.cpp @ b4ddcccc (2026-07-16)
#include "AdjacencyGraph.h"

namespace onecad::core::loop {

namespace {

double distanceSquared(const sk::Vec2d& a, const sk::Vec2d& b) {
    double dx = a.x - b.x;
    double dy = a.y - b.y;
    return dx * dx + dy * dy;
}

} // namespace

int AdjacencyGraph::findOrCreateNode(const sk::Vec2d& pos,
                                     const std::optional<sk::EntityID>& pointId,
                                     double tolerance) {
    double tol2 = tolerance * tolerance;
    if (pointId) {
        auto it = nodeByPointId.find(*pointId);
        if (it != nodeByPointId.end()) {
            return it->second;
        }
    }

    for (size_t i = 0; i < nodes.size(); ++i) {
        if (distanceSquared(nodes[i].position, pos) <= tol2) {
            if (pointId) {
                nodeByPointId[*pointId] = static_cast<int>(i);
                nodes[i].pointIds.push_back(*pointId);
            }
            return static_cast<int>(i);
        }
    }

    GraphNode node;
    node.position = pos;
    if (pointId) {
        node.id = *pointId;
        node.pointIds.push_back(*pointId);
        nodeByPointId[*pointId] = static_cast<int>(nodes.size());
    } else {
        node.id = "virtual_" + std::to_string(nodes.size());
    }

    nodes.push_back(std::move(node));
    return static_cast<int>(nodes.size() - 1);
}

} // namespace onecad::core::loop
