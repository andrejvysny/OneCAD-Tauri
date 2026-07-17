// test_concurrent_lanes.cpp — the two-lane liveness proof (W-WP4 task 5).
//
// The kernel lane runs a ~500 ms "__slow" ExecutePlan; concurrently the solver
// lane must stay responsive. A reader thread timestamps every response: all the
// solver-lane SketchUpsert responses MUST arrive BEFORE the plan's PlanPrepared
// (the single-writer OCCT lane never blocks PlaneGCS — migration plan "solver lane
// in V1"; the kernel executes plans lock-free so frame stamping never contends).
//
// No test framework: exit code == failure count. Usage: <worker-path>.
#include <sys/wait.h>
#include <unistd.h>

#include <atomic>
#include <chrono>
#include <cstdio>
#include <string>
#include <thread>

#include "nlohmann/json.hpp"
#include "protocol/Envelope.h"
#include "protocol/Frame.h"

using nlohmann::json;
using onecad::protocol::Envelope;
using onecad::protocol::Frame;
using onecad::protocol::ReadStatus;
using Clock = std::chrono::steady_clock;

namespace {
int g_failures = 0;
#define CHECK(cond)                                                              \
    do {                                                                         \
        if (!(cond)) {                                                           \
            std::fprintf(stderr, "FAIL %s:%d: %s\n", __FILE__, __LINE__, #cond); \
            ++g_failures;                                                        \
        }                                                                        \
    } while (0)

struct Worker { pid_t pid = -1; int to = -1, from = -1; };

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

json triangle(const std::string& sketch_id) {
    return json{{"sketchId", sketch_id},
                {"plane", {{"kind", "XY"}}},
                {"entities", json::array({json{{"id", "l1"}, {"type", "Line"}, {"p0", {0, 0}}, {"p1", {10, 0}}},
                                          json{{"id", "l2"}, {"type", "Line"}, {"p0", {10, 0}}, {"p1", {5, 8}}},
                                          json{{"id", "l3"}, {"type", "Line"}, {"p0", {5, 8}}, {"p1", {0, 0}}}})},
                {"constraints", json::array()}};
}

json slow_plan() {
    return json{{"jobId", 88}, {"documentRevision", 0}, {"workerEpoch", 3},
                {"expectedBaseHash",
                 "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},
                {"targetStep", 0},
                {"ops", json::array({json{{"opType", "Extrude"},
                                          {"opId", "op0__slow"},
                                          {"stepIndex", 0},
                                          {"params", {{"distance", 10.0}, {"booleanMode", "NewBody"}}}}})}};
}

constexpr std::uint64_t kPlanId = 100;
constexpr std::uint64_t kSolverIdBase = 200;
constexpr int kSolverReqs = 10;
}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <worker-path>\n", argv[0]);
        return 2;
    }
    Worker w;
    if (!spawn(argv[1], w)) { std::fprintf(stderr, "spawn failed\n"); return 2; }

    json resp;
    CHECK(recv(w, resp) && resp.value("t", std::string{}) == "hello");
    send(w, Envelope::request(1, "OpenSession",
                              json{{"documentId", "doc_1"}, {"documentRevision", 0}, {"workerEpoch", 3}}));
    CHECK(recv(w, resp) && resp.value("ok", false));

    std::atomic<int> solver_count{0};
    std::atomic<bool> plan_ok{false};
    double last_solver_ms = -1.0;
    double plan_ms = -1.0;
    const auto t0 = Clock::now();
    auto ms_since = [&] {
        return std::chrono::duration_cast<std::chrono::microseconds>(Clock::now() - t0).count() /
               1000.0;
    };

    // Reader thread: timestamp responses until the plan's PlanPrepared arrives.
    std::thread reader([&] {
        json f;
        while (recv(w, f)) {
            const std::string t = f.value("t", std::string{});
            const std::uint64_t id = f.value("id", std::uint64_t{0});
            if (t == "resp" && id == kPlanId) {
                plan_ms = ms_since();
                plan_ok.store(f.value("ok", false) &&
                              f["result"].value("planPrepared", false));
                break;
            }
            if (t == "resp" && id >= kSolverIdBase && id < kSolverIdBase + 100) {
                last_solver_ms = ms_since();
                solver_count.fetch_add(1);
            }
        }
    });

    // Kick the slow plan, then hammer the solver lane.
    send(w, Envelope::request(kPlanId, "ExecutePlan", slow_plan()));
    for (int i = 0; i < kSolverReqs; ++i) {
        send(w, Envelope::request(kSolverIdBase + static_cast<std::uint64_t>(i), "SketchUpsert",
                                  triangle("sk" + std::to_string(i))));
        std::this_thread::sleep_for(std::chrono::milliseconds(5));
    }

    reader.join();

    std::fprintf(stderr,
                 "concurrent-lanes: solver_count=%d last_solver=%.1fms plan_prepared=%.1fms plan_ok=%d\n",
                 solver_count.load(), last_solver_ms, plan_ms, plan_ok.load() ? 1 : 0);

    CHECK(plan_ok.load());                       // the slow plan still prepared
    CHECK(solver_count.load() == kSolverReqs);   // every solver request answered
    CHECK(last_solver_ms >= 0.0 && plan_ms >= 0.0);
    CHECK(last_solver_ms < plan_ms);             // solver drained BEFORE the plan finished

    send(w, Envelope::request(9, "Shutdown", json::object()));
    // The plan resp was already consumed by the reader; drain the shutdown resp.
    if (recv(w, resp)) CHECK(resp.value("ok", false));

    close(w.to);
    int status = 0;
    waitpid(w.pid, &status, 0);
    close(w.from);

    if (g_failures == 0) std::fprintf(stderr, "concurrent lanes: OK\n");
    return g_failures;
}
