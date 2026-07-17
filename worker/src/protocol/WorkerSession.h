// WorkerSession.h — the worker's minimal session head (pre-W-WP4 placeholder).
//
// SUPERSEDED (W-WP4) by `session/Session.h`, which owns the real per-document
// session (head + bodies + sketches + scratch) and is the Dispatcher's stamp
// source via `Session::head_stamp()`. This header is retained only as historical
// context; nothing includes it anymore. Do not build new code on it.
//
// SCHEMA §2/§3: every worker->Rust frame is stamped with the fencing tokens
// (documentRevision, workerEpoch, snapshotId) of the current session head.
// `OpenSession` sets them, `GetWorkerHead` reads them, `CloseSession` clears the
// open flag, and the Dispatcher stamps EVERY worker frame from this head (via a
// stamp source), so a solver-lane resp and a kernel-lane resp carry the same
// consistent fencing tokens.
//
// This mirrors the Rust stub's `StubState` semantics exactly (a placeholder for
// real per-document session state, which lands with W-WP4): OpenSession echoes
// the request's (documentRevision, workerEpoch) into the head with snapshotId 0;
// there is no real geometry state yet.
//
// Thread-safe: the head is READ concurrently from the kernel and solver lanes
// (stamping) and MUTATED by lifecycle verbs on the kernel lane. Header-only.
#pragma once

#include <cstdint>
#include <mutex>

#include "protocol/Envelope.h"

namespace onecad::session {

class WorkerSession {
public:
    // OpenSession: adopt the request's fencing tokens as the head (snapshotId 0).
    void open(std::uint64_t document_revision, std::uint64_t worker_epoch) {
        std::lock_guard<std::mutex> lk(mu_);
        open_ = true;
        document_revision_ = document_revision;
        worker_epoch_ = worker_epoch;
        snapshot_id_ = 0;
    }

    // CloseSession: drop the open flag (fencing tokens are left as last-seen so a
    // late stamp is still consistent; a fresh OpenSession overwrites them).
    void close() {
        std::lock_guard<std::mutex> lk(mu_);
        open_ = false;
    }

    // Snapshot the current head for a frame stamp (seq is filled by the Dispatcher).
    protocol::Stamp head() const {
        std::lock_guard<std::mutex> lk(mu_);
        protocol::Stamp s;
        s.document_revision = document_revision_;
        s.worker_epoch = worker_epoch_;
        s.snapshot_id = snapshot_id_;
        return s;
    }

    bool is_open() const {
        std::lock_guard<std::mutex> lk(mu_);
        return open_;
    }

private:
    mutable std::mutex mu_;
    bool open_ = false;
    std::uint64_t document_revision_ = 0;
    std::uint64_t worker_epoch_ = 0;
    std::uint64_t snapshot_id_ = 0;
};

}  // namespace onecad::session
