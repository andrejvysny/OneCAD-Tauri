// ExportStep.cpp — see ExportStep.h.
#include "io/ExportStep.h"

#include <cstdint>
#include <filesystem>
#include <string>
#include <vector>

#include <IFSelect_ReturnStatus.hxx>
#include <Interface_Static.hxx>
#include <STEPControl_StepModelType.hxx>
#include <STEPControl_Writer.hxx>
#include <Standard_Failure.hxx>
#include <TopoDS_Shape.hxx>

#include "session/BodyStore.h"

namespace onecad::io {

using nlohmann::json;
using protocol::Envelope;

namespace {

std::string get_str(const json& o, const char* key, const std::string& dflt = "") {
    if (o.is_object() && o.contains(key) && o[key].is_string()) return o[key].get<std::string>();
    return dflt;
}

}  // namespace

Envelope handle_export_step(session::Session& session, const Envelope& req) {
    const json& args = req.args;
    const std::string path = get_str(args, "path");
    if (path.empty()) {
        return Envelope::error_response(
            req.id, protocol::ErrorInfo{"OP_FAILED", "ExportStep: empty path", /*retriable=*/false});
    }
    // SCHEMA §7.8 currently "AP214IS"; forwarded to OCCT's write.step.schema knob.
    const std::string schema = get_str(args, "schema", "AP214IS");

    const session::BodyStore bodies = session.bodies_copy();
    std::vector<std::string> which;
    if (args.contains("bodyIds") && args["bodyIds"].is_array()) {
        for (const auto& b : args["bodyIds"])
            if (b.is_string()) which.push_back(b.get<std::string>());
    } else {
        which = bodies.ids();  // "all"
    }

    try {
        STEPControl_Writer writer;
        Interface_Static::SetCVal("write.step.schema", schema.c_str());

        std::size_t transferred = 0;
        for (const std::string& bid : which) {
            const session::BodyRecord* rec = bodies.get(bid);
            if (!rec || rec->geom.IsNull()) continue;
            const IFSelect_ReturnStatus st = writer.Transfer(rec->geom, STEPControl_AsIs);
            if (st != IFSelect_RetDone) {
                return Envelope::error_response(
                    req.id, protocol::ErrorInfo{"OP_FAILED", "ExportStep: transfer failed for " + bid,
                                                /*retriable=*/false});
            }
            ++transferred;
        }
        if (transferred == 0) {
            return Envelope::error_response(
                req.id,
                protocol::ErrorInfo{"OP_FAILED", "ExportStep: no bodies to export", /*retriable=*/false});
        }

        const IFSelect_ReturnStatus wst = writer.Write(path.c_str());
        if (wst != IFSelect_RetDone) {
            return Envelope::error_response(
                req.id, protocol::ErrorInfo{"OP_FAILED", "ExportStep: write failed", /*retriable=*/false});
        }
    } catch (const Standard_Failure& f) {
        return Envelope::error_response(
            req.id, protocol::ErrorInfo{"OP_FAILED",
                                        std::string("ExportStep raised: ") +
                                            (f.GetMessageString() ? f.GetMessageString() : "OCCT"),
                                        /*retriable=*/false});
    }

    std::error_code ec;
    const std::uintmax_t bytes = std::filesystem::file_size(path, ec);
    return Envelope::ok_response(
        req.id, json{{"written", true}, {"bytes", ec ? 0 : static_cast<std::uint64_t>(bytes)}});
}

}  // namespace onecad::io
