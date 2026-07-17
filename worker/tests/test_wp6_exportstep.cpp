// test_wp6_exportstep.cpp — W-WP6 ExportStep verb (scope D, D2). Runs a real plan,
// publishes a body, exports it to a temp STEP file, and asserts structural validity:
// the file exists + is non-empty AND is re-importable via STEPControl_Reader (a
// roundtrip that recovers a non-null solid). The corpus records no STEP oracle
// (UI-only exporter in the old stack), so validity is asserted structurally.
//
// stdout hygiene (F2 review finding): this binary calls handle_export_step()
// in-process, WITHOUT main.cpp's normal startup sequence, so OCCT's default
// messenger is not yet redirected off std::cout. STEPControl_Writer drives that
// messenger, and unredirected it writes bytes straight to the process's real
// stdout — which in the real worker would corrupt the protocol frame stream.
// Replicate main.cpp's redirect_occt_to_stderr() (not linkable here: main.cpp
// is a separate translation unit, not part of worker_core) and assert 0 bytes
// land on fd 1 during the export call.
// No framework: exit code == failure count.
#include <fcntl.h>
#include <unistd.h>

#include <cstdio>
#include <cstdint>
#include <filesystem>
#include <string>

#include <IFSelect_ReturnStatus.hxx>
#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>
#include <STEPControl_Reader.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS_Shape.hxx>

#include "io/ExportStep.h"
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
void check(bool cond, const std::string& msg) {
    if (!cond) { std::fprintf(stderr, "FAIL: %s\n", msg.c_str()); ++g_failures; }
}
constexpr const char* kEmpty =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

json line_ent(const std::string& id, double x0, double y0, double x1, double y1) {
    return json{{"id", id}, {"type", "Line"}, {"p0", {x0, y0}}, {"p1", {x1, y1}}};
}

// Mirrors main.cpp's redirect_occt_to_stderr() (src/main.cpp ~lines 57-61): drop
// OCCT's default std::cout printer and route diagnostics to std::cerr instead.
void redirect_occt_to_stderr() {
    Handle(Message_Messenger) messenger = Message::DefaultMessenger();
    messenger->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));
    messenger->AddPrinter(new Message_PrinterOStream("cerr", Standard_False, Message_Info));
}

// Run `fn`, capturing every byte written to the process's real stdout (fd 1)
// during the call, via a temp-file redirect (no concurrent reader needed).
// Restores the original fd 1 before returning.
template <typename Fn>
std::uintmax_t capture_stdout_bytes(Fn&& fn) {
    std::fflush(stdout);
    const std::string tmp =
        (std::filesystem::temp_directory_path() / "onecad_wp6_export_stdout.tmp").string();
    const int cap_fd = ::open(tmp.c_str(), O_CREAT | O_WRONLY | O_TRUNC, 0600);
    const int saved_fd = ::dup(STDOUT_FILENO);
    ::dup2(cap_fd, STDOUT_FILENO);
    ::close(cap_fd);

    fn();

    std::fflush(stdout);
    ::dup2(saved_fd, STDOUT_FILENO);
    ::close(saved_fd);

    std::error_code ec;
    const std::uintmax_t bytes = std::filesystem::file_size(tmp, ec);
    std::filesystem::remove(tmp, ec);
    return ec ? 0 : bytes;
}
}  // namespace

int main() {
    redirect_occt_to_stderr();  // stdout hygiene guard, mirrors main.cpp step 2

    Session s;
    s.open("doc", 0, 3, "determinism");

    // Plan: sketch 10×10 → extrude Blind 10 (NewBody body_op1).
    json ops = json::array(
        {json{{"opType", "Sketch"}, {"opId", "op0"}, {"stepIndex", 0},
              {"params", {{"sketchId", "sk"}, {"plane", {{"kind", "XY"}}},
                          {"entities", json::array({line_ent("e1", 0, 0, 10, 0), line_ent("e2", 10, 0, 10, 10),
                                                    line_ent("e3", 10, 10, 0, 10), line_ent("e4", 0, 10, 0, 0)})},
                          {"constraints", json::array()}}}},
         json{{"opType", "Extrude"}, {"opId", "op1"}, {"stepIndex", 1},
              {"params", {{"sketchId", "sk"}, {"distance", 10.0}, {"extrudeMode", "Blind"}, {"booleanMode", "NewBody"}}}}});

    CancelToken tok;
    HandlerContext ctx{tok, [](int) {}, [](Envelope&) {}};
    json args = {{"jobId", 1}, {"documentRevision", 0}, {"workerEpoch", 3},
                 {"expectedBaseHash", kEmpty}, {"prefixHashes", json::array({"a", "b"})},
                 {"targetStep", 1}, {"ops", ops}};
    onecad::session::handle_execute_plan(s, Envelope::request(1, "ExecutePlan", args), ctx);
    Envelope acc = onecad::session::handle_accept_prepared(
        s, Envelope::request(1, "AcceptPrepared",
                             json{{"jobId", 1}, {"documentRevision", 0}, {"workerEpoch", 3}}));
    check(acc.ok.value_or(false), "export: plan accepted (body published)");

    const std::string path =
        (std::filesystem::temp_directory_path() / "onecad_wp6_export.step").string();
    std::error_code rm;
    std::filesystem::remove(path, rm);

    Envelope resp;
    const std::uintmax_t stdout_bytes = capture_stdout_bytes([&]() {
        resp = onecad::io::handle_export_step(
            s, Envelope::request(2, "ExportStep",
                                 json{{"path", path}, {"bodyIds", json::array({"body_op1"})}, {"schema", "AP214IS"}}));
    });
    check(resp.ok.value_or(false), "export: ExportStep ok");
    check(resp.result.value("written", false), "export: written true");
    const std::uint64_t bytes = resp.result.value("bytes", std::uint64_t{0});
    check(bytes > 0, "export: byte count > 0");
    check(stdout_bytes == 0, "export: zero bytes written to real stdout during export (stdout hygiene)");

    // File exists + non-empty.
    std::error_code ec;
    check(std::filesystem::exists(path, ec), "export: file exists on disk");
    check(std::filesystem::file_size(path, ec) == bytes, "export: reported bytes == file size");

    // Roundtrip: STEPControl_Reader recovers a non-null solid.
    STEPControl_Reader reader;
    const IFSelect_ReturnStatus rs = reader.ReadFile(path.c_str());
    check(rs == IFSelect_RetDone, "export: re-import ReadFile ok");
    if (rs == IFSelect_RetDone) {
        reader.TransferRoots();
        const TopoDS_Shape shape = reader.OneShape();
        check(!shape.IsNull(), "export: re-imported shape non-null");
        int solids = 0;
        for (TopExp_Explorer e(shape, TopAbs_SOLID); e.More(); e.Next()) ++solids;
        check(solids >= 1, "export: re-import recovers a solid");
    }

    std::filesystem::remove(path, rm);
    if (g_failures == 0) std::fprintf(stderr, "wp6_exportstep: OK\n");
    return g_failures;
}
