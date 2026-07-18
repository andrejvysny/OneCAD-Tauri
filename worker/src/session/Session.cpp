// Session.cpp — see Session.h.
#include "session/Session.h"

#include <utility>

#include "session/HistoryHash.h"

namespace onecad::session {

using protocol::ErrorInfo;

namespace {
ErrorInfo protocol_error(std::string message, nlohmann::json detail) {
    return ErrorInfo{"PROTOCOL_ERROR", std::move(message), /*retriable=*/false,
                     std::move(detail)};
}
}  // namespace

void Session::open(std::string document_id, std::uint64_t document_revision,
                   std::uint64_t worker_epoch, std::string mode) {
    std::lock_guard<std::mutex> lk(mu_);
    open_ = true;
    document_id_ = std::move(document_id);
    document_revision_ = document_revision;
    worker_epoch_ = worker_epoch;
    snapshot_id_ = 0;
    history_prefix_hash_ = kEmptyPrefixHash;  // fresh document ⇒ empty-prefix anchor
    mode_ = std::move(mode);
    bodies_ = BodyStore{};
    partition_ = elementmap::ElementMapPartition{};
    sketches_.clear();
    scratch_.reset();
    snapshot_counter_ = 0;
}

void Session::close() {
    std::lock_guard<std::mutex> lk(mu_);
    open_ = false;
    // Fencing tokens left as last-seen so a late stamp stays consistent; a fresh
    // OpenSession resets everything.
}

std::uint64_t Session::reset() {
    std::lock_guard<std::mutex> lk(mu_);
    // Drop all session + scratch state; bump the epoch; keep the process alive.
    open_ = false;
    document_id_.clear();
    document_revision_ = 0;
    snapshot_id_ = 0;
    history_prefix_hash_ = kEmptyPrefixHash;
    bodies_ = BodyStore{};
    partition_ = elementmap::ElementMapPartition{};
    sketches_.clear();
    scratch_.reset();
    snapshot_counter_ = 0;
    worker_epoch_ += 1;  // Rust echoes the new epoch in subsequent requests.
    return worker_epoch_;
}

bool Session::is_open() const {
    std::lock_guard<std::mutex> lk(mu_);
    return open_;
}

protocol::Stamp Session::head_stamp() const {
    std::lock_guard<std::mutex> lk(mu_);
    protocol::Stamp s;
    s.document_revision = document_revision_;
    s.worker_epoch = worker_epoch_;
    s.snapshot_id = snapshot_id_;
    return s;
}

WorkerHead Session::head() const {
    std::lock_guard<std::mutex> lk(mu_);
    WorkerHead h;
    h.document_revision = document_revision_;
    h.worker_epoch = worker_epoch_;
    h.snapshot_id = snapshot_id_;
    h.history_prefix_hash = history_prefix_hash_;
    h.has_scratch = scratch_.has_value();
    return h;
}

bool Session::has_scratch() const {
    std::lock_guard<std::mutex> lk(mu_);
    return scratch_.has_value();
}

FenceOutcome Session::fence_and_clone(std::uint64_t job_id,
                                      std::uint64_t /*document_revision*/,  // D4: advisory, not fenced
                                      std::uint64_t worker_epoch,
                                      const std::string& expected_base_hash) {
    std::lock_guard<std::mutex> lk(mu_);
    FenceOutcome out;

    if (!open_) {
        out.status = FenceOutcome::Status::Error;
        out.error = protocol_error("ExecutePlan: no open session", nlohmann::json::object());
        return out;
    }

    // One scratch at a time (SCHEMA §7.2). A re-sent SAME jobId while prepared is
    // idempotent (re-return the cached PlanPrepared); a DIFFERENT jobId is a
    // PROTOCOL_ERROR (reject-and-report — see the W-WP4 report / SCHEMA changelog).
    if (scratch_.has_value()) {
        if (scratch_->job_id == job_id) {
            out.status = FenceOutcome::Status::IdempotentPrepared;
            out.idempotent_result = scratch_->prepared_result;
            return out;
        }
        out.status = FenceOutcome::Status::Error;
        out.error = protocol_error(
            "ExecutePlan: a plan is already prepared; accept or discard it first",
            nlohmann::json{{"preparedJobId", scratch_->job_id}, {"requestedJobId", job_id}});
        return out;
    }

    // Fencing: ONLY workerEpoch gates a plan (D4). documentRevision is a Rust-owned
    // advisory stamp (an edit counter) — the worker MUST NOT reject on it, because a
    // post-edit regen legitimately carries a documentRevision ahead of the worker's
    // last-accepted head. The plan's documentRevision is stored in the scratch and
    // adopted as the head at AcceptPrepared. Epoch mismatch ⇒ PROTOCOL_ERROR (Rust
    // reconciles via GetWorkerHead / restart).
    if (worker_epoch != worker_epoch_) {
        out.status = FenceOutcome::Status::Error;
        out.error = protocol_error(
            "ExecutePlan: workerEpoch fencing mismatch",
            nlohmann::json{{"headEpoch", worker_epoch_}, {"planEpoch", worker_epoch}});
        return out;
    }

    // Fencing: expectedBaseHash must equal the head's historyPrefixHash. Detail
    // carries {expected, actual} for Rust reconciliation (SCHEMA §7.2).
    if (expected_base_hash != history_prefix_hash_) {
        out.status = FenceOutcome::Status::Error;
        out.error = protocol_error(
            "ExecutePlan: expectedBaseHash mismatch",
            nlohmann::json{{"expected", expected_base_hash}, {"actual", history_prefix_hash_}});
        return out;
    }

    // Clone the base state for lock-free execution on the kernel lane. Both the
    // BodyStore and the partition are value-copied (TopoDS_Shape/handle copies).
    out.status = FenceOutcome::Status::Ok;
    out.cloned_bodies = bodies_;        // value copy
    out.cloned_partition = partition_;  // value copy
    out.prepared_snapshot_id = ++snapshot_counter_;
    return out;
}

void Session::store_prepared(ScratchJob job) {
    std::lock_guard<std::mutex> lk(mu_);
    scratch_ = std::move(job);
}

AcceptOutcome Session::accept_prepared(std::uint64_t job_id,
                                       std::uint64_t /*document_revision*/,  // D4: advisory
                                       std::uint64_t worker_epoch) {
    std::lock_guard<std::mutex> lk(mu_);
    AcceptOutcome out;

    if (!scratch_.has_value()) {
        out.error = protocol_error("AcceptPrepared: no prepared plan", nlohmann::json::object());
        return out;
    }
    if (scratch_->job_id != job_id) {
        out.error = protocol_error(
            "AcceptPrepared: jobId does not match the prepared plan",
            nlohmann::json{{"preparedJobId", scratch_->job_id}, {"requestedJobId", job_id}});
        return out;
    }
    // Re-fence at accept time on workerEpoch ONLY (D4): documentRevision is advisory
    // and never fences (a restart between prepare and accept bumps the epoch — that
    // Rust catches here; a Rust-owned revision bump does not invalidate the publish).
    if (worker_epoch != worker_epoch_) {
        out.error = protocol_error(
            "AcceptPrepared: stale workerEpoch",
            nlohmann::json{{"headEpoch", worker_epoch_}, {"acceptEpoch", worker_epoch}});
        return out;
    }

    // Atomic publish: swap scratch bodies + partition in, adopt the opaque head
    // token. (Sketches materialized by the plan are intra-plan only — the solver
    // lane owns sketch authoring; they are not republished here.)
    bodies_ = std::move(scratch_->bodies);
    partition_ = std::move(scratch_->partition);
    history_prefix_hash_ = scratch_->history_prefix_hash;  // opaque; never recomputed
    snapshot_id_ = scratch_->prepared_snapshot_id;
    // D4: ADOPT the accepted plan's documentRevision as the head (Rust-owned edit
    // counter), instead of incrementing a worker-owned accept counter. Head stamps
    // thereafter echo this revision.
    document_revision_ = scratch_->plan_document_revision;

    out.ok = true;
    out.snapshot_id = snapshot_id_;
    out.document_revision = document_revision_;
    scratch_.reset();
    return out;
}

BodyStore Session::bodies_copy() const {
    std::lock_guard<std::mutex> lk(mu_);
    return bodies_;  // value copy (handle copies)
}

elementmap::ElementMapPartition Session::partition_copy() const {
    std::lock_guard<std::mutex> lk(mu_);
    return partition_;  // value copy
}

std::uint64_t Session::current_snapshot_id() const {
    std::lock_guard<std::mutex> lk(mu_);
    return snapshot_id_;
}

bool Session::discard_prepared(std::uint64_t /*job_id*/) {
    std::lock_guard<std::mutex> lk(mu_);
    // Best-effort: only one scratch exists; the jobId is advisory (Rust's discard
    // is best-effort and never changes the outcome).
    if (!scratch_.has_value()) return false;
    scratch_.reset();
    return true;
}

}  // namespace onecad::session
