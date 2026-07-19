// test_wp6_checkpoint.cpp — M5a checkpoints (SaveCheckpoint / RestoreCheckpoint,
// SCHEMA §7.7). Build + publish a box at step 1, SaveCheckpoint(1) → artifacts +
// signatures + BinTools blobs in the resp tail. Then MUTATE the head (a second regen
// producing a different body), RestoreCheckpoint(1) → the head rolls back to the box,
// and its geometry signature is IDENTICAL to the checkpoint's (BinTools round-trips
// exactly; determinism). An absent step ⇒ restored:false (Rust would replay from 0).
// No framework: exit code == failure count.
#include <cstdio>
#include <string>

#include "io/Checkpoint.h"
#include "nlohmann/json.hpp"
#include "protocol/Dispatcher.h"
#include "protocol/Envelope.h"
#include "session/PlanExecutor.h"
#include "session/Session.h"
#include "session/Signatures.h"
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

json rect(double x1, double y1) {
    return json::array({{{"id", "e1"}, {"type", "Line"}, {"p0", {0, 0}}, {"p1", {x1, 0}}},
                        {{"id", "e2"}, {"type", "Line"}, {"p0", {x1, 0}}, {"p1", {x1, y1}}},
                        {{"id", "e3"}, {"type", "Line"}, {"p0", {x1, y1}}, {"p1", {0, y1}}},
                        {{"id", "e4"}, {"type", "Line"}, {"p0", {0, y1}}, {"p1", {0, 0}}}});
}

// Extrude a box of the given size into a fresh from-0 head. Returns the head's
// geometry signature.
std::string build_box(Session& s, double w, double h, double dist) {
    json ops = json::array(
        {json{{"opType", "Sketch"}, {"opId", "op0"}, {"stepIndex", 0},
              {"params", {{"sketchId", "sk"}, {"plane", {{"kind", "XY"}}}, {"entities", rect(w, h)},
                          {"constraints", json::array()}}}},
         json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
              {"params", {{"sketchId", "sk"}, {"distance", dist}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}}});
    CancelToken tok;
    HandlerContext ctx{tok, [](int) {}, [](Envelope&) {}};
    json args = {{"jobId", 1}, {"documentRevision", 0}, {"workerEpoch", 3},
                 {"expectedBaseHash", kEmpty}, {"prefixHashes", json::array({"a", "b"})},
                 {"targetStep", 1}, {"ops", ops}};
    onecad::session::handle_execute_plan(s, Envelope::request(1, "ExecutePlan", args), ctx);
    onecad::session::handle_accept_prepared(
        s, Envelope::request(1, "AcceptPrepared",
                             json{{"jobId", 1}, {"documentRevision", 0}, {"workerEpoch", 3}}));
    return onecad::session::geometry_signature(s.bodies_copy());
}
}  // namespace

int main() {
    Session s;
    s.open("doc", 0, 3, "determinism");

    // Publish a 10x10x10 box, then checkpoint it at step 1.
    const std::string box_sig = build_box(s, 10, 10, 10);
    Envelope save = onecad::io::handle_save_checkpoint(
        s, Envelope::request(2, "SaveCheckpoint", json{{"stepIndex", 1}}));
    check(save.ok.value_or(false), "checkpoint: SaveCheckpoint ok");
    check(save.result.contains("artifacts") && save.result["artifacts"].is_array() &&
              save.result["artifacts"].size() == 1,
          "checkpoint: one per-body artifact");
    check(save.result["artifacts"][0].value("codec", "") == "brep-bintools", "checkpoint: brep-bintools codec");
    check(save.result["artifacts"][0].value("size", std::uint64_t{0}) > 0, "checkpoint: artifact blob non-empty");
    check(!save.out_bin.empty(), "checkpoint: BinTools bytes ride in the resp tail");
    const std::string ckpt_sig = save.result["signatures"].value("geometry", "");
    check(ckpt_sig == box_sig, "checkpoint: saved geometry signature == the box's");

    // Mutate the head: a DIFFERENT box (bigger), replacing the head via a from-0 plan.
    Session s2;  // fresh session to compute the "different" box sig for comparison
    s2.open("doc", 0, 3, "determinism");
    const std::string big_sig = build_box(s2, 20, 20, 20);
    check(big_sig != box_sig, "checkpoint: the mutated box has a different signature");
    // Now mutate s's head to the big box (advance the epoch-consistent head).
    build_box(s, 20, 20, 20);  // s head is now the big box (from-0 replace)
    check(onecad::session::geometry_signature(s.bodies_copy()) == big_sig, "checkpoint: head mutated");

    // Restore the step-1 checkpoint: the head rolls back to the ORIGINAL box.
    const std::string ckpt_hash = save.result.value("historyPrefixHash", std::string(""));
    Envelope restore = onecad::io::handle_restore_checkpoint(
        s, Envelope::request(3, "RestoreCheckpoint",
                             json{{"stepIndex", 1}, {"expectedHistoryPrefixHash", ckpt_hash},
                                  {"workerEpoch", 3}}));
    check(restore.ok.value_or(false), "checkpoint: RestoreCheckpoint ok");
    check(restore.result.value("restored", false), "checkpoint: restored true");
    check(!restore.result.value("driftDetected", true), "checkpoint: no drift");
    check(onecad::session::geometry_signature(s.bodies_copy()) == box_sig,
          "checkpoint: restored head signature IDENTICAL to the checkpoint (BinTools round-trip)");

    // Absent step ⇒ restored:false (Rust replays from 0).
    Envelope absent = onecad::io::handle_restore_checkpoint(
        s, Envelope::request(4, "RestoreCheckpoint",
                             json{{"stepIndex", 99}, {"workerEpoch", 3}}));
    check(absent.ok.value_or(false), "checkpoint: absent-step RestoreCheckpoint is ok (not an error)");
    check(!absent.result.value("restored", true), "checkpoint: absent step ⇒ restored:false");

    if (g_failures == 0) std::fprintf(stderr, "wp6_checkpoint: OK\n");
    return g_failures;
}
