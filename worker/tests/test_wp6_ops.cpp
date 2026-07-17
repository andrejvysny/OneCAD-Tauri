// test_wp6_ops.cpp — W-WP6 new-op numerics (scope B): Fillet / Chamfer / Revolve,
// in-process via the op executors (real OCCT). Covers geometry results, the radius
// guard, and the "edge ref no longer resolves ⇒ NeedsRepair, never a wrong bind".
// No framework: exit code == failure count.
#include <cstdio>
#include <string>
#include <utility>
#include <vector>

#include <BRepPrimAPI_MakeBox.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopoDS_Shape.hxx>

#include "elementmap/ElementMapPartition.h"
#include "nlohmann/json.hpp"
#include "ops/FilletChamferOp.h"
#include "ops/OpTypes.h"
#include "ops/RevolveOp.h"
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

TopoDS_Shape edge_by_center(const TopoDS_Shape& shape, double cx, double cy, double cz) {
    TopTools_IndexedMapOfShape edges;
    TopExp::MapShapes(shape, TopAbs_EDGE, edges);
    TopoDS_Shape best;
    double best_d2 = -1.0;
    for (int i = 1; i <= edges.Extent(); ++i) {
        const km::ElementDescriptor d = em::ElementMapPartition::describe(edges(i));
        const double dx = d.center.X() - cx, dy = d.center.Y() - cy, dz = d.center.Z() - cz;
        const double d2 = dx * dx + dy * dy + dz * dz;
        if (best_d2 < 0.0 || d2 < best_d2) { best_d2 = d2; best = edges(i); }
    }
    return best;
}

std::size_t face_count(const TopoDS_Shape& s) {
    return onecad::session::compute_shape_metrics(s).face_count;
}

// An edge input ref (semantic ref) carrying the edge's frozen descriptor + anchor.
json edge_input(const std::string& body_id, const std::string& elem_id, const TopoDS_Shape& edge,
                double ax, double ay, double az) {
    return json{{"primary", {{"bodyId", body_id}, {"elementId", elem_id}, {"kind", "edge"}}},
                {"intent", {{"kind", "edge"},
                            {"descriptor", em::ElementMapPartition::descriptor_to_json(
                                               em::ElementMapPartition::describe(edge))}}},
                {"anchor", {{"worldPoint", {ax, ay, az}}}}};
}

// A minimal OpContext over `bodies` (no sketches).
struct Ctx {
    std::vector<std::pair<std::string, json>> sketches;
    std::string last_sketch;
    onecad::CancelToken cancel;
    ops::OpContext make(BodyStore& bodies, em::ElementMapPartition& part) {
        return ops::OpContext{bodies, &sketches, part, &last_sketch, false, json::object(), &cancel};
    }
};

// --- Fillet a single vertical box edge → +1 rolled face, small volume loss. ---
void test_fillet_edge() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();  // vol 1000
    BodyStore bodies;
    bodies.create("body_1", "op0", box);
    em::ElementMapPartition part;

    const TopoDS_Shape edge = edge_by_center(box, 0, 0, 5);  // vertical edge at (0,0)
    json op = {{"opType", "Fillet"}, {"opId", "opf"},
               {"inputs", json::array({edge_input("body_1", "el_e", edge, 0, 0, 5)})},
               {"params", {{"mode", "Fillet"}, {"radius", 1.0}, {"edgeIds", json::array({"e:x"})}}}};

    Ctx c;
    ops::OpContext ctx = c.make(bodies, part);
    ops::OpOutcome oc = ops::execute_fillet(ctx, op, "opf");
    check(oc.status == ops::OpOutcome::Status::Ok, "fillet: Ok");
    check(oc.needs_repair.empty(), "fillet: no NeedsRepair (edge resolves)");
    check(oc.body_events.size() == 1 && oc.body_events[0].kind == "modified", "fillet: body modified");
    const double v = onecad::session::shape_volume(bodies.get("body_1")->geom);
    check(v < 1000.0 && v > 990.0, "fillet: small volume loss (rounded corner)");
    check(face_count(bodies.get("body_1")->geom) == 7, "fillet: 6 + 1 rolled face = 7");
}

// --- Chamfer the same edge → +1 flat face, larger volume loss than fillet. ---
void test_chamfer_edge() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
    BodyStore bodies;
    bodies.create("body_1", "op0", box);
    em::ElementMapPartition part;

    const TopoDS_Shape edge = edge_by_center(box, 0, 0, 5);
    json op = {{"opType", "Chamfer"}, {"opId", "opc"},
               {"inputs", json::array({edge_input("body_1", "el_e", edge, 0, 0, 5)})},
               {"params", {{"mode", "Chamfer"}, {"radius", 1.0}, {"edgeIds", json::array({"e:x"})}}}};

    Ctx c;
    ops::OpContext ctx = c.make(bodies, part);
    ops::OpOutcome oc = ops::execute_chamfer(ctx, op, "opc");
    check(oc.status == ops::OpOutcome::Status::Ok, "chamfer: Ok");
    check(oc.needs_repair.empty(), "chamfer: no NeedsRepair");
    const double v = onecad::session::shape_volume(bodies.get("body_1")->geom);
    // Chamfer removes a right-triangle prism ~0.5·1·1·10 = 5 mm^3.
    check_near(v, 995.0, 1.0, "chamfer: volume ~995 (flat cut)");
    check(face_count(bodies.get("body_1")->geom) == 7, "chamfer: 6 + 1 flat face = 7");
}

// --- Radius below kMinValue (1e-3) → recoverable OP_FAILED. ---
void test_fillet_radius_too_small() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
    BodyStore bodies;
    bodies.create("body_1", "op0", box);
    em::ElementMapPartition part;
    const TopoDS_Shape edge = edge_by_center(box, 0, 0, 5);
    json op = {{"opType", "Fillet"}, {"opId", "opf"},
               {"inputs", json::array({edge_input("body_1", "el_e", edge, 0, 0, 5)})},
               {"params", {{"mode", "Fillet"}, {"radius", 0.0}, {"edgeIds", json::array({"e:x"})}}}};
    Ctx c;
    ops::OpContext ctx = c.make(bodies, part);
    ops::OpOutcome oc = ops::execute_fillet(ctx, op, "opf");
    check(oc.status == ops::OpOutcome::Status::Failed && oc.error_code == "OP_FAILED",
          "fillet: radius too small → OP_FAILED (recoverable)");
}

// --- Edge ref that does NOT resolve (symmetric tie) ⇒ NeedsRepair, body untouched. ---
void test_fillet_ambiguous_edge_needs_repair() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
    BodyStore bodies;
    bodies.create("body_1", "op0", box);
    em::ElementMapPartition part;

    // Intent = a vertical edge; anchor at the body centre → all 4 vertical edges tie.
    const TopoDS_Shape edge = edge_by_center(box, 0, 0, 5);
    json op = {{"opType", "Fillet"}, {"opId", "opf"},
               {"inputs", json::array({edge_input("body_1", "el_e", edge, 5, 5, 5)})},
               {"params", {{"mode", "Fillet"}, {"radius", 1.0}, {"edgeIds", json::array({"e:x"})}}}};
    Ctx c;
    ops::OpContext ctx = c.make(bodies, part);
    ops::OpOutcome oc = ops::execute_fillet(ctx, op, "opf");
    check(oc.status == ops::OpOutcome::Status::Ok, "fillet ambiguous: not an Err (state, not error)");
    check(!oc.needs_repair.empty(), "fillet ambiguous: NeedsRepair emitted");
    check(oc.body_events.empty(), "fillet ambiguous: body NOT modified (never a wrong bind)");
    check_near(onecad::session::shape_volume(bodies.get("body_1")->geom), 1000.0, 1e-6,
               "fillet ambiguous: body geometry unchanged");
}

// --- Revolve a rectangle offset from a sketch-line axis (Pappus volume). ---
void test_revolve_pappus() {
    // Sketch (XY plane, world map (u,v)->(-v,u,0)): rectangle u∈[10,20],v∈[0,10]
    // → world x∈[-10,0], y∈[10,20], z=0 (area 100, centroid y=15). Axis line along
    // u=0 → world through origin along X. Revolve 360° ⇒ V = 2π·15·100 = 9424.78.
    json sk;
    sk["sketchId"] = "sk1";
    sk["plane"] = json{{"kind", "XY"}};
    sk["entities"] = json::array({
        json{{"id", "r1"}, {"type", "Line"}, {"p0", {10, 0}}, {"p1", {20, 0}}},
        json{{"id", "r2"}, {"type", "Line"}, {"p0", {20, 0}}, {"p1", {20, 10}}},
        json{{"id", "r3"}, {"type", "Line"}, {"p0", {20, 10}}, {"p1", {10, 10}}},
        json{{"id", "r4"}, {"type", "Line"}, {"p0", {10, 10}}, {"p1", {10, 0}}},
        json{{"id", "axis"}, {"type", "Line"}, {"p0", {0, -5}}, {"p1", {0, 15}}},
    });
    sk["constraints"] = json::array();

    BodyStore bodies;
    em::ElementMapPartition part;
    Ctx c;
    c.sketches.push_back({"sk1", sk});
    c.last_sketch = "sk1";
    ops::OpContext ctx = c.make(bodies, part);

    json op = {{"opType", "Revolve"}, {"opId", "opr"},
               {"params", {{"sketchId", "sk1"}, {"angleDeg", 360.0}, {"booleanMode", "NewBody"},
                           {"axis", {{"kind", "sketchLine"}, {"sketchId", "sk1"}, {"lineId", "axis"}}}}}};
    ops::OpOutcome oc = ops::execute_revolve(ctx, op, "opr");
    check(oc.status == ops::OpOutcome::Status::Ok, "revolve: Ok");
    check(oc.body_events.size() == 1 && oc.body_events[0].kind == "created", "revolve: NewBody created");
    check(bodies.contains("body_opr"), "revolve: NewBody id body_opr (D1)");
    if (bodies.contains("body_opr")) {
        check_near(onecad::session::shape_volume(bodies.get("body_opr")->geom), 9424.78, 30.0,
                   "revolve: Pappus volume 2π·15·100");
    }
}

// --- Revolve angle too small → OP_FAILED. ---
void test_revolve_angle_too_small() {
    json sk;
    sk["sketchId"] = "sk1";
    sk["plane"] = json{{"kind", "XY"}};
    sk["entities"] = json::array({
        json{{"id", "r1"}, {"type", "Line"}, {"p0", {10, 0}}, {"p1", {20, 0}}},
        json{{"id", "r2"}, {"type", "Line"}, {"p0", {20, 0}}, {"p1", {20, 10}}},
        json{{"id", "r3"}, {"type", "Line"}, {"p0", {20, 10}}, {"p1", {10, 10}}},
        json{{"id", "r4"}, {"type", "Line"}, {"p0", {10, 10}}, {"p1", {10, 0}}},
        json{{"id", "axis"}, {"type", "Line"}, {"p0", {0, -5}}, {"p1", {0, 15}}},
    });
    sk["constraints"] = json::array();
    BodyStore bodies;
    em::ElementMapPartition part;
    Ctx c;
    c.sketches.push_back({"sk1", sk});
    c.last_sketch = "sk1";
    ops::OpContext ctx = c.make(bodies, part);
    json op = {{"opType", "Revolve"}, {"opId", "opr"},
               {"params", {{"sketchId", "sk1"}, {"angleDeg", 0.0}, {"booleanMode", "NewBody"},
                           {"axis", {{"kind", "sketchLine"}, {"sketchId", "sk1"}, {"lineId", "axis"}}}}}};
    ops::OpOutcome oc = ops::execute_revolve(ctx, op, "opr");
    check(oc.status == ops::OpOutcome::Status::Failed, "revolve: angle too small → OP_FAILED");
}

}  // namespace

int main() {
    test_fillet_edge();
    test_chamfer_edge();
    test_fillet_radius_too_small();
    test_fillet_ambiguous_edge_needs_repair();
    test_revolve_pappus();
    test_revolve_angle_too_small();
    if (g_failures == 0) std::fprintf(stderr, "wp6_ops: OK\n");
    return g_failures;
}
