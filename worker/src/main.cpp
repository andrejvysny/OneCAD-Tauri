// main.cpp — OneCAD C++ sidecar worker entry point.
//
// Responsibilities at this stage:
//   * assert little-endian host (compile-time + runtime)
//   * route OCCT diagnostics to stderr (proves TKernel linkage; guards stdout)
//   * emit the UNSOLICITED hello frame (SCHEMA §6) as the first output frame
//   * register the lifecycle + solver-lane verbs
//   * run the reader/kernel/solver dispatch loop over stdin/stdout, OR
//   * with --selftest, exercise hello + a solver op in-process and exit 0.
//
// stdout carries protocol frames ONLY. All diagnostics go to stderr via WLOG_*.
#include <chrono>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <string>
#include <unistd.h>

#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>
#include <Standard_Version.hxx>

#include "io/ExportStep.h"
#include "protocol/Dispatcher.h"
#include "protocol/Envelope.h"
#include "protocol/SolverLane.h"
#include "session/ElementIdentity.h"
#include "session/PlanExecutor.h"
#include "session/Session.h"
#include "tess/MeshHandle.h"
#include "tess/Tessellate.h"
#include "util/Hashing.h"
#include "util/LittleEndian.h"
#include "util/Log.h"

namespace {

using onecad::protocol::Dispatcher;
using onecad::protocol::Envelope;
using onecad::protocol::HandlerContext;
using onecad::protocol::SolverLane;
using onecad::session::Session;
using onecad::session::WorkerHead;

constexpr int kProtocolVersion = 1;
constexpr const char* kWorkerVersion = "0.1.0";
constexpr int kQuantizationVersion = 1;
constexpr int kSolverPolicyVersion = 1;
// SCHEMA §6 handshake transport limits (defaults).
constexpr std::uint64_t kChunkSize = 1048576;          // 1 MiB
constexpr std::uint64_t kInitialBulkCredit = 8388608;  // 8 MiB

// Redirect OCCT's default messenger from std::cout to std::cerr. The default
// printer writes to std::cout, which would corrupt our stdout frame stream, so
// this is load-bearing for stdout hygiene as well as proving OCCT linkage.
void redirect_occt_to_stderr() {
    Handle(Message_Messenger) messenger = Message::DefaultMessenger();
    // Drop the default std::cout printer.
    messenger->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));
    // "cerr" is recognized by Message_PrinterOStream as the std::cerr stream.
    messenger->AddPrinter(new Message_PrinterOStream("cerr", Standard_False, Message_Info));
}

// occt.fingerprint (SCHEMA §2/§6): a 64-bit FNV-1a hash of the OCCT version
// string, rendered as 16 lowercase hex chars ($hex64). Deterministic. A real
// fingerprint also folds in build flags + algorithm knobs (W-WP4+); the version
// string is the stable pre-W-WP4 stand-in that still satisfies the wire format.
std::string occt_fingerprint(const std::string& occt_version) {
    std::uint64_t h = 14695981039346656037ULL;  // FNV-1a 64-bit offset basis
    for (unsigned char c : occt_version) {
        h ^= c;
        h *= 1099511628211ULL;  // FNV-1a 64-bit prime
    }
    char buf[17];
    std::snprintf(buf, sizeof(buf), "%016llx", static_cast<unsigned long long>(h));
    return std::string(buf);
}

// SCHEMA §6 hello.result payload (shared by the unsolicited hello frame and the
// --selftest content check).
nlohmann::json make_hello_result() {
    const std::string occt_version = OCC_VERSION_COMPLETE;  // e.g. "7.9.3"
    return {
        {"protocolVersion", kProtocolVersion},
        {"workerVersion", kWorkerVersion},
        {"occt",
         {
             {"version", occt_version},
             {"fingerprint", occt_fingerprint(occt_version)},
         }},
        {"quantizationVersion", kQuantizationVersion},
        {"solverPolicyVersion", kSolverPolicyVersion},
        {"capabilities", nlohmann::json::array({"op.sketch", "op.extrude", "op.revolve", "op.fillet",
                                                "op.chamfer", "op.boolean", "solver.planegcs",
                                                "tessellate.mesh1", "io.step"})},
        {"limits", {{"chunkSize", kChunkSize}, {"initialBulkCredit", kInitialBulkCredit}}},
    };
}

// --- lifecycle verbs (SCHEMA §7.1) ------------------------------------------
//
// W-WP4: OpenSession/CloseSession/ResetSession/GetWorkerHead now drive a REAL
// per-document `Session` (head + bodies + sketches + scratch), superseding the
// pre-W-WP4 flag placeholder. OpenSession adopts the request's fencing tokens and
// resets the document to the empty-prefix head; GetWorkerHead reports the real
// head incl. `historyPrefixHash` + `hasScratch`; ResetSession bumps the epoch.

Envelope handle_open_session(Session& session, const Envelope& req) {
    const nlohmann::json& args = req.args;
    const std::string document_id = args.value("documentId", std::string{});
    const std::uint64_t document_revision = args.value("documentRevision", std::uint64_t{0});
    const std::uint64_t worker_epoch = args.value("workerEpoch", std::uint64_t{0});
    const std::string mode = args.value("mode", std::string{"determinism"});
    session.open(document_id, document_revision, worker_epoch, mode);
    nlohmann::json result = {
        {"sessionOpen", true},
        {"workerHead", {{"documentRevision", document_revision}, {"snapshotId", 0}}},
    };
    return Envelope::ok_response(req.id, std::move(result));
}

Envelope handle_close_session(Session& session, const Envelope& req) {
    session.close();
    return Envelope::ok_response(req.id, nlohmann::json{{"sessionClosed", true}});
}

Envelope handle_reset_session(Session& session, const Envelope& req) {
    const std::uint64_t new_epoch = session.reset();
    return Envelope::ok_response(
        req.id, nlohmann::json{{"reset", true}, {"workerEpoch", new_epoch}});
}

Envelope handle_get_worker_head(Session& session, const Envelope& req) {
    const WorkerHead head = session.head();
    nlohmann::json result = {
        {"documentRevision", head.document_revision},
        {"workerEpoch", head.worker_epoch},
        {"snapshotId", head.snapshot_id},
        {"historyPrefixHash", head.history_prefix_hash},
        {"hasScratch", head.has_scratch},
    };
    return Envelope::ok_response(req.id, std::move(result));
}

// Tessellate (SCHEMA §7.6): mesh the requested live bodies into MESH1 blobs. Small
// blobs are inlined in the resp binary tail (§5.2 permits inline ≤ chunkSize); the
// result references each by bin section name + carries totalBytes + sha256.
Envelope handle_tessellate(Session& session, const Envelope& req) {
    const nlohmann::json& args = req.args;
    const std::string lod = args.value("lod", std::string("coarse"));
    const bool include_edges = args.value("includeEdges", true);
    const onecad::session::BodyStore bodies = session.bodies_copy();
    const onecad::elementmap::ElementMapPartition part = session.partition_copy();
    const std::uint64_t snapshot_id = session.current_snapshot_id();

    // bodyIds: "all" or an explicit array.
    std::vector<std::string> which;
    if (args.contains("bodyIds") && args["bodyIds"].is_array()) {
        for (const auto& b : args["bodyIds"])
            if (b.is_string()) which.push_back(b.get<std::string>());
    } else {
        which = bodies.ids();  // "all" (or missing) → every body
    }

    nlohmann::json meshes = nlohmann::json::array();
    Envelope resp = Envelope::ok_response(req.id, nlohmann::json::object());
    for (const std::string& bid : which) {
        const onecad::session::BodyRecord* rec = bodies.get(bid);
        if (!rec) continue;
        onecad::tess::BodyMesh bm =
            onecad::tess::tessellate_body(rec->geom, bid, lod, include_edges, &part);
        if (!bm.ok) continue;
        const std::uint64_t off = resp.out_bin.size();
        resp.out_bin.insert(resp.out_bin.end(), bm.blob.begin(), bm.blob.end());
        const std::string section = "mesh:" + bid;
        resp.bin.push_back(onecad::protocol::BinSection{section, off, bm.blob.size()});
        // Shared §7.6 handle builder (identical shape as ExecutePlan's inline artifact
        // — MeshHandle.h). The inline handle keys the resp-tail section by "bin".
        meshes.push_back(onecad::tess::mesh_handle_json(
            bid, section, lod, bm.blob.size(), bm.triangle_count,
            onecad::hashing::sha256_hex(bm.blob.data(), bm.blob.size()), snapshot_id));
    }
    resp.result = nlohmann::json{{"meshes", std::move(meshes)}};
    return resp;
}

Envelope handle_shutdown(const Envelope& req, const std::vector<std::uint8_t>&,
                         HandlerContext& ctx) {
    ctx.request_shutdown(0);
    return Envelope::ok_response(req.id, nlohmann::json{{"goodbye", true}});
}

// Debug.Busy — busy-SPINS the kernel thread for `durationMs` (default 200ms).
// Compiled always; harmless (a Rust core never sends it in production). Used by
// the solver benchmark to prove drag latency is unaffected by a busy kernel lane.
Envelope handle_debug_busy(const Envelope& req, const std::vector<std::uint8_t>&,
                           HandlerContext&) {
    const int duration_ms =
        (req.args.is_object() && req.args.contains("durationMs") &&
         req.args["durationMs"].is_number())
            ? req.args["durationMs"].get<int>()
            : 200;
    const auto deadline =
        std::chrono::steady_clock::now() + std::chrono::milliseconds(duration_ms);
    volatile std::uint64_t spin = 0;
    while (std::chrono::steady_clock::now() < deadline) {
        spin = spin + 1;  // occupy the core, not just yield
    }
    nlohmann::json result = {{"busySpunMs", duration_ms}};
    return Envelope::ok_response(req.id, std::move(result));
}

void register_verbs(Dispatcher& dispatcher, SolverLane& solver_lane, Session& session) {
    dispatcher.register_verb(
        "OpenSession",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return handle_open_session(session, r);
        });
    dispatcher.register_verb(
        "CloseSession",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return handle_close_session(session, r);
        });
    dispatcher.register_verb(
        "ResetSession",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return handle_reset_session(session, r);
        });
    dispatcher.register_verb(
        "GetWorkerHead",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return handle_get_worker_head(session, r);
        });
    // --- W-WP4: transactional regen (kernel lane, single-writer) ---
    dispatcher.register_verb(
        "ExecutePlan",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext& ctx) {
            return onecad::session::handle_execute_plan(session, r, ctx);
        });
    dispatcher.register_verb(
        "AcceptPrepared",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return onecad::session::handle_accept_prepared(session, r);
        });
    dispatcher.register_verb(
        "DiscardPrepared",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return onecad::session::handle_discard_prepared(session, r);
        });
    // --- W-WP5: geometry + element identity (SCHEMA §7.5/§7.6) ---
    dispatcher.register_verb(
        "Tessellate",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return handle_tessellate(session, r);
        });
    dispatcher.register_verb(
        "AcquireElementIds",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return onecad::session::handle_acquire_element_ids(session, r);
        });
    dispatcher.register_verb(
        "QueryElement",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return onecad::session::handle_query_element(session, r);
        });
    dispatcher.register_verb(
        "ResolveRefs",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return onecad::session::handle_resolve_refs(session, r);
        });
    // --- W-WP6: STEP export (SCHEMA §7.8, D2) ---
    dispatcher.register_verb(
        "ExportStep",
        [&session](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return onecad::io::handle_export_step(session, r);
        });
    dispatcher.register_verb("Shutdown", handle_shutdown);
    dispatcher.register_verb("Debug.Busy", handle_debug_busy);
    solver_lane.register_verbs(dispatcher);  // Sketch* verbs -> solver lane
}

// Validate the hello payload + run a SketchUpsert through the solver lane
// in-process; return exit code (0 == OK).
int run_selftest() {
    Dispatcher dispatcher;
    Session session;
    SolverLane solver_lane(session.sketches());
    register_verbs(dispatcher, solver_lane, session);

    // The hello is unsolicited (SCHEMA §6), not a verb — validate its payload.
    const nlohmann::json hello = make_hello_result();
    const bool hello_ok =
        hello.value("protocolVersion", 0) == kProtocolVersion && hello.contains("occt") &&
        hello["occt"].contains("version") &&
        hello["occt"].value("fingerprint", std::string{}).size() == 16;
    if (!hello_ok) {
        WLOG_ERROR("selftest FAILED: unexpected hello payload: %s", hello.dump().c_str());
        return 1;
    }

    // Exercise the solver lane in-process: a triangle upsert must solve + report.
    nlohmann::json args = {
        {"sketchId", "selftest"},
        {"plane", {{"kind", "XY"}}},
        {"entities",
         {{{"id", "l1"}, {"type", "Line"}, {"p0", {0, 0}}, {"p1", {10, 0}}},
          {{"id", "l2"}, {"type", "Line"}, {"p0", {10, 0}}, {"p1", {5, 8}}},
          {{"id", "l3"}, {"type", "Line"}, {"p0", {5, 8}}, {"p1", {0, 0}}}}},
        {"constraints", nlohmann::json::array()},
    };
    Envelope up = dispatcher.dispatch_once(Envelope::request(1, "SketchUpsert", args));
    if (!up.ok.value_or(false) || !up.result.value("upserted", false)) {
        WLOG_ERROR("selftest FAILED: SketchUpsert response: %s",
                   onecad::protocol::serialize(up).c_str());
        return 1;
    }

    WLOG_INFO("selftest OK (occt %s, upsert dof=%d)",
              hello["occt"]["version"].get<std::string>().c_str(),
              up.result.value("dof", -1));
    return 0;
}

}  // namespace

int main(int argc, char** argv) {
    // 1. Little-endian guarantee (compile-time static_assert lives in the
    //    header; this is the runtime belt-and-suspenders check).
    if (!onecad::endian::host_is_little_endian()) {
        WLOG_ERROR("fatal: host is not little-endian; worker requires LE");
        return 2;
    }

    // 2. OCCT diagnostics -> stderr (and proves TKernel is linked).
    redirect_occt_to_stderr();

    // 3. Argument handling.
    bool selftest = false;
    for (int i = 1; i < argc; ++i) {
        if (std::strcmp(argv[i], "--selftest") == 0) {
            selftest = true;
        } else {
            WLOG_WARN("ignoring unknown argument: %s", argv[i]);
        }
    }

    if (selftest) {
        return run_selftest();
    }

    // 4. Normal operation: emit the unsolicited hello, then dispatch stdin/stdout.
    WLOG_INFO("onecad-worker %s starting (protocol v%d, occt %s)", kWorkerVersion,
              kProtocolVersion, OCC_VERSION_COMPLETE);
    Dispatcher dispatcher;
    Session session;
    SolverLane solver_lane(session.sketches());
    register_verbs(dispatcher, solver_lane, session);
    // Every worker frame is stamped from the session head (SCHEMA §3).
    dispatcher.set_stamp_source([&session] { return session.head_stamp(); });

    const Envelope hello = Envelope::hello(make_hello_result());
    const int code = dispatcher.run(STDIN_FILENO, STDOUT_FILENO, &hello);
    WLOG_INFO("onecad-worker exiting with code %d", code);
    return code;
}
