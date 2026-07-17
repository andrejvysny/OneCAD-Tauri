// solverbench — the SOLVER-LANE LATENCY GATE (W-WP3b).
//
// Spawns the real onecad-worker and drives the SCHEMA §7.4 gesture protocol over
// stdio, measuring per-request round-trip latency (steady_clock at write ->
// response read) for SolveDrag across sketches of 10/50/200/500 entities plus a
// pathological set (near-singular / redundant / conflicting). Per scenario it
// reports p50/p95/p99 round-trip AND the solveMicros vs transport-overhead split.
// One run also fires a Debug.Busy that spins the KERNEL lane, to prove drag
// latency is unaffected by a busy kernel lane.
//
// Output: a markdown table written to worker/tools/solverbench/RESULTS.md and
// printed to stdout, followed by the GATE verdict (pass/fallback/fail). Numbers
// are recorded honestly — if the targets miss, it says so loudly.
//
//   solverbench --worker <path> [--iters N] [--quick] [--out RESULTS.md]
#include <sys/wait.h>
#include <unistd.h>

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <sstream>
#include <string>
#include <unordered_map>
#include <vector>

#include "nlohmann/json.hpp"
#include "protocol/Envelope.h"
#include "protocol/Frame.h"

using nlohmann::json;
using onecad::protocol::Envelope;
using onecad::protocol::Frame;
using onecad::protocol::ReadStatus;
using Clock = std::chrono::steady_clock;

namespace {

// ---- worker process + framed transport ------------------------------------

struct Client {
    pid_t pid = -1;
    int to = -1;
    int from = -1;
    std::uint64_t next_id = 1;                        // SCHEMA §2: u64 correlation ids
    std::unordered_map<std::uint64_t, json> buffered;  // responses read out of order

    bool spawn(const std::string& path) {
        int p2c[2], c2p[2];
        if (pipe(p2c) != 0 || pipe(c2p) != 0) return false;
        pid = fork();
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
        to = p2c[1]; from = c2p[0];
        return true;
    }

    // Frame a req with a fresh u64 id; return the id to correlate the response.
    std::uint64_t send(const std::string& verb, const json& args = json::object()) {
        const std::uint64_t id = next_id++;
        Frame f;
        f.json = onecad::protocol::serialize(Envelope::request(id, verb, args));
        onecad::protocol::write_frame(to, f);
        return id;
    }

    // Read the unsolicited hello (SCHEMA §6). Returns true iff it is a hello frame.
    bool recv_hello() {
        auto rr = onecad::protocol::read_frame(from);
        if (rr.status != ReadStatus::Ok) return false;
        json j = json::parse(rr.frame.json);
        return j.value("t", std::string{}) == "hello";
    }

    // Read frames until the response with `id` arrives (buffering the rest).
    bool recv_id(std::uint64_t id, json& out) {
        auto bit = buffered.find(id);
        if (bit != buffered.end()) {
            out = bit->second;
            buffered.erase(bit);
            return true;
        }
        for (;;) {
            auto rr = onecad::protocol::read_frame(from);
            if (rr.status != ReadStatus::Ok) return false;
            json j = json::parse(rr.frame.json);
            if (!j.contains("id") || !j["id"].is_number()) continue;  // e.g. a stray hello
            const std::uint64_t rid = j["id"].get<std::uint64_t>();
            if (rid == id) { out = std::move(j); return true; }
            buffered[rid] = std::move(j);
        }
    }

    void close_and_reap() {
        if (to >= 0) { ::close(to); to = -1; }
        int status = 0;
        waitpid(pid, &status, 0);
        if (from >= 0) { ::close(from); from = -1; }
    }
};

// ---- sketch generators -----------------------------------------------------

// Consistent H/V staircase chain: seg i horizontal (even) / vertical (odd), each
// length 10, joined by Coincident. Distinct endpoints exercise Coincident. Total
// entities = 3*nseg (2 points + 1 line per segment). Drag point is "s0".
json make_chain(const std::string& sketch_id, int nseg, int& entity_count) {
    json ents = json::array(), cons = json::array();
    double x = 0, y = 0;
    std::string prev_end;
    for (int i = 0; i < nseg; ++i) {
        const bool horiz = (i % 2 == 0);
        const double ex = horiz ? x + 10 : x;
        const double ey = horiz ? y : y + 10;
        const std::string s = "s" + std::to_string(i), e = "e" + std::to_string(i),
                          l = "L" + std::to_string(i);
        ents.push_back({{"id", s}, {"type", "Point"}, {"at", {x, y}}});
        ents.push_back({{"id", e}, {"type", "Point"}, {"at", {ex, ey}}});
        ents.push_back({{"id", l}, {"type", "Line"}, {"p0Ref", s}, {"p1Ref", e}});
        cons.push_back({{"id", "d" + std::to_string(i)}, {"type", "Distance"},
                        {"entities", {l}}, {"value", 10.0}});
        cons.push_back({{"id", "hv" + std::to_string(i)},
                        {"type", horiz ? "Horizontal" : "Vertical"}, {"entities", {l}}});
        if (!prev_end.empty())
            cons.push_back({{"id", "c" + std::to_string(i)}, {"type", "Coincident"},
                            {"entities", {prev_end, s}}, {"positions", {"", ""}}});
        prev_end = e;
        x = ex; y = ey;
    }
    entity_count = static_cast<int>(ents.size());
    return {{"sketchId", sketch_id}, {"plane", {{"kind", "XY"}}},
            {"entities", ents}, {"constraints", cons}};
}

// Near-singular: two nearly-parallel lines joined at a shared vertex with a tiny
// (0.05deg) angle constraint -> ill-conditioned Jacobian.
json make_near_singular() {
    json ents = {
        {{"id", "a"}, {"type", "Point"}, {"at", {0, 0}}},
        {{"id", "b"}, {"type", "Point"}, {"at", {100, 0}}},
        {{"id", "c"}, {"type", "Point"}, {"at", {200, 0.1}}},
        {{"id", "l1"}, {"type", "Line"}, {"p0Ref", "a"}, {"p1Ref", "b"}},
        {{"id", "l2"}, {"type", "Line"}, {"p0Ref", "b"}, {"p1Ref", "c"}},
    };
    json cons = {
        {{"id", "ca"}, {"type", "Angle"}, {"entities", {"l1", "l2"}}, {"value", 0.05}},
    };
    return {{"sketchId", "path_singular"}, {"plane", {{"kind", "XY"}}},
            {"entities", ents}, {"constraints", cons}};
}

// Redundant: a line with two identical Horizontal constraints (benign redundancy).
json make_redundant() {
    json ents = {
        {{"id", "a"}, {"type", "Point"}, {"at", {0, 0}}},
        {{"id", "b"}, {"type", "Point"}, {"at", {10, 0}}},
        {{"id", "l1"}, {"type", "Line"}, {"p0Ref", "a"}, {"p1Ref", "b"}},
    };
    json cons = {
        {{"id", "h1"}, {"type", "Horizontal"}, {"entities", {"l1"}}},
        {{"id", "h2"}, {"type", "Horizontal"}, {"entities", {"l1"}}},
    };
    return {{"sketchId", "path_redundant"}, {"plane", {{"kind", "XY"}}},
            {"entities", ents}, {"constraints", cons}};
}

// Conflicting: two Fixed points + an unsatisfiable HorizontalDistance.
json make_conflicting() {
    json ents = {
        {{"id", "a"}, {"type", "Point"}, {"at", {0, 0}}},
        {{"id", "b"}, {"type", "Point"}, {"at", {10, 0}}},
    };
    json cons = {
        {{"id", "f1"}, {"type", "Fixed"}, {"entities", {"a"}}},
        {{"id", "f2"}, {"type", "Fixed"}, {"entities", {"b"}}},
        {{"id", "hd"}, {"type", "HorizontalDistance"}, {"entities", {"a", "b"}}, {"value", 25.0}},
    };
    return {{"sketchId", "path_conflicting"}, {"plane", {{"kind", "XY"}}},
            {"entities", ents}, {"constraints", cons}};
}

// ---- stats -----------------------------------------------------------------

double pct(std::vector<double> v, double q) {
    if (v.empty()) return 0.0;
    std::sort(v.begin(), v.end());
    const std::size_t idx =
        std::min(v.size() - 1, static_cast<std::size_t>(q * (v.size() - 1) + 0.5));
    return v[idx];
}
double us_to_ms(double us) { return us / 1000.0; }

struct Scenario {
    std::string name;
    int entities = 0;
    std::vector<double> rtt_us;      // round-trip
    std::vector<double> solve_us;    // solveMicros
    std::vector<double> transport_us;
    std::string status_note;         // dominant SolveDrag status observed
};

// Run one gesture: `n_small` small-move drags + `n_large` large jumps. Records
// RTT/solve/transport samples into `sc`.
void run_gesture(Client& c, const std::string& sketch_id, std::uint64_t gid,
                 const std::string& drag_pt, int n_small, int n_large, Scenario& sc) {
    json bargs = {{"sketchId", sketch_id}, {"sketchRevision", 1}, {"gestureId", gid},
                  {"drag", {{"pointId", drag_pt}}}};
    json resp;
    c.recv_id(c.send("BeginGesture", bargs), resp);

    std::unordered_map<std::string, int> statuses;
    int seq = 0;
    auto one_drag = [&](double tx, double ty) {
        ++seq;
        json d = {{"gestureId", gid}, {"seq", seq}, {"pointId", drag_pt}, {"target", {tx, ty}}};
        const auto t0 = Clock::now();
        const std::uint64_t id = c.send("SolveDrag", d);
        json r;
        if (!c.recv_id(id, r)) return;
        const auto t1 = Clock::now();
        const double rtt =
            std::chrono::duration_cast<std::chrono::nanoseconds>(t1 - t0).count() / 1000.0;
        double solve = 0.0;
        std::string status = "?";
        if (r.value("ok", false) && r.contains("result")) {
            solve = r["result"].value("solveMicros", 0.0);
            status = r["result"].value("status", std::string{"?"});
        }
        sc.rtt_us.push_back(rtt);
        sc.solve_us.push_back(solve);
        sc.transport_us.push_back(std::max(0.0, rtt - solve));
        statuses[status]++;
    };

    for (int i = 0; i < n_small; ++i) one_drag(0.01 * (i % 20) - 0.1, 0.01 * (i % 15));
    for (int i = 0; i < n_large; ++i) one_drag(50.0 * ((i % 2) ? 1 : -1), 40.0 * (i % 3));

    c.recv_id(c.send("EndGesture", json{{"gestureId", gid}}), resp);

    int best = -1;
    for (const auto& [k, v] : statuses) {
        if (v > best) { best = v; sc.status_note = k; }
    }
}

std::string fmt(double v) {
    char b[32];
    std::snprintf(b, sizeof(b), "%.3f", v);
    return b;
}

}  // namespace

int main(int argc, char** argv) {
    std::string worker;
    std::string out_path = "RESULTS.md";
    int iters = 1000;
    int large = 50;
    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        if (a == "--worker" && i + 1 < argc) worker = argv[++i];
        else if (a == "--out" && i + 1 < argc) out_path = argv[++i];
        else if (a == "--iters" && i + 1 < argc) iters = std::atoi(argv[++i]);
        else if (a == "--quick") { iters = 40; large = 8; }
    }
    if (worker.empty()) {
        std::fprintf(stderr, "usage: %s --worker <path> [--iters N] [--quick] [--out FILE]\n",
                     argv[0]);
        return 2;
    }

    Client c;
    if (!c.spawn(worker)) { std::fprintf(stderr, "spawn failed\n"); return 2; }

    // Handshake: the worker's first frame is an unsolicited hello (SCHEMA §6).
    json resp;
    if (!c.recv_hello()) {
        std::fprintf(stderr, "worker handshake failed (no hello)\n");
        return 2;
    }

    std::vector<Scenario> scenarios;

    // --- entity-count sweep ---
    const int targets[] = {10, 50, 200, 500};
    std::uint64_t gid = 1;
    for (int t : targets) {
        const int nseg = std::max(1, (t + 1) / 3);
        int ec = 0;
        const std::string sid = "chain" + std::to_string(t);
        json args = make_chain(sid, nseg, ec);
        c.recv_id(c.send("SketchUpsert", args), resp);
        Scenario sc;
        sc.name = "chain " + std::to_string(t);
        sc.entities = ec;
        run_gesture(c, sid, gid++, "s0", iters, large, sc);
        scenarios.push_back(std::move(sc));
    }

    // --- pathological set (fixed small; fewer iters) ---
    struct Path { std::string name; json (*gen)(); std::string sid; std::string pt; };
    std::vector<Path> paths = {
        {"pathological near-singular", make_near_singular, "path_singular", "a"},
        {"pathological redundant", make_redundant, "path_redundant", "a"},
        {"pathological conflicting", make_conflicting, "path_conflicting", "a"},
    };
    const int path_iters = std::min(iters, 200);
    for (auto& p : paths) {
        json args = p.gen();
        c.recv_id(c.send("SketchUpsert", args), resp);
        Scenario sc;
        sc.name = p.name;
        sc.entities = static_cast<int>(args["entities"].size());
        run_gesture(c, p.sid, gid++, p.pt, path_iters, 4, sc);
        scenarios.push_back(std::move(sc));
    }

    // --- concurrent busy-kernel scenario @200 entities ---
    Scenario busy;
    busy.name = "chain 200 (kernel BUSY)";
    {
        const int nseg = (200 + 1) / 3;
        int ec = 0;
        json args = make_chain("chain200b", nseg, ec);
        c.recv_id(c.send("SketchUpsert", args), resp);
        busy.entities = ec;
        // Kick off a long kernel-lane spin WITHOUT waiting for its response.
        const int busy_ms = 500;
        const std::uint64_t busy_id = c.send("Debug.Busy", json{{"durationMs", busy_ms}});
        // Drive drags concurrently; the kernel is pinned but the solver lane is free.
        run_gesture(c, "chain200b", gid++, "s0", std::min(iters, 500), 0, busy);
        // Drain the busy response.
        c.recv_id(busy_id, resp);
    }
    scenarios.push_back(std::move(busy));

    c.close_and_reap();

    // --- build markdown ---
    std::ostringstream md;
    md << "# Solver-lane latency benchmark (W-WP3b GATE)\n\n";
    md << "Round-trip = steady_clock at request write -> response read. "
          "solveMicros is the worker-reported PlaneGCS solve time; transport = "
          "round-trip - solveMicros. Each row: small-move drags"
       << " (+ large jumps) over one gesture.\n\n";
    md << "| scenario | entities | samples | status | rtt p50 (ms) | rtt p95 (ms) | rtt p99 (ms) "
          "| solve p50 | solve p95 | solve p99 | transport p95 (ms) |\n";
    md << "|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|\n";
    for (auto& sc : scenarios) {
        md << "| " << sc.name << " | " << sc.entities << " | " << sc.rtt_us.size() << " | "
           << sc.status_note << " | " << fmt(us_to_ms(pct(sc.rtt_us, 0.50))) << " | "
           << fmt(us_to_ms(pct(sc.rtt_us, 0.95))) << " | " << fmt(us_to_ms(pct(sc.rtt_us, 0.99)))
           << " | " << fmt(us_to_ms(pct(sc.solve_us, 0.50))) << " | "
           << fmt(us_to_ms(pct(sc.solve_us, 0.95))) << " | "
           << fmt(us_to_ms(pct(sc.solve_us, 0.99))) << " | "
           << fmt(us_to_ms(pct(sc.transport_us, 0.95))) << " |\n";
    }
    md << "\n";

    // --- gate verdict (@200 entities) ---
    const Scenario* at200 = nullptr;
    for (auto& sc : scenarios) {
        if (sc.name == "chain 200") at200 = &sc;
    }
    std::string verdict = "INDETERMINATE (no 200-entity sample)";
    if (at200) {
        const double solve_p95_ms = us_to_ms(pct(at200->solve_us, 0.95));
        const double rtt_p95_ms = us_to_ms(pct(at200->rtt_us, 0.95));
        md << "## GATE verdict (@200 entities)\n\n";
        md << "- solver-only p95 = **" << fmt(solve_p95_ms) << " ms** (target <= 2-3 ms)\n";
        md << "- round-trip p95 = **" << fmt(rtt_p95_ms) << " ms** (target <= 6 ms; fallback <= 12-16 ms)\n\n";
        if (solve_p95_ms <= 3.0 && rtt_p95_ms <= 6.0) verdict = "PASS (120Hz-exact target met)";
        else if (rtt_p95_ms <= 16.0) verdict = "FALLBACK (120Hz preview + 30-60Hz exact)";
        else verdict = "FAIL (tails exceed fallback budget)";
        md << "**VERDICT: " << verdict << "**\n";
    }

    // --- write + print ---
    {
        std::ofstream f(out_path);
        f << md.str();
    }
    std::fprintf(stdout, "%s\n", md.str().c_str());
    std::fprintf(stderr, "solverbench: wrote %s\n", out_path.c_str());
    return 0;
}
