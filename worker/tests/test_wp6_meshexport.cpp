// test_wp6_meshexport.cpp — M5a mesh export (ExportStl / ExportObj, SCHEMA §7.8).
// Runs a real plan, publishes a 10×10×10 box, exports it to binary STL + ASCII STL
// + OBJ temp files and asserts structural validity:
//   * the STL triangle count == the tessellation's triangle count (a box = 12 tris,
//     2 per planar face × 6 faces), and the binary STL file is exactly
//     84 + triCount·50 bytes with a matching in-file count field;
//   * the OBJ has vertex (`v`) + face (`f`) lines and a per-body `g` group.
//
// stdout hygiene (mirrors test_wp6_exportstep): the export path must write ZERO
// bytes to the process's real stdout (fd 1) — the exporters use std::ofstream to the
// target file only, so nothing leaks onto the protocol frame stream.
// No framework: exit code == failure count.
#include <fcntl.h>
#include <unistd.h>

#include <cstdint>
#include <cstdio>
#include <filesystem>
#include <fstream>
#include <string>

#include <Message.hxx>
#include <Message_Messenger.hxx>
#include <Message_PrinterOStream.hxx>

#include "io/MeshExport.h"
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

void redirect_occt_to_stderr() {
    Handle(Message_Messenger) messenger = Message::DefaultMessenger();
    messenger->RemovePrinters(STANDARD_TYPE(Message_PrinterOStream));
    messenger->AddPrinter(new Message_PrinterOStream("cerr", Standard_False, Message_Info));
}

template <typename Fn>
std::uintmax_t capture_stdout_bytes(Fn&& fn) {
    std::fflush(stdout);
    const std::string tmp =
        (std::filesystem::temp_directory_path() / "onecad_meshexport_stdout.tmp").string();
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

std::uint32_t read_le_u32(const std::string& path, std::size_t off) {
    std::ifstream f(path, std::ios::binary);
    f.seekg(static_cast<std::streamoff>(off));
    std::uint8_t b[4] = {0, 0, 0, 0};
    f.read(reinterpret_cast<char*>(b), 4);
    return static_cast<std::uint32_t>(b[0]) | (static_cast<std::uint32_t>(b[1]) << 8) |
           (static_cast<std::uint32_t>(b[2]) << 16) | (static_cast<std::uint32_t>(b[3]) << 24);
}

std::size_t count_lines_prefixed(const std::string& path, const std::string& prefix) {
    std::ifstream f(path);
    std::string line;
    std::size_t n = 0;
    while (std::getline(f, line)) {
        if (line.rfind(prefix, 0) == 0) ++n;
    }
    return n;
}
}  // namespace

int main() {
    redirect_occt_to_stderr();

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
    check(acc.ok.value_or(false), "meshexport: plan accepted (body published)");

    const auto dir = std::filesystem::temp_directory_path();
    const std::string stl_bin = (dir / "onecad_meshexport.stl").string();
    const std::string stl_ascii = (dir / "onecad_meshexport_ascii.stl").string();
    const std::string obj = (dir / "onecad_meshexport.obj").string();
    std::error_code rm;
    for (const auto& p : {stl_bin, stl_ascii, obj}) std::filesystem::remove(p, rm);

    // --- binary STL ---
    Envelope stl_resp;
    const std::uintmax_t stdout_bytes = capture_stdout_bytes([&]() {
        stl_resp = onecad::io::handle_export_stl(
            s, Envelope::request(2, "ExportStl",
                                 json{{"path", stl_bin}, {"bodyIds", json::array({"body_op1"})}, {"binary", true}}));
    });
    check(stl_resp.ok.value_or(false), "meshexport: ExportStl ok");
    check(stdout_bytes == 0, "meshexport: zero bytes to real stdout during export (stdout hygiene)");
    const std::uint64_t tri = stl_resp.result.value("triangleCount", std::uint64_t{0});
    const std::uint64_t stl_bytes = stl_resp.result.value("bytes", std::uint64_t{0});
    check(tri >= 12, "meshexport: box tessellates to >= 12 triangles (2 per face x 6)");
    check(stl_bytes == 84 + tri * 50, "meshexport: binary STL size == 84 + triCount*50");
    std::error_code ec;
    check(std::filesystem::exists(stl_bin, ec), "meshexport: STL file exists");
    check(std::filesystem::file_size(stl_bin, ec) == stl_bytes, "meshexport: reported bytes == STL file size");
    check(read_le_u32(stl_bin, 80) == tri, "meshexport: in-file STL triangle count == reported");

    // --- ASCII STL (same triangle count) ---
    Envelope ascii_resp = onecad::io::handle_export_stl(
        s, Envelope::request(3, "ExportStl",
                             json{{"path", stl_ascii}, {"bodyIds", json::array({"body_op1"})}, {"binary", false}}));
    check(ascii_resp.ok.value_or(false), "meshexport: ASCII ExportStl ok");
    check(ascii_resp.result.value("triangleCount", std::uint64_t{0}) == tri,
          "meshexport: ASCII STL triangle count == binary");
    check(count_lines_prefixed(stl_ascii, "  facet normal ") == tri, "meshexport: ASCII STL facet count == triCount");

    // --- OBJ ---
    Envelope obj_resp = onecad::io::handle_export_obj(
        s, Envelope::request(4, "ExportObj", json{{"path", obj}, {"bodyIds", json::array({"body_op1"})}}));
    check(obj_resp.ok.value_or(false), "meshexport: ExportObj ok");
    check(obj_resp.result.value("bytes", std::uint64_t{0}) > 0, "meshexport: OBJ byte count > 0");
    const std::size_t v_lines = count_lines_prefixed(obj, "v ");
    const std::size_t f_lines = count_lines_prefixed(obj, "f ");
    check(f_lines == tri, "meshexport: OBJ face count == triCount");
    // Faces are not shared across body faces (each face owns its nodes), so V >= T.
    check(v_lines >= tri, "meshexport: OBJ vertex count sane (>= triCount)");
    check(count_lines_prefixed(obj, "g body_op1") == 1, "meshexport: OBJ carries the body group");

    // Empty-body-set export is a recoverable OP_FAILED (no meshable bodies).
    Envelope empty_resp = onecad::io::handle_export_stl(
        s, Envelope::request(5, "ExportStl", json{{"path", stl_bin}, {"bodyIds", json::array({"body_nope"})}}));
    check(!empty_resp.ok.value_or(false), "meshexport: unknown-body export fails (OP_FAILED)");

    for (const auto& p : {stl_bin, stl_ascii, obj}) std::filesystem::remove(p, rm);
    if (g_failures == 0) std::fprintf(stderr, "wp6_meshexport: OK\n");
    return g_failures;
}
