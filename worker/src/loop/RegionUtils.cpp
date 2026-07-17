// Ported from OneCAD-CPP src/core/loop/RegionUtils.cpp @ b4ddcccc (2026-07-16)
/**
 * @file RegionUtils.cpp
 * @brief Shared helpers for region IDs and hierarchy.
 */
#include "RegionUtils.h"

#include "../sketch/Sketch.h"
#include "../sketch/SketchArc.h"
#include "../sketch/SketchCircle.h"
#include "../sketch/SketchEllipse.h"
#include "../sketch/SketchLine.h"
#include "../sketch/SketchTypes.h"

#include <algorithm>
#include <limits>
#include <unordered_set>

namespace onecad::core::loop {

namespace {
constexpr double kGeometryEpsilon = 1e-9;

bool polygonContainsPolygon(const std::vector<sk::Vec2d>& outer,
                            const std::vector<sk::Vec2d>& inner,
                            double tolerance) {
    if (outer.size() < 3 || inner.size() < 3) {
        return false;
    }

    sk::Vec2d outerMin = outer.front();
    sk::Vec2d outerMax = outer.front();
    for (const auto& p : outer) {
        outerMin.x = std::min(outerMin.x, p.x);
        outerMin.y = std::min(outerMin.y, p.y);
        outerMax.x = std::max(outerMax.x, p.x);
        outerMax.y = std::max(outerMax.y, p.y);
    }

    for (const auto& p : inner) {
        if (p.x < outerMin.x - tolerance || p.y < outerMin.y - tolerance ||
            p.x > outerMax.x + tolerance || p.y > outerMax.y + tolerance) {
            return false;
        }
    }

    for (const auto& p : inner) {
        if (!isPointInPolygon(p, outer)) {
            return false;
        }
    }

    if (polygonsIntersect(outer, inner)) {
        return false;
    }

    return true;
}

sk::EntityID toBaseEdgeId(const sk::EntityID& loopEdgeId) {
    const size_t splitPos = loopEdgeId.find("#seg");
    if (splitPos == std::string::npos) {
        return loopEdgeId;
    }
    return loopEdgeId.substr(0, splitPos);
}

const sk::SketchEntity* resolveLoopEdgeEntity(const sk::Sketch& sketch,
                                              const sk::EntityID& loopEdgeId) {
    return sketch.getEntity(toBaseEdgeId(loopEdgeId));
}

} // namespace

std::string regionKey(const Loop& loop) {
    std::vector<sk::EntityID> edges = loop.wire.edges;
    std::sort(edges.begin(), edges.end());
    std::string key;
    key.reserve(edges.size() * 40);
    for (const auto& id : edges) {
        key.append(id);
        key.push_back('|');
    }
    return key;
}

std::string regionSignature(const Loop& outerLoop, const std::vector<Loop>& holes) {
    std::vector<std::string> holeKeys;
    holeKeys.reserve(holes.size());
    for (const auto& hole : holes) {
        holeKeys.push_back(regionKey(hole));
    }
    std::sort(holeKeys.begin(), holeKeys.end());

    std::string signature = "outer:" + regionKey(outerLoop) + ";holes:";
    for (const auto& holeKey : holeKeys) {
        signature.append(holeKey);
        signature.push_back(';');
    }
    return signature;
}

std::string regionSignature(const RegionDefinition& region) {
    return regionSignature(region.outerLoop, region.holes);
}

std::vector<RegionDefinition> buildRegionDefinitions(const LoopDetectionResult& result,
                                                     double tolerance) {
    std::vector<RegionDefinition> regions;
    if (!result.success) {
        return regions;
    }

    std::vector<Loop> loops;
    std::unordered_set<std::string> seen;

    auto addLoop = [&](const Loop& loop) {
        std::string key = regionKey(loop);
        if (key.empty()) {
            return;
        }
        if (seen.insert(key).second) {
            loops.push_back(loop);
        }
    };

    for (const auto& face : result.faces) {
        addLoop(face.outerLoop);
        for (const auto& hole : face.innerLoops) {
            addLoop(hole);
        }
    }

    if (loops.empty()) {
        return regions;
    }

    std::vector<size_t> order(loops.size());
    for (size_t i = 0; i < loops.size(); ++i) {
        order[i] = i;
    }
    std::sort(order.begin(), order.end(), [&](size_t a, size_t b) {
        return loops[a].area() > loops[b].area();
    });

    std::vector<int> parent(loops.size(), -1);
    for (size_t i = 0; i < loops.size(); ++i) {
        size_t loopIdx = order[i];
        double bestArea = std::numeric_limits<double>::infinity();
        int bestParent = -1;
        for (size_t j = 0; j < loops.size(); ++j) {
            size_t candidateIdx = order[j];
            if (candidateIdx == loopIdx) {
                continue;
            }
            if (loops[candidateIdx].area() <= loops[loopIdx].area()) {
                continue;
            }
            if (!polygonContainsPolygon(loops[candidateIdx].polygon, loops[loopIdx].polygon,
                                        tolerance)) {
                continue;
            }
            if (polygonsIntersect(loops[candidateIdx].polygon, loops[loopIdx].polygon)) {
                continue;
            }
            double area = loops[candidateIdx].area();
            if (area < bestArea) {
                bestArea = area;
                bestParent = static_cast<int>(candidateIdx);
            }
        }

        parent[loopIdx] = bestParent;
    }

    std::vector<std::vector<size_t>> children(loops.size());
    for (size_t i = 0; i < loops.size(); ++i) {
        if (parent[i] >= 0) {
            children[static_cast<size_t>(parent[i])].push_back(i);
        }
    }

    regions.reserve(loops.size());
    for (size_t i = 0; i < loops.size(); ++i) {
        if (loops[i].area() <= kGeometryEpsilon) {
            continue;
        }
        RegionDefinition region;
        region.id = regionKey(loops[i]);
        region.outerLoop = loops[i];
        for (size_t childIdx : children[i]) {
            if (loops[childIdx].area() <= kGeometryEpsilon) {
                continue;
            }
            region.holes.push_back(loops[childIdx]);
        }
        region.signature = regionSignature(region);
        regions.push_back(std::move(region));
    }

    return regions;
}

std::optional<RegionDefinition> findRegionDefinition(const LoopDetectionResult& result,
                                                     const std::string& regionId,
                                                     double tolerance) {
    if (regionId.empty()) {
        return std::nullopt;
    }
    auto regions = buildRegionDefinitions(result, tolerance);
    for (auto& region : regions) {
        if (region.id == regionId) {
            return region;
        }
    }
    return std::nullopt;
}

LoopDetectorConfig makeRegionDetectionConfig() {
    LoopDetectorConfig config;
    config.findAllLoops = false;
    config.computeAreas = true;
    config.resolveHoles = true;
    config.validate = true;
    config.planarizeIntersections = true;
    return config;
}

std::optional<Face> resolveRegionFace(const sk::Sketch& sketch,
                                      const std::string& regionId) {
    return resolveRegionFace(sketch, regionId, makeRegionDetectionConfig());
}

std::optional<Face> resolveRegionFace(const sk::Sketch& sketch,
                                      const std::string& regionId,
                                      const LoopDetectorConfig& config) {
    LoopDetector detector;
    detector.setConfig(config);
    auto result = detector.detect(sketch);
    if (!result.success) {
        return std::nullopt;
    }

    auto region = findRegionDefinition(result, regionId, sk::constants::COINCIDENCE_TOLERANCE);
    if (!region.has_value()) {
        return std::nullopt;
    }

    Face face;
    face.outerLoop = region->outerLoop;
    face.innerLoops = std::move(region->holes);
    return face;
}

namespace {

void collectPointIdsFromLoop(const sk::Sketch& sketch,
                             const Loop& loop,
                             std::unordered_set<sk::EntityID>& outPointIds) {
    for (const auto& edgeId : loop.wire.edges) {
        const auto* entity = resolveLoopEdgeEntity(sketch, edgeId);
        if (!entity) {
            continue;
        }
        if (auto* line = dynamic_cast<const sk::SketchLine*>(entity)) {
            outPointIds.insert(line->startPointId());
            outPointIds.insert(line->endPointId());
        } else if (auto* arc = dynamic_cast<const sk::SketchArc*>(entity)) {
            outPointIds.insert(arc->centerPointId());
        } else if (auto* circle = dynamic_cast<const sk::SketchCircle*>(entity)) {
            outPointIds.insert(circle->centerPointId());
        } else if (auto* ellipse = dynamic_cast<const sk::SketchEllipse*>(entity)) {
            outPointIds.insert(ellipse->centerPointId());
        }
    }
}

bool loopContainsEntity(const sk::Sketch& sketch,
                        const Loop& loop,
                        const sk::EntityID& entityId) {
    const auto* entity = sketch.getEntity(entityId);
    if (!entity) {
        return false;
    }
    if (entity->type() == sk::EntityType::Point) {
        for (const auto& edgeId : loop.wire.edges) {
            const auto* edge = resolveLoopEdgeEntity(sketch, edgeId);
            if (!edge) {
                continue;
            }
            if (auto* line = dynamic_cast<const sk::SketchLine*>(edge)) {
                if (line->startPointId() == entityId || line->endPointId() == entityId) {
                    return true;
                }
            } else if (auto* arc = dynamic_cast<const sk::SketchArc*>(edge)) {
                if (arc->centerPointId() == entityId) {
                    return true;
                }
            } else if (auto* circle = dynamic_cast<const sk::SketchCircle*>(edge)) {
                if (circle->centerPointId() == entityId) {
                    return true;
                }
            } else if (auto* ellipse = dynamic_cast<const sk::SketchEllipse*>(edge)) {
                if (ellipse->centerPointId() == entityId) {
                    return true;
                }
            }
        }
        return false;
    }
    sk::EntityID normalizedEntityId = toBaseEdgeId(entityId);
    for (const auto& edgeId : loop.wire.edges) {
        if (toBaseEdgeId(edgeId) == normalizedEntityId) {
            return true;
        }
    }
    return false;
}

} // namespace

std::vector<sk::EntityID> getEntityIdsInRegion(const sk::Sketch& sketch,
                                                const std::string& regionId) {
    std::vector<sk::EntityID> out;
    LoopDetector detector;
    detector.setConfig(makeRegionDetectionConfig());
    auto result = detector.detect(sketch);
    if (!result.success) {
        return out;
    }
    auto region = findRegionDefinition(result, regionId, sk::constants::COINCIDENCE_TOLERANCE);
    if (!region.has_value()) {
        return out;
    }
    std::unordered_set<sk::EntityID> pointIds;
    std::unordered_set<sk::EntityID> edgeIds;
    collectPointIdsFromLoop(sketch, region->outerLoop, pointIds);
    for (const auto& hole : region->holes) {
        collectPointIdsFromLoop(sketch, hole, pointIds);
    }
    for (const auto& id : region->outerLoop.wire.edges) {
        sk::EntityID baseId = toBaseEdgeId(id);
        if (!baseId.empty()) {
            edgeIds.insert(baseId);
        }
    }
    for (const auto& hole : region->holes) {
        for (const auto& id : hole.wire.edges) {
            sk::EntityID baseId = toBaseEdgeId(id);
            if (!baseId.empty()) {
                edgeIds.insert(baseId);
            }
        }
    }
    out.reserve(pointIds.size() + edgeIds.size());
    for (const auto& id : pointIds) {
        out.push_back(id);
    }
    for (const auto& id : edgeIds) {
        out.push_back(id);
    }
    return out;
}

std::vector<sk::EntityID> getOrderedBoundaryPointIds(const sk::Sketch& sketch,
                                                      const Loop& loop) {
    std::vector<sk::EntityID> ordered;
    if (loop.wire.edges.empty()) {
        return ordered;
    }

    struct Endpoints {
        sk::EntityID a;
        sk::EntityID b;
    };
    std::vector<Endpoints> edgeEndpoints;
    edgeEndpoints.reserve(loop.wire.edges.size());
    for (const auto& edgeId : loop.wire.edges) {
        const auto* entity = resolveLoopEdgeEntity(sketch, edgeId);
        auto* line = dynamic_cast<const sk::SketchLine*>(entity);
        if (!line) {
            return {};
        }
        if (line->startPointId().empty() || line->endPointId().empty()) {
            return {};
        }
        edgeEndpoints.push_back({line->startPointId(), line->endPointId()});
    }
    if (edgeEndpoints.size() < 2) {
        return {};
    }

    auto sharedEndpoint = [](const Endpoints& lhs, const Endpoints& rhs) -> std::optional<sk::EntityID> {
        if (lhs.a == rhs.a || lhs.a == rhs.b) {
            return lhs.a;
        }
        if (lhs.b == rhs.a || lhs.b == rhs.b) {
            return lhs.b;
        }
        return std::nullopt;
    };

    sk::EntityID startPoint;
    sk::EntityID currentPoint;

    if (loop.wire.forward.size() == edgeEndpoints.size()) {
        const auto& first = edgeEndpoints.front();
        if (loop.wire.forward.front()) {
            startPoint = first.a;
            currentPoint = first.b;
        } else {
            startPoint = first.b;
            currentPoint = first.a;
        }
    } else {
        const auto& first = edgeEndpoints.front();
        const auto& second = edgeEndpoints[1];
        auto shared = sharedEndpoint(first, second);
        if (!shared.has_value()) {
            return {};
        }
        startPoint = (first.a == *shared) ? first.b : first.a;
        currentPoint = *shared;
    }

    if (startPoint.empty() || currentPoint.empty()) {
        return {};
    }

    ordered.reserve(edgeEndpoints.size());
    ordered.push_back(startPoint);

    for (size_t i = 1; i < edgeEndpoints.size(); ++i) {
        const auto& edge = edgeEndpoints[i];
        ordered.push_back(currentPoint);
        if (currentPoint == edge.a) {
            currentPoint = edge.b;
        } else if (currentPoint == edge.b) {
            currentPoint = edge.a;
        } else {
            return {};
        }
    }

    if (currentPoint != startPoint) {
        return {};
    }

    std::unordered_set<sk::EntityID> uniquePoints(ordered.begin(), ordered.end());
    if (uniquePoints.size() != ordered.size()) {
        return {};
    }

    return ordered;
}

std::optional<std::string> getRegionIdContainingEntity(const sk::Sketch& sketch,
                                                        const sk::EntityID& entityId) {
    if (entityId.empty()) {
        return std::nullopt;
    }
    LoopDetector detector;
    detector.setConfig(makeRegionDetectionConfig());
    auto result = detector.detect(sketch);
    if (!result.success) {
        return std::nullopt;
    }
    auto regions = buildRegionDefinitions(result, sk::constants::COINCIDENCE_TOLERANCE);
    for (const auto& region : regions) {
        if (loopContainsEntity(sketch, region.outerLoop, entityId)) {
            return region.id;
        }
        for (const auto& hole : region.holes) {
            if (loopContainsEntity(sketch, hole, entityId)) {
                return region.id;
            }
        }
    }
    return std::nullopt;
}

} // namespace onecad::core::loop
