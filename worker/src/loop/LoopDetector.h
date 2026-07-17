// Ported from OneCAD-CPP src/core/loop/LoopDetector.h @ b4ddcccc (2026-07-16)
/**
 * @file LoopDetector.h
 * @brief Closed loop detection for sketch profiles
 *
 * Loop detection is essential for identifying extrudable profiles in sketches.
 * A closed loop is a sequence of connected edges (lines, arcs) that form a
 * continuous boundary.
 *
 * IMPLEMENTATION STATUS: COMPLETE
 * Graph-based algorithms implemented in LoopDetector.cpp (1506 lines).
 *
 * Key concepts:
 * - Wire: Ordered sequence of connected edges forming a path
 * - Loop: A closed wire (start == end)
 * - Face: A loop with optional inner loops (holes)
 * - Profile: Collection of non-intersecting faces for extrusion
 */
#ifndef ONECAD_CORE_LOOP_LOOP_DETECTOR_H
#define ONECAD_CORE_LOOP_LOOP_DETECTOR_H

#include "../sketch/SketchTypes.h"
#include <cmath>
#include <memory>
#include <optional>
#include <string>
#include <unordered_set>
#include <vector>

namespace onecad::core::sketch {
class Sketch;
class SketchEntity;
class SketchPoint;
class SketchLine;
class SketchArc;
} // namespace onecad::core::sketch

namespace onecad::core::loop {

namespace sk = onecad::core::sketch;

struct AdjacencyGraph;  // W-WP3a: was `class` (defined as struct in AdjacencyGraph.h) — fixes -Wmismatched-tags

/**
 * @brief Represents a connected wire (sequence of edges)
 */
struct Wire {
    /// Ordered list of entity IDs forming the wire
    std::vector<sk::EntityID> edges;

    /// Whether each edge is traversed forward or backward
    std::vector<bool> forward;

    /// Start point ID
    sk::EntityID startPoint{};

    /// End point ID
    sk::EntityID endPoint{};

    /// Total length of the wire
    double length = 0.0;

    /**
     * @brief Check if wire is closed (forms a loop)
     */
    bool isClosed() const { return startPoint == endPoint && !edges.empty(); }

    /**
     * @brief Get the point where this wire connects to another
     */
    std::optional<sk::EntityID> getConnectionPoint(const Wire& other) const {
        if (endPoint == other.startPoint) return endPoint;
        if (endPoint == other.endPoint) return endPoint;
        if (startPoint == other.startPoint) return startPoint;
        if (startPoint == other.endPoint) return startPoint;
        return std::nullopt;
    }
};

/**
 * @brief Represents a closed loop with computed properties
 */
struct Loop {
    /// The wire forming this loop
    Wire wire;

    /// Signed area (positive = CCW, negative = CW)
    double signedArea = 0.0;

    /// Sampled polygon used for area/containment tests
    std::vector<sk::Vec2d> polygon;

    /// Bounding box
    sk::Vec2d boundsMin{0, 0};
    sk::Vec2d boundsMax{0, 0};

    /// Centroid of the loop
    sk::Vec2d centroid{0, 0};

    /**
     * @brief Check if loop is counter-clockwise
     */
    bool isCCW() const { return signedArea > 0; }  // signedArea == 0 is degenerate (non-CCW)

    /**
     * @brief Get absolute area
     */
    double area() const { return std::abs(signedArea); }

    /**
     * @brief Check if a point is inside this loop
     *
     * Uses ray casting algorithm for point-in-polygon test.
     */
    bool contains(const sk::Vec2d& point) const;

    /**
     * @brief Check if another loop is completely inside this one
     */
    bool contains(const Loop& other) const;
};

/**
 * @brief Represents a face (outer loop with optional holes)
 */
struct Face {
    /// Outer boundary loop (CCW orientation)
    Loop outerLoop;

    /// Inner loops (holes, CW orientation)
    std::vector<Loop> innerLoops;

    /**
     * @brief Total area (outer - holes)
     */
    double area() const {
        double a = outerLoop.area();
        for (const auto& hole : innerLoops) {
            a -= hole.area();
        }
        return a;
    }

    /**
     * @brief Validate face topology
     * - Outer loop is CCW
     * - All holes are CW
     * - Holes don't overlap
     * - Holes are inside outer
     */
    bool isValid() const;
};

/**
 * @brief Result of loop detection
 */
struct LoopDetectionResult {
    /// All detected faces (with holes resolved)
    std::vector<Face> faces;

    /// Open wires (unclosed paths)
    std::vector<Wire> openWires;

    /// Isolated points (not connected to any edge)
    std::vector<sk::EntityID> isolatedPoints;

    /// Edges not part of any wire
    std::vector<sk::EntityID> unusedEdges;

    /// Whether detection succeeded without errors
    bool success = true;

    /// Error message if failed
    std::string errorMessage;

    /// Statistics
    int totalLoopsFound = 0;
    int facesWithHoles = 0;
};

/**
 * @brief Configuration for loop detection
 */
struct LoopDetectorConfig {
    /// Tolerance for considering points coincident (mm)
    double coincidenceTolerance = 1e-4;

    /// Whether to find all loops or just valid extrudable ones
    bool findAllLoops = false;

    /// Whether to compute areas and orientations
    bool computeAreas = true;

    /// Whether to resolve holes (assign to outer loops)
    bool resolveHoles = true;

    /// Maximum loops to find (0 = unlimited)
    size_t maxLoops = 0;

    /// Whether to validate results
    bool validate = true;

    /// Whether to planarize intersections by splitting edges
    bool planarizeIntersections = false;

    /// Tessellation tolerance for arcs/circles (mm)
    double tessellationTolerance = 0.05;

    /// Relative tessellation tolerance (fraction of radius)
    double tessellationRelative = 0.02;

    /// Minimum arc segments
    int minArcSegments = 8;

    /// Maximum arc segments
    int maxArcSegments = 256;

    /// Minimum circle segments
    int minCircleSegments = 32;

    /// Maximum circle segments
    int maxCircleSegments = 512;
};

/**
 * @brief Loop detector for sketch profiles
 *
 * Finds closed loops in sketch geometry suitable for extrusion.
 * Uses graph-based algorithms to detect and classify loops.
 *
 * IMPLEMENTATION STATUS: COMPLETE
 * Core algorithms implemented:
 * - Graph construction from sketch entities
 * - Depth-first search for cycle detection
 * - Shoelace formula for area/orientation
 * - Point-in-polygon for hole detection
 */
class LoopDetector {
public:
    /**
     * @brief Construct with default configuration
     */
    LoopDetector();

    /**
     * @brief Construct with custom configuration
     */
    explicit LoopDetector(const LoopDetectorConfig& config);

    /**
     * @brief Detect all loops in a sketch
     * @param sketch Sketch to analyze
     * @return Detection result with all found loops
     *
     * ALGORITHM (to be implemented):
     * 1. Build adjacency graph from sketch entities
     * 2. Find connected components
     * 3. For each component, find all simple cycles (DFS)
     * 4. Compute area and orientation for each cycle
     * 5. Build face hierarchy (outer loops containing holes)
     * 6. Validate and return results
     *
     */
    LoopDetectionResult detect(const sk::Sketch& sketch) const;

    /**
     * @brief Detect loops considering only selected entities
     */
    LoopDetectionResult detect(const sk::Sketch& sketch,
                                const std::vector<sk::EntityID>& selectedEntities) const;

    /**
     * @brief Find the smallest loop containing a point
     *
     * Useful for face selection during extrusion
     */
    std::optional<Face> findLoopAtPoint(const sk::Sketch& sketch, const sk::Vec2d& point) const;

    /**
     * @brief Check if a set of entities forms a closed loop
     */
    bool isClosedLoop(const sk::Sketch& sketch,
                      const std::vector<sk::EntityID>& entities) const;

    /**
     * @brief Get the wire from a set of entities
     *
     * Orders entities into a connected path if possible
     */
    std::optional<Wire> buildWire(const sk::Sketch& sketch,
                                   const std::vector<sk::EntityID>& entities) const;

    /**
     * @brief Update configuration
     */
    void setConfig(const LoopDetectorConfig& config) { config_ = config; }
    const LoopDetectorConfig& getConfig() const { return config_; }

private:
    LoopDetectorConfig config_;

    /**
     * @brief Build adjacency graph from sketch
     *
     * Each point becomes a node, each edge (line/arc) connects nodes.
     * Supports optional planarization for intersection handling.
     */
    std::unique_ptr<AdjacencyGraph> buildGraph(
        const sk::Sketch& sketch,
        const std::unordered_set<sk::EntityID>* selection = nullptr,
        bool planarize = false) const;

    /**
     * @brief Find all simple cycles in graph using DFS
     *
     * Uses depth-first search for cycle detection.
     */
    std::vector<Wire> findCycles(const AdjacencyGraph& graph) const;

    /**
     * @brief Extract planar faces (bounded cycles) from a planar graph
     */
    std::vector<Loop> findFaces(const AdjacencyGraph& graph) const;

    /**
     * @brief Compute loop properties (area, centroid, bounds)
     *
     * Uses Shoelace formula: Area = 0.5 * |Σ(x_i * y_{i+1} - x_{i+1} * y_i)|
     */
    void computeLoopProperties(Loop& loop, const sk::Sketch& sketch) const;

    /**
     * @brief Compute loop properties from a polygon
     */
    void computeLoopPropertiesFromPolygon(Loop& loop) const;

    /**
     * @brief Build face hierarchy from loops
     *
     * Uses point-in-polygon to determine nesting.
     * Note: loops are copied and may be reordered for hierarchy building.
     */
    std::vector<Face> buildFaceHierarchy(std::vector<Loop> loops) const;

    /**
     * @brief Validate loop geometry
     *
     * Checks for self-intersection, degenerate edges, etc.
     */
    bool validateLoop(const Loop& loop, const sk::Sketch& sketch) const;
};

// ========== Utility Functions ==========

/**
 * @brief Compute signed area of a polygon
 *
 * Uses Shoelace formula. Positive = CCW, Negative = CW
 */
double computeSignedArea(const std::vector<sk::Vec2d>& polygon);

/**
 * @brief Check if point is inside polygon
 *
 * Uses ray casting algorithm.
 */
bool isPointInPolygon(const sk::Vec2d& point, const std::vector<sk::Vec2d>& polygon);

/**
 * @brief Compute centroid of polygon
 */
sk::Vec2d computeCentroid(const std::vector<sk::Vec2d>& polygon);

/**
 * @brief Check if polygon edges intersect
 */
bool polygonsIntersect(const std::vector<sk::Vec2d>& poly1,
                       const std::vector<sk::Vec2d>& poly2);

} // namespace onecad::core::loop

#endif // ONECAD_CORE_LOOP_LOOP_DETECTOR_H
