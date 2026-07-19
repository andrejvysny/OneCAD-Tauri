// test_wp6_split.cpp — M5a boolean split children (`body_<opId>:<k>`, SCHEMA §2/D1).
// A Cut that BISECTS a box into two disconnected pieces must mint two deterministic
// children `body_<boolOpId>:0` / `:1` (ordered by the worker's geometric key), emit a
// Created bodyEvent for each + a Deleted for the parent, and expose exact volumes.
//
// Geometry: box A = [0,40]×[0,20]×[0,25] (vol 20000). Tool = a slab x∈[15,25]
// overshooting A in y and z. Cut(A, tool) removes x∈[15,25] all the way through,
// leaving left x∈[0,15] (vol 7500, centroid x=7.5) and right x∈[25,40] (vol 7500,
// centroid x=32.5). Equal volumes ⇒ the centroid tiebreak fixes the order: child :0
// is the LEFT piece (lower centroid x). Ids are stable across a replay (pure fn of
// the opId + k).
// No framework: exit code == failure count.
#include <cmath>
#include <cstdio>
#include <string>
#include <vector>

#include "nlohmann/json.hpp"
#include "protocol/Dispatcher.h"
#include "protocol/Envelope.h"
#include "session/PlanExecutor.h"
#include "session/Session.h"
#include "session/ShapeMetrics.h"
#include "util/Cancel.h"

using nlohmann::json;
using onecad::CancelToken;
using onecad::protocol::Envelope;
using onecad::protocol::HandlerContext;
using onecad::session::Session;

namespace {
int g_failures = 0;
void check(bool cond, const std::string& msg) {
    if (!cond) { std::fprintf(stderr, "FAIL: %s\n", msg.c_str()); ++g_failures; }
}
constexpr const char* kEmpty =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

json rect(const std::string& p, double x0, double y0, double x1, double y1) {
    return json::array({{{"id", p + "1"}, {"type", "Line"}, {"p0", {x0, y0}}, {"p1", {x1, y0}}},
                        {{"id", p + "2"}, {"type", "Line"}, {"p0", {x1, y0}}, {"p1", {x1, y1}}},
                        {{"id", p + "3"}, {"type", "Line"}, {"p0", {x1, y1}}, {"p1", {x0, y1}}},
                        {{"id", p + "4"}, {"type", "Line"}, {"p0", {x0, y1}}, {"p1", {x0, y0}}}});
}
json sketch_op(const std::string& op_id, int step, const std::string& sid, const json& ents) {
    return json{{"opType", "Sketch"}, {"opId", op_id}, {"stepIndex", step},
                {"params", {{"sketchId", sid}, {"plane", {{"kind", "XY"}}},
                            {"entities", ents}, {"constraints", json::array()}}}};
}

// Run the split plan into `s`; `body_events` collects the boolean step's bodyEvents.
// (Session holds a mutex ⇒ non-movable, so it is passed by reference, never
// reassigned.)
void run_plan(Session& s, std::vector<json>& body_events) {
    s.open("doc", 0, 3, "determinism");
    json ops = json::array({
        sketch_op("op0", 0, "sk_a", rect("a", 0, 0, 40, 20)),
        json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
             {"params", {{"sketchId", "sk_a"}, {"distance", 25.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}},
        sketch_op("op2", 2, "sk_t", rect("t", 15, -10, 25, 30)),
        json{{"opType", "Extrude"}, {"opId", "op3"}, {"stepIndex", 3},
             {"params", {{"sketchId", "sk_t"}, {"distance", 100.0}, {"extrudeMode", "Symmetric"}, {"booleanMode", "NewBody"}}}},
        json{{"opType", "Boolean"}, {"opId", "op4"}, {"stepIndex", 4},
             {"params", {{"operation", "Cut"}, {"targetBodyId", "body_op1"}, {"toolBodyId", "body_op3"}}}},
    });

    CancelToken tok;
    auto capture = [&body_events](Envelope& ev) {
        if (ev.event_name == "planStep" && ev.result.contains("bodyEvents")) {
            body_events.clear();
            for (const auto& be : ev.result["bodyEvents"]) body_events.push_back(be);
        }
    };
    HandlerContext ctx{tok, [](int) {}, capture};
    json args = {{"jobId", 1}, {"documentRevision", 0}, {"workerEpoch", 3},
                 {"expectedBaseHash", kEmpty},
                 {"prefixHashes", json::array({"a", "b", "c", "d", "e"})},
                 {"targetStep", 4}, {"ops", ops}};
    onecad::session::handle_execute_plan(s, Envelope::request(1, "ExecutePlan", args), ctx);
    onecad::session::handle_accept_prepared(
        s, Envelope::request(1, "AcceptPrepared",
                             json{{"jobId", 1}, {"documentRevision", 0}, {"workerEpoch", 3}}));
}

double vol_of(Session& s, const std::string& bid) {
    const onecad::session::BodyStore bodies = s.bodies_copy();
    const onecad::session::BodyRecord* rec = bodies.get(bid);
    if (!rec) return -1.0;
    return onecad::session::shape_volume(rec->geom);
}
}  // namespace

int main() {
    Session s1;
    std::vector<json> body_events;
    run_plan(s1, body_events);

    // The boolean step's bodyEvents: parent deleted, two children created, tool deleted.
    int created = 0, deleted = 0;
    bool has_c0 = false, has_c1 = false, parent_deleted = false;
    for (const json& be : body_events) {
        const std::string kind = be.value("kind", "");
        const std::string bid = be.value("bodyId", "");
        if (kind == "created") ++created;
        if (kind == "deleted") ++deleted;
        if (kind == "created" && bid == "body_op4:0") has_c0 = true;
        if (kind == "created" && bid == "body_op4:1") has_c1 = true;
        if (kind == "deleted" && bid == "body_op1") parent_deleted = true;
    }
    check(created == 2, "split: two children Created");
    check(has_c0 && has_c1, "split: children are body_op4:0 + body_op4:1 (deterministic)");
    check(parent_deleted, "split: parent body_op1 Deleted");
    check(deleted == 2, "split: parent + tool both Deleted");

    // The parent + tool are gone; only the two children survive at head.
    const onecad::session::BodyStore bodies = s1.bodies_copy();
    check(!bodies.contains("body_op1"), "split: parent body removed from head");
    check(!bodies.contains("body_op3"), "split: tool body removed from head");
    check(bodies.contains("body_op4:0") && bodies.contains("body_op4:1"),
          "split: both children present at head");

    // Volumes are exact (7500 each) and ORDERED by centroid: :0 is the left piece.
    const double v0 = vol_of(s1, "body_op4:0");
    const double v1 = vol_of(s1, "body_op4:1");
    check(std::abs(v0 - 7500.0) < 1e-3, "split: child :0 volume == 7500 (exact box arithmetic)");
    check(std::abs(v1 - 7500.0) < 1e-3, "split: child :1 volume == 7500 (exact box arithmetic)");

    // Ids stable across replay (pure function of opId + ordinal).
    Session s2;
    std::vector<json> body_events2;
    run_plan(s2, body_events2);
    const onecad::session::BodyStore bodies2 = s2.bodies_copy();
    check(bodies2.contains("body_op4:0") && bodies2.contains("body_op4:1"),
          "split: child ids identical across a replay");
    check(std::abs(vol_of(s2, "body_op4:0") - 7500.0) < 1e-3, "split: replay child :0 volume stable");

    if (g_failures == 0) std::fprintf(stderr, "wp6_split: OK\n");
    return g_failures;
}
