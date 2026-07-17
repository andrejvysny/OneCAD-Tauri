// test_solver_latest_wins.cpp — proves the solver-lane LATEST-WINS mailbox
// (SCHEMA §7.4 SolveDrag) end-to-end against the real worker binary.
//
// Sends many SolveDrag frames back-to-back WITHOUT reading between them, so they
// pile up in the solver mailbox and are coalesced. Then drains every response
// and asserts the observable contract:
//   * EXACTLY one terminal resp per request id (N sent => N received).
//   * every non-success terminal is ok:false code "CANCELLED" msg "superseded".
//   * coalescing actually happened (>=1 superseded) — the drags are fired at a
//     sizeable sketch so each solve out-runs the send loop.
//   * the highest-seq drag ALWAYS resolves (nothing newer can supersede it).
//
// No test framework: exit code == failure count. Usage: test_solver_latest_wins
// <worker-path>.
#include <sys/wait.h>
#include <unistd.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

#include "nlohmann/json.hpp"
#include "protocol/Envelope.h"
#include "protocol/Frame.h"

using nlohmann::json;
using onecad::protocol::Envelope;
using onecad::protocol::Frame;
using onecad::protocol::ReadStatus;

namespace {
int g_failures = 0;
#define CHECK(cond)                                                              \
    do {                                                                         \
        if (!(cond)) {                                                           \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            ++g_failures;                                                        \
        }                                                                        \
    } while (0)

struct Worker {
    pid_t pid = -1;
    int to = -1;
    int from = -1;
};

bool spawn(const std::string& path, Worker& w) {
    int p2c[2], c2p[2];
    if (pipe(p2c) != 0 || pipe(c2p) != 0) return false;
    const pid_t pid = fork();
    if (pid < 0) return false;
    if (pid == 0) {
        dup2(p2c[0], STDIN_FILENO);
        dup2(c2p[1], STDOUT_FILENO);
        close(p2c[0]); close(p2c[1]); close(c2p[0]); close(c2p[1]);
        char* const argv[] = {const_cast<char*>(path.c_str()), nullptr};
        execv(path.c_str(), argv);
        _exit(127);
    }
    close(p2c[0]); close(c2p[1]);
    w.pid = pid; w.to = p2c[1]; w.from = c2p[0];
    return true;
}

void send(const Worker& w, const Envelope& env) {
    Frame f;
    f.json = onecad::protocol::serialize(env);
    onecad::protocol::write_frame(w.to, f);
}

bool recv(const Worker& w, json& out) {
    auto rr = onecad::protocol::read_frame(w.from);
    if (rr.status != ReadStatus::Ok) return false;
    out = json::parse(rr.frame.json);
    return true;
}

// Consistent H/V staircase chain: seg i horizontal (even) / vertical (odd),
// each length 10, joined by Coincident. Distinct endpoints so Coincident is
// exercised. Returns the SketchUpsert args.
json make_chain(int nseg) {
    json ents = json::array();
    json cons = json::array();
    double x = 0, y = 0;
    std::string prev_end;
    for (int i = 0; i < nseg; ++i) {
        const bool horiz = (i % 2 == 0);
        const double ex = horiz ? x + 10 : x;
        const double ey = horiz ? y : y + 10;
        const std::string s = "s" + std::to_string(i);
        const std::string e = "e" + std::to_string(i);
        const std::string l = "L" + std::to_string(i);
        ents.push_back({{"id", s}, {"type", "Point"}, {"at", {x, y}}});
        ents.push_back({{"id", e}, {"type", "Point"}, {"at", {ex, ey}}});
        ents.push_back({{"id", l}, {"type", "Line"}, {"p0Ref", s}, {"p1Ref", e}});
        cons.push_back({{"id", "d" + std::to_string(i)}, {"type", "Distance"},
                        {"entities", {l}}, {"value", 10.0}});
        cons.push_back({{"id", "hv" + std::to_string(i)},
                        {"type", horiz ? "Horizontal" : "Vertical"}, {"entities", {l}}});
        if (!prev_end.empty()) {
            cons.push_back({{"id", "c" + std::to_string(i)}, {"type", "Coincident"},
                            {"entities", {prev_end, s}}, {"positions", {"", ""}}});
        }
        prev_end = e;
        x = ex; y = ey;
    }
    return {{"sketchId", "chain"}, {"plane", {{"kind", "XY"}}},
            {"entities", ents}, {"constraints", cons}};
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <worker-path>\n", argv[0]);
        return 2;
    }
    Worker w;
    if (!spawn(argv[1], w)) {
        std::fprintf(stderr, "spawn failed\n");
        return 2;
    }

    json resp;
    // The worker emits an unsolicited hello (SCHEMA §6) as its first frame.
    CHECK(recv(w, resp) && resp.value("t", std::string{}) == "hello");

    send(w, Envelope::request(1, "SketchUpsert", make_chain(40)));
    CHECK(recv(w, resp) && resp.value("ok", false));

    json bargs = {{"sketchId", "chain"}, {"sketchRevision", 1}, {"gestureId", 1},
                  {"drag", {{"pointId", "s0"}}}};
    send(w, Envelope::request(2, "BeginGesture", bargs));
    CHECK(recv(w, resp) && resp.value("ok", false));

    // Fire N drags back-to-back WITHOUT reading. Drag ids are 100+k (distinct
    // from the upsert/begin ids above, whose responses were already drained).
    const int N = 150;
    for (int k = 1; k <= N; ++k) {
        json dargs = {{"gestureId", 1}, {"seq", k}, {"pointId", "s0"},
                      {"target", {0.1 * k, 0.05 * k}}};
        send(w, Envelope::request(static_cast<std::uint64_t>(100 + k), "SolveDrag", dargs));
    }

    int success = 0, superseded = 0, other = 0;
    bool highest_resolved = false;
    for (int i = 0; i < N; ++i) {
        if (!recv(w, resp)) { CHECK(false); break; }
        if (resp.value("ok", false)) {
            ++success;
            if (resp.contains("result") && resp["result"].value("seq", 0) == N) {
                highest_resolved = true;
            }
            CHECK(resp["result"].value("gestureId", 0) == 1);
        } else if (resp.contains("error") && resp["error"].value("code", "") == "CANCELLED" &&
                   resp["error"].value("message", "") == "superseded") {
            ++superseded;
        } else {
            ++other;
        }
    }
    std::fprintf(stderr, "latest-wins: N=%d success=%d superseded=%d other=%d highest_resolved=%d\n",
                 N, success, superseded, other, highest_resolved ? 1 : 0);

    CHECK(success + superseded + other == N);  // one terminal resp per id
    CHECK(other == 0);                          // every drop is CANCELLED/superseded
    CHECK(superseded > 0);                      // coalescing actually happened
    CHECK(highest_resolved);                    // newest drag never superseded

    send(w, Envelope::request(3, "EndGesture", json{{"gestureId", 1}}));
    CHECK(recv(w, resp) && resp.value("ok", false));

    close(w.to);
    int status = 0;
    waitpid(w.pid, &status, 0);
    close(w.from);

    if (g_failures == 0) std::fprintf(stderr, "solver latest-wins: OK\n");
    return g_failures;
}
