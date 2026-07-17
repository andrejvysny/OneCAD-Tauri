// SolverLane.cpp — see SolverLane.h. SCHEMA §7.4 solver-lane verb handlers.
#include "protocol/SolverLane.h"

#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstring>
#include <optional>
#include <utility>

#include "loop/LoopDetector.h"
#include "loop/RegionUtils.h"
#include "sketch/RegionId.h"
#include "sketch/SketchPoint.h"

namespace onecad::protocol {

namespace sk = onecad::core::sketch;
namespace loop = onecad::core::loop;
using nlohmann::json;

namespace {

constexpr double kPosEpsilon = 1e-7;  // "changed point" threshold (mm)

Envelope err(const Envelope& req, const char* code, const std::string& msg) {
    // §8: OP_FAILED / REF_UNRESOLVED are recoverable (session intact); retriable
    // is false — these are input/state errors, not transient failures.
    return Envelope::error_response(req.id,
                                    ErrorInfo{code, msg, /*retriable=*/false});
}

bool read_target(const json& p, double& x, double& y) {
    if (!p.contains("target")) return false;
    const json& t = p["target"];
    if (!t.is_array() || t.size() < 2 || !t[0].is_number() || !t[1].is_number()) return false;
    x = t[0].get<double>();
    y = t[1].get<double>();
    return std::isfinite(x) && std::isfinite(y);
}

std::uint64_t u64(const json& p, const char* key) {
    if (p.is_object() && p.contains(key) && p[key].is_number()) return p[key].get<std::uint64_t>();
    return 0;
}

// Snapshot every Point entity's position by internal id.
std::unordered_map<sk::EntityID, std::pair<double, double>> collect_positions(
    const sk::Sketch& sketch) {
    std::unordered_map<sk::EntityID, std::pair<double, double>> out;
    for (const auto& e : sketch.getAllEntities()) {
        if (e && e->type() == sk::EntityType::Point) {
            const auto* p = dynamic_cast<const sk::SketchPoint*>(e.get());
            if (p) out[p->id()] = {p->position().X(), p->position().Y()};
        }
    }
    return out;
}

// {handle: [x,y]} for points whose position moved beyond eps vs `prev`.
json changed_positions(const std::unordered_map<sk::EntityID, std::pair<double, double>>& prev,
                       const std::unordered_map<sk::EntityID, std::pair<double, double>>& cur,
                       const wire::WireIndex& index) {
    json out = json::object();
    for (const auto& [id, pos] : cur) {
        auto it = prev.find(id);
        const bool moved =
            it == prev.end() ||
            std::abs(pos.first - it->second.first) > kPosEpsilon ||
            std::abs(pos.second - it->second.second) > kPosEpsilon;
        if (!moved) continue;
        const std::string handle = index.handle_for(id);
        if (!handle.empty()) out[handle] = json::array({pos.first, pos.second});
    }
    return out;
}

std::vector<std::string> map_conflicting(const wire::WireIndex& index,
                                         const std::vector<sk::ConstraintID>& internal) {
    std::vector<std::string> out;
    out.reserve(internal.size());
    for (const auto& cid : internal) {
        auto it = index.internal_constraint_to_wire.find(cid);
        out.push_back(it != index.internal_constraint_to_wire.end() ? it->second : cid);
    }
    return out;
}

// SketchUpsert state ∈ UnderConstrained|FullyConstrained|OverConstrained|Conflicting.
// The ported PlaneGCS wrapper distinguishes genuine conflicts and DOF; benign
// redundancy is DOF-preserving (corpus g), so OverConstrained is not surfaced
// here (documented deviation) — only the three states below.
std::string upsert_state(int dof, bool conflicting) {
    if (conflicting) return "Conflicting";
    if (dof == 0) return "FullyConstrained";
    return "UnderConstrained";
}

// --- preview triangulation (ear clipping over the outer loop polygon) --------

std::string strip_seg(const std::string& id) {
    const std::size_t at = id.find("#seg");
    return at == std::string::npos ? id : id.substr(0, at);
}

double signed_area(const std::vector<sk::Vec2d>& poly) {
    double a = 0.0;
    for (std::size_t i = 0, n = poly.size(); i < n; ++i) {
        const auto& p = poly[i];
        const auto& q = poly[(i + 1) % n];
        a += p.x * q.y - q.x * p.y;
    }
    return 0.5 * a;
}

bool point_in_triangle(double px, double py, const sk::Vec2d& a, const sk::Vec2d& b,
                       const sk::Vec2d& c) {
    const double d1 = (px - b.x) * (a.y - b.y) - (a.x - b.x) * (py - b.y);
    const double d2 = (px - c.x) * (b.y - c.y) - (b.x - c.x) * (py - c.y);
    const double d3 = (px - a.x) * (c.y - a.y) - (c.x - a.x) * (py - a.y);
    const bool has_neg = (d1 < 0) || (d2 < 0) || (d3 < 0);
    const bool has_pos = (d1 > 0) || (d2 > 0) || (d3 > 0);
    return !(has_neg && has_pos);
}

// Ear-clip a simple polygon (assumed non-self-intersecting). Emits triangle
// index triples into the polygon's own vertex list. Holes are NOT subtracted
// (documented V1 limitation — see SolverLane region docs).
std::vector<std::uint32_t> ear_clip(const std::vector<sk::Vec2d>& poly_in) {
    std::vector<std::uint32_t> tris;
    std::vector<sk::Vec2d> poly = poly_in;
    // Drop a trailing point coincident with the first.
    if (poly.size() >= 2) {
        const auto& f = poly.front();
        const auto& l = poly.back();
        if (std::abs(f.x - l.x) < 1e-12 && std::abs(f.y - l.y) < 1e-12) poly.pop_back();
    }
    const std::size_t n = poly.size();
    if (n < 3) return tris;

    std::vector<std::uint32_t> v(n);
    for (std::size_t i = 0; i < n; ++i) v[i] = static_cast<std::uint32_t>(i);
    if (signed_area(poly) < 0.0) std::reverse(v.begin(), v.end());  // force CCW

    std::size_t guard = 0;
    const std::size_t guard_max = n * n + 8;
    while (v.size() > 2 && guard++ < guard_max) {
        bool clipped = false;
        const std::size_t m = v.size();
        for (std::size_t i = 0; i < m; ++i) {
            const std::uint32_t ia = v[(i + m - 1) % m];
            const std::uint32_t ib = v[i];
            const std::uint32_t ic = v[(i + 1) % m];
            const sk::Vec2d& a = poly[ia];
            const sk::Vec2d& b = poly[ib];
            const sk::Vec2d& c = poly[ic];
            const double cross = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            if (cross <= 0.0) continue;  // reflex or degenerate (CCW convex ear needs >0)
            bool contains = false;
            for (std::size_t k = 0; k < m; ++k) {
                const std::uint32_t iv = v[k];
                if (iv == ia || iv == ib || iv == ic) continue;
                if (point_in_triangle(poly[iv].x, poly[iv].y, a, b, c)) {
                    contains = true;
                    break;
                }
            }
            if (contains) continue;
            tris.push_back(ia);
            tris.push_back(ib);
            tris.push_back(ic);
            v.erase(v.begin() + static_cast<long>(i));
            clipped = true;
            break;
        }
        if (!clipped) break;  // no ear found (degenerate) — stop
    }
    return tris;
}

void append_f32(std::vector<std::uint8_t>& buf, float f) {
    std::uint8_t tmp[4];
    std::memcpy(tmp, &f, 4);  // host is little-endian (asserted at startup)
    buf.insert(buf.end(), tmp, tmp + 4);
}

void append_u32(std::vector<std::uint8_t>& buf, std::uint32_t u) {
    std::uint8_t tmp[4];
    std::memcpy(tmp, &u, 4);
    buf.insert(buf.end(), tmp, tmp + 4);
}

// Map a loop's internal edge ids to wire ids (dedup consecutive #seg splits).
std::vector<std::string> loop_wire_edges(const loop::Loop& lp, const wire::WireIndex& index) {
    std::vector<std::string> out;
    for (const auto& e : lp.wire.edges) {
        const std::string base = strip_seg(e);
        auto it = index.internal_edge_to_wire.find(base);
        const std::string wid = it != index.internal_edge_to_wire.end() ? it->second : base;
        if (out.empty() || out.back() != wid) out.push_back(wid);
    }
    return out;
}

}  // namespace

// --- verb registration ------------------------------------------------------

void SolverLane::register_verbs(Dispatcher& dispatcher) {
    dispatcher.register_solver_verb(
        "SketchUpsert",
        [this](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return on_upsert(r);
        });
    dispatcher.register_solver_verb(
        "BeginGesture",
        [this](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return on_begin(r);
        });
    dispatcher.register_solver_verb(
        "SolveDrag",
        [this](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return on_drag(r);
        });
    dispatcher.register_solver_verb(
        "EndGesture",
        [this](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return on_end(r);
        });
    dispatcher.register_solver_verb(
        "SketchRegions",
        [this](const Envelope& r, const std::vector<std::uint8_t>&, HandlerContext&) {
            return on_regions(r);
        });
}

// --- SketchUpsert -----------------------------------------------------------

Envelope SolverLane::on_upsert(const Envelope& req) {
    const json& args = req.args;
    const std::string sketch_id = args.value("sketchId", std::string{});
    if (sketch_id.empty()) return err(req, "OP_FAILED", "SketchUpsert: missing sketchId");

    wire::TranslateResult tr = wire::translate(args);
    if (!tr.ok) return err(req, "OP_FAILED", "SketchUpsert: " + tr.error);

    tr.sketch->solve();  // full solve so dof/state reflect the solved system
    const int dof = tr.sketch->getDegreesOfFreedom();
    const auto conflicting = tr.sketch->getConflictingConstraints();
    const std::string state = upsert_state(dof, !conflicting.empty());

    const std::uint64_t revision = store_.upsert(sketch_id, args);

    json result = {
        {"upserted", true},
        {"sketchId", sketch_id},
        {"sketchRevision", revision},
        {"dof", dof},
        {"state", state},
    };
    return Envelope::ok_response(req.id, std::move(result));
}

// --- BeginGesture -----------------------------------------------------------

Envelope SolverLane::on_begin(const Envelope& req) {
    const json& args = req.args;
    const std::string sketch_id = args.value("sketchId", std::string{});
    const std::uint64_t gesture_id = u64(args, "gestureId");

    std::optional<session::StoredSketch> stored = store_.snapshot(sketch_id);
    if (!stored) return err(req, "REF_UNRESOLVED", "BeginGesture: unknown sketch " + sketch_id);
    if (args.contains("sketchRevision") &&
        u64(args, "sketchRevision") != stored->revision) {
        return err(req, "REF_UNRESOLVED", "BeginGesture: stale sketchRevision");
    }

    wire::TranslateResult tr = wire::translate(stored->wire_args);
    if (!tr.ok) return err(req, "OP_FAILED", "BeginGesture: " + tr.error);

    // Resolve the drag point handle ("drag":{"pointId"} or "pointId").
    std::string point_id;
    if (args.contains("drag") && args["drag"].is_object()) {
        point_id = args["drag"].value("pointId", std::string{});
    }
    if (point_id.empty()) point_id = args.value("pointId", std::string{});
    sk::EntityID drag_internal;
    {
        auto it = tr.index.handle_to_point.find(point_id);
        if (it != tr.index.handle_to_point.end()) drag_internal = it->second;
    }
    if (drag_internal.empty()) {
        return err(req, "REF_UNRESOLVED", "BeginGesture: unknown drag point '" + point_id + "'");
    }

    // Build + diagnose the GCS system ONCE.
    tr.sketch->solve();
    const int dof = tr.sketch->getDegreesOfFreedom();
    const auto conflicting_internal = tr.sketch->getConflictingConstraints();

    tr.sketch->beginPointDrag(drag_internal);  // drag-fix strategy + rollback snapshot

    Gesture g;
    g.id = gesture_id;
    g.sketch_id = sketch_id;
    g.sketch_revision = stored->revision;
    g.drag_point = drag_internal;
    g.dof = dof;
    g.conflicting = map_conflicting(tr.index, conflicting_internal);
    g.baseline = collect_positions(*tr.sketch);
    g.last_reported = g.baseline;
    g.sketch = std::move(tr.sketch);
    g.index = std::move(tr.index);
    gestures_[gesture_id] = std::move(g);

    json result = {{"gestureId", gesture_id}, {"ready", true}};
    return Envelope::ok_response(req.id, std::move(result));
}

// --- SolveDrag --------------------------------------------------------------

Envelope SolverLane::on_drag(const Envelope& req) {
    const json& args = req.args;
    const std::uint64_t gesture_id = u64(args, "gestureId");
    const std::uint64_t seq = u64(args, "seq");

    auto git = gestures_.find(gesture_id);
    if (git == gestures_.end()) {
        return err(req, "REF_UNRESOLVED", "SolveDrag: unknown or ended gesture");
    }
    Gesture& g = git->second;

    double tx, ty;
    if (!read_target(args, tx, ty)) return err(req, "OP_FAILED", "SolveDrag: invalid target");

    const auto before = g.last_reported;  // deltas are reported incrementally
    const auto t0 = std::chrono::steady_clock::now();
    sk::SolveResult r = g.sketch->solveWithDrag(g.drag_point, sk::Vec2d{tx, ty});
    const auto t1 = std::chrono::steady_clock::now();
    const auto solve_micros =
        std::chrono::duration_cast<std::chrono::microseconds>(t1 - t0).count();

    const auto cur = collect_positions(*g.sketch);

    std::string status;
    std::vector<std::string> conflicting;
    if (!r.conflictingConstraints.empty()) {
        status = "conflicting";
        conflicting = map_conflicting(g.index, r.conflictingConstraints);
    } else if (!g.conflicting.empty()) {
        status = "conflicting";
        conflicting = g.conflicting;
    } else if (r.success) {
        status = "success";
    } else {
        status = "partial";
    }

    json positions = changed_positions(before, cur, g.index);
    g.last_reported = cur;
    g.last_success = r.success;

    json result = {
        {"gestureId", gesture_id},
        {"seq", seq},
        {"status", status},
        {"dof", g.dof},
        {"conflicting", conflicting},
        {"positions", std::move(positions)},
        {"solveMicros", solve_micros},
    };
    return Envelope::ok_response(req.id, std::move(result));
}

// --- EndGesture -------------------------------------------------------------

Envelope SolverLane::on_end(const Envelope& req) {
    const json& args = req.args;
    const std::uint64_t gesture_id = u64(args, "gestureId");

    auto git = gestures_.find(gesture_id);
    if (git == gestures_.end()) {
        return err(req, "REF_UNRESOLVED", "EndGesture: unknown or ended gesture");
    }
    Gesture g = std::move(git->second);
    gestures_.erase(git);

    // Final EXACT solve: to the committed pointer-up target if provided, else a
    // plain full solve. endPointDrag() rolls back to the drag-start pose on a
    // failed gesture (rollback determinism, corpus g).
    sk::SolveResult r;
    bool did_final_drag = false;
    if (args.contains("commit") && args["commit"].is_object() &&
        args["commit"].contains("finalTarget")) {
        const json ft = args["commit"]["finalTarget"];
        if (ft.is_array() && ft.size() >= 2 && ft[0].is_number() && ft[1].is_number()) {
            r = g.sketch->solveWithDrag(g.drag_point, sk::Vec2d{ft[0].get<double>(),
                                                                ft[1].get<double>()});
            did_final_drag = true;
        }
    }
    g.sketch->endPointDrag();
    if (!did_final_drag) r = g.sketch->solve();

    const int dof = g.dof;
    const auto cur = collect_positions(*g.sketch);

    std::string status;
    if (!r.conflictingConstraints.empty() || !g.conflicting.empty()) status = "conflicting";
    else if (r.success) status = "success";
    else status = "partial";

    // Commit into the session store: bump revision + write back solved positions.
    const std::uint64_t new_rev = g.sketch_revision + 1;
    if (std::optional<session::StoredSketch> s = store_.snapshot(g.sketch_id)) {
        wire::apply_solved_positions(s->wire_args, *g.sketch, g.index);
        store_.put(g.sketch_id, std::move(s->wire_args), new_rev);
    }

    json result = {
        {"gestureId", gesture_id},
        {"status", status},
        {"dof", dof},
        {"positions", changed_positions(g.baseline, cur, g.index)},
        {"sketchRevision", new_rev},
    };
    return Envelope::ok_response(req.id, std::move(result));
}

// --- SketchRegions ----------------------------------------------------------

Envelope SolverLane::on_regions(const Envelope& req) {
    const json& args = req.args;
    const std::string sketch_id = args.value("sketchId", std::string{});
    std::optional<session::StoredSketch> stored = store_.snapshot(sketch_id);
    if (!stored) return err(req, "REF_UNRESOLVED", "SketchRegions: unknown sketch " + sketch_id);

    wire::TranslateResult tr = wire::translate(stored->wire_args);
    if (!tr.ok) return err(req, "OP_FAILED", "SketchRegions: " + tr.error);

    loop::LoopDetector detector;
    detector.setConfig(loop::makeRegionDetectionConfig());
    const loop::LoopDetectionResult det = detector.detect(*tr.sketch);

    json regions = json::array();
    std::vector<std::uint8_t> tail;
    json bin_sections = json::array();

    // One region per detected FACE (outer loop + hole loops) — matches the
    // LoopDetector face semantics (corpus i: square-with-hole => 1 region, 1 hole).
    for (const auto& face : det.faces) {
        const std::vector<std::string> outer = loop_wire_edges(face.outerLoop, tr.index);
        const std::string region_id =
            onecad::region::derive_region_id(outer, onecad::region::Winding::Ccw);

        json holes = json::array();
        for (const auto& hole : face.innerLoops) {
            holes.push_back(loop_wire_edges(hole, tr.index));
        }

        // previewTriangles: f32 xyz positions (z = 0, sketch-local) then u32
        // indices. Holes are NOT subtracted from the fill (V1 limitation).
        const std::vector<sk::Vec2d>& poly = face.outerLoop.polygon;
        const std::vector<std::uint32_t> indices = ear_clip(poly);
        const std::size_t vertex_count =
            (poly.size() >= 2 && std::abs(poly.front().x - poly.back().x) < 1e-12 &&
             std::abs(poly.front().y - poly.back().y) < 1e-12)
                ? poly.size() - 1
                : poly.size();

        const std::uint64_t off = tail.size();
        for (std::size_t i = 0; i < vertex_count; ++i) {
            append_f32(tail, static_cast<float>(poly[i].x));
            append_f32(tail, static_cast<float>(poly[i].y));
            append_f32(tail, 0.0f);
        }
        for (std::uint32_t idx : indices) append_u32(tail, idx);
        const std::uint64_t len = tail.size() - off;

        const std::string section = "region:" + region_id;
        bin_sections.push_back({{"name", section}, {"off", off}, {"len", len}});

        json region = {
            {"regionId", region_id},
            {"outerLoop", outer},
            {"holes", holes},
            {"previewTriangles",
             {{"format", "f32xyz+u32idx"},
              {"vertexCount", vertex_count},
              {"triangleCount", indices.size() / 3},
              {"bin", section}}},
        };
        regions.push_back(std::move(region));
    }

    json result = {
        {"sketchId", sketch_id},
        {"sketchRevision", stored->revision},
        {"regions", std::move(regions)},
    };
    Envelope resp = Envelope::ok_response(req.id, std::move(result));
    // Attach the binary tail + section table.
    for (const auto& s : bin_sections) {
        resp.bin.push_back(BinSection{s["name"].get<std::string>(),
                                      s["off"].get<std::uint64_t>(),
                                      s["len"].get<std::uint64_t>()});
    }
    resp.out_bin = std::move(tail);
    return resp;
}

}  // namespace onecad::protocol
