// ElementMapPartition.h — ElementMap V2: per-BodyId partition onto body-independent
// ElementIds, with snapshot-scoped TopoKeys + OCCT-history rebinding (W-WP5).
//
// ── What this is (and what W-WP6 adds) ───────────────────────────────────────
// The V1 OneCAD-CPP `ElementMap` (kernel/elementmap/ElementMap.h) is a persistent
// name table whose IDs EMBED the BodyId (`bodyId/kind-…`). The migration plan
// replaces that with globally-unique, body-INDEPENDENT ElementIds whose partition
// membership (which body an element belongs to) is a MAPPING, not part of the id.
// This partition is that mapping.
//
//   * Entries exist ONLY for elements that were referenced by an op input or
//     minted on demand — never one-per-face (ID-on-demand, SCHEMA §7.5). A
//     NewBody op with no referenced sub-elements produces an EMPTY delta.
//   * A `TopoKey` ("f:N"/"e:N"/"v:N") is the sub-shape's 1-based ordinal in
//     `TopExp::MapShapes(bodyShape, kind)` — DETERMINISTIC for a given shape,
//     SNAPSHOT-SCOPED (valid only within the snapshot that produced it), and
//     NON-PERSISTENT (recomputed each snapshot; never stored across a publish).
//   * Descriptors are REUSED VERBATIM from the kernel `ElementMap` (its private
//     `computeDescriptor`, reached via a throwaway instance so no constant is
//     forked and `proto_elementmap_rigorous` stays green). They are EVIDENCE,
//     never identity (Invariant 2).
//   * After each op the worker applies OCCT history (Modified/Generated/IsDeleted
//     from the kept builder) to the entries of the AFFECTED bodies and emits an
//     `elementMapDelta` {added, removed, relabeled} whose added/relabeled entries
//     carry a REQUIRED `bodyId` (SCHEMA §7.2, amended 2026-07-17).
//
// DEFERRED to W-WP6 (documented placeholders here): descriptor/anchor SCORING and
// the confidence-gated resolution ladder. When history alone cannot rebind a
// referenced element, this WP emits a NeedsRepair item with reason "no-candidates"
// (the ladder's terminal state) rather than scoring candidates — see PlanExecutor.
#pragma once

#include <cstdint>
#include <map>
#include <string>
#include <vector>

#include <BRepBuilderAPI_MakeShape.hxx>
#include <TopoDS_Shape.hxx>

#include "kernel/elementmap/ElementMap.h"
#include "nlohmann/json.hpp"

namespace onecad::elementmap {

namespace km = onecad::kernel::elementmap;

// One {elementId, topoKey, kind, bodyId} tuple as it appears in an
// elementMapDelta added/relabeled entry (SCHEMA §7.2).
struct DeltaEntry {
    std::string element_id;
    std::string topo_key;
    std::string kind;     // "face" | "edge" | "vertex" | "body"
    std::string body_id;

    nlohmann::json to_json() const {
        return nlohmann::json{{"elementId", element_id},
                              {"topoKey", topo_key},
                              {"kind", kind},
                              {"bodyId", body_id}};
    }
};

// The per-step partition delta (SCHEMA §7.2 `elementMapDelta`).
struct ElementMapDelta {
    std::vector<DeltaEntry> added;
    std::vector<std::string> removed;  // elementIds
    std::vector<DeltaEntry> relabeled;

    bool empty() const { return added.empty() && removed.empty() && relabeled.empty(); }

    nlohmann::json to_json() const {
        nlohmann::json a = nlohmann::json::array();
        for (const auto& e : added) a.push_back(e.to_json());
        nlohmann::json r = nlohmann::json::array();
        for (const auto& e : relabeled) r.push_back(e.to_json());
        nlohmann::json rem = nlohmann::json::array();
        for (const auto& id : removed) rem.push_back(id);
        return nlohmann::json{{"added", std::move(a)},
                              {"removed", std::move(rem)},
                              {"relabeled", std::move(r)}};
    }
};

// One tracked element: identity + its current (snapshot-scoped) binding + evidence.
struct PartitionEntry {
    std::string element_id;   // globally-unique, body-independent (Rust-minted)
    std::string body_id;      // partition membership (moves on split/merge)
    km::ElementKind kind = km::ElementKind::Unknown;
    std::string topo_key;     // snapshot-scoped "f:N"/"e:N"/"v:N"
    TopoDS_Shape shape;       // the live sub-shape this snapshot (rebound by history)
    km::ElementDescriptor descriptor;  // frozen evidence (kernel-computed)
    nlohmann::json anchor;    // anchor evidence echo (from the ref that minted it)
};

class ElementMapPartition {
public:
    // --- queries ---
    const PartitionEntry* find(const std::string& element_id) const;
    bool contains(const std::string& element_id) const;
    std::size_t size() const { return entries_.size(); }
    std::vector<const PartitionEntry*> entries_for_body(const std::string& body_id) const;

    // --- minting (ID-on-demand) ---
    // Mint (or refresh) an entry for `element_id` bound to `sub_shape` within
    // `body_shape`. Computes the TopoKey (ordinal in body_shape) + descriptor.
    // Returns the DeltaEntry describing the binding (caller decides added vs
    // relabeled). No-op-safe: minting an existing id refreshes its binding.
    DeltaEntry mint(const std::string& body_id, const std::string& element_id,
                    km::ElementKind kind, const TopoDS_Shape& sub_shape,
                    const TopoDS_Shape& body_shape, nlohmann::json anchor = {});

    // --- history application ---
    // Apply OCCT history from `hist` to every entry of `body_id`, rebinding to the
    // shape's image in `new_body_shape` and recomputing its TopoKey. Appends
    // removed/relabeled to `delta`. Entries whose old shape is IsDeleted (or whose
    // image is not in the new body) are removed. `unresolved_out` collects the
    // elementIds that history could not rebind (candidate NeedsRepair — W-WP6
    // scoring; here surfaced as reason "no-candidates").
    void apply_history(const std::string& body_id, const TopoDS_Shape& new_body_shape,
                       BRepBuilderAPI_MakeShape& hist, ElementMapDelta& delta,
                       std::vector<std::string>* unresolved_out = nullptr);

    // Drop every entry of a body that was consumed/deleted (e.g. a boolean tool);
    // appends each removed elementId to `delta.removed`.
    void remove_body(const std::string& body_id, ElementMapDelta& delta);

    // --- evidence helpers (stateless; SCHEMA §7.5/§10) ---
    // Resolve a TopoKey to its sub-shape within a body shape (null if absent).
    static TopoDS_Shape shape_for_topokey(const TopoDS_Shape& body_shape,
                                          const std::string& topo_key);
    // The TopoKey ("f:N"/…) of `sub_shape` within `body_shape` ("" if not found).
    static std::string topokey_for_shape(const TopoDS_Shape& body_shape,
                                         const TopoDS_Shape& sub_shape, km::ElementKind kind);
    // The sub-shape of `kind` in `body_shape` whose bbox center is nearest
    // `world` (anchor-based mint fallback; null if the body has none of `kind`).
    static TopoDS_Shape nearest_subshape(const TopoDS_Shape& body_shape, km::ElementKind kind,
                                         double wx, double wy, double wz);
    // Kernel descriptor of a shape (REUSED VERBATIM via a throwaway ElementMap).
    static km::ElementDescriptor describe(const TopoDS_Shape& shape);
    // Descriptor → JSON evidence (SCHEMA §10 fields), for QueryElement/ResolveRefs.
    static nlohmann::json descriptor_to_json(const km::ElementDescriptor& d);
    // "face"|"edge"|"vertex"|"body"|"unknown".
    static std::string kind_name(km::ElementKind kind);
    static km::ElementKind kind_from_name(const std::string& s);

private:
    std::map<std::string, PartitionEntry> entries_;  // elementId → entry (sorted)
};

}  // namespace onecad::elementmap
