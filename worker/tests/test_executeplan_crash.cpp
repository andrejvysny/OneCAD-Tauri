// test_executeplan_crash.cpp — the chaos drill (SCHEMA §8 worker crash).
//
// An ExecutePlan whose op carries the "__crash" hook makes the worker std::abort()
// mid-plan — NO terminal frame arrives (a crash, distinct from a PROTOCOL_ERROR
// resp). The parent observes the frame stream close and the worker exit
// abnormally (killed by a signal / nonzero code). Rust's recovery is restart +
// replay with a circuit breaker; here we only assert the abnormal exit.
//
// No test framework: exit code == failure count. Usage: <worker-path>.
#include <sys/wait.h>
#include <unistd.h>

#include <cstdio>
#include <string>

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
}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <worker-path>\n", argv[0]);
        return 2;
    }
    // Ignore SIGPIPE — writing to the crashed worker's stdin must not kill us.
    signal(SIGPIPE, SIG_IGN);

    Worker w;
    if (!spawn(argv[1], w)) { std::fprintf(stderr, "spawn failed\n"); return 2; }

    json resp;
    CHECK(recv(w, resp) && resp.value("t", std::string{}) == "hello");
    send(w, Envelope::request(1, "OpenSession",
                              json{{"documentId", "doc_1"}, {"documentRevision", 0}, {"workerEpoch", 3}}));
    CHECK(recv(w, resp) && resp.value("ok", false));

    json plan = {{"jobId", 88}, {"documentRevision", 0}, {"workerEpoch", 3},
                 {"expectedBaseHash",
                  "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"},
                 {"targetStep", 0},
                 {"ops", json::array({json{{"opType", "Extrude"},
                                           {"opId", "op0__crash"},
                                           {"stepIndex", 0},
                                           {"params", {{"distance", 10.0}, {"booleanMode", "NewBody"}}}}})}};
    send(w, Envelope::request(2, "ExecutePlan", plan));

    // No terminal frame ever arrives — the stream closes (crash).
    CHECK(!recv(w, resp));

    close(w.to);
    int status = 0;
    waitpid(w.pid, &status, 0);
    close(w.from);

    const bool abnormal =
        (WIFSIGNALED(status)) || (WIFEXITED(status) && WEXITSTATUS(status) != 0);
    if (WIFSIGNALED(status)) {
        std::fprintf(stderr, "worker crashed by signal %d (expected)\n", WTERMSIG(status));
    } else if (WIFEXITED(status)) {
        std::fprintf(stderr, "worker exited code %d\n", WEXITSTATUS(status));
    }
    CHECK(abnormal);  // __crash must NOT be a clean exit

    if (g_failures == 0) std::fprintf(stderr, "executeplan crash drill: OK\n");
    return g_failures;
}
