// ScratchJob.h — the scratch state one ExecutePlan builds, before publish (W-WP5).
//
// SCHEMA §7.2: the worker executes a plan step-by-step into SCRATCH job state,
// NEVER mutating the active session mid-plan; it stops at the first failure /
// NeedsRepair preparing snapshot `m−1`, and ends with a terminal `PlanPrepared`.
// Rust then publishes (`AcceptPrepared`) or drops (`DiscardPrepared`).
//
// A `ScratchJob` is:
//   * a CLONE of the session `BodyStore` at fence time (real TopoDS_Shape, W-WP5),
//     mutated by each op;
//   * a CLONE of the session `ElementMapPartition` (referenced/minted elements),
//     rebound by each op's OCCT history → per-step elementMapDelta;
//   * the sketch definitions materialized by Sketch ops in THIS plan (raw Sketch op
//     params, so a later Extrude can rebuild its profile without touching the live
//     session);
//   * the prepared opaque `historyPrefixHash` token (echoed on PlanPrepared,
//     adopted as the session head on accept — the worker never computes it;
//     see HistoryHash.h);
//   * the terminal bookkeeping + the cached `PlanPrepared` result json for
//     idempotent re-return of a re-sent jobId.
//
// Exactly one scratch exists at a time (SCHEMA §7.2): `Session` holds an
// `optional<ScratchJob>`; a second ExecutePlan with a DIFFERENT jobId while one is
// prepared is a PROTOCOL_ERROR (Session.cpp / the W-WP4 report).
#pragma once

#include <cstdint>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include "elementmap/ElementMapPartition.h"
#include "nlohmann/json.hpp"
#include "session/BodyStore.h"

namespace onecad::session {

// One entry of the PlanPrepared per-step summary (SCHEMA §7.2 `perStepResults`).
struct StepResult {
    std::uint64_t step_index = 0;
    std::string status;                  // "ok" | "opFailed" | "needsRepair"
    std::vector<std::string> body_ids;   // bodies present/produced at this step
    std::optional<std::uint64_t> ref_count;  // needsRepair: number of unresolved refs
    std::string message;                 // opFailed: the §8 recoverable message (why)
};

struct ScratchJob {
    std::uint64_t job_id = 0;

    // The plan's documentRevision (D4): Rust-owned advisory edit counter carried in
    // the ExecutePlan args. The worker NEVER fences on it (only workerEpoch +
    // expectedBaseHash gate a plan); it is stored here and ADOPTED as the session
    // head documentRevision at AcceptPrepared (head stamps thereafter echo it).
    std::uint64_t plan_document_revision = 0;

    // The scratch body state (clone of live at fence time, mutated by ops).
    BodyStore bodies;

    // The scratch element-map partition (clone of live at fence time; rebound by
    // each op's OCCT history).
    elementmap::ElementMapPartition partition;

    // Sketches materialized by this plan's Sketch ops — raw Sketch op params keyed
    // by sketchId, in materialization order (a later op reads its profile from
    // here). Intra-plan only; the solver lane owns authoritative sketch state, so
    // these are NOT republished on accept.
    std::vector<std::pair<std::string, nlohmann::json>> sketches;

    // Terminal bookkeeping.
    std::uint64_t prepared_snapshot_id = 0;
    std::optional<std::uint64_t> last_valid_step;  // nullopt ⇒ only the base is valid
    std::string stopped_reason;                    // "completed"|"opFailed"|"needsRepair"
    std::vector<StepResult> per_step;

    // The prepared opaque head token: prefixHashes[lastExecutedIdx] (or the plan's
    // expectedBaseHash when only the base is valid). Echoed as PlanPrepared
    // historyPrefixHash; adopted as the session head on accept.
    std::string history_prefix_hash;

    // The cached PlanPrepared result json, re-returned verbatim if the same jobId
    // is re-sent while still prepared (idempotent request ids — migration plan).
    nlohmann::json prepared_result;
};

}  // namespace onecad::session
