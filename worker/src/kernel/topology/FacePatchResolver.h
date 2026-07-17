// Ported from OneCAD-CPP src/kernel/topology/FacePatchResolver.h @ b4ddcccc (2026-07-16)
/**
 * @file FacePatchResolver.h
 * @brief Deterministic connected coplanar patch resolution for face selections.
 */
#ifndef ONECAD_KERNEL_TOPOLOGY_FACEPATCHRESOLVER_H
#define ONECAD_KERNEL_TOPOLOGY_FACEPATCHRESOLVER_H

#include "CoplanarFacePatch.h"
#include "elementmap/ElementMap.h"

#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>

#include <optional>
#include <string>
#include <unordered_map>
#include <vector>

namespace onecad::core::modeling {

struct FacePatchSelection {
    std::string leaderFaceId;
    std::vector<std::string> memberFaceIds;
    std::vector<TopoDS_Face> memberFaces;
};

class FacePatchResolver {
public:
    /**
     * @brief Resolve connected coplanar patch from an element-map face ID.
     */
    static std::optional<FacePatchSelection> resolveFromSeedFaceId(
        const TopoDS_Shape& body,
        const kernel::elementmap::ElementMap& elementMap,
        const std::string& seedFaceId,
        const CoplanarFacePatch::Options& options = {});

    /**
     * @brief Resolve connected coplanar patch from a seed face shape.
     */
    static std::optional<FacePatchSelection> resolveFromSeedFace(
        const TopoDS_Shape& body,
        const kernel::elementmap::ElementMap& elementMap,
        const TopoDS_Face& seedFace,
        const std::string& fallbackSeedFaceId = {},
        const CoplanarFacePatch::Options& options = {});

    /**
     * @brief Build deterministic leader mapping for a face-id set.
     *
     * Returns a map {faceId -> patchLeaderFaceId}. Unresolvable IDs map to themselves.
     */
    static std::unordered_map<std::string, std::string> buildLeaderMapForFaceIds(
        const TopoDS_Shape& body,
        const kernel::elementmap::ElementMap& elementMap,
        const std::vector<std::string>& faceIds,
        const CoplanarFacePatch::Options& options = {});
};

} // namespace onecad::core::modeling

#endif // ONECAD_KERNEL_TOPOLOGY_FACEPATCHRESOLVER_H
