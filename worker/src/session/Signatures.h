// Signatures.h — the THREE per-step topology signatures (SCHEMA §12).
//
// All three are 64-bit FNV-1a hex strings (`signatureVersion = 1`). Counts alone
// cannot detect a symmetric ElementId swap, so `referencedBinding` is carried
// separately (SCHEMA §12 rationale).
//
//   * geometry          — over per-body OCCT metrics (face/edge/vertex counts,
//                         quantized bbox, quantized volume via BRepGProp), for
//                         every body in the step's state (sorted by BodyId ⇒
//                         deterministic). W-WP5: computed from the real
//                         TopoDS_Shape (session/ShapeMetrics.*), superseding the
//                         W-WP4 StubBody counts. The metrics are threading-
//                         independent, so the signature is identical in
//                         `determinism` and `fast` session modes (Invariant 5).
//   * bodyLifecycle     — over the ordered create/modify/delete/… events of the
//                         step.
//   * referencedBinding — over the (refId → ElementId) bindings the step resolved
//                         for its referenced inputs.
//
// Quantization step is 1e-6 (llround(value / 1e-6)), matching the descriptor
// quantization (SCHEMA §10, quantizationVersion = 1).
#pragma once

#include <cmath>
#include <cstdint>
#include <string>
#include <vector>

#include "session/BodyStore.h"
#include "util/Hashing.h"

namespace onecad::session {

// One body lifecycle event of a step (SCHEMA §7.2 `bodyEvents[]`).
// kind ∈ "created" | "modified" | "deleted" | "split" | "merged".
struct BodyEvent {
    std::string kind;
    std::string body_id;
};

// One resolved reference binding (refId → ElementId) produced by a step.
struct RefBinding {
    std::string ref_id;
    std::string element_id;
};

inline std::int64_t quantize(double v) {
    // Normalize -0.0 → 0 (SCHEMA §4) before quantizing.
    if (v == 0.0) v = 0.0;
    return std::llround(v / 1e-6);
}

inline std::uint64_t fold_u64(std::uint64_t h, std::uint64_t v) {
    unsigned char bytes[8];
    for (int i = 0; i < 8; ++i) bytes[i] = static_cast<unsigned char>((v >> (i * 8)) & 0xff);
    return hashing::fnv1a_update(h, bytes, 8);
}

inline std::uint64_t fold_i64(std::uint64_t h, std::int64_t v) {
    return fold_u64(h, static_cast<std::uint64_t>(v));
}

inline std::uint64_t fold_str(std::uint64_t h, const std::string& s) {
    h = fold_u64(h, s.size());
    return hashing::fnv1a_update(h, s.data(), s.size());
}

// geometry signature — over every body in the store (sorted id order). Folds each
// body's OCCT metrics (counts + quantized bbox + quantized volume). Defined in
// Signatures.cpp because it inspects the TopoDS_Shape (needs OCCT).
std::string geometry_signature(const BodyStore& bodies);

// bodyLifecycle signature — over the ordered step events.
inline std::string body_lifecycle_signature(const std::vector<BodyEvent>& events) {
    std::uint64_t h = hashing::kFnvOffset;
    for (const auto& e : events) {
        h = fold_str(h, e.kind);
        h = fold_str(h, e.body_id);
    }
    return hashing::hex16(h);
}

// referencedBinding signature — over the ordered (refId → ElementId) bindings.
inline std::string referenced_binding_signature(const std::vector<RefBinding>& bindings) {
    std::uint64_t h = hashing::kFnvOffset;
    for (const auto& b : bindings) {
        h = fold_str(h, b.ref_id);
        h = fold_str(h, b.element_id);
    }
    return hashing::hex16(h);
}

}  // namespace onecad::session
