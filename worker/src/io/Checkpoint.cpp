// Checkpoint.cpp — see Checkpoint.h.
#include "io/Checkpoint.h"

#include <cstdint>
#include <sstream>
#include <string>
#include <vector>

#include <BinTools.hxx>
#include <Standard_Failure.hxx>
#include <TopoDS_Shape.hxx>

#include "session/BodyStore.h"
#include "session/Signatures.h"
#include "util/Hashing.h"

namespace onecad::io {

using nlohmann::json;
using protocol::Envelope;

namespace {

std::uint64_t get_u64(const json& o, const char* key, std::uint64_t dflt = 0) {
    if (o.is_object() && o.contains(key) && o[key].is_number()) return o[key].get<std::uint64_t>();
    return dflt;
}
std::string get_str(const json& o, const char* key, const std::string& dflt = "") {
    if (o.is_object() && o.contains(key) && o[key].is_string()) return o[key].get<std::string>();
    return dflt;
}

// Serialize a shape to BinTools bytes ("" codec failure ⇒ empty vector).
std::vector<std::uint8_t> bintools_write(const TopoDS_Shape& shape) {
    std::ostringstream oss(std::ios::binary);
    try {
        BinTools::Write(shape, oss);
    } catch (const Standard_Failure&) {
        return {};
    }
    const std::string s = oss.str();
    return std::vector<std::uint8_t>(s.begin(), s.end());
}

json signatures_json(const session::BodyStore& bodies) {
    return json{{"geometry", session::geometry_signature(bodies)},
                {"bodyLifecycle", session::body_lifecycle_signature({})},
                {"referencedBinding", session::referenced_binding_signature({})}};
}

}  // namespace

Envelope handle_save_checkpoint(session::Session& session, const Envelope& req) {
    const std::uint64_t step = get_u64(req.args, "stepIndex");
    const session::CheckpointState st = session.save_checkpoint(step);

    Envelope resp = Envelope::ok_response(req.id, json::object());
    json artifacts = json::array();
    for (const auto& [bid, rec] : st.bodies.all()) {
        const std::vector<std::uint8_t> blob = bintools_write(rec.geom);
        const std::uint64_t off = resp.out_bin.size();
        resp.out_bin.insert(resp.out_bin.end(), blob.begin(), blob.end());
        const std::string section = "ckpt:body:" + bid;
        resp.bin.push_back(protocol::BinSection{section, off, blob.size()});
        artifacts.push_back(json{{"bodyId", bid},
                                 {"bin", section},
                                 {"codec", "brep-bintools"},
                                 {"size", blob.size()},
                                 {"contentHash", hashing::sha256_hex(blob.data(), blob.size())}});
    }

    // ElementMap partition blob (V1 placeholder JSON — the in-session restore uses the
    // RETAINED partition copy, not this serialized form; it is emitted only for
    // Rust-side container durability). Documented divergence.
    const std::string part_json = json{{"format", "elementmap-json"}, {"entries", json::array()}}.dump();
    const std::vector<std::uint8_t> part_bytes(part_json.begin(), part_json.end());
    const std::uint64_t part_off = resp.out_bin.size();
    resp.out_bin.insert(resp.out_bin.end(), part_bytes.begin(), part_bytes.end());
    resp.bin.push_back(protocol::BinSection{"ckpt:partition", part_off, part_bytes.size()});

    resp.result = json{
        {"checkpointId", "ckpt_" + std::to_string(step)},
        {"stepIndex", step},
        {"historyPrefixHash", st.history_prefix_hash},
        {"signatures", signatures_json(st.bodies)},
        {"artifacts", std::move(artifacts)},
        {"elementMapPartition",
         json{{"bin", "ckpt:partition"},
              {"format", "elementmap-json"},
              {"size", part_bytes.size()},
              {"sha256", hashing::sha256_hex(part_bytes.data(), part_bytes.size())}}},
    };
    return resp;
}

Envelope handle_restore_checkpoint(session::Session& session, const Envelope& req) {
    const json& args = req.args;
    const std::uint64_t step = get_u64(args, "stepIndex");
    const std::string expected_hash = get_str(args, "expectedHistoryPrefixHash");
    const std::uint64_t worker_epoch = get_u64(args, "workerEpoch");

    // Fence on workerEpoch (session-mutating; D4). A restart between the plan build and
    // the restore bumps the epoch ⇒ PROTOCOL_ERROR (Rust reconciles / replays).
    const session::WorkerHead head = session.head();
    if (worker_epoch != 0 && worker_epoch != head.worker_epoch) {
        return Envelope::error_response(
            req.id, protocol::ErrorInfo{"PROTOCOL_ERROR", "RestoreCheckpoint: workerEpoch fencing mismatch",
                                        /*retriable=*/false,
                                        json{{"headEpoch", head.worker_epoch}, {"reqEpoch", worker_epoch}}});
    }

    const session::RestoreOutcome out = session.restore_checkpoint(step, expected_hash);
    json drift_detail = json();  // null unless drift
    if (out.drift_detected) {
        drift_detail = json{{"signature", "geometry"},
                            {"expected", expected_hash},
                            {"actual", out.stored_hash}};
    }
    return Envelope::ok_response(
        req.id, json{{"restored", out.restored},
                     {"snapshotId", out.snapshot_id},
                     {"driftDetected", out.drift_detected},
                     {"driftDetail", drift_detail}});
}

}  // namespace onecad::io
