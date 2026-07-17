// Signatures.cpp — the geometry signature (OCCT-backed). See Signatures.h.
#include "session/Signatures.h"

#include "session/ShapeMetrics.h"

namespace onecad::session {

std::string geometry_signature(const BodyStore& bodies) {
    std::uint64_t h = hashing::kFnvOffset;
    // bodies.all() iterates in ascending BodyId order (std::map), so the fold is
    // deterministic regardless of insertion order.
    for (const auto& [id, rec] : bodies.all()) {
        const ShapeMetrics m = compute_shape_metrics(rec.geom);
        h = fold_str(h, id);
        h = fold_u64(h, m.face_count);
        h = fold_u64(h, m.edge_count);
        h = fold_u64(h, m.vertex_count);
        for (double c : m.bbox_min) h = fold_i64(h, quantize(c));
        for (double c : m.bbox_max) h = fold_i64(h, quantize(c));
        h = fold_i64(h, quantize(m.volume));
    }
    return hashing::hex16(h);
}

}  // namespace onecad::session
