// Ported from OneCAD-CPP src/core/loop/RegionUtils.h @ b4ddcccc (2026-07-16)
/**
 * @file RegionUtils.h
 * @brief Shared utilities for sketch region identification.
 *
 * Provides stable region IDs derived from loop edge sets and helpers to
 * build region definitions (outer loop + holes) from loop detection results.
 */
#ifndef ONECAD_CORE_LOOP_REGIONUTILS_H
#define ONECAD_CORE_LOOP_REGIONUTILS_H

#include "LoopDetector.h"

#include <optional>
#include <string>
#include <vector>

namespace onecad::core::loop {

/**
 * @brief Region definition derived from loop detection.
 */
struct RegionDefinition {
    std::string id;
    std::string signature;
    Loop outerLoop;
    std::vector<Loop> holes;
};

/**
 * @brief Stable region key based on loop edge IDs.
 */
std::string regionKey(const Loop& loop);

/**
 * @brief Stable region signature based on outer loop and hole topology.
 */
std::string regionSignature(const Loop& outerLoop, const std::vector<Loop>& holes);

/**
 * @brief Stable region signature for an existing region definition.
 */
std::string regionSignature(const RegionDefinition& region);

/**
 * @brief Build region definitions (outer + holes) from loop detection result.
 */
std::vector<RegionDefinition> buildRegionDefinitions(const LoopDetectionResult& result,
                                                     double tolerance);

/**
 * @brief Find a region definition by region ID.
 */
std::optional<RegionDefinition> findRegionDefinition(const LoopDetectionResult& result,
                                                     const std::string& regionId,
                                                     double tolerance);

/**
 * @brief Default loop detector configuration for region selection.
 */
LoopDetectorConfig makeRegionDetectionConfig();

/**
 * @brief Resolve a sketch region ID into a loop::Face (outer + holes).
 */
std::optional<Face> resolveRegionFace(const sk::Sketch& sketch,
                                      const std::string& regionId);

/**
 * @brief Resolve a sketch region ID using a custom loop detector config.
 */
std::optional<Face> resolveRegionFace(const sk::Sketch& sketch,
                                      const std::string& regionId,
                                      const LoopDetectorConfig& config);

/**
 * @brief Collect all entity IDs (points and edges) that belong to a region.
 * Used for region selection and translateSketchRegion.
 */
std::vector<sk::EntityID> getEntityIdsInRegion(const sk::Sketch& sketch,
                                                const std::string& regionId);

/**
 * @brief Return line-loop boundary point IDs in traversal order.
 *
 * For non-line loops or invalid topology returns an empty vector.
 */
std::vector<sk::EntityID> getOrderedBoundaryPointIds(const sk::Sketch& sketch,
                                                      const Loop& loop);

/**
 * @brief Find the region ID that contains the given entity (edge or point).
 * Returns the first region whose loop contains the entity.
 */
std::optional<std::string> getRegionIdContainingEntity(const sk::Sketch& sketch,
                                                        const sk::EntityID& entityId);

} // namespace onecad::core::loop

#endif // ONECAD_CORE_LOOP_REGIONUTILS_H
