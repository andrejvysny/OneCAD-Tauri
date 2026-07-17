// Ported from OneCAD-CPP tests/prototypes/proto_elementmap_rigorous.cpp @ b4ddcccc (2026-07-16)
#include <algorithm>
#include <cmath>
#include <iostream>
#include <string>
#include <vector>

// OCCT
#include <BRepAlgoAPI_Cut.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRep_Tool.hxx>
#include <TopExp.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <gp_Pnt.hxx>

#include "kernel/elementmap/ElementMap.h"

using onecad::kernel::elementmap::ElementDescriptor;
using onecad::kernel::elementmap::ElementId;
using onecad::kernel::elementmap::ElementKind;
using onecad::kernel::elementmap::ElementMap;

namespace {

struct TestContext {
    int failures{0};

    void expect(bool condition, const std::string& message) {
        if (!condition) {
            ++failures;
            std::cerr << "FAIL: " << message << std::endl;
        }
    }
};

bool startsWith(const std::string& value, const std::string& prefix) {
    return value.size() >= prefix.size() && value.compare(0, prefix.size(), prefix) == 0;
}

bool nearlyEqual(double a, double b, double tol = 1e-6) {
    return std::abs(a - b) <= tol;
}

TopoDS_Face findTopFace(const TopoDS_Shape& shape) {
    TopoDS_Face topFace;
    double maxZ = -1e9;

    for (TopExp_Explorer exp(shape, TopAbs_FACE); exp.More(); exp.Next()) {
        const TopoDS_Face face = TopoDS::Face(exp.Current());
        TopExp_Explorer expWire(face, TopAbs_WIRE);
        if (!expWire.More()) {
            continue;
        }
        TopExp_Explorer expEdge(expWire.Current(), TopAbs_EDGE);
        if (!expEdge.More()) {
            continue;
        }
        const TopoDS_Edge edge = TopoDS::Edge(expEdge.Current());
        const gp_Pnt p = BRep_Tool::Pnt(TopExp::FirstVertex(edge));
        if (p.Z() > maxZ) {
            maxZ = p.Z();
            topFace = face;
        }
    }

    return topFace;
}

TopoDS_Shape makeSplitCutter() {
    // Thin slab that cuts entirely through the box to force a face split.
    return BRepPrimAPI_MakeBox(gp_Pnt(4.5, -1.0, -1.0), gp_Pnt(5.5, 11.0, 11.0)).Shape();
}

void testBasicCutPreservesFace(TestContext& ctx) {
    ElementMap emap;

    BRepPrimAPI_MakeBox mkBox(10.0, 10.0, 10.0);
    mkBox.Build();
    const TopoDS_Shape boxShape = mkBox.Shape();

    const TopoDS_Face topFace = findTopFace(boxShape);
    ctx.expect(!topFace.IsNull(), "Top face should be found");

    const ElementId topId{"face-top"};
    emap.registerElement(topId, ElementKind::Face, topFace, "op-box");

    BRepPrimAPI_MakeBox mkTool(gp_Pnt(3.0, 3.0, -1.0), gp_Pnt(7.0, 7.0, 11.0));
    mkTool.Build();

    BRepAlgoAPI_Cut cut(boxShape, mkTool.Shape());
    cut.Build();
    ctx.expect(cut.IsDone(), "Cut should succeed");

    emap.update(cut, "op-cut");
    const auto* entry = emap.find(topId);
    ctx.expect(entry != nullptr, "Top face ID should remain after cut");
    ctx.expect(entry != nullptr && !entry->shape.IsNull(), "Top face should have a shape after cut");
}

void testSplitCreatesSiblingIds(TestContext& ctx) {
    ElementMap emap;

    BRepPrimAPI_MakeBox mkBox(10.0, 10.0, 10.0);
    mkBox.Build();
    const TopoDS_Shape boxShape = mkBox.Shape();

    const TopoDS_Face topFace = findTopFace(boxShape);
    ctx.expect(!topFace.IsNull(), "Top face should be found for split test");

    const ElementId topId{"face-top"};
    emap.registerElement(topId, ElementKind::Face, topFace, "op-box");

    BRepAlgoAPI_Cut cut(boxShape, makeSplitCutter());
    cut.Build();
    ctx.expect(cut.IsDone(), "Split cut should succeed");

    emap.update(cut, "op-split");

    const auto ids = emap.ids();
    ctx.expect(ids.size() >= 2, "Split should create at least one sibling ID");

    const std::string prefix = topId.value + "/face-split-";
    bool foundSplit = false;
    for (const auto& id : ids) {
        if (id.value != topId.value && startsWith(id.value, prefix)) {
            foundSplit = true;
            if (const auto* entry = emap.find(id)) {
                bool hasSource = false;
                for (const auto& source : entry->sources) {
                    if (source.value == topId.value) {
                        hasSource = true;
                        break;
                    }
                }
                ctx.expect(hasSource, "Split child should reference source face");
            }
        }
    }
    ctx.expect(foundSplit, "Split sibling ID should exist with expected prefix");
}

std::vector<std::string> collectSplitIds() {
    ElementMap emap;

    BRepPrimAPI_MakeBox mkBox(10.0, 10.0, 10.0);
    mkBox.Build();
    const TopoDS_Shape boxShape = mkBox.Shape();

    const TopoDS_Face topFace = findTopFace(boxShape);
    const ElementId topId{"face-top"};
    emap.registerElement(topId, ElementKind::Face, topFace, "op-box");

    BRepAlgoAPI_Cut cut(boxShape, makeSplitCutter());
    cut.Build();
    emap.update(cut, "op-split");

    std::vector<std::string> ids;
    for (const auto& id : emap.ids()) {
        ids.push_back(id.value);
    }
    std::sort(ids.begin(), ids.end());
    return ids;
}

void testDeterministicIds(TestContext& ctx) {
    const auto first = collectSplitIds();
    const auto second = collectSplitIds();
    ctx.expect(first == second, "Split IDs should be deterministic across runs");
}

void testSerializationRoundTrip(TestContext& ctx) {
    ElementMap emap;

    BRepPrimAPI_MakeBox mkBox(10.0, 10.0, 10.0);
    mkBox.Build();
    const TopoDS_Shape boxShape = mkBox.Shape();

    const TopoDS_Face topFace = findTopFace(boxShape);
    ctx.expect(!topFace.IsNull(), "Top face should be found for serialization test");

    const ElementId topId{"face-top"};
    emap.registerElement(topId, ElementKind::Face, topFace, "op-box");

    const std::string serialized = emap.toString();

    ElementMap restored;
    ctx.expect(restored.fromString(serialized), "ElementMap should deserialize");
    const auto* entry = restored.find(topId);
    ctx.expect(entry != nullptr, "Restored map should contain top face id");
    if (entry) {
        ctx.expect(nearlyEqual(entry->descriptor.center.Z(), 10.0), "Restored descriptor center Z");
        ctx.expect(entry->descriptor.shapeType == TopAbs_FACE, "Restored descriptor shape type");
    }
}

void testReverseMapMultiId(TestContext& ctx) {
    ElementMap emap;

    BRepPrimAPI_MakeBox mkBox(10.0, 10.0, 10.0);
    mkBox.Build();
    const TopoDS_Shape boxShape = mkBox.Shape();
    const TopoDS_Face topFace = findTopFace(boxShape);

    const ElementId idA{"face-a"};
    const ElementId idB{"face-b"};
    emap.registerElement(idA, ElementKind::Face, topFace, "op-box");
    emap.registerElement(idB, ElementKind::Face, topFace, "op-box");

    const auto ids = emap.findIdsByShape(topFace);
    bool hasA = false;
    bool hasB = false;
    for (const auto& id : ids) {
        hasA = hasA || id.value == idA.value;
        hasB = hasB || id.value == idB.value;
    }
    ctx.expect(hasA && hasB, "Reverse map should keep multiple IDs for same shape");
}

void testResolveWithFallbackRematchesAndRejects(TestContext& ctx) {
    BRepPrimAPI_MakeBox mkA(10.0, 10.0, 10.0);
    mkA.Build();
    const TopoDS_Shape boxA = mkA.Shape();
    const TopoDS_Face topA = findTopFace(boxA);

    BRepPrimAPI_MakeBox mkB(10.0, 10.0, 10.0);
    mkB.Build();
    const TopoDS_Face topB = findTopFace(mkB.Shape());

    ElementMap emap;
    emap.registerElement(ElementId{"b/face/x"}, ElementKind::Face, topA, "op");

    // Literal hit: a live id resolves with score 0.
    double score = -1.0;
    const TopoDS_Shape hit = emap.resolveWithFallback(ElementId{"b/face/x"}, 5.0, score);
    ctx.expect(!hit.IsNull() && nearlyEqual(score, 0.0), "Fallback returns live shape (score 0)");

    // Re-match: stale id recovers a near-identical same-owner face.
    emap.registerElement(ElementId{"b/face/y"}, ElementKind::Face, topB, "op");
    emap.clearShape(ElementId{"b/face/x"});
    double score2 = -1.0;
    const TopoDS_Shape rematched = emap.resolveWithFallback(ElementId{"b/face/x"}, 5.0, score2);
    ctx.expect(!rematched.IsNull() && score2 < 5.0, "Fallback re-matches a stale face by descriptor");

    // Reject: no same-kind, same-owner candidate -> null.
    ElementMap emap2;
    emap2.registerElement(ElementId{"c/face/x"}, ElementKind::Face, topA, "op");
    emap2.clearShape(ElementId{"c/face/x"});
    TopExp_Explorer edgeExp(boxA, TopAbs_EDGE);
    ctx.expect(edgeExp.More(), "Box should have an edge");
    emap2.registerElement(ElementId{"c/edge/e"}, ElementKind::Edge,
                          TopoDS::Edge(edgeExp.Current()), "op");
    double score3 = -1.0;
    const TopoDS_Shape rejected = emap2.resolveWithFallback(ElementId{"c/face/x"}, 5.0, score3);
    ctx.expect(rejected.IsNull(), "Fallback rejects when no same-kind candidate exists");
}

void testRebindTranslationKeepsIds(TestContext& ctx) {
    // A pure rigid translation of the whole body must keep every ID: the
    // rebind compensates stale descriptors by the body-center delta.
    ElementMap emap;
    const std::string bodyId = "body-move";

    const TopoDS_Shape box = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
    emap.rebindBody(bodyId, box, "op-base");
    const std::size_t idsBefore = emap.ids().size();

    const TopoDS_Shape movedBox =
        BRepPrimAPI_MakeBox(gp_Pnt(100.0, 0.0, 0.0), gp_Pnt(110.0, 10.0, 10.0)).Shape();
    emap.rebindBody(bodyId, movedBox, "op-move");

    ctx.expect(emap.ids().size() == idsBefore,
               "Translation rebind creates no new generated children");
    for (const auto& id : emap.ids()) {
        const auto* entry = emap.find(id);
        ctx.expect(entry != nullptr && !entry->shape.IsNull(),
                   "Translation rebind keeps every ID bound: " + id.value);
    }
}

void testRebindRejectsFarMatches(TestContext& ctx) {
    // When an entry's geometry is gone and every candidate is far away, the
    // entry must LOSE its shape (explicit downstream failure), not silently
    // migrate onto unrelated geometry.
    ElementMap emap;
    const std::string bodyId = "body-reject";

    const TopoDS_Shape box = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
    emap.rebindBody(bodyId, box, "op-base");

    // Register an extra face entry whose descriptor sits far outside any
    // face of the replacement body (same surface type, ~60mm away).
    const TopoDS_Shape farBox =
        BRepPrimAPI_MakeBox(gp_Pnt(60.0, 60.0, 60.0), gp_Pnt(70.0, 70.0, 70.0)).Shape();
    const TopoDS_Face farFace = findTopFace(farBox);
    const ElementId farId{bodyId + "/face-far"};
    emap.registerElement(farId, ElementKind::Face, farFace, "op-far");

    // Rebind to the original box again: nothing matches the far entry within
    // max(1mm, 0.5 x bbox diagonal ~ 8.7mm).
    emap.rebindBody(bodyId, box, "op-rebind");
    const auto* farEntry = emap.find(farId);
    ctx.expect(farEntry != nullptr, "Far entry survives as an entry");
    ctx.expect(farEntry != nullptr && farEntry->shape.IsNull(),
               "Far entry loses its shape instead of migrating to a wrong face");
}

void testFallbackRefusesSymmetricTwins(TestContext& ctx) {
    // Two equidistant same-kind candidates: a re-match would be a coin flip,
    // so resolveWithFallback must refuse (uniqueness margin).
    ElementMap emap;
    const std::string owner = "twin";

    const TopoDS_Shape leftBox =
        BRepPrimAPI_MakeBox(gp_Pnt(-20.0, 0.0, 0.0), gp_Pnt(-10.0, 10.0, 10.0)).Shape();
    const TopoDS_Shape rightBox =
        BRepPrimAPI_MakeBox(gp_Pnt(10.0, 0.0, 0.0), gp_Pnt(20.0, 10.0, 10.0)).Shape();
    emap.registerElement(ElementId{owner + "/face-left"}, ElementKind::Face,
                         findTopFace(leftBox), "op");
    emap.registerElement(ElementId{owner + "/face-right"}, ElementKind::Face,
                         findTopFace(rightBox), "op");

    // Stale entry exactly between the twins (top face of a centered box).
    const TopoDS_Shape centerBox =
        BRepPrimAPI_MakeBox(gp_Pnt(-5.0, 0.0, 0.0), gp_Pnt(5.0, 10.0, 10.0)).Shape();
    const ElementId staleId{owner + "/face-center"};
    emap.registerElement(staleId, ElementKind::Face, findTopFace(centerBox), "op");
    emap.clearShape(staleId);

    double score = 0.0;
    const TopoDS_Shape rematched = emap.resolveWithFallback(staleId, 100.0, score);
    ctx.expect(rematched.IsNull(),
               "Fallback refuses equidistant symmetric candidates");
}

} // namespace

int main() {
    std::cout << "--- ElementMap Rigorous Tests ---" << std::endl;

    TestContext ctx;
    testBasicCutPreservesFace(ctx);
    testSplitCreatesSiblingIds(ctx);
    testDeterministicIds(ctx);
    testSerializationRoundTrip(ctx);
    testReverseMapMultiId(ctx);
    testResolveWithFallbackRematchesAndRejects(ctx);
    testRebindTranslationKeepsIds(ctx);
    testRebindRejectsFarMatches(ctx);
    testFallbackRefusesSymmetricTwins(ctx);

    if (ctx.failures > 0) {
        std::cerr << "Tests failed: " << ctx.failures << std::endl;
        return 1;
    }

    std::cout << "All ElementMap tests passed." << std::endl;
    return 0;
}
