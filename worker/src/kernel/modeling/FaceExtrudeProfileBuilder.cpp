// Ported from OneCAD-CPP src/kernel/modeling/FaceExtrudeProfileBuilder.cpp @ b4ddcccc (2026-07-16)
#include "FaceExtrudeProfileBuilder.h"

#include <BRepAdaptor_Surface.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRep_Builder.hxx>
#include <BRepBuilderAPI_MakeFace.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepGProp.hxx>
#include <BRep_Tool.hxx>
#include <GeomAbs_SurfaceType.hxx>
#include <GProp_GProps.hxx>
#include <ShapeAnalysis_FreeBounds.hxx>
#include <ShapeFix_Face.hxx>
#include <ShapeUpgrade_UnifySameDomain.hxx>
#include <Standard_Failure.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_HSequenceOfShape.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Wire.hxx>
#include <gp_Dir.hxx>
#include <gp_Pln.hxx>
#include <gp_Vec.hxx>

#include <algorithm>
#include <cmath>
#include <limits>

namespace onecad::core::modeling {
namespace {
constexpr double kMinFaceArea = 1e-8;
constexpr double kMinEdgeLength = 1e-8;

double sanitizePositive(double value, double fallback) {
    return value > 0.0 ? value : fallback;
}

double sanitizeNormalDotTolerance(double value, double fallback) {
    if (value > 0.0 && value <= 1.0) {
        return value;
    }
    return fallback;
}

bool planarFacePlaneAndNormal(const TopoDS_Face& face, gp_Pln& planeOut, gp_Dir& normalOut) {
    try {
        if (face.IsNull()) {
            return false;
        }
        BRepAdaptor_Surface surface(face, true);
        if (surface.GetType() != GeomAbs_Plane) {
            return false;
        }
        planeOut = surface.Plane();
        normalOut = planeOut.Axis().Direction();
        if (face.Orientation() == TopAbs_REVERSED) {
            normalOut.Reverse();
        }
        return true;
    } catch (...) {
        return false;
    }
}

double faceArea(const TopoDS_Face& face) {
    try {
        if (face.IsNull()) {
            return 0.0;
        }
        GProp_GProps props;
        BRepGProp::SurfaceProperties(face, props);
        return std::abs(props.Mass());
    } catch (...) {
        return 0.0;
    }
}

double edgeLength(const TopoDS_Edge& edge) {
    try {
        if (edge.IsNull()) {
            return 0.0;
        }
        GProp_GProps props;
        BRepGProp::LinearProperties(edge, props);
        return std::abs(props.Mass());
    } catch (...) {
        return 0.0;
    }
}

double projectedPlaneDistance(const gp_Dir& normal, const gp_Pln& fromPlane, const gp_Pln& toPlane) {
    const gp_Vec delta(fromPlane.Location(), toPlane.Location());
    const gp_Vec normalVec(normal.X(), normal.Y(), normal.Z());
    return std::abs(delta.Dot(normalVec));
}

TopoDS_Shape makeFaceCompound(const std::vector<TopoDS_Face>& faces) {
    if (faces.empty()) {
        return {};
    }
    if (faces.size() == 1) {
        return faces.front();
    }

    BRep_Builder builder;
    TopoDS_Compound compound;
    builder.MakeCompound(compound);
    for (const TopoDS_Face& face : faces) {
        if (!face.IsNull()) {
            builder.Add(compound, face);
        }
    }
    return compound;
}

std::vector<TopoDS_Face> uniqueFaces(const std::vector<TopoDS_Face>& faces) {
    std::vector<TopoDS_Face> unique;
    unique.reserve(faces.size());
    for (const TopoDS_Face& face : faces) {
        if (face.IsNull()) {
            continue;
        }
        bool alreadyPresent = false;
        for (const TopoDS_Face& existing : unique) {
            if (existing.IsSame(face)) {
                alreadyPresent = true;
                break;
            }
        }
        if (!alreadyPresent) {
            unique.push_back(face);
        }
    }
    return unique;
}

std::optional<TopoDS_Shape> booleanMergeFaces(const std::vector<TopoDS_Face>& faces) {
    try {
        if (faces.empty()) {
            return std::nullopt;
        }
        TopoDS_Shape merged = faces.front();
        if (merged.IsNull()) {
            return std::nullopt;
        }

        for (std::size_t i = 1; i < faces.size(); ++i) {
            if (faces[i].IsNull()) {
                continue;
            }
            BRepAlgoAPI_Fuse fuse(merged, faces[i]);
            fuse.Build();
            if (!fuse.IsDone()) {
                return std::nullopt;
            }
            merged = fuse.Shape();
            if (merged.IsNull()) {
                return std::nullopt;
            }
        }

        try {
            ShapeUpgrade_UnifySameDomain unify(merged, true, true, true);
            unify.Build();
            TopoDS_Shape unified = unify.Shape();
            if (!unified.IsNull()) {
                merged = unified;
            }
        } catch (...) {
            // Keep the pre-unified merged shape if unification fails.
        }

        if (merged.IsNull()) {
            return std::nullopt;
        }
        return merged;
    } catch (...) {
        // Fusing can throw on imperfect input geometry. Let boundary-based merge handle it.
        return std::nullopt;
    }
}

std::vector<TopoDS_Face> collectCoplanarFacesFromShape(const TopoDS_Shape& shape,
                                                       const gp_Pln& referencePlane,
                                                       const gp_Dir& referenceNormal,
                                                       double normalDotTolerance,
                                                       double planeDistanceTolerance) {
    std::vector<TopoDS_Face> faces;
    try {
        if (shape.IsNull()) {
            return faces;
        }
        for (TopExp_Explorer exp(shape, TopAbs_FACE); exp.More(); exp.Next()) {
            TopoDS_Face face = TopoDS::Face(exp.Current());
            if (face.IsNull()) {
                continue;
            }
            gp_Pln plane;
            gp_Dir normal;
            if (!planarFacePlaneAndNormal(face, plane, normal)) {
                continue;
            }
            const double dot = std::abs(referenceNormal.Dot(normal));
            if (dot < normalDotTolerance) {
                continue;
            }
            const double planeDistance = projectedPlaneDistance(referenceNormal, referencePlane, plane);
            if (planeDistance > planeDistanceTolerance) {
                continue;
            }
            faces.push_back(face);
        }
    } catch (...) {
        // Keep any faces collected before failure and continue with boundary fallback.
    }
    return uniqueFaces(faces);
}

std::vector<TopoDS_Edge> collectBoundaryEdges(const std::vector<TopoDS_Face>& patchFaces) {
    std::vector<TopoDS_Edge> boundaryEdges;
    try {
        const TopoDS_Shape patchShape = makeFaceCompound(patchFaces);
        if (patchShape.IsNull()) {
            return boundaryEdges;
        }

        TopTools_IndexedDataMapOfShapeListOfShape edgeFaceMap;
        TopExp::MapShapesAndAncestors(patchShape, TopAbs_EDGE, TopAbs_FACE, edgeFaceMap);
        boundaryEdges.reserve(static_cast<std::size_t>(edgeFaceMap.Extent()));
        for (int i = 1; i <= edgeFaceMap.Extent(); ++i) {
            const TopTools_ListOfShape& adjacentFaces = edgeFaceMap.FindFromIndex(i);
            if (adjacentFaces.Extent() != 1) {
                continue;
            }
            TopoDS_Edge edge = TopoDS::Edge(edgeFaceMap.FindKey(i));
            if (edge.IsNull()) {
                continue;
            }
            if (BRep_Tool::Degenerated(edge)) {
                continue;
            }
            if (edgeLength(edge) <= kMinEdgeLength) {
                continue;
            }
            boundaryEdges.push_back(edge);
        }
    } catch (...) {
        // Return partial edge set if available.
    }
    return boundaryEdges;
}

std::optional<TopoDS_Face> buildMergedFaceFromBoundary(const gp_Pln& referencePlane,
                                                       const std::vector<TopoDS_Edge>& boundaryEdges,
                                                       double wireConnectTolerance,
                                                       std::string& errorOut) {
    if (boundaryEdges.empty()) {
        errorOut = "Patch boundary is empty";
        return std::nullopt;
    }

    Handle(TopTools_HSequenceOfShape) edges = new TopTools_HSequenceOfShape();
    for (const TopoDS_Edge& edge : boundaryEdges) {
        if (!edge.IsNull()) {
            edges->Append(edge);
        }
    }
    if (edges->Length() == 0) {
        errorOut = "Patch boundary has no usable edges";
        return std::nullopt;
    }

    Handle(TopTools_HSequenceOfShape) wires = new TopTools_HSequenceOfShape();
    try {
        ShapeAnalysis_FreeBounds::ConnectEdgesToWires(
            edges,
            wireConnectTolerance,
            Standard_True,
            wires);
    } catch (...) {
        errorOut = "Could not chain patch boundary edges into wires";
        return std::nullopt;
    }

    if (wires->Length() == 0) {
        errorOut = "Could not chain patch boundary edges into wires";
        return std::nullopt;
    }

    std::vector<TopoDS_Wire> wireList;
    std::vector<double> wireAreas;
    wireList.reserve(static_cast<std::size_t>(wires->Length()));
    wireAreas.reserve(static_cast<std::size_t>(wires->Length()));

    for (int i = 1; i <= wires->Length(); ++i) {
        TopoDS_Wire wire = TopoDS::Wire(wires->Value(i));
        if (wire.IsNull()) {
            continue;
        }
        try {
            BRepBuilderAPI_MakeFace wireFaceMaker(referencePlane, wire, true);
            if (!wireFaceMaker.IsDone()) {
                continue;
            }
            const double area = faceArea(wireFaceMaker.Face());
            if (area <= 0.0) {
                continue;
            }
            wireList.push_back(wire);
            wireAreas.push_back(area);
        } catch (...) {
            continue;
        }
    }

    if (wireList.empty()) {
        errorOut = "Could not build valid wires from patch boundary";
        return std::nullopt;
    }

    std::size_t outerIndex = 0;
    double outerArea = -std::numeric_limits<double>::infinity();
    for (std::size_t i = 0; i < wireAreas.size(); ++i) {
        if (wireAreas[i] > outerArea) {
            outerArea = wireAreas[i];
            outerIndex = i;
        }
    }
    if (outerArea <= 0.0) {
        errorOut = "Outer wire area is invalid";
        return std::nullopt;
    }

    TopoDS_Face mergedFace;
    try {
        BRepBuilderAPI_MakeFace mergedFaceMaker(referencePlane, wireList[outerIndex], true);
        for (std::size_t i = 0; i < wireList.size(); ++i) {
            if (i == outerIndex) {
                continue;
            }
            mergedFaceMaker.Add(wireList[i]);
        }
        if (!mergedFaceMaker.IsDone()) {
            errorOut = "Failed to build merged profile face from wires";
            return std::nullopt;
        }
        mergedFace = mergedFaceMaker.Face();
    } catch (...) {
        errorOut = "Failed to build merged profile face from wires";
        return std::nullopt;
    }

    if (mergedFace.IsNull()) {
        errorOut = "Merged profile face is null";
        return std::nullopt;
    }

    try {
        ShapeFix_Face faceFix(mergedFace);
        faceFix.Perform();
        TopoDS_Face fixedFace = faceFix.Face();
        if (!fixedFace.IsNull()) {
            mergedFace = fixedFace;
        }
    } catch (...) {
        // Keep the un-fixed face; validity is checked below.
    }

    bool validFace = true;
    try {
        BRepCheck_Analyzer analyzer(mergedFace);
        validFace = analyzer.IsValid();
    } catch (...) {
        // Some OCCT analyzers can throw on otherwise usable planar faces.
        // Keep the face and rely on downstream build checks.
        validFace = true;
    }
    if (!validFace) {
        errorOut = "Merged profile face is invalid";
        return std::nullopt;
    }

    return mergedFace;
}

std::size_t faceCount(const TopoDS_Shape& shape) {
    std::size_t count = 0;
    try {
        if (shape.IsNull()) {
            return count;
        }
        for (TopExp_Explorer exp(shape, TopAbs_FACE); exp.More(); exp.Next()) {
            ++count;
        }
    } catch (...) {
        return 0;
    }
    return count;
}

} // namespace

std::optional<FaceExtrudeProfileBuilder::Result> FaceExtrudeProfileBuilder::build(
    const TopoDS_Face& seedFace,
    const std::vector<TopoDS_Face>& patchFaces,
    std::string& errorOut) {
    return build(seedFace, patchFaces, errorOut, Options{});
}

std::optional<FaceExtrudeProfileBuilder::Result> FaceExtrudeProfileBuilder::build(
    const TopoDS_Face& seedFace,
    const std::vector<TopoDS_Face>& patchFaces,
    std::string& errorOut,
    const Options& options) {
    errorOut.clear();
    const char* stage = "init";
    try {
        stage = "unique-faces";
        std::vector<TopoDS_Face> faces = uniqueFaces(patchFaces);
        if (faces.empty()) {
            errorOut = "Patch contains no valid faces";
            return std::nullopt;
        }

        stage = "sanitize-options";
        const double normalDotTolerance = sanitizeNormalDotTolerance(options.normalDotTolerance, 0.9999);
        const double planeDistanceTolerance = sanitizePositive(options.planeDistanceTolerance, 1e-3);
        const double pointClassifyTolerance = sanitizePositive(options.pointClassifyTolerance, 1e-4);

        stage = "seed-plane";
        gp_Pln referencePlane;
        gp_Dir referenceNormal;
        if (!planarFacePlaneAndNormal(seedFace, referencePlane, referenceNormal)) {
            if (!planarFacePlaneAndNormal(faces.front(), referencePlane, referenceNormal)) {
                errorOut = "Seed face is not planar";
                return std::nullopt;
            }
        }

        std::vector<TopoDS_Face> alignedFaces;
        alignedFaces.reserve(faces.size());

        stage = "align-faces";
        for (const TopoDS_Face& face : faces) {
            const double area = faceArea(face);
            if (area <= kMinFaceArea) {
                continue;
            }

            const char* alignStage = "begin";
            try {
                alignStage = "planar-face";
                gp_Pln plane;
                gp_Dir normal;
                if (!planarFacePlaneAndNormal(face, plane, normal)) {
                    errorOut = "Patch contains non-planar face";
                    return std::nullopt;
                }

                alignStage = "normal-dot";
                const double dot = referenceNormal.Dot(normal);
                if (std::abs(dot) < normalDotTolerance) {
                    errorOut = "Patch face normal differs from seed plane";
                    return std::nullopt;
                }

                alignStage = "plane-distance";
                const double planeDistance = projectedPlaneDistance(referenceNormal, referencePlane, plane);
                if (planeDistance > planeDistanceTolerance) {
                    errorOut = "Patch face is not coplanar with seed plane";
                    return std::nullopt;
                }

                alignStage = "orient-face";
                TopoDS_Face aligned = face;
                if (dot < 0.0) {
                    aligned.Reverse();
                }
                alignStage = "append-face";
                alignedFaces.push_back(aligned);
            } catch (const Standard_Failure& failure) {
                errorOut = "Patch face alignment failed at " + std::string(alignStage) + ": " +
                           std::string(failure.GetMessageString());
                return std::nullopt;
            } catch (...) {
                errorOut = "Patch face alignment failed at " + std::string(alignStage);
                return std::nullopt;
            }
        }

        if (alignedFaces.empty()) {
            errorOut = "Patch contains no non-degenerate planar faces";
            return std::nullopt;
        }

        TopoDS_Shape profileShape;
        if (alignedFaces.size() == 1) {
            profileShape = alignedFaces.front();
        } else {
            std::vector<TopoDS_Face> mergeCandidates;

            stage = "boolean-merge";
            if (auto booleanMerged = booleanMergeFaces(alignedFaces)) {
                stage = "collect-coplanar-faces";
                mergeCandidates = collectCoplanarFacesFromShape(
                    *booleanMerged,
                    referencePlane,
                    referenceNormal,
                    normalDotTolerance,
                    planeDistanceTolerance);
            }

            if (mergeCandidates.size() == 1) {
                profileShape = mergeCandidates.front();
            } else {
                if (mergeCandidates.empty()) {
                    mergeCandidates = alignedFaces;
                }
                stage = "collect-boundary-edges";
                std::vector<TopoDS_Edge> boundaryEdges = collectBoundaryEdges(mergeCandidates);
                stage = "build-boundary-face";
                auto mergedFace = buildMergedFaceFromBoundary(
                    referencePlane, boundaryEdges, pointClassifyTolerance, errorOut);
                if (!mergedFace || mergedFace->IsNull()) {
                    if (errorOut.empty()) {
                        errorOut = "Failed to build merged patch profile";
                    }
                    return std::nullopt;
                }

                profileShape = *mergedFace;
            }
        }

        if (profileShape.IsNull()) {
            errorOut = "Merged patch profile is null";
            return std::nullopt;
        }

        stage = "count-profile-faces";
        const std::size_t facesInProfile = faceCount(profileShape);
        if (facesInProfile != 1) {
            errorOut = "Merged patch profile is not a single face";
            return std::nullopt;
        }

        Result result;
        result.profileShape = profileShape;
        result.inputFaceCount = faces.size();
        result.mergedFaceCount = facesInProfile;
        return result;
    } catch (const Standard_Failure& failure) {
        errorOut = "Merged patch profile build failed at " + std::string(stage) + ": " +
                   std::string(failure.GetMessageString());
        return std::nullopt;
    } catch (...) {
        errorOut = "Merged patch profile build failed at " + std::string(stage);
        return std::nullopt;
    }
}

} // namespace onecad::core::modeling
