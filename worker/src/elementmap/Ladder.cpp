// Ladder.cpp — see Ladder.h. Descriptor+anchor stage: score → assign → gate.
#include "elementmap/Ladder.h"

#include <algorithm>
#include <cmath>
#include <cstdio>
#include <numeric>

#include <BRepBndLib.hxx>
#include <Bnd_Box.hxx>
#include <GeomAbs_CurveType.hxx>
#include <GeomAbs_SurfaceType.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>

#include "elementmap/Assignment.h"
#include "elementmap/ElementMapPartition.h"

namespace onecad::elementmap {

namespace {

TopAbs_ShapeEnum topabs_of(km::ElementKind kind) {
    switch (kind) {
        case km::ElementKind::Face: return TopAbs_FACE;
        case km::ElementKind::Edge: return TopAbs_EDGE;
        case km::ElementKind::Vertex: return TopAbs_VERTEX;
        default: return TopAbs_SHAPE;
    }
}

char topokey_prefix(km::ElementKind kind) {
    switch (kind) {
        case km::ElementKind::Face: return 'f';
        case km::ElementKind::Edge: return 'e';
        case km::ElementKind::Vertex: return 'v';
        default: return '?';
    }
}

double body_diagonal(const TopoDS_Shape& body_shape) {
    if (body_shape.IsNull()) return 1.0;
    Bnd_Box box;
    BRepBndLib::Add(body_shape, box);
    if (box.IsVoid()) return 1.0;
    Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
    box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
    const double dx = xmax - xmin, dy = ymax - ymin, dz = zmax - zmin;
    const double diag = std::sqrt(dx * dx + dy * dy + dz * dz);
    return diag > 1e-9 ? diag : 1.0;
}

std::string surface_type_name(GeomAbs_SurfaceType t) {
    switch (t) {
        case GeomAbs_Plane: return "planar";
        case GeomAbs_Cylinder: return "cylindrical";
        case GeomAbs_Cone: return "conical";
        case GeomAbs_Sphere: return "spherical";
        case GeomAbs_Torus: return "toroidal";
        default: return "curved";
    }
}

std::string curve_type_name(GeomAbs_CurveType t) {
    switch (t) {
        case GeomAbs_Line: return "line";
        case GeomAbs_Circle: return "circular";
        case GeomAbs_Ellipse: return "elliptical";
        default: return "spline";
    }
}

std::string candidate_summary(km::ElementKind kind, const km::ElementDescriptor& d) {
    char buf[96];
    if (kind == km::ElementKind::Face) {
        std::snprintf(buf, sizeof(buf), "%s face, area~%.0fmm2", surface_type_name(d.surfaceType).c_str(),
                      d.magnitude);
    } else if (kind == km::ElementKind::Edge) {
        std::snprintf(buf, sizeof(buf), "%s edge, len~%.1fmm", curve_type_name(d.curveType).c_str(),
                      d.magnitude);
    } else {
        std::snprintf(buf, sizeof(buf), "vertex at (%.1f,%.1f,%.1f)", d.center.X(), d.center.Y(),
                      d.center.Z());
    }
    return std::string(buf);
}

// The enumerated candidate pool for one element kind of a body.
struct CandidatePool {
    std::vector<TopoDS_Shape> shapes;
    std::vector<std::string> topo_keys;
    std::vector<km::ElementDescriptor> descriptors;
};

CandidatePool enumerate_candidates(const TopoDS_Shape& body_shape, km::ElementKind kind) {
    CandidatePool pool;
    const TopAbs_ShapeEnum type = topabs_of(kind);
    if (body_shape.IsNull() || type == TopAbs_SHAPE) return pool;
    TopTools_IndexedMapOfShape map;
    TopExp::MapShapes(body_shape, type, map);
    const char prefix = topokey_prefix(kind);
    for (int i = 1; i <= map.Extent(); ++i) {
        pool.shapes.push_back(map(i));
        pool.topo_keys.push_back(std::string(1, prefix) + ":" + std::to_string(i));
        pool.descriptors.push_back(ElementMapPartition::describe(map(i)));
    }
    return pool;
}

}  // namespace

nlohmann::json LadderResolution::to_needs_repair_json() const {
    nlohmann::json cands = nlohmann::json::array();
    for (const LadderCandidate& c : candidates) {
        nlohmann::json contrib = nlohmann::json::object();
        for (const auto& [k, v] : c.contributions) contrib[k] = v;
        cands.push_back(nlohmann::json{
            {"topoKey", c.topo_key},
            {"score", c.score},
            {"margin", c.margin},
            {"worldPos", {c.world_pos.X(), c.world_pos.Y(), c.world_pos.Z()}},
            {"summary", c.summary},
            {"featureContributions", std::move(contrib)},
        });
    }
    return nlohmann::json{
        {"refId", ref_id},
        {"elementId", element_id},
        {"ladderFailed", "descriptor"},  // this stage (history handled upstream)
        {"reason", reason},
        {"scoringVersion", kResolverVersion},
        {"candidates", std::move(cands)},
        {"anchor", anchor_json.is_null() ? nlohmann::json::object() : anchor_json},
        {"uiLabel", ui_label},
    };
}

std::vector<LadderResolution> resolve_descriptor_stage(const TopoDS_Shape& body_shape,
                                                       const std::string& body_id,
                                                       const std::vector<LadderRef>& refs) {
    (void)body_id;  // evidence label only
    std::vector<LadderResolution> out(refs.size());
    const double body_diag = body_diagonal(body_shape);

    // Group ref indices by element kind (disjoint candidate pools).
    std::map<km::ElementKind, std::vector<std::size_t>> by_kind;
    for (std::size_t i = 0; i < refs.size(); ++i) by_kind[refs[i].kind].push_back(i);

    for (const auto& [kind, idxs] : by_kind) {
        const CandidatePool pool = enumerate_candidates(body_shape, kind);
        const int c = static_cast<int>(pool.shapes.size());
        const int n = static_cast<int>(idxs.size());

        // Score matrix + kept per-candidate evidence, per ref of this kind.
        std::vector<std::vector<double>> score(n, std::vector<double>(std::max(c, 1), 0.0));
        std::vector<std::vector<std::map<std::string, double>>> contribs(n);
        for (int i = 0; i < n; ++i) {
            const LadderRef& r = refs[idxs[i]];
            contribs[i].resize(std::max(c, 0));
            for (int j = 0; j < c; ++j) {
                const ScoreResult s = score_candidate(r.descriptor, r.has_descriptor, r.anchor,
                                                      pool.descriptors[j], body_diag);
                score[i][j] = s.score;
                contribs[i][j] = s.contributions;
            }
        }

        // Optimal distinct assignment (pad columns to ≥ n with dummy score-0 cols so
        // it is always solvable; a ref landing on a dummy has no real candidate).
        std::vector<int> assignment;
        if (n > 0) {
            const int cols = std::max(n, c);
            std::vector<std::vector<double>> cost(n, std::vector<double>(cols, 1.0));
            for (int i = 0; i < n; ++i)
                for (int j = 0; j < c; ++j) cost[i][j] = 1.0 - score[i][j];
            assignment = min_cost_assignment(cost);
        }

        for (int i = 0; i < n; ++i) {
            LadderResolution& res = out[idxs[i]];
            const LadderRef& r = refs[idxs[i]];
            res.ref_id = r.ref_id;
            res.element_id = r.element_id;
            res.kind = kind;
            res.anchor_json = r.anchor_json;
            res.ui_label = r.ui_label.empty() ? ("unresolved ref " + r.ref_id) : r.ui_label;
            res.ladder_level = "descriptor";

            // Ranked candidate evidence (real candidates only, desc by score).
            std::vector<int> order(c);
            std::iota(order.begin(), order.end(), 0);
            std::sort(order.begin(), order.end(), [&](int a, int b) {
                if (score[i][a] != score[i][b]) return score[i][a] > score[i][b];
                return a < b;  // deterministic tie-break
            });
            const int keep = std::min(c, 5);
            for (int k = 0; k < keep; ++k) {
                const int j = order[k];
                LadderCandidate cand;
                cand.topo_key = pool.topo_keys[j];
                cand.shape = pool.shapes[j];
                cand.score = score[i][j];
                cand.margin = (k + 1 < c) ? (score[i][j] - score[i][order[k + 1]]) : score[i][j];
                cand.world_pos = pool.descriptors[j].center;
                cand.summary = candidate_summary(kind, pool.descriptors[j]);
                cand.contributions = contribs[i][j];
                res.candidates.push_back(std::move(cand));
            }

            const int aj = assignment.empty() ? -1 : assignment[i];
            if (c == 0 || aj < 0 || aj >= c) {
                res.outcome = LadderOutcome::NeedsRepair;
                res.reason = "no-candidates";
                continue;
            }
            const double assigned = score[i][aj];
            double runner_up = 0.0;
            for (int j = 0; j < c; ++j)
                if (j != aj) runner_up = std::max(runner_up, score[i][j]);
            const double margin = assigned - runner_up;

            if (assigned >= kAutoBindMinScore && margin >= kAutoBindMinMargin) {
                res.outcome = LadderOutcome::AutoBind;
                res.bound_shape = pool.shapes[aj];
                res.bound_topo_key = pool.topo_keys[aj];
                res.score = assigned;
                res.margin = margin;
            } else {
                res.outcome = LadderOutcome::NeedsRepair;
                res.reason = (margin < kAutoBindMinMargin) ? "ambiguous" : "low-confidence";
                res.score = assigned;
                res.margin = margin;
            }
        }
    }
    return out;
}

LadderRef ladder_ref_from_input(const nlohmann::json& input, const std::string& ref_id) {
    LadderRef r;
    r.ref_id = ref_id;
    if (input.contains("primary") && input["primary"].is_object()) {
        const nlohmann::json& pr = input["primary"];
        if (pr.contains("elementId") && pr["elementId"].is_string())
            r.element_id = pr["elementId"].get<std::string>();
        if (pr.contains("kind") && pr["kind"].is_string())
            r.kind = ElementMapPartition::kind_from_name(pr["kind"].get<std::string>());
    }
    // Frozen intent.descriptor (structured object → parsed evidence; a string
    // placeholder or absence ⇒ anchor-only).
    if (input.contains("intent") && input["intent"].is_object()) {
        const nlohmann::json& intent = input["intent"];
        if (r.kind == km::ElementKind::Unknown && intent.contains("kind") && intent["kind"].is_string())
            r.kind = ElementMapPartition::kind_from_name(intent["kind"].get<std::string>());
        if (intent.contains("descriptor") && intent["descriptor"].is_object()) {
            r.descriptor = ElementMapPartition::descriptor_from_json(intent["descriptor"]);
            r.has_descriptor = true;
        }
    }
    // Anchor (world point narrows a descriptor tie / is the sole evidence otherwise).
    if (input.contains("anchor") && input["anchor"].is_object()) {
        r.anchor_json = input["anchor"];
        const nlohmann::json& a = input["anchor"];
        if (a.contains("worldPoint") && a["worldPoint"].is_array() && a["worldPoint"].size() >= 3) {
            const nlohmann::json& wp = a["worldPoint"];
            if (wp[0].is_number() && wp[1].is_number() && wp[2].is_number()) {
                r.anchor.has_world_point = true;
                r.anchor.world_point =
                    gp_Pnt(wp[0].get<double>(), wp[1].get<double>(), wp[2].get<double>());
            }
        }
    }
    return r;
}

}  // namespace onecad::elementmap
