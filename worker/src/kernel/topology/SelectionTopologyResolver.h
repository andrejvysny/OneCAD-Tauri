// Ported from OneCAD-CPP src/kernel/topology/SelectionTopologyResolver.h @ b4ddcccc (2026-07-16)
#ifndef ONECAD_KERNEL_TOPOLOGY_SELECTIONTOPOLOGYRESOLVER_H
#define ONECAD_KERNEL_TOPOLOGY_SELECTIONTOPOLOGYRESOLVER_H

#include "elementmap/ElementMap.h"

#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>

#include <optional>
#include <string>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace onecad::core::modeling {

struct PromotedSelectionTopology {
    std::unordered_map<std::string, std::string> faceLeaderByFaceId;
    std::unordered_map<std::string, std::vector<std::string>> faceMembersByLeader;
    std::unordered_map<std::string, std::string> edgeLeaderByEdgeId;
    std::unordered_map<std::string, std::vector<std::string>> edgeMembersByLeader;
    std::unordered_map<std::string, bool> edgeGroupClosedByLeader;
    std::unordered_set<std::string> suppressedVertexIds;
};

struct PromotedFaceSelection {
    std::string leaderFaceId;
    std::vector<std::string> memberFaceIds;
    std::vector<TopoDS_Face> memberFaces;
};

class SelectionTopologyResolver {
public:
    struct Options {
        double smoothEdgeAngleDeg = 15.0;
        double tangentToleranceCosine = 0.9999;
    };

    static PromotedSelectionTopology resolve(
        const TopoDS_Shape& body,
        const kernel::elementmap::ElementMap& elementMap) {
        return resolve(body, elementMap, Options{});
    }

    static PromotedSelectionTopology resolve(
        const TopoDS_Shape& body,
        const kernel::elementmap::ElementMap& elementMap,
        const Options& options);

    static std::optional<PromotedFaceSelection> resolvePromotedFaceSelection(
        const TopoDS_Shape& body,
        const kernel::elementmap::ElementMap& elementMap,
        const std::string& selectedFaceId) {
        return resolvePromotedFaceSelection(body, elementMap, selectedFaceId, Options{});
    }

    static std::optional<PromotedFaceSelection> resolvePromotedFaceSelection(
        const TopoDS_Shape& body,
        const kernel::elementmap::ElementMap& elementMap,
        const std::string& selectedFaceId,
        const Options& options);
};

} // namespace onecad::core::modeling

#endif // ONECAD_KERNEL_TOPOLOGY_SELECTIONTOPOLOGYRESOLVER_H
