// test_executeplan_determinism.cpp — Invariant 5 (same plan+base ⇒ identical
// lifecycle/mappings/quantized signatures) over REAL OCCT geometry (W-WP5). Runs
// the SAME 3-op plan (sketch → extrude NewBody → extrude Add with a tracked face
// ref) against TWO fresh worker processes and asserts byte-identical:
//   * per-step signatures (geometry / bodyLifecycle / referencedBinding),
//   * per-step elementMapDelta (added/removed/relabeled — the TopoKey tables),
//   * the prepared historyPrefixHash (the opaque echoed token),
//   * the inline MESH1 artifact bytes (artifacts.tessellate; the mesh blob in the
//     terminal resp's binary tail).
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
constexpr const char* kEmpty =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

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

bool recv(const Worker& w, json& out, std::vector<std::uint8_t>& bin) {
    auto rr = onecad::protocol::read_frame(w.from);
    if (rr.status != ReadStatus::Ok) return false;
    out = json::parse(rr.frame.json);
    bin = rr.frame.bin;
    return true;
}

std::string hex(const std::vector<std::uint8_t>& b) {
    static const char* d = "0123456789abcdef";
    std::string s;
    s.reserve(b.size() * 2);
    for (std::uint8_t c : b) { s.push_back(d[c >> 4]); s.push_back(d[c & 0xf]); }
    return s;
}

json plan() {
    return json{
        {"jobId", 88}, {"documentRevision", 0}, {"workerEpoch", 3},
        {"expectedBaseHash", kEmpty},
        {"prefixHashes", json::array({"t0", "t1", "t2"})},
        {"targetStep", 2},
        {"artifacts", {{"tessellate", {{"lod", "coarse"}, {"includeEdges", true}}}}},
        {"ops",
         json::array(
             {json{{"opType", "Sketch"}, {"opId", "op0"}, {"stepIndex", 0},
                   {"params", {{"sketchId", "sk1"}, {"plane", {{"kind", "XY"}}},
                               {"entities", json::array({json{{"id", "e1"}, {"type", "Line"}, {"p0", {0, 0}}, {"p1", {40, 0}}},
                                                         json{{"id", "e2"}, {"type", "Line"}, {"p0", {40, 0}}, {"p1", {40, 20}}},
                                                         json{{"id", "e3"}, {"type", "Line"}, {"p0", {40, 20}}, {"p1", {0, 20}}},
                                                         json{{"id", "e4"}, {"type", "Line"}, {"p0", {0, 20}}, {"p1", {0, 0}}}})},
                               {"constraints", json::array()}}}},
              json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
                   {"inputs", json::array({json{{"primary", {{"bodyId", ""}, {"elementId", "sk1.region.r0"}, {"kind", "face"}}}}})},
                   {"params", {{"sketchId", "sk1"}, {"distance", 25.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}},
              json{{"opType", "Extrude"}, {"opId", "op2"}, {"stepIndex", 2},
                   // A tracked face ref (mints an entry → added/relabeled TopoKeys).
                   {"inputs", json::array({json{{"primary", {{"bodyId", "body_op1"}, {"elementId", "el_face1"}, {"kind", "face"}, {"topoKey", "f:1"}}}}})},
                   {"params", {{"sketchId", "sk1"}, {"distance", 30.0}, {"extrudeMode", "Blind"}, {"booleanMode", "Add"}, {"targetBodyId", "body_op1"}}}}})}};
}

// One run: fingerprint = per-step payloads + prepared hash + inline MESH1 bytes.
std::string run(const std::string& worker_path) {
    Worker w;
    if (!spawn(worker_path, w)) return "";
    std::string fp;
    json resp;
    std::vector<std::uint8_t> bin;
    if (!recv(w, resp, bin)) return "";  // hello
    send(w, Envelope::request(1, "OpenSession",
                              json{{"documentId", "doc_1"}, {"documentRevision", 0}, {"workerEpoch", 3}}));
    if (!recv(w, resp, bin)) return "";
    send(w, Envelope::request(2, "ExecutePlan", plan()));
    for (;;) {
        if (!recv(w, resp, bin)) { fp = ""; break; }
        const std::string t = resp.value("t", std::string{});
        if (t == "event" && resp.value("event", std::string{}) == "planStep") {
            // Full payload: signatures + elementMapDelta (TopoKey tables) + events.
            fp += "S" + std::to_string(resp.value("stepIndex", 0)) + ":" + resp["payload"].dump() + "|";
        } else if (t == "resp" && resp.value("id", 0) == 2) {
            fp += "H:" + resp["result"].value("historyPrefixHash", std::string{});
            fp += "|MESH:" + hex(bin);  // inline tessellate artifact bytes
            break;
        }
    }
    send(w, Envelope::request(9, "Shutdown", json::object()));
    recv(w, resp, bin);
    close(w.to);
    int status = 0;
    waitpid(w.pid, &status, 0);
    close(w.from);
    return fp;
}
}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <worker-path>\n", argv[0]);
        return 2;
    }
    const std::string a = run(argv[1]);
    const std::string b = run(argv[1]);

    if (a.empty()) { std::fprintf(stderr, "FAIL: run 1 produced no fingerprint\n"); ++g_failures; }
    // Sanity: the fingerprint must actually contain the mesh artifact bytes.
    if (a.find("|MESH:") == std::string::npos || a.find("|MESH:|") != std::string::npos) {
        std::fprintf(stderr, "FAIL: no inline MESH1 artifact bytes captured\n");
        ++g_failures;
    }
    if (a != b) {
        std::fprintf(stderr, "FAIL: non-deterministic across runs\n  run1 len=%zu\n  run2 len=%zu\n",
                     a.size(), b.size());
        ++g_failures;
    }
    if (g_failures == 0)
        std::fprintf(stderr, "executeplan determinism: OK (%zu-byte fingerprint incl. MESH1)\n", a.size());
    return g_failures;
}
