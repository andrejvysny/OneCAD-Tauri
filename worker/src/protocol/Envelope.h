// Envelope.h — JSON control envelope carried in a frame's json section.
//
// Wire contract: ../../protocol/SCHEMA.md §3 (NORMATIVE). Every envelope is a
// JSON object with `v` (protocol version, 1) and `t` (frame type). Types:
//   hello | req | resp | progress | event | cancel | credit | chunk
//
//   req   (Rust->worker):   { v, t:"req",  id, verb, args, bin? }
//   resp  (worker->Rust):   { v, t:"resp", id, ok, result|error, <stamp>, bin? }
//   hello (worker->Rust):   { v, t:"hello", seq, result }   (no id, unsolicited)
//   cancel(Rust->worker):   { v, t:"cancel", id }
//
//   error object (§8):      { code, message, detail?, retriable }
//   <stamp>  (§2/§3):       documentRevision, workerEpoch, snapshotId, jobId?, seq
//
// `id` is a u64 JSON number (§2). 64-bit hashes are lowercase hex strings.
// Serialization rejects NaN/Inf floats (nlohmann would coerce them to null).
#pragma once

#include <cstdint>
#include <optional>
#include <stdexcept>
#include <string>
#include <vector>

#include "nlohmann/json.hpp"

namespace onecad::protocol {

// Thrown on serialize (non-finite float) or parse (malformed / bad type) errors.
struct EnvelopeError : std::runtime_error {
    using std::runtime_error::runtime_error;
};

// SCHEMA §3 frame-type discriminator (`t`).
enum class MsgType { Hello, Req, Resp, Progress, Event, Cancel, Credit, Chunk };

std::string to_string(MsgType t);
MsgType msg_type_from_string(const std::string& s);  // throws EnvelopeError if unknown

// SCHEMA §8 error object. `code` is the fixed taxonomy: OP_FAILED |
// REF_UNRESOLVED | GEOMETRY_INVALID | UNSUPPORTED | CANCELLED | PROTOCOL_ERROR.
// `retriable` = whether the caller may retry the request as-is (distinct from
// "recoverable": every code except PROTOCOL_ERROR leaves the session intact by
// definition, so recoverability is implied by the code, not a wire field).
struct ErrorInfo {
    std::string code;
    std::string message;
    bool retriable = false;
    std::optional<nlohmann::json> detail;  // optional structured detail

    ErrorInfo() = default;
    ErrorInfo(std::string code_, std::string message_, bool retriable_,
              std::optional<nlohmann::json> detail_ = std::nullopt)
        : code(std::move(code_)),
          message(std::move(message_)),
          retriable(retriable_),
          detail(std::move(detail_)) {}
};

// One entry of the binary section table describing a slice of the frame's bin.
struct BinSection {
    std::string name;
    std::uint64_t off = 0;
    std::uint64_t len = 0;
};

// SCHEMA §2/§3 worker->Rust frame stamp (fencing + ordering tokens). Every
// worker-originated frame except `hello` carries it. Pre-session (pre-W-WP4) the
// fencing tokens are the session head OpenSession last set (0/0/0 before any
// OpenSession); `seq` is the monotonic output counter assigned by the Dispatcher
// at write time; `jobId` is present only while an ExecutePlan job is in flight.
struct Stamp {
    std::uint64_t document_revision = 0;
    std::uint64_t worker_epoch = 0;
    std::uint64_t snapshot_id = 0;
    std::optional<std::uint64_t> job_id;
    std::uint64_t seq = 0;
};

struct Envelope {
    int v = 1;                          // protocol version
    MsgType type = MsgType::Req;
    std::uint64_t id = 0;               // §2 u64 correlation id (absent on hello/credit)
    std::string verb;                   // request verb (req only)
    nlohmann::json args = nlohmann::json::object();    // request args (§3.1)
    std::optional<bool> ok;             // resp: success flag
    nlohmann::json result = nlohmann::json::object();  // resp result (ok:true) / hello result
                                        // / event payload (§3.4 `payload`)
    std::optional<ErrorInfo> error;     // resp error (ok:false)
    std::optional<std::string> event_name;    // §3.4 event: the event name ("planStep")
    std::optional<std::uint64_t> step_index;  // §3.4 event: hoisted stepIndex
    Stamp stamp;                        // §3 worker-frame stamp (seq filled at write time)
    std::vector<BinSection> bin;        // binary section table

    // Frame-level binary tail bytes (NOT serialized into JSON). When a handler
    // emits a binary payload (e.g. SketchRegions preview triangles) it fills
    // `out_bin` with the raw bytes and `bin` with the section table describing
    // them; the Dispatcher copies `out_bin` into the outgoing Frame's bin.
    std::vector<std::uint8_t> out_bin;

    // --- constructors for common shapes ---
    static Envelope hello(nlohmann::json result);
    static Envelope request(std::uint64_t id, std::string verb,
                            nlohmann::json args = nlohmann::json::object());
    static Envelope ok_response(std::uint64_t id,
                                nlohmann::json result = nlohmann::json::object());
    static Envelope error_response(std::uint64_t id, ErrorInfo error);
    // §3.4 non-terminal event frame (e.g. ExecutePlan `planStep`). `payload` is
    // the event-specific body; the stamp (incl. jobId) is set by the caller.
    static Envelope event(std::uint64_t id, std::string name, std::uint64_t step_index,
                          nlohmann::json payload);
};

// Serialize to a compact JSON string. Throws EnvelopeError on any NaN/Inf float.
std::string serialize(const Envelope& env);

// Parse a JSON envelope string. Throws EnvelopeError on malformed input.
Envelope parse(const std::string& json_text);

}  // namespace onecad::protocol
