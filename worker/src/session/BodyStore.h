// BodyStore.h — session body registry holding REAL OCCT geometry (W-WP5).
//
// A `BodyStore` maps `BodyId` (string) → a body record: its id, its provenance
// (the producing opId), and a geometry payload. W-WP5 swapped the payload from the
// W-WP4 `StubBody` to a real `TopoDS_Shape`; the store itself is payload-agnostic
// (it never inspects `BodyGeometry`) so the swap was a single alias line. Only the
// free metric/signature functions (`session/ShapeMetrics.*`, `session/Signatures.*`)
// inspect the shape.
//
// ── The `BodyGeometry` alias (single swap point) ─────────────────────────────
// `using BodyGeometry = TopoDS_Shape;`. A `TopoDS_Shape` is a cheap handle
// (reference-counted TShape + location + orientation), so a `BodyStore` stays
// value-semantic and clones cheaply into a `ScratchJob` — the clone shares the
// underlying TShape until an op replaces a record's shape with a fresh build.
//
// Thread-safety: a `BodyStore` is a plain value container with NO internal lock.
// The live store lives inside `Session` and is only mutated on the kernel lane
// (ExecutePlan accept); scratch stores are kernel-lane-local. `Session` guards
// cross-lane access (see Session.h).
#pragma once

#include <map>
#include <string>
#include <utility>
#include <vector>

#include <TopoDS_Shape.hxx>

namespace onecad::session {

// The single swap point: W-WP4 had `using BodyGeometry = StubBody;`. W-WP5 holds
// a real OCCT shape. Nothing else in BodyStore references the concrete payload.
using BodyGeometry = TopoDS_Shape;

// One registered body: identity, provenance, geometry, visibility.
struct BodyRecord {
    std::string id;            // BodyId (e.g. "body_op_5")
    std::string provenance;    // producing opId (SCHEMA §7.3 op.opId)
    BodyGeometry geom;         // real TopoDS_Shape (W-WP5)
    bool visible = true;
};

// A registry of bodies keyed by BodyId, iterated in sorted id order for
// deterministic signatures/lifecycle. Value-semantic: cloned cheaply for scratch.
class BodyStore {
public:
    // Create (or replace) a body. Returns a reference to the stored record.
    BodyRecord& create(const std::string& id, std::string provenance, BodyGeometry geom) {
        BodyRecord& r = bodies_[id];
        r.id = id;
        r.provenance = std::move(provenance);
        r.geom = std::move(geom);
        r.visible = true;
        return r;
    }

    bool contains(const std::string& id) const { return bodies_.count(id) != 0; }

    const BodyRecord* get(const std::string& id) const {
        auto it = bodies_.find(id);
        return it != bodies_.end() ? &it->second : nullptr;
    }

    BodyRecord* get_mut(const std::string& id) {
        auto it = bodies_.find(id);
        return it != bodies_.end() ? &it->second : nullptr;
    }

    // Remove a body (e.g. a boolean tool consumed by the operation). No-op if
    // absent. Returns whether a body was removed.
    bool erase(const std::string& id) { return bodies_.erase(id) != 0; }

    std::size_t size() const { return bodies_.size(); }

    // Ids in ascending sorted order (deterministic iteration).
    std::vector<std::string> ids() const {
        std::vector<std::string> out;
        out.reserve(bodies_.size());
        for (const auto& [id, _] : bodies_) out.push_back(id);
        return out;
    }

    const std::map<std::string, BodyRecord>& all() const { return bodies_; }

private:
    std::map<std::string, BodyRecord> bodies_;
};

}  // namespace onecad::session
