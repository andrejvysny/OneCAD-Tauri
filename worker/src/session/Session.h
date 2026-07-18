// Session.h — the worker's per-document session (W-WP4).
//
// SUPERSEDES the pre-W-WP4 `protocol/WorkerSession.h` placeholder. One session
// per document (V1). Owns:
//   * the head fencing tokens {documentRevision, workerEpoch, snapshotId} + the
//     `historyPrefixHash` (SCHEMA §7.1/§7.2), and stamps every worker frame via
//     `head_stamp()` (the Dispatcher's stamp source);
//   * the live `BodyStore` (published bodies — stub geometry, W-WP5 → TopoDS);
//   * the session-owned, mutex-guarded `SketchStore` (shared with the solver lane
//     — see SketchStore.h for the cross-lane handoff);
//   * exactly one optional `ScratchJob` (the prepared-but-unpublished plan state);
//   * the committed op-line prefix backing `historyPrefixHash` (see HistoryHash.h);
//   * an ElementMap-partition placeholder (real partitions land in W-WP5).
//
// ── Locking model (solver lane ↔ kernel lane) ────────────────────────────────
// `Session::mu_` guards the head + bodies + scratch + committed prefix. It is
// held only BRIEFLY: fence-and-clone, store-prepared, accept, discard, and each
// `head_stamp()` read. Plan OP EXECUTION runs on the kernel lane WITHOUT the lock
// (on the scratch clone), so a slow/`__slow` plan never blocks the solver lane's
// frame stamping — the solver stays responsive (test_concurrent_lanes).
//
// The `SketchStore` carries its OWN mutex (it is touched by both lanes
// independently of the head), so it is NOT guarded by `mu_`; the solver lane
// writes committed sketches, the kernel lane reads snapshots, with no head-lock
// contention. Live PlaneGCS solve state stays lane-local in SolverLane.
#pragma once

#include <cstdint>
#include <mutex>
#include <optional>
#include <string>
#include <vector>

#include "protocol/Envelope.h"
#include "session/BodyStore.h"
#include "session/ScratchJob.h"
#include "session/SketchStore.h"

namespace onecad::session {

// The full head reported by GetWorkerHead / OpenSession (SCHEMA §7.1).
struct WorkerHead {
    std::uint64_t document_revision = 0;
    std::uint64_t worker_epoch = 0;
    std::uint64_t snapshot_id = 0;
    std::string history_prefix_hash;
    bool has_scratch = false;
};

// Outcome of fence-and-clone at ExecutePlan entry.
struct FenceOutcome {
    enum class Status { Ok, IdempotentPrepared, Error };
    Status status = Status::Error;
    protocol::ErrorInfo error;                       // when Error
    nlohmann::json idempotent_result;                // when IdempotentPrepared
    BodyStore cloned_bodies;                         // when Ok
    elementmap::ElementMapPartition cloned_partition;  // when Ok
    std::uint64_t prepared_snapshot_id = 0;          // when Ok
};

// Outcome of AcceptPrepared.
struct AcceptOutcome {
    bool ok = false;
    protocol::ErrorInfo error;      // when !ok
    std::uint64_t snapshot_id = 0;
    std::uint64_t document_revision = 0;
};

class Session {
public:
    Session() = default;

    // --- lifecycle (SCHEMA §7.1) ---
    // OpenSession: adopt the request's fencing tokens; reset geometry + history.
    void open(std::string document_id, std::uint64_t document_revision,
              std::uint64_t worker_epoch, std::string mode);
    // CloseSession: drop the open flag (state left as last-seen; a fresh open resets).
    void close();
    // ResetSession: drop ALL session + scratch state, increment workerEpoch, keep
    // the process alive. Returns the new epoch (SCHEMA §7.1).
    std::uint64_t reset();

    bool is_open() const;

    // --- head ---
    // The §3 frame stamp (documentRevision/workerEpoch/snapshotId); seq is filled
    // by the Dispatcher. Thread-safe; the Dispatcher's stamp source.
    protocol::Stamp head_stamp() const;
    // The full head (incl. historyPrefixHash + hasScratch) for GetWorkerHead.
    WorkerHead head() const;

    // The session-owned sketch store (self-locked; shared with the solver lane).
    SketchStore& sketches() { return sketches_; }

    // --- ExecutePlan transaction machinery ---
    // Validate fencing + reserve a prepared snapshot id + clone the base bodies /
    // committed prefix. Called at ExecutePlan entry (kernel lane) BEFORE the
    // lock-free op execution. Fencing is workerEpoch + expectedBaseHash ONLY (D4):
    // documentRevision is a Rust-owned advisory stamp and never rejects a plan.
    // D5: a from-0 plan (no base checkpoint AND expectedBaseHash == kEmptyPrefixHash)
    // is ALWAYS base-valid — the head-hash comparison is SKIPPED and the scratch is
    // cloned from an EMPTY base (full replay + wholesale publish at accept), so
    // sequential regens keep working after the head token advances. Incremental plans
    // (expectedBaseHash != the empty anchor) keep the strict head-hash fence.
    FenceOutcome fence_and_clone(std::uint64_t job_id, std::uint64_t document_revision,
                                 std::uint64_t worker_epoch,
                                 const std::string& expected_base_hash);

    // Install the finished scratch as the (single) prepared job. `mu_`-guarded.
    void store_prepared(ScratchJob job);

    // AcceptPrepared: publish the prepared scratch atomically (swap bodies +
    // partition in, advance snapshotId, adopt the opaque head token, and ADOPT the
    // plan's documentRevision as the head — D4). Re-fences workerEpoch ONLY (a
    // restart between prepare/accept bumps the epoch). (Sketches materialized by the
    // plan are intra-plan only — the solver lane owns sketch authoring — so they are
    // not republished here.)
    AcceptOutcome accept_prepared(std::uint64_t job_id, std::uint64_t document_revision,
                                  std::uint64_t worker_epoch);

    // Copies of the live published state, taken under `mu_`, for the element-
    // identity handlers (AcquireElementIds / QueryElement / ResolveRefs, SCHEMA
    // §7.5). Those verbs are stateless w.r.t. the worker (they resolve evidence
    // from the current snapshot's shapes and never mutate), so operating on a copy
    // keeps them off the head lock while they run.
    BodyStore bodies_copy() const;
    elementmap::ElementMapPartition partition_copy() const;
    std::uint64_t current_snapshot_id() const;

    // DiscardPrepared / cancel / failure: drop the scratch (best-effort). Returns
    // whether a scratch was dropped.
    bool discard_prepared(std::uint64_t job_id);

    bool has_scratch() const;

private:
    mutable std::mutex mu_;
    bool open_ = false;
    std::string document_id_;
    std::uint64_t document_revision_ = 0;
    std::uint64_t worker_epoch_ = 0;
    std::uint64_t snapshot_id_ = 0;
    std::string history_prefix_hash_;  // == kEmptyPrefixHash after open()
    std::string mode_ = "determinism";

    BodyStore bodies_;                          // live published bodies (real TopoDS_Shape)
    elementmap::ElementMapPartition partition_; // live published element-map partition
    SketchStore sketches_;                      // self-locked, shared with solver lane
    std::optional<ScratchJob> scratch_;         // the single prepared job
    std::uint64_t snapshot_counter_ = 0;        // monotonic prepared-snapshot ids
};

}  // namespace onecad::session
