// Ported from OneCAD-CPP src/kernel/topology/SelectionTopologyResolver.cpp @ b4ddcccc (2026-07-16)
#include "SelectionTopologyResolver.h"

#include "TopologyVisibility.h"
#include "geometry/EdgeChainer.h"

#include <BRep_Tool.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ShapeMapHasher.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Vertex.hxx>

#include <algorithm>
#include <cmath>
#include <numbers>
#include <optional>
#include <unordered_map>
#include <unordered_set>

namespace onecad::core::modeling {
namespace {

using kernel::elementmap::ElementId;
using kernel::elementmap::ElementKind;

template <typename T>
void sortAndUnique(std::vector<T>* values) {
    if (!values) {
        return;
    }
    std::sort(values->begin(), values->end());
    values->erase(std::unique(values->begin(), values->end()), values->end());
}

std::vector<std::string> idsForShapeOfKind(const kernel::elementmap::ElementMap& elementMap,
                                           const TopoDS_Shape& shape,
                                           ElementKind kind) {
    std::vector<std::string> ids;
    for (const auto& id : elementMap.findIdsByShape(shape)) {
        const auto* entry = elementMap.find(id);
        if (!entry || entry->kind != kind || entry->shape.IsNull()) {
            continue;
        }
        ids.push_back(id.value);
    }
    sortAndUnique(&ids);
    return ids;
}

std::optional<std::string> firstIdForShapeOfKind(const kernel::elementmap::ElementMap& elementMap,
                                                 const TopoDS_Shape& shape,
                                                 ElementKind kind) {
    const auto ids = idsForShapeOfKind(elementMap, shape, kind);
    if (ids.empty()) {
        return std::nullopt;
    }
    return ids.front();
}

struct StringDisjointSet {
    std::unordered_map<std::string, std::string> parent;

    void add(const std::string& value) {
        parent.emplace(value, value);
    }

    std::string find(const std::string& value) {
        auto it = parent.find(value);
        if (it == parent.end()) {
            return value;
        }
        if (it->second == value) {
            return value;
        }
        it->second = find(it->second);
        return it->second;
    }

    void unite(const std::string& first, const std::string& second) {
        const std::string rootFirst = find(first);
        const std::string rootSecond = find(second);
        if (rootFirst == rootSecond) {
            return;
        }
        if (rootFirst < rootSecond) {
            parent[rootSecond] = rootFirst;
        } else {
            parent[rootFirst] = rootSecond;
        }
    }
};

struct QuantizedPoint {
    std::int64_t x = 0;
    std::int64_t y = 0;
    std::int64_t z = 0;

    bool operator==(const QuantizedPoint& other) const {
        return x == other.x && y == other.y && z == other.z;
    }
};

struct QuantizedPointHash {
    std::size_t operator()(const QuantizedPoint& point) const noexcept {
        std::size_t seed = std::hash<std::int64_t>{}(point.x);
        seed ^= std::hash<std::int64_t>{}(point.y) + 0x9e3779b97f4a7c15ULL + (seed << 6) + (seed >> 2);
        seed ^= std::hash<std::int64_t>{}(point.z) + 0x9e3779b97f4a7c15ULL + (seed << 6) + (seed >> 2);
        return seed;
    }
};

QuantizedPoint quantizePoint(const gp_Pnt& point) {
    auto quantize = [](double value) -> std::int64_t {
        return static_cast<std::int64_t>(std::llround(value * 1e6));
    };
    return {quantize(point.X()), quantize(point.Y()), quantize(point.Z())};
}

std::vector<std::string> adjacentFaceLeadersForEdge(
    const TopoDS_Edge& edge,
    const TopTools_IndexedDataMapOfShapeListOfShape& edgeFaceMap,
    const kernel::elementmap::ElementMap& elementMap,
    const std::unordered_map<std::string, std::string>& faceLeaderByFaceId) {
    std::vector<std::string> leaders;
    const int edgeIndex = edgeFaceMap.FindIndex(edge);
    if (edgeIndex <= 0) {
        return leaders;
    }

    const TopTools_ListOfShape& faces = edgeFaceMap(edgeIndex);
    leaders.reserve(static_cast<std::size_t>(faces.Extent()));
    for (TopTools_ListIteratorOfListOfShape it(faces); it.More(); it.Next()) {
        const auto faceId = firstIdForShapeOfKind(elementMap, it.Value(), ElementKind::Face);
        if (!faceId.has_value()) {
            continue;
        }
        auto leaderIt = faceLeaderByFaceId.find(*faceId);
        leaders.push_back(leaderIt != faceLeaderByFaceId.end() ? leaderIt->second : *faceId);
    }
    sortAndUnique(&leaders);
    return leaders;
}

} // namespace

PromotedSelectionTopology SelectionTopologyResolver::resolve(
    const TopoDS_Shape& body,
    const kernel::elementmap::ElementMap& elementMap,
    const Options& options) {
    PromotedSelectionTopology result;
    if (body.IsNull()) {
        return result;
    }

    const double smoothEdgeCosine =
        std::cos(std::clamp(options.smoothEdgeAngleDeg, 0.0, 180.0) * std::numbers::pi / 180.0);

    std::unordered_map<TopoDS_Face, std::string, TopTools_ShapeMapHasher, TopTools_ShapeMapHasher>
        faceIdByShape;
    std::unordered_map<TopoDS_Edge, std::string, TopTools_ShapeMapHasher, TopTools_ShapeMapHasher>
        edgeIdByShape;

    StringDisjointSet faceSet;
    StringDisjointSet edgeSet;

    for (TopExp_Explorer faceExp(body, TopAbs_FACE); faceExp.More(); faceExp.Next()) {
        TopoDS_Face face = TopoDS::Face(faceExp.Current());
        const auto faceId = firstIdForShapeOfKind(elementMap, face, ElementKind::Face);
        if (!faceId.has_value()) {
            continue;
        }
        faceIdByShape.emplace(face, *faceId);
        faceSet.add(*faceId);
    }

    for (TopExp_Explorer edgeExp(body, TopAbs_EDGE); edgeExp.More(); edgeExp.Next()) {
        TopoDS_Edge edge = TopoDS::Edge(edgeExp.Current());
        const auto edgeId = firstIdForShapeOfKind(elementMap, edge, ElementKind::Edge);
        if (!edgeId.has_value()) {
            continue;
        }
        edgeIdByShape.emplace(edge, *edgeId);
        edgeSet.add(*edgeId);
    }

    TopTools_IndexedDataMapOfShapeListOfShape edgeFaceMap;
    TopTools_IndexedDataMapOfShapeListOfShape vertexEdgeMap;
    TopExp::MapShapesAndAncestors(body, TopAbs_EDGE, TopAbs_FACE, edgeFaceMap);
    TopExp::MapShapesAndAncestors(body, TopAbs_VERTEX, TopAbs_EDGE, vertexEdgeMap);

    std::unordered_map<std::string, TopoDS_Edge> visibleEdgeShapeById;
    std::unordered_map<std::string, std::vector<std::string>> visibleEdgeAdjacentFaceLeaders;

    for (int i = 1; i <= edgeFaceMap.Extent(); ++i) {
        const TopoDS_Edge& edge = TopoDS::Edge(edgeFaceMap.FindKey(i));
        const TopTools_ListOfShape& adjacentFaces = edgeFaceMap.FindFromIndex(i);
        const bool visible = isVisibleTopologyEdge(edge, adjacentFaces, smoothEdgeCosine);

        if (!visible) {
            std::string firstFaceId;
            for (TopTools_ListIteratorOfListOfShape it(adjacentFaces); it.More(); it.Next()) {
                const auto faceId = firstIdForShapeOfKind(elementMap, it.Value(), ElementKind::Face);
                if (!faceId.has_value()) {
                    continue;
                }
                if (firstFaceId.empty()) {
                    firstFaceId = *faceId;
                } else {
                    faceSet.unite(firstFaceId, *faceId);
                }
            }
            continue;
        }

        const auto edgeId = firstIdForShapeOfKind(elementMap, edge, ElementKind::Edge);
        if (!edgeId.has_value()) {
            continue;
        }
        visibleEdgeShapeById.emplace(*edgeId, edge);
    }

    std::unordered_map<std::string, std::vector<std::string>> faceMembersByRoot;
    for (const auto& [faceShape, faceId] : faceIdByShape) {
        faceMembersByRoot[faceSet.find(faceId)].push_back(faceId);
        (void)faceShape;
    }
    for (auto& [root, members] : faceMembersByRoot) {
        sortAndUnique(&members);
        if (members.empty()) {
            continue;
        }
        const std::string leader = members.front();
        result.faceMembersByLeader.emplace(leader, members);
        for (const auto& faceId : members) {
            result.faceLeaderByFaceId[faceId] = leader;
        }
    }

    for (const auto& [edgeId, edgeShape] : visibleEdgeShapeById) {
        visibleEdgeAdjacentFaceLeaders[edgeId] =
            adjacentFaceLeadersForEdge(edgeShape, edgeFaceMap, elementMap, result.faceLeaderByFaceId);
    }

    for (int i = 1; i <= vertexEdgeMap.Extent(); ++i) {
        const TopoDS_Vertex& vertex = TopoDS::Vertex(vertexEdgeMap.FindKey(i));
        const TopTools_ListOfShape& adjacentEdges = vertexEdgeMap.FindFromIndex(i);

        std::vector<std::pair<std::string, TopoDS_Edge>> incidentVisibleEdges;
        incidentVisibleEdges.reserve(static_cast<std::size_t>(adjacentEdges.Extent()));
        for (TopTools_ListIteratorOfListOfShape it(adjacentEdges); it.More(); it.Next()) {
            TopoDS_Edge edge = TopoDS::Edge(it.Value());
            const auto edgeId = firstIdForShapeOfKind(elementMap, edge, ElementKind::Edge);
            if (!edgeId.has_value()) {
                continue;
            }
            if (visibleEdgeShapeById.find(*edgeId) == visibleEdgeShapeById.end()) {
                continue;
            }
            incidentVisibleEdges.emplace_back(*edgeId, edge);
        }
        std::sort(incidentVisibleEdges.begin(), incidentVisibleEdges.end(),
                  [](const auto& first, const auto& second) {
                      return first.first < second.first;
                  });
        incidentVisibleEdges.erase(
            std::unique(incidentVisibleEdges.begin(),
                        incidentVisibleEdges.end(),
                        [](const auto& first, const auto& second) {
                            return first.first == second.first;
                        }),
            incidentVisibleEdges.end());

        if (incidentVisibleEdges.size() != 2) {
            (void)vertex;
            continue;
        }

        const auto& [firstEdgeId, firstEdge] = incidentVisibleEdges[0];
        const auto& [secondEdgeId, secondEdge] = incidentVisibleEdges[1];
        if (visibleEdgeAdjacentFaceLeaders[firstEdgeId] != visibleEdgeAdjacentFaceLeaders[secondEdgeId]) {
            continue;
        }
        if (!EdgeChainer::areTangentContinuous(firstEdge, secondEdge, options.tangentToleranceCosine)) {
            continue;
        }
        edgeSet.unite(firstEdgeId, secondEdgeId);
    }

    std::unordered_map<std::string, std::vector<std::string>> edgeMembersByRoot;
    for (const auto& [edgeShape, edgeId] : edgeIdByShape) {
        if (visibleEdgeShapeById.find(edgeId) == visibleEdgeShapeById.end()) {
            continue;
        }
        edgeMembersByRoot[edgeSet.find(edgeId)].push_back(edgeId);
        (void)edgeShape;
    }

    for (auto& [root, members] : edgeMembersByRoot) {
        sortAndUnique(&members);
        if (members.empty()) {
            continue;
        }
        const std::string leader = members.front();
        result.edgeMembersByLeader.emplace(leader, members);
        for (const auto& edgeId : members) {
            result.edgeLeaderByEdgeId[edgeId] = leader;
        }
        (void)root;
    }

    std::unordered_map<std::string,
                       std::unordered_map<QuantizedPoint, int, QuantizedPointHash>>
        groupEndpointCounts;
    std::unordered_map<QuantizedPoint, std::unordered_set<std::string>, QuantizedPointHash>
        leadersByEndpointPosition;
    std::unordered_map<QuantizedPoint,
                       std::unordered_map<std::string, int>,
                       QuantizedPointHash>
        leaderEndpointCountsByPosition;
    std::unordered_map<QuantizedPoint, std::unordered_set<std::string>, QuantizedPointHash>
        vertexIdsByEndpointPosition;

    for (const auto& [edgeId, edgeShape] : visibleEdgeShapeById) {
        auto leaderIt = result.edgeLeaderByEdgeId.find(edgeId);
        if (leaderIt == result.edgeLeaderByEdgeId.end()) {
            continue;
        }
        TopoDS_Vertex firstVertex;
        TopoDS_Vertex lastVertex;
        TopExp::Vertices(edgeShape, firstVertex, lastVertex);
        for (const auto& vertex : {firstVertex, lastVertex}) {
            if (vertex.IsNull()) {
                continue;
            }
            // BRep_Tool::Pnt already returns the located (world) point; applying
            // the vertex Location again double-transformed moved geometry.
            const gp_Pnt point = BRep_Tool::Pnt(vertex);
            const QuantizedPoint quantizedPoint = quantizePoint(point);
            groupEndpointCounts[leaderIt->second][quantizedPoint]++;
            leadersByEndpointPosition[quantizedPoint].insert(leaderIt->second);
            leaderEndpointCountsByPosition[quantizedPoint][leaderIt->second]++;

            const auto vertexId = firstIdForShapeOfKind(elementMap, vertex, ElementKind::Vertex);
            if (vertexId.has_value()) {
                vertexIdsByEndpointPosition[quantizedPoint].insert(*vertexId);
            }
        }
    }

    for (const auto& [leader, members] : result.edgeMembersByLeader) {
        auto countsIt = groupEndpointCounts.find(leader);
        if (countsIt == groupEndpointCounts.end() || countsIt->second.empty()) {
            result.edgeGroupClosedByLeader[leader] = false;
            continue;
        }
        bool closed = true;
        for (const auto& [point, count] : countsIt->second) {
            (void)point;
            if (count != 2) {
                closed = false;
                break;
            }
        }
        result.edgeGroupClosedByLeader[leader] = closed && !members.empty();
    }

    for (const auto& [position, leaders] : leadersByEndpointPosition) {
        if (leaders.size() != 1) {
            continue;
        }
        const std::string& leader = *leaders.begin();
        auto leaderCountIt = leaderEndpointCountsByPosition.find(position);
        if (leaderCountIt == leaderEndpointCountsByPosition.end()) {
            continue;
        }
        auto countIt = leaderCountIt->second.find(leader);
        if (countIt == leaderCountIt->second.end() || countIt->second != 2) {
            continue;
        }
        auto vertexIdsIt = vertexIdsByEndpointPosition.find(position);
        if (vertexIdsIt == vertexIdsByEndpointPosition.end()) {
            continue;
        }
        for (const auto& vertexId : vertexIdsIt->second) {
            result.suppressedVertexIds.insert(vertexId);
        }
    }

    return result;
}

std::optional<PromotedFaceSelection> SelectionTopologyResolver::resolvePromotedFaceSelection(
    const TopoDS_Shape& body,
    const kernel::elementmap::ElementMap& elementMap,
    const std::string& selectedFaceId,
    const Options& options) {
    if (body.IsNull() || selectedFaceId.empty()) {
        return std::nullopt;
    }

    const PromotedSelectionTopology topology = resolve(body, elementMap, options);
    const auto leaderIt = topology.faceLeaderByFaceId.find(selectedFaceId);
    const std::string leader =
        leaderIt != topology.faceLeaderByFaceId.end() ? leaderIt->second : selectedFaceId;

    auto membersIt = topology.faceMembersByLeader.find(leader);
    if (membersIt == topology.faceMembersByLeader.end() || membersIt->second.empty()) {
        return std::nullopt;
    }

    PromotedFaceSelection selection;
    selection.leaderFaceId = leader;
    selection.memberFaceIds = membersIt->second;
    selection.memberFaces.reserve(selection.memberFaceIds.size());

    for (const auto& faceId : selection.memberFaceIds) {
        const auto* entry = elementMap.find(ElementId::From(faceId));
        if (!entry || entry->kind != ElementKind::Face || entry->shape.IsNull()) {
            continue;
        }
        selection.memberFaces.push_back(TopoDS::Face(entry->shape));
    }

    if (selection.memberFaces.empty()) {
        return std::nullopt;
    }
    return selection;
}

} // namespace onecad::core::modeling
