// test_wp5_plan.cpp — W-WP5 numeric + protocol checks driving PlanExecutor
// in-process (no fork). Verifies the corpus volumes (BRepGProp), BodyId
// preservation through booleans, per-step elementMapDelta bodyId, and cancellation
// leaving the session intact. No test framework: exit code == failure count.
#include <cstdio>
#include <string>
#include <vector>

#include <Standard_Failure.hxx>

#include "nlohmann/json.hpp"
#include "protocol/Dispatcher.h"
#include "protocol/Envelope.h"
#include "session/ElementIdentity.h"
#include "session/PlanExecutor.h"
#include "session/Session.h"
#include "session/ShapeMetrics.h"
#include "util/Cancel.h"

using nlohmann::json;
using onecad::CancelToken;
using onecad::protocol::Dispatcher;
using onecad::protocol::Envelope;
using onecad::protocol::HandlerContext;
using onecad::session::Session;

namespace {
int g_failures = 0;
constexpr const char* kEmpty =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

void check(bool cond, const std::string& msg) {
    if (!cond) {
        std::fprintf(stderr, "FAIL: %s\n", msg.c_str());
        ++g_failures;
    }
}
void check_near(double got, double want, double tol, const std::string& msg) {
    if (std::abs(got - want) > tol) {
        std::fprintf(stderr, "FAIL: %s (got %.4f want %.4f tol %.4f)\n", msg.c_str(), got, want, tol);
        ++g_failures;
    }
}

json line_ent(const std::string& id, double x0, double y0, double x1, double y1) {
    return json{{"id", id}, {"type", "Line"}, {"p0", json::array({x0, y0})}, {"p1", json::array({x1, y1})}};
}

// A rectangle Sketch op (w×h at origin) on the XY plane.
json rect_sketch(const std::string& op_id, const std::string& sid, double w, double h) {
    json params;
    params["sketchId"] = sid;
    params["plane"] = json{{"kind", "XY"}};
    params["entities"] = json::array({line_ent("e1", 0, 0, w, 0), line_ent("e2", w, 0, w, h),
                                      line_ent("e3", w, h, 0, h), line_ent("e4", 0, h, 0, 0)});
    params["constraints"] = json::array();
    return json{{"opType", "Sketch"}, {"opId", op_id}, {"stepIndex", 0}, {"params", params}};
}

struct PlanRun {
    Envelope resp;
    std::vector<json> steps;  // planStep payloads (in order)
};

// Run one ExecutePlan in-process, capturing planStep payloads. `pre_cancel` flips
// the cancel token before execution (deterministic cancellation).
PlanRun run_plan(Session& s, std::uint64_t job_id, const json& ops, const json& prefix_hashes,
                 const json& artifacts = json::object(), bool pre_cancel = false) {
    CancelToken tok;
    if (pre_cancel) tok.cancel();
    PlanRun run;
    HandlerContext ctx{tok, [](int) {}, [&](Envelope& e) { run.steps.push_back(e.result); }};
    json args = {{"jobId", job_id}, {"documentRevision", 0}, {"workerEpoch", 3},
                 {"expectedBaseHash", kEmpty}, {"prefixHashes", prefix_hashes},
                 {"targetStep", ops.size()}, {"ops", ops}};
    if (!artifacts.empty()) args["artifacts"] = artifacts;
    run.resp = onecad::session::handle_execute_plan(s, Envelope::request(job_id, "ExecutePlan", args), ctx);
    return run;
}

void accept(Session& s, std::uint64_t job_id) {
    Envelope req = Envelope::request(job_id, "AcceptPrepared",
                                     json{{"jobId", job_id}, {"documentRevision", 0}, {"workerEpoch", 3}});
    onecad::session::handle_accept_prepared(s, req);
}

double body_volume(Session& s, const std::string& id) {
    onecad::session::BodyStore b = s.bodies_copy();
    const onecad::session::BodyRecord* r = b.get(id);
    return r ? onecad::session::shape_volume(r->geom) : -1.0;
}

// Session holds a std::mutex (non-movable), so it cannot be returned by value;
// open it in place instead.
void open_fresh(Session& s) { s.open("doc", 0, 3, "determinism"); }

// --- corpus a: extrude Blind volume 2000 (10×10 × 20) ---
void test_blind_volume() {
    Session s; open_fresh(s);
    json ops = json::array(
        {rect_sketch("op0", "sk1", 10, 10),
         json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
              {"params", {{"sketchId", "sk1"}, {"distance", 20.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}}});
    PlanRun r = run_plan(s, 1, ops, json::array({"h0", "h1"}));
    check(r.resp.result.value("stoppedReason", "") == "completed", "blind: completed");
    accept(s, 1);
    check_near(body_volume(s, "body_op1"), 2000.0, 10.0, "corpus a: extrude Blind volume 2000");
}

// --- corpus b: ThroughAll cut (4000→3750) + two-direction (800) ---
void test_throughall_and_twodir() {
    {  // ThroughAll cut: 20×20×10 base (4000), 5×5 ThroughAll Cut → 3750.
        Session s; open_fresh(s);
        json ops = json::array(
            {rect_sketch("op0", "base", 20, 20),
             json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
                  {"params", {{"sketchId", "base"}, {"distance", 10.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}},
             rect_sketch("op2", "cut", 5, 5),
             json{{"opType", "Extrude"}, {"opId", "op3"}, {"stepIndex", 3},
                  {"inputs", json::array({json{{"primary", {{"bodyId", "body_op1"}, {"elementId", "body_op1"}, {"kind", "body"}}}}})},
                  {"params", {{"sketchId", "cut"}, {"distance", 1.0}, {"extrudeMode", "ThroughAll"}, {"booleanMode", "Cut"}, {"targetBodyId", "body_op1"}}}}});
        PlanRun r = run_plan(s, 1, ops, json::array({"a", "b", "c", "d"}));
        check(r.resp.result.value("stoppedReason", "") == "completed", "throughall: completed");
        accept(s, 1);
        check_near(body_volume(s, "body_op1"), 3750.0, 1.0, "corpus b: ThroughAll cut volume 3750");
    }
    {  // Two-direction: 10×10 footprint, dir1 Blind 5 + dir2 Blind 3 → 800.
        Session s; open_fresh(s);
        json ops = json::array(
            {rect_sketch("op0", "sk", 10, 10),
             json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
                  {"params", {{"sketchId", "sk"}, {"distance", 5.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"},
                              {"twoDirections", true}, {"extrudeMode2", "Blind"}, {"distance2", 3.0}}}}});
        PlanRun r = run_plan(s, 1, ops, json::array({"a", "b"}));
        accept(s, 1);
        check_near(body_volume(s, "body_op1"), 800.0, 1.0, "corpus b: two-direction volume 800");
    }
}

// --- corpus c: boolean Fuse grows / Cut shrinks + BodyId preserved ---
void test_boolean_grow_shrink_bodyid() {
    {  // Add/Fuse: 20×20×10 (4000), 4×4 pad extruded to z=14 → +64 (grow), id kept.
        Session s; open_fresh(s);
        json ops = json::array(
            {rect_sketch("op0", "base", 20, 20),
             json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
                  {"params", {{"sketchId", "base"}, {"distance", 10.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}},
             rect_sketch("op2", "pad", 4, 4),
             json{{"opType", "Extrude"}, {"opId", "op3"}, {"stepIndex", 3},
                  {"inputs", json::array({json{{"primary", {{"bodyId", "body_op1"}, {"elementId", "body_op1"}, {"kind", "body"}}}}})},
                  {"params", {{"sketchId", "pad"}, {"distance", 14.0}, {"extrudeMode", "Blind"}, {"booleanMode", "Add"}, {"targetBodyId", "body_op1"}}}}});
        PlanRun r = run_plan(s, 1, ops, json::array({"a", "b", "c", "d"}));
        accept(s, 1);
        const double v = body_volume(s, "body_op1");
        check_near(v, 4064.0, 1.0, "corpus c: Fuse grows body to 4064");
        check(s.bodies_copy().size() == 1, "corpus c: Fuse keeps a single body");
        check(s.bodies_copy().contains("body_op1"), "corpus c: Fuse preserves target BodyId");
    }
    {  // Cut: 20×20×10 (4000), 4×4 pocket extruded up 4 (overlapping) → −64 (shrink).
        Session s; open_fresh(s);
        json ops = json::array(
            {rect_sketch("op0", "base", 20, 20),
             json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
                  {"params", {{"sketchId", "base"}, {"distance", 10.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}},
             rect_sketch("op2", "pocket", 4, 4),
             json{{"opType", "Extrude"}, {"opId", "op3"}, {"stepIndex", 3},
                  {"inputs", json::array({json{{"primary", {{"bodyId", "body_op1"}, {"elementId", "body_op1"}, {"kind", "body"}}}}})},
                  {"params", {{"sketchId", "pocket"}, {"distance", 4.0}, {"extrudeMode", "Blind"}, {"booleanMode", "Cut"}, {"targetBodyId", "body_op1"}}}}});
        PlanRun r = run_plan(s, 1, ops, json::array({"a", "b", "c", "d"}));
        accept(s, 1);
        const double v = body_volume(s, "body_op1");
        check_near(v, 3936.0, 1.0, "corpus c: Cut shrinks body to 3936");
        check(s.bodies_copy().contains("body_op1"), "corpus c: Cut preserves target BodyId");
    }
}

// --- standalone Boolean Union + tool deleted + delta bodyId ---
void test_standalone_boolean_and_delta_bodyid() {
    Session s; open_fresh(s);
    json ops = json::array(
        {rect_sketch("op0", "a", 20, 20),
         json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
              {"params", {{"sketchId", "a"}, {"distance", 10.0}, {"booleanMode", "NewBody"}}}},
         rect_sketch("op2", "b", 10, 10),
         json{{"opType", "Extrude"}, {"opId", "op3"}, {"stepIndex", 3},
              {"params", {{"sketchId", "b"}, {"distance", 20.0}, {"booleanMode", "NewBody"}}}},
         json{{"opType", "Boolean"}, {"opId", "op4"}, {"stepIndex", 4},
              // A face ref on the target (mints an entry — delta.added carries bodyId).
              {"inputs", json::array(
                             {json{{"primary", {{"bodyId", "body_op1"}, {"elementId", "body_op1"}, {"kind", "body"}}}},
                              json{{"primary", {{"bodyId", "body_op3"}, {"elementId", "body_op3"}, {"kind", "body"}}}},
                              json{{"primary", {{"bodyId", "body_op1"}, {"elementId", "el_tf"}, {"kind", "face"}, {"topoKey", "f:1"}}}}})},
              {"params", {{"operation", "Union"}, {"targetBodyId", "body_op1"}, {"toolBodyId", "body_op3"}}}}});
    PlanRun r = run_plan(s, 1, ops, json::array({"a", "b", "c", "d", "e"}));
    check(r.resp.result.value("stoppedReason", "") == "completed", "standalone boolean completed");
    accept(s, 1);
    check(s.bodies_copy().contains("body_op1"), "boolean: target BodyId preserved");
    check(!s.bodies_copy().contains("body_op3"), "boolean: tool body consumed");

    // The boolean step (last planStep) delta.added must carry the minted face with
    // a REQUIRED bodyId (SCHEMA §7.2 amendment); bodyEvents include deleted tool.
    const json& last = r.steps.back();
    const json& added = last["elementMapDelta"]["added"];
    bool found_added_bodyid = false;
    for (const auto& e : added) {
        if (e.value("elementId", "") == "el_tf") {
            found_added_bodyid = e.value("bodyId", "") == "body_op1" && e.contains("topoKey") && e.contains("kind");
        }
    }
    check(found_added_bodyid, "delta.added entry carries {elementId, topoKey, kind, bodyId}");
    bool tool_deleted = false;
    for (const auto& be : last["bodyEvents"])
        if (be.value("kind", "") == "deleted" && be.value("bodyId", "") == "body_op3") tool_deleted = true;
    check(tool_deleted, "boolean step: tool bodyEvent deleted");
}

// --- cancellation before the op executes → CANCELLED, session intact ---
void test_cancellation_session_intact() {
    Session s; open_fresh(s);
    json ops = json::array(
        {rect_sketch("op0", "a", 20, 20),
         json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
              {"params", {{"sketchId", "a"}, {"distance", 10.0}, {"booleanMode", "NewBody"}}}}});
    PlanRun r = run_plan(s, 1, ops, json::array({"a", "b"}), json::object(), /*pre_cancel=*/true);
    check(r.resp.error.has_value() && r.resp.error->code == "CANCELLED", "cancel: terminal CANCELLED");
    // Session intact: no scratch stored, head unchanged (empty anchor, revision 0).
    onecad::session::WorkerHead h = s.head();
    check(!h.has_scratch, "cancel: hasScratch false (session intact)");
    check(h.document_revision == 0 && h.snapshot_id == 0, "cancel: head unchanged");
    check(h.history_prefix_hash == kEmpty, "cancel: head token unchanged");
}

// --- Standard_Failure at the Dispatcher::execute boundary -> recoverable
// OP_FAILED, not std::terminate (SCHEMA §8; OCCT throws derive from
// Standard_Transient, not std::exception, so they need their own catch). ---
void test_dispatcher_occt_exception_recoverable() {
    Dispatcher d;
    d.register_verb("__ThrowOcct", [](const Envelope&, const std::vector<std::uint8_t>&,
                                      HandlerContext&) -> Envelope {
        throw Standard_Failure("injected OCCT failure");
    });
    Envelope resp = d.dispatch_once(Envelope::request(1, "__ThrowOcct"));
    check(resp.error.has_value() && resp.error->code == "OP_FAILED",
          "OCCT Standard_Failure at dispatcher boundary yields OP_FAILED");

    // Dispatcher (and by extension the session) stays usable afterwards.
    d.register_verb("__Noop", [](const Envelope& req, const std::vector<std::uint8_t>&,
                                 HandlerContext&) { return Envelope::ok_response(req.id); });
    Envelope ok = d.dispatch_once(Envelope::request(2, "__Noop"));
    check(ok.ok.has_value() && *ok.ok, "dispatcher stays usable after an OCCT throw");
}

}  // namespace

int main() {
    test_blind_volume();
    test_throughall_and_twodir();
    test_boolean_grow_shrink_bodyid();
    test_standalone_boolean_and_delta_bodyid();
    test_cancellation_session_intact();
    test_dispatcher_occt_exception_recoverable();
    if (g_failures == 0) std::fprintf(stderr, "wp5_plan: OK\n");
    return g_failures;
}
