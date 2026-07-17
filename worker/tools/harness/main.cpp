// worker_harness — drives onecad-worker over its stdio frame protocol.
//
//   worker_harness --worker <path> --fixture <file.ndjson>
//   worker_harness --worker <path> --repl
//
// Fixture format — NDJSON directives (one JSON object per line), matching the
// canonical protocol/fixtures/*.ndjson exactly (the Rust-authored contract):
//   #  ...                comment (ignored)
//   { "send":   { ... } } a frame envelope -> sent to the worker (SCHEMA §3)
//   { "expect": { ... } } subset matcher checked against the next response frame
//
// Subset match: every field present in the matcher must be present, and equal,
// in the response (recursively). Extra response fields are allowed. Matcher
// value placeholders:
//   "$any" / "$present"  assert the key exists without pinning its value.
//   "$hex64"             assert a lowercase-hex string (16 or 64 chars) — a
//                        64-bit (or SHA-256) hash per SCHEMA §2.
//
// The worker emits an UNSOLICITED hello as its first frame (SCHEMA §6), so a
// fixture's first `expect` matches that hello.
//
// --repl: read one envelope JSON per line from stdin, frame it to the worker,
// print the response envelope JSON to stdout. Loop until EOF.
//
// Exit code: 0 iff every expectation matched (and the worker did not die
// mid-exchange); non-zero otherwise.
#include <sys/wait.h>
#include <unistd.h>

#include <cstdint>
#include <cstring>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

#include "nlohmann/json.hpp"
#include "protocol/Frame.h"

using nlohmann::json;
using onecad::protocol::Frame;
using onecad::protocol::ReadStatus;

namespace {

struct WorkerProc {
    pid_t pid = -1;
    int to_worker = -1;    // parent writes requests here (worker stdin)
    int from_worker = -1;  // parent reads responses here (worker stdout)
};

// Spawn the worker with stdin/stdout wired to pipes; stderr is inherited.
bool spawn_worker(const std::string& path, WorkerProc& out) {
    int p2c[2];  // parent -> child (child stdin)
    int c2p[2];  // child  -> parent (child stdout)
    if (pipe(p2c) != 0 || pipe(c2p) != 0) {
        std::cerr << "harness: pipe() failed: " << std::strerror(errno) << "\n";
        return false;
    }

    const pid_t pid = fork();
    if (pid < 0) {
        std::cerr << "harness: fork() failed: " << std::strerror(errno) << "\n";
        return false;
    }

    if (pid == 0) {
        // Child: wire pipes to stdin/stdout, then exec the worker.
        dup2(p2c[0], STDIN_FILENO);
        dup2(c2p[1], STDOUT_FILENO);
        close(p2c[0]);
        close(p2c[1]);
        close(c2p[0]);
        close(c2p[1]);
        char* const argv[] = {const_cast<char*>(path.c_str()), nullptr};
        execv(path.c_str(), argv);
        std::cerr << "harness: execv('" << path << "') failed: " << std::strerror(errno)
                  << "\n";
        _exit(127);
    }

    // Parent.
    close(p2c[0]);
    close(c2p[1]);
    out.pid = pid;
    out.to_worker = p2c[1];
    out.from_worker = c2p[0];
    return true;
}

// Wait for the worker to exit; return its exit code (or 128+signal).
int reap_worker(WorkerProc& w) {
    if (w.to_worker >= 0) {
        close(w.to_worker);  // EOF -> clean worker shutdown
        w.to_worker = -1;
    }
    int status = 0;
    waitpid(w.pid, &status, 0);
    if (w.from_worker >= 0) {
        close(w.from_worker);
        w.from_worker = -1;
    }
    if (WIFEXITED(status)) return WEXITSTATUS(status);
    if (WIFSIGNALED(status)) return 128 + WTERMSIG(status);
    return -1;
}

// A lowercase-hex string of 16 (64-bit) or 64 (SHA-256) chars — SCHEMA §2 hash
// wire form, matched by the "$hex64" placeholder.
bool is_hex_hash(const std::string& s) {
    if (s.size() != 16 && s.size() != 64) return false;
    for (char c : s) {
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f'))) return false;
    }
    return true;
}

// Recursive subset match: is `expected` contained within `actual`?
// String placeholders: "$any"/"$present" match any value (presence-only);
// "$hex64" matches a lowercase-hex hash string.
bool json_subset(const json& expected, const json& actual, std::string& where) {
    if (expected.is_string()) {
        const std::string tok = expected.get<std::string>();
        if (tok == "$present" || tok == "$any") {
            return true;  // key existence already verified by the parent object
        }
        if (tok == "$hex64") {
            if (actual.is_string() && is_hex_hash(actual.get<std::string>())) return true;
            where = "value: expected $hex64 hex hash, got " + actual.dump();
            return false;
        }
    }
    if (expected.is_object()) {
        if (!actual.is_object()) {
            where = "expected object, got " + std::string(actual.type_name());
            return false;
        }
        for (const auto& [key, val] : expected.items()) {
            if (!actual.contains(key)) {
                where = "missing key '" + key + "'";
                return false;
            }
            std::string sub;
            if (!json_subset(val, actual.at(key), sub)) {
                where = key + (sub.empty() ? "" : "." + sub);
                return false;
            }
        }
        return true;
    }
    if (expected.is_array()) {
        if (!actual.is_array() || actual.size() < expected.size()) {
            where = "array size/type mismatch";
            return false;
        }
        for (std::size_t i = 0; i < expected.size(); ++i) {
            std::string sub;
            if (!json_subset(expected[i], actual[i], sub)) {
                where = "[" + std::to_string(i) + "]" + (sub.empty() ? "" : "." + sub);
                return false;
            }
        }
        return true;
    }
    if (expected != actual) {
        where = "value: expected " + expected.dump() + ", got " + actual.dump();
        return false;
    }
    return true;
}

bool send_request(const WorkerProc& w, const json& envelope) {
    Frame f;
    f.json = envelope.dump();  // normalize
    if (!onecad::protocol::write_frame(w.to_worker, f)) {
        std::cerr << "harness: write_frame to worker failed\n";
        return false;
    }
    return true;
}

// Read one response frame; parse its JSON envelope into `out`.
bool read_response(const WorkerProc& w, json& out) {
    onecad::protocol::ReadResult rr = onecad::protocol::read_frame(w.from_worker);
    if (rr.status != ReadStatus::Ok) {
        std::cerr << "harness: no response frame (status "
                  << static_cast<int>(rr.status) << "): " << rr.error << "\n";
        return false;
    }
    try {
        out = json::parse(rr.frame.json);
    } catch (const json::parse_error& e) {
        std::cerr << "harness: response is not valid JSON: " << e.what() << "\n";
        return false;
    }
    return true;
}

int run_fixture(const std::string& worker_path, const std::string& fixture_path) {
    std::ifstream in(fixture_path);
    if (!in) {
        std::cerr << "harness: cannot open fixture '" << fixture_path << "'\n";
        return 2;
    }

    WorkerProc w;
    if (!spawn_worker(worker_path, w)) return 2;

    int checks = 0;
    bool ok = true;
    int lineno = 0;
    std::string line;
    while (ok && std::getline(in, line)) {
        ++lineno;
        // Trim leading whitespace.
        std::size_t start = line.find_first_not_of(" \t\r");
        if (start == std::string::npos) continue;  // blank
        const std::string trimmed = line.substr(start);
        if (trimmed[0] == '#') continue;  // comment

        json directive;
        try {
            directive = json::parse(trimmed);
        } catch (const json::parse_error& e) {
            std::cerr << "harness: line " << lineno << ": bad directive JSON: " << e.what()
                      << "\n";
            ok = false;
            break;
        }

        if (directive.contains("send")) {
            if (!send_request(w, directive.at("send"))) {
                ok = false;
                break;
            }
        } else if (directive.contains("expect")) {
            const json& matcher = directive.at("expect");
            json response;
            if (!read_response(w, response)) {
                ok = false;
                break;
            }
            std::string where;
            if (!json_subset(matcher, response, where)) {
                std::cerr << "harness: line " << lineno << ": MISMATCH at " << where << "\n"
                          << "  matcher:  " << matcher.dump() << "\n"
                          << "  response: " << response.dump() << "\n";
                ok = false;
                break;
            }
            ++checks;
        } else {
            std::cerr << "harness: line " << lineno
                      << ": directive has neither 'send' nor 'expect'\n";
            ok = false;
            break;
        }
    }

    const int worker_code = reap_worker(w);
    if (ok && worker_code != 0) {
        std::cerr << "harness: worker exited non-zero (" << worker_code << ")\n";
        ok = false;
    }

    if (ok) {
        std::cerr << "harness: OK (" << checks << " expectation(s) matched)\n";
        return 0;
    }
    return 1;
}

int run_repl(const std::string& worker_path) {
    WorkerProc w;
    if (!spawn_worker(worker_path, w)) return 2;

    // Drain the unsolicited hello (SCHEMA §6) and echo it once.
    json hello;
    if (read_response(w, hello)) {
        std::cout << hello.dump() << "\n";
        std::cout.flush();
    }

    std::string line;
    while (std::getline(std::cin, line)) {
        std::size_t start = line.find_first_not_of(" \t\r");
        if (start == std::string::npos) continue;
        json request;
        try {
            request = json::parse(line.substr(start));
        } catch (const json::parse_error& e) {
            std::cerr << "harness: repl request is not valid JSON: " << e.what() << "\n";
            break;
        }
        if (!send_request(w, request)) break;
        json response;
        if (!read_response(w, response)) break;
        std::cout << response.dump() << "\n";
        std::cout.flush();
    }

    const int worker_code = reap_worker(w);
    return worker_code == 0 ? 0 : 1;
}

}  // namespace

int main(int argc, char** argv) {
    std::string worker_path;
    std::string fixture_path;
    bool repl = false;

    for (int i = 1; i < argc; ++i) {
        const std::string arg = argv[i];
        if (arg == "--worker" && i + 1 < argc) {
            worker_path = argv[++i];
        } else if (arg == "--fixture" && i + 1 < argc) {
            fixture_path = argv[++i];
        } else if (arg == "--repl") {
            repl = true;
        } else {
            std::cerr << "harness: unknown/incomplete argument: " << arg << "\n";
            return 2;
        }
    }

    if (worker_path.empty()) {
        std::cerr << "usage: worker_harness --worker <path> "
                     "(--fixture <file.ndjson> | --repl)\n";
        return 2;
    }

    if (repl) return run_repl(worker_path);
    if (fixture_path.empty()) {
        std::cerr << "harness: --fixture <file> or --repl required\n";
        return 2;
    }
    return run_fixture(worker_path, fixture_path);
}
