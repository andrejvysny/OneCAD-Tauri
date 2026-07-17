// Ported from OneCAD-CPP src/kernel/topology/FacePatchResolver.cpp @ b4ddcccc (2026-07-16)
#include "FacePatchResolver.h"

#include <TopAbs_ShapeEnum.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>

#include <algorithm>
#include <unordered_map>
#include <unordered_set>

namespace onecad::core::modeling {
namespace {

std::vector<std::string> sortedUnique(const std::vector<std::string>& values) {
    std::vector<std::string> out = values;
    std::sort(out.begin(), out.end());
    out.erase(std::unique(out.begin(), out.end()), out.end());
    return out;
}

std::vector<std::string> faceIdsForShape(const kernel::elementmap::ElementMap& elementMap,
                                         const TopoDS_Face& face) {
    if (face.IsNull()) {
        return {};
    }
    std::vector<std::string> ids;
    for (const auto& id : elementMap.findIdsByShape(face)) {
        const auto* entry = elementMap.find(id);
        if (!entry || entry->kind != kernel::elementmap::ElementKind::Face || entry->shape.IsNull()) {
            continue;
        }
        ids.push_back(id.value);
    }
    return sortedUnique(ids);
}

bool faceBelongsToBody(const TopoDS_Shape& body, const TopoDS_Face& face) {
    if (body.IsNull() || face.IsNull()) {
        return false;
    }
    for (TopExp_Explorer exp(body, TopAbs_FACE); exp.More(); exp.Next()) {
        if (exp.Current().IsSame(face)) {
            return true;
        }
    }
    return false;
}

} // namespace

std::optional<FacePatchSelection> FacePatchResolver::resolveFromSeedFaceId(
    const TopoDS_Shape& body,
    const kernel::elementmap::ElementMap& elementMap,
    const std::string& seedFaceId,
    const CoplanarFacePatch::Options& options) {
    if (seedFaceId.empty()) {
        return std::nullopt;
    }
    const auto* entry = elementMap.find(kernel::elementmap::ElementId::From(seedFaceId));
    if (!entry || entry->kind != kernel::elementmap::ElementKind::Face || entry->shape.IsNull()) {
        return std::nullopt;
    }
    TopoDS_Face seedFace = TopoDS::Face(entry->shape);
    if (!faceBelongsToBody(body, seedFace)) {
        return std::nullopt;
    }
    return resolveFromSeedFace(body, elementMap, seedFace, seedFaceId, options);
}

std::optional<FacePatchSelection> FacePatchResolver::resolveFromSeedFace(
    const TopoDS_Shape& body,
    const kernel::elementmap::ElementMap& elementMap,
    const TopoDS_Face& seedFace,
    const std::string& fallbackSeedFaceId,
    const CoplanarFacePatch::Options& options) {
    if (body.IsNull() || seedFace.IsNull() || !faceBelongsToBody(body, seedFace)) {
        return std::nullopt;
    }

    const auto patchFaces = CoplanarFacePatch::collectConnectedFaces(body, seedFace, options);
    if (patchFaces.empty()) {
        return std::nullopt;
    }

    std::unordered_map<std::string, TopoDS_Face> facesById;
    for (const TopoDS_Face& face : patchFaces) {
        if (face.IsNull()) {
            continue;
        }

        auto ids = faceIdsForShape(elementMap, face);
        if (ids.empty() && !fallbackSeedFaceId.empty() && face.IsSame(seedFace)) {
            ids.push_back(fallbackSeedFaceId);
        }

        if (ids.empty()) {
            continue;
        }
        const std::string& canonicalId = ids.front();
        if (facesById.find(canonicalId) == facesById.end()) {
            facesById.emplace(canonicalId, face);
        }
    }

    if (facesById.empty()) {
        return std::nullopt;
    }

    FacePatchSelection patch;
    patch.memberFaceIds.reserve(facesById.size());
    for (const auto& [faceId, face] : facesById) {
        (void)face;
        patch.memberFaceIds.push_back(faceId);
    }
    std::sort(patch.memberFaceIds.begin(), patch.memberFaceIds.end());
    patch.memberFaceIds.erase(std::unique(patch.memberFaceIds.begin(), patch.memberFaceIds.end()),
                              patch.memberFaceIds.end());
    patch.leaderFaceId = patch.memberFaceIds.front();

    patch.memberFaces.reserve(patch.memberFaceIds.size());
    for (const auto& faceId : patch.memberFaceIds) {
        patch.memberFaces.push_back(facesById.at(faceId));
    }

    return patch;
}

std::unordered_map<std::string, std::string> FacePatchResolver::buildLeaderMapForFaceIds(
    const TopoDS_Shape& body,
    const kernel::elementmap::ElementMap& elementMap,
    const std::vector<std::string>& faceIds,
    const CoplanarFacePatch::Options& options) {
    std::unordered_map<std::string, std::string> leaderByFace;
    if (faceIds.empty()) {
        return leaderByFace;
    }

    std::vector<std::string> sortedIds = sortedUnique(faceIds);
    leaderByFace.reserve(sortedIds.size());
    std::unordered_set<std::string> candidateSet(sortedIds.begin(), sortedIds.end());
    std::unordered_set<std::string> assigned;
    assigned.reserve(sortedIds.size());

    for (const auto& faceId : sortedIds) {
        leaderByFace[faceId] = faceId;
    }

    for (const auto& seedId : sortedIds) {
        if (assigned.find(seedId) != assigned.end()) {
            continue;
        }

        auto patch = resolveFromSeedFaceId(body, elementMap, seedId, options);
        if (!patch) {
            assigned.insert(seedId);
            continue;
        }

        std::vector<std::string> members;
        members.reserve(patch->memberFaceIds.size());
        for (const auto& faceId : patch->memberFaceIds) {
            if (candidateSet.find(faceId) != candidateSet.end()) {
                members.push_back(faceId);
            }
        }

        if (members.empty()) {
            assigned.insert(seedId);
            continue;
        }

        std::sort(members.begin(), members.end());
        members.erase(std::unique(members.begin(), members.end()), members.end());
        const std::string leader = members.front();
        for (const auto& faceId : members) {
            leaderByFace[faceId] = leader;
            assigned.insert(faceId);
        }
    }

    return leaderByFace;
}

} // namespace onecad::core::modeling
