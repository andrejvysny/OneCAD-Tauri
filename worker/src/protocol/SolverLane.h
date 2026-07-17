// SolverLane.h — SCHEMA §7.4 sketch-solver-lane verbs (W-WP3b; W-WP4 rewire).
//
// Holds the live drag gestures (lane-local, unlocked) and a REFERENCE to the
// session-owned, mutex-guarded `SketchStore` (W-WP4: sketch state is
// session-owned — see Session.h / SketchStore.h). All five verbs (SketchUpsert /
// BeginGesture / SolveDrag / EndGesture / SketchRegions) run on the Dispatcher's
// SOLVER lane; the store's own mutex makes the committed-sketch handoff to the
// kernel lane (ExecutePlan regen reads) safe, while the warm PlaneGCS systems +
// gestures stay lane-local here (never crossing lanes).
//
// Gesture model: BeginGesture translates a fresh working `Sketch` from the
// stored wire, builds + diagnoses the GCS system ONCE (warm start held for the
// gesture), then each SolveDrag re-solves warm via the ported
// `Sketch::solveWithDrag` (which rebuilds the solver only when dirty — it never
// is mid-gesture). EndGesture does the final exact solve, writes the committed
// positions back into the store, and drops the gesture.
#pragma once

#include <cstdint>
#include <memory>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

#include "protocol/Dispatcher.h"
#include "protocol/Envelope.h"
#include "session/SketchStore.h"
#include "sketch/Sketch.h"
#include "sketch/WireSketch.h"

namespace onecad::protocol {

class SolverLane {
public:
    // The store is owned by the Session (shared across lanes); the lane keeps a
    // reference. It must outlive the SolverLane (main owns both).
    explicit SolverLane(session::SketchStore& store) : store_(store) {}

    // Register all five §7.4 verbs on the dispatcher's solver lane.
    void register_verbs(Dispatcher& dispatcher);

private:
    // Point position by internal id (x,y).
    using PosMap = std::unordered_map<core::sketch::EntityID, std::pair<double, double>>;

    struct Gesture {
        std::uint64_t id = 0;
        std::string sketch_id;
        std::uint64_t sketch_revision = 0;
        std::unique_ptr<core::sketch::Sketch> sketch;
        wire::WireIndex index;
        core::sketch::EntityID drag_point;
        int dof = 0;
        std::vector<std::string> conflicting;  // wire constraint ids (structural)
        PosMap baseline;       // positions at BeginGesture
        PosMap last_reported;  // for incremental deltas
        bool last_success = true;
    };

    Envelope on_upsert(const Envelope& req);
    Envelope on_begin(const Envelope& req);
    Envelope on_drag(const Envelope& req);
    Envelope on_end(const Envelope& req);
    Envelope on_regions(const Envelope& req);

    session::SketchStore& store_;  // session-owned, self-locked (see Session.h)
    std::unordered_map<std::uint64_t, Gesture> gestures_;
};

}  // namespace onecad::protocol
