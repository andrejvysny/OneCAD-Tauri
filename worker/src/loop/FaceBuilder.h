// Ported from OneCAD-CPP src/core/loop/FaceBuilder.h @ b4ddcccc (2026-07-16)
/**
 * @file FaceBuilder.h
 * @brief Converts LoopDetector results to OCCT TopoDS_Face
 *
 * This wrapper bridges the sketch loop detection (2D) with OCCT solid modeling.
 * It creates TopoDS_Face objects from detected Face structures, which can then
 * be extruded, revolved, or used in other solid modeling operations.
 */
#ifndef ONECAD_CORE_LOOP_FACE_BUILDER_H
#define ONECAD_CORE_LOOP_FACE_BUILDER_H

#include "LoopDetector.h"
#include "../sketch/Sketch.h"

#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Pln.hxx>
#include <gp_Pnt.hxx>

#include <optional>
#include <string>
#include <vector>

namespace onecad::core::loop {

/**
 * @brief Result of face building operation
 */
struct FaceBuildResult {
    /// Successfully built OCCT face (may be null if failed)
    TopoDS_Face face;

    /// Whether build succeeded
    bool success = false;

    /// Error message if failed
    std::string errorMessage;

    /// Warnings (non-fatal issues)
    std::vector<std::string> warnings;
};

/**
 * @brief Result of wire building operation
 */
struct WireBuildResult {
    /// Successfully built OCCT wire
    TopoDS_Wire wire;

    /// Whether build succeeded
    bool success = false;

    /// Error message if failed
    std::string errorMessage;

    /// Warnings (non-fatal issues)
    std::vector<std::string> warnings;
};

/**
 * @brief Configuration for face building
 */
struct FaceBuilderConfig {
    /// Tolerance for edge connections
    double edgeTolerance = 1e-4;

    /// Arc tessellation segments (for validation, not OCCT)
    int arcSegments = 32;

    /// Whether to validate the result
    bool validate = true;

    /// Whether to attempt repair of small gaps
    bool repairGaps = true;

    /// Maximum gap size to repair (mm)
    double maxGapSize = 0.1;
};

/**
 * @brief Builds OCCT faces from LoopDetector results
 *
 * This class converts the 2D loop detection results (which use the Sketch
 * coordinate system) into 3D OCCT faces on a specified plane. The resulting
 * faces can be used for extrusion, revolution, or other solid modeling.
 *
 * Usage:
 * @code
 * FaceBuilder builder;
 * LoopDetector detector;
 * auto loops = detector.detect(sketch);
 *
 * for (const auto& face : loops.faces) {
 *     auto result = builder.buildFace(face, sketch);
 *     if (result.success) {
 *         // Use result.face for extrusion, etc.
 *     }
 * }
 * @endcode
 */
class FaceBuilder {
public:
    /**
     * @brief Construct with default configuration
     */
    FaceBuilder();

    /**
     * @brief Construct with custom configuration
     */
    explicit FaceBuilder(const FaceBuilderConfig& config);

    /**
     * @brief Build a TopoDS_Face from a LoopDetector Face
     *
     * @param face The detected face (outer loop + holes)
     * @param sketch The sketch containing the geometry
     * @return Build result with the face or error info
     *
     * The face is built on the sketch's plane in 3D space.
     * Inner loops become holes in the face.
     */
    FaceBuildResult buildFace(const Face& face, const sk::Sketch& sketch) const;

    /**
     * @brief Build a TopoDS_Face on a specific plane
     *
     * @param face The detected face
     * @param sketch The sketch containing geometry
     * @param plane The 3D plane to build the face on
     * @return Build result with the face or error info
     */
    FaceBuildResult buildFace(const Face& face, const sk::Sketch& sketch,
                               const gp_Pln& plane) const;

    /**
     * @brief Build a TopoDS_Wire from a Loop
     *
     * @param loop The loop to convert
     * @param sketch The sketch containing geometry
     * @return Wire result or error info
     *
     * The wire is built in 3D using the sketch's plane.
     */
    WireBuildResult buildWire(const Loop& loop, const sk::Sketch& sketch) const;

    /**
     * @brief Build a wire on a specific plane
     */
    WireBuildResult buildWire(const Loop& loop, const sk::Sketch& sketch,
                               const gp_Pln& plane) const;

    /**
     * @brief Build multiple faces from a LoopDetectionResult
     *
     * @param result The loop detection result
     * @param sketch The sketch
     * @return Vector of face build results
     */
    std::vector<FaceBuildResult> buildAllFaces(const LoopDetectionResult& result,
                                                const sk::Sketch& sketch) const;

    /**
     * @brief Set configuration
     */
    void setConfig(const FaceBuilderConfig& config) { config_ = config; }
    const FaceBuilderConfig& getConfig() const { return config_; }

private:
    FaceBuilderConfig config_;

    /**
     * @brief Create an OCCT edge from a sketch entity
     */
    std::optional<TopoDS_Edge> createEdge(const sk::EntityID& entityId,
                                           const sk::Sketch& sketch,
                                           const gp_Pln& plane,
                                           bool forward) const;

    /**
     * @brief Create OCCT plane from sketch plane
     */
    static gp_Pln sketchPlaneToGpPln(const sk::SketchPlane& sketchPlane);

    /**
     * @brief Convert 2D sketch point to 3D point on plane
     */
    static gp_Pnt toGpPnt(const sk::Vec2d& p2d, const gp_Pln& plane);
    static gp_Pnt toGpPnt(double x, double y, const gp_Pln& plane);
};

} // namespace onecad::core::loop

#endif // ONECAD_CORE_LOOP_FACE_BUILDER_H
