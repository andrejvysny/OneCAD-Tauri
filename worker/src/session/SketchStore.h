// SketchStore.h — session-owned authoritative wire-sketch store (W-WP4).
//
// Holds the last-committed wire sketch (plane + entities + constraints) and its
// revision per `sketchId`. In W-WP3b this was a PRE-SESSION, solver-lane-only,
// lock-free holder; W-WP4 folds it into `Session` (see Session.h) and makes it
// MUTEX-GUARDED, because two lanes now touch it:
//
//   * SOLVER lane (owner of sketch SOLVE state) writes committed sketches on
//     SketchUpsert / EndGesture (`upsert` / `put`). The live, warm PlaneGCS
//     systems + drag gestures stay lane-local in SolverLane (never here) — only
//     the committed authoritative wire lands in the store.
//   * KERNEL lane READS committed sketches during ExecutePlan regen (`snapshot`),
//     e.g. a Sketch op materializing / an Extrude reading its profile.
//
// The handoff is value-copy under the lock: cross-lane readers get a `snapshot`
// (a copy), so no pointer into the map escapes the lock (the old pointer-
// returning `get`/`get_mut` are gone). Copies are cheap (sketches are small).
#pragma once

#include <cstdint>
#include <mutex>
#include <optional>
#include <string>
#include <unordered_map>

#include "nlohmann/json.hpp"

namespace onecad::session {

struct StoredSketch {
    nlohmann::json wire_args;  // {plane, entities[], constraints[]} as upserted
    std::uint64_t revision = 0;
};

class SketchStore {
public:
    SketchStore() = default;
    // Movable/copyable value semantics require a fresh mutex on copy/move (a
    // std::mutex is not copyable). Sketch state is copied; the lock is not.
    SketchStore(const SketchStore& other) { copy_from(other); }
    SketchStore& operator=(const SketchStore& other) {
        if (this != &other) copy_from(other);
        return *this;
    }

    // Replace the sketch's full state; bump + return the new revision.
    std::uint64_t upsert(const std::string& sketch_id, nlohmann::json wire_args) {
        std::lock_guard<std::mutex> lk(mu_);
        StoredSketch& s = sketches_[sketch_id];
        s.wire_args = std::move(wire_args);
        s.revision += 1;
        return s.revision;
    }

    // Write an exact (wire, revision) — used by EndGesture to commit solved
    // positions at the gesture's post-solve revision.
    void put(const std::string& sketch_id, nlohmann::json wire_args, std::uint64_t revision) {
        std::lock_guard<std::mutex> lk(mu_);
        StoredSketch& s = sketches_[sketch_id];
        s.wire_args = std::move(wire_args);
        s.revision = revision;
    }

    // Cross-lane-safe read: returns a COPY (or nullopt). No pointer escapes.
    std::optional<StoredSketch> snapshot(const std::string& sketch_id) const {
        std::lock_guard<std::mutex> lk(mu_);
        auto it = sketches_.find(sketch_id);
        if (it == sketches_.end()) return std::nullopt;
        return it->second;
    }

    bool contains(const std::string& sketch_id) const {
        std::lock_guard<std::mutex> lk(mu_);
        return sketches_.count(sketch_id) != 0;
    }

    // Drop all sketches (a fresh OpenSession / ResetSession).
    void clear() {
        std::lock_guard<std::mutex> lk(mu_);
        sketches_.clear();
    }

private:
    void copy_from(const SketchStore& other) {
        std::lock_guard<std::mutex> lk(other.mu_);
        sketches_ = other.sketches_;
    }

    mutable std::mutex mu_;
    std::unordered_map<std::string, StoredSketch> sketches_;
};

}  // namespace onecad::session
