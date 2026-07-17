#include "protocol/Envelope.h"

#include <cmath>

namespace onecad::protocol {

using nlohmann::json;

std::string to_string(MsgType t) {
    switch (t) {
        case MsgType::Hello:    return "hello";
        case MsgType::Req:      return "req";
        case MsgType::Resp:     return "resp";
        case MsgType::Progress: return "progress";
        case MsgType::Event:    return "event";
        case MsgType::Cancel:   return "cancel";
        case MsgType::Credit:   return "credit";
        case MsgType::Chunk:    return "chunk";
    }
    throw EnvelopeError("unreachable MsgType");
}

MsgType msg_type_from_string(const std::string& s) {
    if (s == "hello")    return MsgType::Hello;
    if (s == "req")      return MsgType::Req;
    if (s == "resp")     return MsgType::Resp;
    if (s == "progress") return MsgType::Progress;
    if (s == "event")    return MsgType::Event;
    if (s == "cancel")   return MsgType::Cancel;
    if (s == "credit")   return MsgType::Credit;
    if (s == "chunk")    return MsgType::Chunk;
    throw EnvelopeError("unknown envelope type: " + s);
}

Envelope Envelope::hello(json result) {
    Envelope e;
    e.type = MsgType::Hello;
    e.result = std::move(result);
    return e;
}

Envelope Envelope::request(std::uint64_t id, std::string verb, json args) {
    Envelope e;
    e.type = MsgType::Req;
    e.id = id;
    e.verb = std::move(verb);
    e.args = std::move(args);
    return e;
}

Envelope Envelope::ok_response(std::uint64_t id, json result) {
    Envelope e;
    e.type = MsgType::Resp;
    e.id = id;
    e.ok = true;
    e.result = std::move(result);
    return e;
}

Envelope Envelope::error_response(std::uint64_t id, ErrorInfo error) {
    Envelope e;
    e.type = MsgType::Resp;
    e.id = id;
    e.ok = false;
    e.error = std::move(error);
    return e;
}

Envelope Envelope::event(std::uint64_t id, std::string name, std::uint64_t step_index,
                         json payload) {
    Envelope e;
    e.type = MsgType::Event;
    e.id = id;
    e.event_name = std::move(name);
    e.step_index = step_index;
    e.result = std::move(payload);  // carried as `payload` on the wire (§3.4)
    return e;
}

namespace {

// Recursively reject any non-finite floating-point number. nlohmann would
// otherwise emit NaN/Inf as `null`, silently corrupting the payload.
void reject_non_finite(const json& j) {
    switch (j.type()) {
        case json::value_t::number_float:
            if (!std::isfinite(j.get<double>())) {
                throw EnvelopeError("non-finite float (NaN/Inf) rejected in envelope");
            }
            break;
        case json::value_t::array:
            for (const auto& e : j) reject_non_finite(e);
            break;
        case json::value_t::object:
            for (const auto& [key, val] : j.items()) reject_non_finite(val);
            break;
        default:
            break;
    }
}

// Emit the §2/§3 worker-frame stamp onto `j`.
void write_stamp(json& j, const Stamp& s) {
    j["documentRevision"] = s.document_revision;
    j["workerEpoch"] = s.worker_epoch;
    j["snapshotId"] = s.snapshot_id;
    if (s.job_id.has_value()) j["jobId"] = *s.job_id;
    j["seq"] = s.seq;
}

}  // namespace

std::string serialize(const Envelope& env) {
    json j;
    j["v"] = env.v;
    j["t"] = to_string(env.type);

    switch (env.type) {
        case MsgType::Hello:
            // §6: unsolicited handshake — only seq + result (no id, no stamp).
            j["seq"] = env.stamp.seq;
            j["result"] = env.result;
            break;
        case MsgType::Req:
            // §3.1: Rust->worker. The worker never produces these; the test
            // drivers do (to drive the worker), so keep it complete.
            j["id"] = env.id;
            j["verb"] = env.verb;
            j["args"] = env.args;
            break;
        case MsgType::Resp:
            // §3.2: terminal. `result` iff ok, `error` iff !ok. Always stamped.
            j["id"] = env.id;
            j["ok"] = env.ok.value_or(false);
            if (env.ok.value_or(false)) {
                if (!env.result.is_null()) j["result"] = env.result;
            } else if (env.error.has_value()) {
                json e = {
                    {"code", env.error->code},
                    {"message", env.error->message},
                    {"retriable", env.error->retriable},
                };
                if (env.error->detail.has_value()) e["detail"] = *env.error->detail;
                j["error"] = std::move(e);
            }
            write_stamp(j, env.stamp);
            break;
        case MsgType::Cancel:
            j["id"] = env.id;
            break;
        case MsgType::Credit:
            // Rust->worker only; the worker never produces credit frames.
            break;
        case MsgType::Event:
            // §3.4: non-terminal, correlation-scoped. `event` name + hoisted
            // `stepIndex` + `payload`, then the stamp (with jobId in flight).
            j["id"] = env.id;
            if (env.event_name.has_value()) j["event"] = *env.event_name;
            if (env.step_index.has_value()) j["stepIndex"] = *env.step_index;
            if (!env.result.is_null()) j["payload"] = env.result;
            write_stamp(j, env.stamp);
            break;
        case MsgType::Progress:
        case MsgType::Chunk:
            // Not emitted by the worker yet (bulk/progress lands in later WPs);
            // serialized id+stamp keep the shape §3-correct if ever produced.
            j["id"] = env.id;
            write_stamp(j, env.stamp);
            break;
    }

    if (!env.bin.empty()) {
        json sections = json::array();
        for (const auto& s : env.bin) {
            sections.push_back({{"name", s.name}, {"off", s.off}, {"len", s.len}});
        }
        j["bin"] = std::move(sections);
    }

    // Reject NaN/Inf anywhere in the assembled object before dumping.
    reject_non_finite(j);
    return j.dump();
}

Envelope parse(const std::string& json_text) {
    json j;
    try {
        j = json::parse(json_text);
    } catch (const json::parse_error& e) {
        throw EnvelopeError(std::string("envelope JSON parse error: ") + e.what());
    }
    if (!j.is_object()) {
        throw EnvelopeError("envelope must be a JSON object");
    }

    Envelope e;
    try {
        e.v = j.value("v", 1);
        e.type = msg_type_from_string(j.at("t").get<std::string>());
        if (j.contains("id") && j.at("id").is_number()) {
            e.id = j.at("id").get<std::uint64_t>();
        }
        e.verb = j.value("verb", std::string{});
        if (j.contains("args")) e.args = j.at("args");
        if (j.contains("ok")) e.ok = j.at("ok").get<bool>();
        if (j.contains("result")) e.result = j.at("result");
        if (j.contains("error")) {
            const auto& je = j.at("error");
            ErrorInfo info;
            info.code = je.value("code", std::string{});
            info.message = je.value("message", std::string{});
            info.retriable = je.value("retriable", false);
            if (je.contains("detail")) info.detail = je.at("detail");
            e.error = std::move(info);
        }
        if (j.contains("bin")) {
            for (const auto& s : j.at("bin")) {
                BinSection sec;
                sec.name = s.value("name", std::string{});
                sec.off = s.value("off", std::uint64_t{0});
                sec.len = s.value("len", std::uint64_t{0});
                e.bin.push_back(std::move(sec));
            }
        }
    } catch (const json::exception& ex) {
        throw EnvelopeError(std::string("envelope field error: ") + ex.what());
    }
    return e;
}

}  // namespace onecad::protocol
