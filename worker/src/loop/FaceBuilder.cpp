// Ported from OneCAD-CPP src/core/loop/FaceBuilder.cpp @ b4ddcccc (2026-07-16)
/**
 * @file FaceBuilder.cpp
 * @brief Implementation of FaceBuilder - converts LoopDetector results to OCCT faces
 */
#include "FaceBuilder.h"

#include "../sketch/SketchArc.h"
#include "../sketch/SketchCircle.h"
#include "../sketch/SketchEllipse.h"
#include "../sketch/SketchLine.h"
#include "../sketch/SketchPoint.h"

#include <BRepBuilderAPI_MakeEdge.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepBuilderAPI_MakeWire.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <GC_MakeArcOfCircle.hxx>
#include <GC_MakeCircle.hxx>
#include <GC_MakeSegment.hxx>
#include <Geom_Circle.hxx>
#include <Geom_Ellipse.hxx>
#include <Geom_TrimmedCurve.hxx>
#include <ShapeFix_Wire.hxx>
#include <TopoDS.hxx>
#include <gp_Ax2.hxx>
#include <gp_Ax3.hxx>
#include <gp_Circ.hxx>
#include <gp_Elips.hxx>

#include <algorithm>
#include <cmath>
#include <numbers>

namespace onecad::core::loop {

namespace {

constexpr double kAxisEpsilon = 1e-9;
constexpr double kMinArcSweep = 1e-6;

double dot(const sk::Vec3d& a, const sk::Vec3d& b) {
    return a.x * b.x + a.y * b.y + a.z * b.z;
}

double distanceSquared(const sk::Vec2d& a, const sk::Vec2d& b) {
    double dx = a.x - b.x;
    double dy = a.y - b.y;
    return dx * dx + dy * dy;
}

sk::Vec3d cross(const sk::Vec3d& a, const sk::Vec3d& b) {
    return {
        a.y * b.z - a.z * b.y,
        a.z * b.x - a.x * b.z,
        a.x * b.y - a.y * b.x
    };
}

bool normalize(sk::Vec3d& v) {
    double len = std::sqrt(v.x * v.x + v.y * v.y + v.z * v.z);
    if (len < kAxisEpsilon) {
        return false;
    }
    v.x /= len;
    v.y /= len;
    v.z /= len;
    return true;
}

sk::EntityID baseEdgeId(const sk::EntityID& entityId) {
    const size_t splitPos = entityId.find("#seg");
    if (splitPos == std::string::npos) {
        return entityId;
    }
    return entityId.substr(0, splitPos);
}

std::optional<sk::EntityID> singleClosedCurveBaseId(const Loop& loop, const sk::Sketch& sketch) {
    if (loop.wire.edges.empty()) {
        return std::nullopt;
    }

    const sk::EntityID baseId = baseEdgeId(loop.wire.edges.front());
    const auto* entity = sketch.getEntity(baseId);
    if (!entity || (entity->type() != sk::EntityType::Circle && entity->type() != sk::EntityType::Ellipse)) {
        return std::nullopt;
    }

    for (const auto& edgeId : loop.wire.edges) {
        if (baseEdgeId(edgeId) != baseId) {
            return std::nullopt;
        }
    }
    return baseId;
}

sk::Vec3d pickPerpendicular(const sk::Vec3d& n) {
    sk::Vec3d basis = (std::abs(n.z) < 0.9) ? sk::Vec3d{0.0, 0.0, 1.0} : sk::Vec3d{0.0, 1.0, 0.0};
    sk::Vec3d perp = cross(n, basis);
    if (!normalize(perp)) {
        return {1.0, 0.0, 0.0};
    }
    return perp;
}

Loop orientLoop(const Loop& loop, bool shouldBeCCW) {
    Loop oriented = loop;
    if (oriented.polygon.size() < 3) {
        return oriented;
    }
    bool isCCW = oriented.signedArea > 0.0;
    if (isCCW != shouldBeCCW) {
        std::reverse(oriented.wire.edges.begin(), oriented.wire.edges.end());
        std::reverse(oriented.wire.forward.begin(), oriented.wire.forward.end());
        for (size_t i = 0; i < oriented.wire.forward.size(); ++i) {
            oriented.wire.forward[i] = !oriented.wire.forward[i];
        }
        std::reverse(oriented.polygon.begin(), oriented.polygon.end());
        oriented.signedArea = -oriented.signedArea;
    }
    return oriented;
}

const char* wireErrorToString(BRepBuilderAPI_WireError error) {
    switch (error) {
        case BRepBuilderAPI_WireDone:
            return "WireDone";
        case BRepBuilderAPI_EmptyWire:
            return "EmptyWire";
        case BRepBuilderAPI_DisconnectedWire:
            return "DisconnectedWire";
        case BRepBuilderAPI_NonManifoldWire:
            return "NonManifoldWire";
        default:
            return "UnknownWireError";
    }
}

} // namespace

FaceBuilder::FaceBuilder()
    : config_() {}

FaceBuilder::FaceBuilder(const FaceBuilderConfig& config)
    : config_(config) {}

gp_Pln FaceBuilder::sketchPlaneToGpPln(const sk::SketchPlane& sketchPlane) {
    gp_Pnt origin(sketchPlane.origin.x, sketchPlane.origin.y, sketchPlane.origin.z);
    sk::Vec3d normal = sketchPlane.normal;
    sk::Vec3d xAxis = sketchPlane.xAxis;
    sk::Vec3d yAxis = sketchPlane.yAxis;

    if (!normalize(normal)) {
        normal = cross(xAxis, yAxis);
        if (!normalize(normal)) {
            normal = {0.0, 0.0, 1.0};
        }
    }

    if (!normalize(xAxis)) {
        xAxis = cross(yAxis, normal);
        if (!normalize(xAxis)) {
            xAxis = pickPerpendicular(normal);
        }
    }

    // Orthonormalize xAxis against normal
    double proj = dot(normal, xAxis);
    xAxis = {xAxis.x - proj * normal.x,
             xAxis.y - proj * normal.y,
             xAxis.z - proj * normal.z};
    if (!normalize(xAxis)) {
        xAxis = pickPerpendicular(normal);
    }

    gp_Dir normalDir(normal.x, normal.y, normal.z);
    gp_Dir xDir(xAxis.x, xAxis.y, xAxis.z);
    gp_Ax3 ax3(origin, normalDir, xDir);
    return gp_Pln(ax3);
}

gp_Pnt FaceBuilder::toGpPnt(const sk::Vec2d& p2d, const gp_Pln& plane) {
    return toGpPnt(p2d.x, p2d.y, plane);
}

gp_Pnt FaceBuilder::toGpPnt(double x, double y, const gp_Pln& plane) {
    const gp_Ax3& ax3 = plane.Position();
    const gp_Pnt& origin = ax3.Location();
    const gp_Dir& xDir = ax3.XDirection();
    const gp_Dir& yDir = ax3.YDirection();

    return gp_Pnt(
        origin.X() + x * xDir.X() + y * yDir.X(),
        origin.Y() + x * xDir.Y() + y * yDir.Y(),
        origin.Z() + x * xDir.Z() + y * yDir.Z()
    );
}

std::optional<TopoDS_Edge> FaceBuilder::createEdge(const sk::EntityID& entityId,
                                                    const sk::Sketch& sketch,
                                                    const gp_Pln& plane,
                                                    bool forward) const {
    const auto* entity = sketch.getEntity(entityId);
    if (!entity) {
        return std::nullopt;
    }

    try {
        switch (entity->type()) {
            case sk::EntityType::Line: {
                auto* line = sketch.getEntityAs<sk::SketchLine>(entityId);
                if (!line) return std::nullopt;

                auto* startPt = sketch.getEntityAs<sk::SketchPoint>(line->startPointId());
                auto* endPt = sketch.getEntityAs<sk::SketchPoint>(line->endPointId());
                if (!startPt || !endPt) return std::nullopt;

                gp_Pnt p1 = toGpPnt(startPt->x(), startPt->y(), plane);
                gp_Pnt p2 = toGpPnt(endPt->x(), endPt->y(), plane);

                if (p1.Distance(p2) < config_.edgeTolerance) {
                    return std::nullopt;  // Degenerate edge
                }

                // Respect direction
                if (!forward) {
                    std::swap(p1, p2);
                }

                GC_MakeSegment segmentMaker(p1, p2);
                if (!segmentMaker.IsDone()) {
                    return std::nullopt;
                }

                BRepBuilderAPI_MakeEdge edgeMaker(segmentMaker.Value());
                if (!edgeMaker.IsDone()) {
                    return std::nullopt;
                }

                return edgeMaker.Edge();
            }

            case sk::EntityType::Arc: {
                auto* arc = sketch.getEntityAs<sk::SketchArc>(entityId);
                if (!arc) return std::nullopt;

                auto* centerPt = sketch.getEntityAs<sk::SketchPoint>(arc->centerPointId());
                if (!centerPt) return std::nullopt;

                gp_Pnt center = toGpPnt(centerPt->x(), centerPt->y(), plane);
                const gp_Dir& normal = plane.Axis().Direction();

                // Create circle
                gp_Ax2 circleAxis(center, normal);
                gp_Circ circle(circleAxis, arc->radius());

                // Get start and end points
                double startAngle = arc->startAngle();
                double endAngle = arc->endAngle();

                if (!forward) {
                    std::swap(startAngle, endAngle);
                }

                gp_Pnt startPnt = toGpPnt(
                    centerPt->x() + arc->radius() * std::cos(startAngle),
                    centerPt->y() + arc->radius() * std::sin(startAngle),
                    plane
                );
                gp_Pnt endPnt = toGpPnt(
                    centerPt->x() + arc->radius() * std::cos(endAngle),
                    centerPt->y() + arc->radius() * std::sin(endAngle),
                    plane
                );

                double sweep = arc->sweepAngle();
                if (sweep < kMinArcSweep || startPnt.Distance(endPnt) < config_.edgeTolerance) {
                    return std::nullopt;  // Degenerate arc
                }

                if (!forward) {
                    sweep = -sweep;
                }

                double midAngle = startAngle + sweep / 2.0;

                gp_Pnt midPnt = toGpPnt(
                    centerPt->x() + arc->radius() * std::cos(midAngle),
                    centerPt->y() + arc->radius() * std::sin(midAngle),
                    plane
                );

                GC_MakeArcOfCircle arcMaker(startPnt, midPnt, endPnt);
                if (!arcMaker.IsDone()) {
                    return std::nullopt;
                }

                BRepBuilderAPI_MakeEdge edgeMaker(arcMaker.Value());
                if (!edgeMaker.IsDone()) {
                    return std::nullopt;
                }

                return edgeMaker.Edge();
            }

            case sk::EntityType::Circle: {
                auto* circle = sketch.getEntityAs<sk::SketchCircle>(entityId);
                if (!circle) return std::nullopt;

                auto* centerPt = sketch.getEntityAs<sk::SketchPoint>(circle->centerPointId());
                if (!centerPt) return std::nullopt;

                gp_Pnt center = toGpPnt(centerPt->x(), centerPt->y(), plane);
                const gp_Dir& normal = plane.Axis().Direction();

                gp_Ax2 circleAxis(center, normal);
                gp_Circ gcirc(circleAxis, circle->radius());

                Handle(Geom_Circle) geomCircle = new Geom_Circle(gcirc);
                BRepBuilderAPI_MakeEdge edgeMaker(geomCircle);
                if (!edgeMaker.IsDone()) {
                    return std::nullopt;
                }

                TopoDS_Edge edge = edgeMaker.Edge();
                if (!forward) {
                    edge.Reverse();
                }
                return edge;
            }

            case sk::EntityType::Ellipse: {
                auto* ellipse = sketch.getEntityAs<sk::SketchEllipse>(entityId);
                if (!ellipse) return std::nullopt;

                auto* centerPt = sketch.getEntityAs<sk::SketchPoint>(ellipse->centerPointId());
                if (!centerPt) return std::nullopt;

                gp_Pnt center = toGpPnt(centerPt->x(), centerPt->y(), plane);
                const gp_Ax3& ax3 = plane.Position();
                const gp_Dir& normal = plane.Axis().Direction();
                const gp_Dir& xDir = ax3.XDirection();
                const gp_Dir& yDir = ax3.YDirection();
                const double cosR = std::cos(ellipse->rotation());
                const double sinR = std::sin(ellipse->rotation());
                gp_Dir majorDir(xDir.X() * cosR + yDir.X() * sinR,
                                xDir.Y() * cosR + yDir.Y() * sinR,
                                xDir.Z() * cosR + yDir.Z() * sinR);

                gp_Ax2 ellipseAxis(center, normal, majorDir);
                gp_Elips gelips(ellipseAxis, ellipse->majorRadius(), ellipse->minorRadius());

                Handle(Geom_Ellipse) geomEllipse = new Geom_Ellipse(gelips);
                if (geomEllipse.IsNull()) {
                    return std::nullopt;
                }

                BRepBuilderAPI_MakeEdge edgeMaker(geomEllipse);
                if (!edgeMaker.IsDone()) {
                    return std::nullopt;
                }

                TopoDS_Edge edge = edgeMaker.Edge();
                if (!forward) {
                    edge.Reverse();
                }
                return edge;
            }

            default:
                return std::nullopt;
        }
    } catch (const Standard_Failure&) {
        return std::nullopt;
    } catch (const std::exception&) {
        return std::nullopt;
    }
}

WireBuildResult FaceBuilder::buildWire(const Loop& loop, const sk::Sketch& sketch) const {
    gp_Pln plane = sketchPlaneToGpPln(sketch.getPlane());
    return buildWire(loop, sketch, plane);
}

WireBuildResult FaceBuilder::buildWire(const Loop& loop, const sk::Sketch& sketch,
                                        const gp_Pln& plane) const {
    WireBuildResult result;

    bool canUseEntities = !loop.wire.edges.empty();
    if (canUseEntities) {
        for (const auto& entityId : loop.wire.edges) {
            if (!sketch.getEntity(entityId)) {
                canUseEntities = false;
                break;
            }
        }
    }

    if (!canUseEntities && loop.polygon.size() < 3) {
        result.errorMessage = "Wire has no valid entities or polygon data";
        return result;
    }

    try {
        BRepBuilderAPI_MakeWire wireMaker;

        if (auto closedBaseId = singleClosedCurveBaseId(loop, sketch)) {
            auto edge = createEdge(*closedBaseId, sketch, plane, loop.signedArea >= 0.0);
            if (!edge.has_value()) {
                result.errorMessage = "Failed to create edge for entity: " + *closedBaseId;
                return result;
            }
            wireMaker.Add(edge.value());
            if (wireMaker.Error() != BRepBuilderAPI_WireDone) {
                result.warnings.push_back(
                    std::string("Wire build reported: ") + wireErrorToString(wireMaker.Error()));
            }
        } else if (canUseEntities) {
            for (size_t i = 0; i < loop.wire.edges.size(); ++i) {
                const auto& entityId = loop.wire.edges[i];
                bool forward = (i < loop.wire.forward.size()) ? loop.wire.forward[i] : true;

                auto edge = createEdge(entityId, sketch, plane, forward);
                if (!edge.has_value()) {
                    result.errorMessage = "Failed to create edge for entity: " + entityId;
                    return result;
                }

                wireMaker.Add(edge.value());
                if (wireMaker.Error() != BRepBuilderAPI_WireDone) {
                    result.warnings.push_back(
                        std::string("Wire build reported: ") + wireErrorToString(wireMaker.Error()));
                }
            }
        } else {
            std::vector<sk::Vec2d> points = loop.polygon;
            double tol2 = config_.edgeTolerance * config_.edgeTolerance;
            if (points.size() > 1 && distanceSquared(points.front(), points.back()) <= tol2) {
                points.pop_back();
            }
            if (points.size() < 3) {
                result.errorMessage = "Polygon wire has insufficient points";
                return result;
            }

            for (size_t i = 0; i < points.size(); ++i) {
                const sk::Vec2d& from = points[i];
                const sk::Vec2d& to = points[(i + 1) % points.size()];
                if (distanceSquared(from, to) <= tol2) {
                    continue;
                }

                gp_Pnt p1 = toGpPnt(from, plane);
                gp_Pnt p2 = toGpPnt(to, plane);
                GC_MakeSegment segmentMaker(p1, p2);
                if (!segmentMaker.IsDone()) {
                    result.errorMessage = "Failed to create polygon segment";
                    return result;
                }

                BRepBuilderAPI_MakeEdge edgeMaker(segmentMaker.Value());
                if (!edgeMaker.IsDone()) {
                    result.errorMessage = "Failed to create polygon edge";
                    return result;
                }

                wireMaker.Add(edgeMaker.Edge());
                if (wireMaker.Error() != BRepBuilderAPI_WireDone) {
                    result.warnings.push_back(
                        std::string("Wire build reported: ") + wireErrorToString(wireMaker.Error()));
                }
            }

            result.warnings.push_back("Using polygon wire for planarized loop");
        }

        if (!wireMaker.IsDone()) {
            result.errorMessage = "Wire construction failed";
            return result;
        }

        TopoDS_Wire wire = wireMaker.Wire();

        // Attempt gap repair if enabled
        if (config_.repairGaps) {
            Handle(ShapeFix_Wire) wireFix = new ShapeFix_Wire(wire, TopoDS_Face(), config_.edgeTolerance);
            wireFix->SetMaxTolerance(config_.maxGapSize);
            wireFix->FixConnected();
            wireFix->FixClosed();
            wire = wireFix->Wire();
        }

        result.wire = wire;
        result.success = true;

    } catch (const Standard_Failure& e) {
        result.errorMessage = std::string("OCCT exception: ") + e.GetMessageString();
    } catch (const std::exception& e) {
        result.errorMessage = std::string("Exception: ") + e.what();
    } catch (...) {
        result.errorMessage = "Unknown exception during wire construction";
    }

    return result;
}

FaceBuildResult FaceBuilder::buildFace(const Face& face, const sk::Sketch& sketch) const {
    gp_Pln plane = sketchPlaneToGpPln(sketch.getPlane());
    return buildFace(face, sketch, plane);
}

FaceBuildResult FaceBuilder::buildFace(const Face& face, const sk::Sketch& sketch,
                                        const gp_Pln& plane) const {
    FaceBuildResult result;

    // Build outer wire
    Loop outerLoop = orientLoop(face.outerLoop, true);
    auto outerWireResult = buildWire(outerLoop, sketch, plane);
    if (!outerWireResult.success) {
        result.errorMessage = "Failed to build outer wire: " + outerWireResult.errorMessage;
        return result;
    }
    result.warnings.insert(result.warnings.end(),
                           outerWireResult.warnings.begin(),
                           outerWireResult.warnings.end());

    try {
        // Create face from outer wire
        BRepBuilderAPI_MakeFace faceMaker(plane, outerWireResult.wire, true);

        if (!faceMaker.IsDone()) {
            switch (faceMaker.Error()) {
                case BRepBuilderAPI_NoFace:
                    result.errorMessage = "No face created";
                    break;
                case BRepBuilderAPI_NotPlanar:
                    result.errorMessage = "Wire is not planar";
                    break;
                case BRepBuilderAPI_CurveProjectionFailed:
                    result.errorMessage = "Curve projection failed";
                    break;
                default:
                    result.errorMessage = "Face construction failed";
            }
            return result;
        }

        TopoDS_Face topoFace = faceMaker.Face();

        // Add inner wires (holes)
        for (const auto& hole : face.innerLoops) {
            Loop orientedHole = orientLoop(hole, false);
            auto holeWireResult = buildWire(orientedHole, sketch, plane);
            if (!holeWireResult.success) {
                result.warnings.push_back("Failed to build hole wire: " + holeWireResult.errorMessage);
                continue;
            }
            result.warnings.insert(result.warnings.end(),
                                   holeWireResult.warnings.begin(),
                                   holeWireResult.warnings.end());

            BRepBuilderAPI_MakeFace faceWithHole(topoFace);
            faceWithHole.Add(holeWireResult.wire);

            if (faceWithHole.IsDone()) {
                topoFace = faceWithHole.Face();
            } else {
                result.warnings.push_back("Failed to add hole to face");
            }
        }

        // Validate if requested
        if (config_.validate) {
            BRepCheck_Analyzer analyzer(topoFace);
            if (!analyzer.IsValid()) {
                result.errorMessage = "Face failed OCCT validation";
                return result;
            }
        }

        result.face = topoFace;
        result.success = true;

    } catch (const Standard_Failure& e) {
        result.errorMessage = std::string("OCCT exception: ") + e.GetMessageString();
    } catch (const std::exception& e) {
        result.errorMessage = std::string("Exception: ") + e.what();
    } catch (...) {
        result.errorMessage = "Unknown exception during face construction";
    }

    return result;
}

std::vector<FaceBuildResult> FaceBuilder::buildAllFaces(const LoopDetectionResult& loopResult,
                                                         const sk::Sketch& sketch) const {
    std::vector<FaceBuildResult> results;
    results.reserve(loopResult.faces.size());

    for (const auto& face : loopResult.faces) {
        results.push_back(buildFace(face, sketch));
    }

    return results;
}

} // namespace onecad::core::loop
