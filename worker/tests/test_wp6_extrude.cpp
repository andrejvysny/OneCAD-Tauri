// test_wp6_extrude.cpp — W-WP6 extrude completion (scope C): ToFace / ToNext end
// conditions (typed targetFace ref via the ladder) + draft angle. In-process via
// execute_extrude with real OCCT. No framework: exit code == failure count.
#include <cstdio>
#include <string>
#include <utility>
#include <vector>

#include <BRepPrimAPI_MakeBox.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Pnt.hxx>

#include "elementmap/ElementMapPartition.h"
#include "nlohmann/json.hpp"
#include "ops/ExtrudeOp.h"
#include "ops/OpTypes.h"
#include "session/BodyStore.h"
#include "session/ShapeMetrics.h"
#include "util/Cancel.h"

using nlohmann::json;
namespace ops = onecad::ops;
namespace em = onecad::elementmap;
namespace km = onecad::kernel::elementmap;
using onecad::session::BodyStore;

namespace {
int g_failures = 0;
void check(bool cond, const std::string& msg) {
    if (!cond) { std::fprintf(stderr, "FAIL: %s\n", msg.c_str()); ++g_failures; }
}
void check_near(double got, double want, double tol, const std::string& msg) {
    if (std::abs(got - want) > tol) {
        std::fprintf(stderr, "FAIL: %s (got %.4f want %.4f)\n", msg.c_str(), got, want);
        ++g_failures;
    }
}

TopoDS_Shape face_by_center(const TopoDS_Shape& shape, double cx, double cy, double cz) {
    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(shape, TopAbs_FACE, faces);
    TopoDS_Shape best;
    double best_d2 = -1.0;
    for (int i = 1; i <= faces.Extent(); ++i) {
        const km::ElementDescriptor d = em::ElementMapPartition::describe(faces(i));
        const double dx = d.center.X() - cx, dy = d.center.Y() - cy, dz = d.center.Z() - cz;
        const double d2 = dx * dx + dy * dy + dz * dz;
        if (best_d2 < 0.0 || d2 < best_d2) { best_d2 = d2; best = faces(i); }
    }
    return best;
}

// Sketch params: a w×h rectangle on the XY plane. World map (u,v)->(-v,u,0), so a
// w(u)×h(v) rect → world x∈[-h,0], y∈[0,w], z=0 (area w·h).
json rect_sketch(const std::string& sid, double w, double h) {
    json sk;
    sk["sketchId"] = sid;
    sk["plane"] = json{{"kind", "XY"}};
    sk["entities"] = json::array({
        json{{"id", "e1"}, {"type", "Line"}, {"p0", {0, 0}}, {"p1", {w, 0}}},
        json{{"id", "e2"}, {"type", "Line"}, {"p0", {w, 0}}, {"p1", {w, h}}},
        json{{"id", "e3"}, {"type", "Line"}, {"p0", {w, h}}, {"p1", {0, h}}},
        json{{"id", "e4"}, {"type", "Line"}, {"p0", {0, h}}, {"p1", {0, 0}}},
    });
    sk["constraints"] = json::array();
    return sk;
}

struct Ctx {
    std::vector<std::pair<std::string, json>> sketches;
    std::string last_sketch;
    onecad::CancelToken cancel;
    ops::OpContext make(BodyStore& b, em::ElementMapPartition& p) {
        return ops::OpContext{b, &sketches, p, &last_sketch, false, json::object(), &cancel};
    }
};

// A 10×10 profile at z=0 extruded ToFace up to the bottom face (z=20) of a target
// box at z∈[20,30] ⇒ height 20, volume 2000.
void test_to_face() {
    // Target box: world x∈[-10,0], y∈[0,10], z∈[20,30]; bottom face z=20 centre (-5,5,20).
    const TopoDS_Shape target = BRepPrimAPI_MakeBox(gp_Pnt(-10, 0, 20), 10.0, 10.0, 10.0).Shape();
    BodyStore bodies;
    bodies.create("body_target", "op_t", target);
    em::ElementMapPartition part;

    const TopoDS_Shape bottom = face_by_center(target, -5, 5, 20);
    json target_face = {
        {"primary", {{"bodyId", "body_target"}, {"elementId", "el_tf"}, {"kind", "face"}}},
        {"intent", {{"kind", "face"}, {"descriptor", em::ElementMapPartition::descriptor_to_json(
                                                          em::ElementMapPartition::describe(bottom))}}},
        {"anchor", {{"worldPoint", {-5.0, 5.0, 20.0}}}}};

    Ctx c;
    c.sketches.push_back({"sk1", rect_sketch("sk1", 10, 10)});
    c.last_sketch = "sk1";
    ops::OpContext ctx = c.make(bodies, part);
    json op = {{"opType", "Extrude"}, {"opId", "ope"},
               {"params", {{"sketchId", "sk1"}, {"extrudeMode", "ToFace"}, {"booleanMode", "NewBody"},
                           {"targetFace", target_face}}}};
    ops::OpOutcome oc = ops::execute_extrude(ctx, op, "ope");
    check(oc.status == ops::OpOutcome::Status::Ok, "toFace: Ok");
    check(oc.needs_repair.empty(), "toFace: resolved (no NeedsRepair)");
    check(bodies.contains("body_ope"), "toFace: NewBody created");
    if (bodies.contains("body_ope"))
        check_near(onecad::session::shape_volume(bodies.get("body_ope")->geom), 2000.0, 1.0,
                   "toFace: reaches z=20 → volume 2000");
}

// A ToFace target whose body is gone ⇒ NeedsRepair STATE (never Err, never a bind).
void test_to_face_unresolved_needs_repair() {
    BodyStore bodies;
    em::ElementMapPartition part;
    Ctx c;
    c.sketches.push_back({"sk1", rect_sketch("sk1", 10, 10)});
    c.last_sketch = "sk1";
    ops::OpContext ctx = c.make(bodies, part);
    json target_face = {
        {"primary", {{"bodyId", "body_missing"}, {"elementId", "el_tf"}, {"kind", "face"}}},
        {"anchor", {{"worldPoint", {0.0, 0.0, 20.0}}}}};
    json op = {{"opType", "Extrude"}, {"opId", "ope"},
               {"params", {{"sketchId", "sk1"}, {"extrudeMode", "ToFace"}, {"booleanMode", "NewBody"},
                           {"targetFace", target_face}}}};
    ops::OpOutcome oc = ops::execute_extrude(ctx, op, "ope");
    check(oc.status == ops::OpOutcome::Status::Ok, "toFace unresolved: state not Err");
    check(!oc.needs_repair.empty(), "toFace unresolved: NeedsRepair emitted");
    check(!bodies.contains("body_ope"), "toFace unresolved: no body created (never a wrong bind)");
}

// ToNext: extrude toward a target body, stopping at its NEAREST planar face (z=20),
// not its far face (z=30) ⇒ height 20, volume 2000.
void test_to_next() {
    const TopoDS_Shape target = BRepPrimAPI_MakeBox(gp_Pnt(-10, 0, 20), 10.0, 10.0, 10.0).Shape();
    BodyStore bodies;
    bodies.create("body_target", "op_t", target);
    em::ElementMapPartition part;
    Ctx c;
    c.sketches.push_back({"sk1", rect_sketch("sk1", 10, 10)});
    c.last_sketch = "sk1";
    ops::OpContext ctx = c.make(bodies, part);
    json op = {{"opType", "Extrude"}, {"opId", "ope"},
               {"params", {{"sketchId", "sk1"}, {"extrudeMode", "ToNext"}, {"booleanMode", "NewBody"},
                           {"targetBodyId", "body_target"}}}};
    ops::OpOutcome oc = ops::execute_extrude(ctx, op, "ope");
    check(oc.status == ops::OpOutcome::Status::Ok, "toNext: Ok");
    check(bodies.contains("body_ope"), "toNext: NewBody created");
    if (bodies.contains("body_ope"))
        check_near(onecad::session::shape_volume(bodies.get("body_ope")->geom), 2000.0, 1.0,
                   "toNext: stops at nearest face z=20 → volume 2000");
}

// Draft: a 10×10 profile extruded 10mm with a 10° draft tapers the side faces
// inward ⇒ volume strictly below the 1000 straight prism.
void test_draft() {
    BodyStore bodies;
    em::ElementMapPartition part;
    Ctx c;
    c.sketches.push_back({"sk1", rect_sketch("sk1", 10, 10)});
    c.last_sketch = "sk1";
    ops::OpContext ctx = c.make(bodies, part);
    json op = {{"opType", "Extrude"}, {"opId", "ope"},
               {"params", {{"sketchId", "sk1"}, {"distance", 10.0}, {"draftAngleDeg", 10.0},
                           {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}};
    ops::OpOutcome oc = ops::execute_extrude(ctx, op, "ope");
    check(oc.status == ops::OpOutcome::Status::Ok, "draft: Ok");
    if (bodies.contains("body_ope")) {
        const double v = onecad::session::shape_volume(bodies.get("body_ope")->geom);
        check(v < 990.0 && v > 500.0, "draft: tapered volume clearly below the 1000 straight prism");
    }
}

}  // namespace

int main() {
    test_to_face();
    test_to_face_unresolved_needs_repair();
    test_to_next();
    test_draft();
    if (g_failures == 0) std::fprintf(stderr, "wp6_extrude: OK\n");
    return g_failures;
}
