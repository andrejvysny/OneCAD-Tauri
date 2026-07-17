// OpTypes.h — the shared inputs/outputs of a real OCCT op executor (W-WP5).
//
// PlanExecutor drives one op at a time into the scratch clone; each op executor
// (ops::execute_extrude / ops::execute_boolean) reads the op JSON + the scratch
// `OpContext`, mutates the scratch `BodyStore` + `ElementMapPartition`, and returns
// an `OpOutcome` (body events, referenced-body ids, the per-step element-map delta,
// any NeedsRepair items, and — on failure — the recoverable §8 error code).
//
// The op executor OWNS OCCT-history application for the bodies IT touches: it keeps
// its builder(s) alive (SCHEMA §10 ladder level 1 — "builder objects are kept alive
// for the step") and folds Modified/Generated/IsDeleted into the partition of each
// affected body before returning (delta.relabeled / delta.removed). Minting of
// referenced input elements (delta.added) is done by PlanExecutor BEFORE the op
// runs (at the predecessor snapshot), so a step's delta is: added (refs) then
// relabeled/removed (this op's geometry change).
#pragma once

#include <string>
#include <vector>

#include "elementmap/ElementMapPartition.h"
#include "nlohmann/json.hpp"
#include "session/BodyStore.h"
#include "session/Signatures.h"
#include "util/Cancel.h"

namespace onecad::ops {

// Scratch state + policy handed to an op executor. References are into the
// kernel-lane-local ScratchJob (never the live session), so op execution is
// lock-free (Session.h locking model).
struct OpContext {
    session::BodyStore& bodies;                                     // scratch bodies (mutable)
    const std::vector<std::pair<std::string, nlohmann::json>>* sketches;  // sketchId → Sketch op params
    elementmap::ElementMapPartition& partition;                     // scratch partition (mutable)
    std::string* last_sketch_id;                                    // most-recent materialized sketch id
    bool parallel = false;                                          // determinism.parallel (§7.3)
    nlohmann::json occt_options = nlohmann::json::object();         // determinism.occtOptions (§7.3)
    const onecad::CancelToken* cancel = nullptr;                    // cooperative cancel token
};

// One op's result. On Ok: body_events / body_ids / delta / needs_repair are the
// step payload. On Failed/Unsupported: error_code is the §8 code (recoverable —
// scratch only), error_message the detail.
struct OpOutcome {
    enum class Status { Ok, Failed, Unsupported, Cancelled };
    Status status = Status::Ok;

    // Failure info (Status != Ok).
    std::string error_code;     // OP_FAILED | REF_UNRESOLVED | GEOMETRY_INVALID | UNSUPPORTED
    std::string error_message;

    // Success payload.
    std::vector<session::BodyEvent> body_events;
    std::vector<std::string> body_ids;              // bodies present/produced at this step
    elementmap::ElementMapDelta delta;              // relabeled/removed from this op's history
    std::vector<nlohmann::json> needs_repair;       // §9 items (STATE, not error)

    static OpOutcome ok() { return OpOutcome{}; }
    static OpOutcome fail(std::string code, std::string msg) {
        OpOutcome o;
        o.status = Status::Failed;
        o.error_code = std::move(code);
        o.error_message = std::move(msg);
        return o;
    }
    static OpOutcome unsupported(std::string msg) {
        OpOutcome o;
        o.status = Status::Unsupported;
        o.error_code = "UNSUPPORTED";
        o.error_message = std::move(msg);
        return o;
    }
    static OpOutcome cancelled() {
        OpOutcome o;
        o.status = Status::Cancelled;
        o.error_code = "CANCELLED";
        return o;
    }
};

}  // namespace onecad::ops
