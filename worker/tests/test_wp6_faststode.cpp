// test_wp6_faststode.cpp — W-WP6 fast-mode ordering verification (scope E; closes
// review finding 3). Invariant 5: meshing/execution parallelism must NEVER change
// IDs or mappings. This runs the SAME plan twice — once with determinism.parallel
// = false (determinism mode) and once with true (fast mode, parallel BOPs) — and
// DIFFS the complete per-step payloads: the elementMapDelta TopoKey→ElementId
// tables (added/relabeled) AND the three §12 signatures. If parallel BOPs perturbed
// TopExp::MapShapes ordinals the payloads would diverge and this test FAILS.
// No framework: exit code == failure count.
#include <cstdio>
#include <string>
#include <vector>

#include "nlohmann/json.hpp"
#include "protocol/Dispatcher.h"
#include "protocol/Envelope.h"
#include "session/PlanExecutor.h"
#include "session/Session.h"
#include "util/Cancel.h"

using nlohmann::json;
using onecad::CancelToken;
using onecad::protocol::Envelope;
using onecad::protocol::HandlerContext;
using onecad::session::Session;

namespace {
int g_failures = 0;
constexpr const char* kEmpty =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

json line_ent(const std::string& id, double x0, double y0, double x1, double y1) {
    return json{{"id", id}, {"type", "Line"}, {"p0", {x0, y0}}, {"p1", {x1, y1}}};
}
json rect(const std::string& op, const std::string& sid, double w, double h) {
    json p;
    p["sketchId"] = sid;
    p["plane"] = json{{"kind", "XY"}};
    p["entities"] = json::array({line_ent("e1", 0, 0, w, 0), line_ent("e2", w, 0, w, h),
                                 line_ent("e3", w, h, 0, h), line_ent("e4", 0, h, 0, 0)});
    p["constraints"] = json::array();
    return json{{"opType", "Sketch"}, {"opId", op}, {"stepIndex", 0}, {"params", p}};
}

// A boolean-Cut plan that rebinds a tracked (surviving) side face via OCCT history.
// `parallel` is stamped into each op's determinism (drives SetRunParallel).
json cut_plan(bool parallel) {
    json det = {{"parallel", parallel}};
    return json::array(
        {rect("op0", "base", 20, 20),
         json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1}, {"determinism", det},
              {"params", {{"sketchId", "base"}, {"distance", 10.0}, {"booleanMode", "NewBody"}}}},
         rect("op2", "pocket", 4, 4),
         json{{"opType", "Extrude"}, {"opId", "op3"}, {"stepIndex", 3}, {"determinism", det},
              // Track the x=-20 side face (survives the corner cut) via anchor.
              {"inputs", json::array({json{{"primary", {{"bodyId", "body_op1"}, {"elementId", "el_side"}, {"kind", "face"}}},
                                           {"anchor", {{"worldPoint", {-20.0, 10.0, 5.0}}}}}})},
              {"params", {{"sketchId", "pocket"}, {"distance", 4.0}, {"extrudeMode", "Blind"},
                          {"booleanMode", "Cut"}, {"targetBodyId", "body_op1"}}}}});
}

// Run the plan in-process, return the concatenated per-step payloads (elementMapDelta
// TopoKey tables + signatures) — the exact bytes Invariant 5 must hold constant.
std::string run(bool parallel) {
    Session s;
    s.open("doc", 0, 3, "determinism");
    CancelToken tok;
    std::string fp;
    HandlerContext ctx{tok, [](int) {}, [&](Envelope& e) {
                           fp += "S" + std::to_string(e.result.value("stepIndex", 0)) + ":";
                           fp += e.result.value("elementMapDelta", json::object()).dump();
                           fp += "|SIG:" + e.result.value("signatures", json::object()).dump() + "\n";
                       }};
    json ops = cut_plan(parallel);
    json args = {{"jobId", 1}, {"documentRevision", 0}, {"workerEpoch", 3},
                 {"expectedBaseHash", kEmpty}, {"prefixHashes", json::array({"a", "b", "c", "d"})},
                 {"targetStep", 3}, {"ops", ops}};
    onecad::session::handle_execute_plan(s, Envelope::request(1, "ExecutePlan", args), ctx);
    return fp;
}
}  // namespace

int main() {
    const std::string determinism = run(/*parallel=*/false);
    const std::string fast = run(/*parallel=*/true);

    if (determinism.empty()) {
        std::fprintf(stderr, "FAIL: determinism run produced no steps\n");
        ++g_failures;
    }
    // Sanity: the fingerprint must contain relabeled TopoKeys (proves the tracked
    // face actually rebound via history — otherwise the diff would be vacuous).
    if (determinism.find("relabeled") == std::string::npos) {
        std::fprintf(stderr, "FAIL: no elementMapDelta captured\n");
        ++g_failures;
    }
    if (determinism != fast) {
        std::fprintf(stderr,
                     "FAIL: fast mode perturbed IDs/mappings (Invariant 5 VIOLATED)\n"
                     "  determinism:\n%s\n  fast:\n%s\n",
                     determinism.c_str(), fast.c_str());
        ++g_failures;
    }
    if (g_failures == 0)
        std::fprintf(stderr,
                     "wp6_faststode: OK — parallel BOP preserves TopoKey→ElementId tables + "
                     "signatures (Invariant 5)\n");
    return g_failures;
}
