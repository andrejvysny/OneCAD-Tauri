// test_executeplan_cancel.cpp — cooperative cancellation of a running plan.
//
// Drives the real worker: OpenSession, then an ExecutePlan whose op carries the
// "__slow" hook (~500 ms). Mid-sleep a `cancel` frame is sent for the plan's id.
// The worker MUST emit a terminal `resp` with error.code "CANCELLED" (the
// terminal is never dropped, SCHEMA §3.5/§5.4) and drop the scratch — GetWorkerHead
// then shows hasScratch=false with the head unchanged (session intact, §8).
//
// No test framework: exit code == failure count. Usage: <worker-path>.
#include <sys/wait.h>
#include <unistd.h>

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
using onecad::protocol::MsgType;
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
    int to = -1, from = -1;
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

void send_cancel(const Worker& w, std::uint64_t id) {
    Envelope c;
    c.type = MsgType::Cancel;
    c.id = id;
    send(w, c);
}

bool recv(const Worker& w, json& out) {
    auto rr = onecad::protocol::read_frame(w.from);
    if (rr.status != ReadStatus::Ok) return false;
    out = json::parse(rr.frame.json);
    return true;
}

json slow_plan(std::uint64_t job_id) {
    return json{{"jobId", job_id},
                {"documentRevision", 0},
                {"workerEpoch", 3},
                {"expectedBaseHash",
                 "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},
                {"targetStep", 0},
                {"ops", json::array({json{{"opType", "Extrude"},
                                          {"opId", "op0__slow"},
                                          {"stepIndex", 0},
                                          {"params", {{"distance", 10.0}, {"booleanMode", "NewBody"}}}}})}};
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
    CHECK(recv(w, resp) && resp.value("t", std::string{}) == "hello");
    send(w, Envelope::request(1, "OpenSession",
                              json{{"documentId", "doc_1"}, {"documentRevision", 0}, {"workerEpoch", 3}}));
    CHECK(recv(w, resp) && resp.value("ok", false));

    // Start the slow plan, then cancel it mid-sleep.
    const std::uint64_t plan_id = 2;
    send(w, Envelope::request(plan_id, "ExecutePlan", slow_plan(88)));
    std::this_thread::sleep_for(std::chrono::milliseconds(100));
    send_cancel(w, plan_id);

    // The next frame(s) for plan_id: a terminal CANCELLED resp (any stray planStep
    // that raced the cancel is tolerated).
    bool got_cancelled = false;
    for (int i = 0; i < 5 && !got_cancelled; ++i) {
        if (!recv(w, resp)) { CHECK(false); break; }
        if (resp.value("t", std::string{}) == "resp" && resp.value("id", 0) == plan_id) {
            CHECK(!resp.value("ok", true));
            CHECK(resp.contains("error") && resp["error"].value("code", "") == "CANCELLED");
            got_cancelled = true;
        }
    }
    CHECK(got_cancelled);

    // Session intact: no scratch, head unchanged.
    send(w, Envelope::request(3, "GetWorkerHead", json::object()));
    CHECK(recv(w, resp) && resp.value("ok", false));
    CHECK(resp["result"].value("hasScratch", true) == false);
    CHECK(resp["result"].value("documentRevision", 999) == 0);

    send(w, Envelope::request(9, "Shutdown", json::object()));
    CHECK(recv(w, resp) && resp.value("ok", false));

    close(w.to);
    int status = 0;
    waitpid(w.pid, &status, 0);
    close(w.from);

    if (g_failures == 0) std::fprintf(stderr, "executeplan cancel: OK\n");
    return g_failures;
}
