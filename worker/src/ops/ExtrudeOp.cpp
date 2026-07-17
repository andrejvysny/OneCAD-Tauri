// ExtrudeOp.cpp — see ExtrudeOp.h. Ports RegenerationEngine.cpp buildExtrude.
#include "ops/ExtrudeOp.h"

#include <algorithm>
#include <cmath>
#include <memory>
#include <optional>

#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBndLib.hxx>
#include <BRepPrimAPI_MakePrism.hxx>
#include <Bnd_Box.hxx>
#include <Standard_Failure.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Dir.hxx>
#include <gp_Pln.hxx>
#include <gp_Pnt.hxx>
#include <gp_Vec.hxx>

#include "modeling/BooleanMode.h"
#include "ops/OpCommon.h"

namespace onecad::ops {

using nlohmann::json;

namespace {

constexpr double kMinValue = 1e-6;   // RegenerationEngine kMinValue (degenerate guard)
constexpr double kThroughAllFallback = 1.0e5;  // RegenerationEngine.cpp:856

// primary.bodyId of the op's input at `index` (semantic ref, SCHEMA §7.3).
std::string input_body(const json& op, std::size_t index) {
    if (!op.contains("inputs") || !op["inputs"].is_array() || op["inputs"].size() <= index) return "";
    const json& in = op["inputs"][index];
    if (in.is_object() && in.contains("primary") && in["primary"].is_object()) {
        return read_str(in["primary"], "bodyId");
    }
    return "";
}

app::BooleanMode boolean_mode_of(const std::string& s) {
    if (s == "Add") return app::BooleanMode::Add;
    if (s == "Cut") return app::BooleanMode::Cut;
    if (s == "Intersect") return app::BooleanMode::Intersect;
    return app::BooleanMode::NewBody;
}

// Look up a sketch materialized earlier in this plan (params.sketchId, else the
// most-recently materialized one).
const json* find_sketch(const OpContext& ctx, const json& params) {
    std::string sid = read_str(params, "sketchId");
    if (sid.empty() && ctx.last_sketch_id) sid = *ctx.last_sketch_id;
    if (!ctx.sketches) return nullptr;
    for (const auto& [id, p] : *ctx.sketches) {
        if (id == sid) return &p;
    }
    return nullptr;
}

// ThroughAll extent: max projection of the boolean-target bbox corners onto refDir
// from the prism origin, + margin. Fallback constant when no target resolves.
// RegenerationEngine.cpp:826-856.
double through_all_distance(double blind_sign_source, const gp_Pnt& origin, const gp_Dir& ref_dir,
                            const TopoDS_Shape* target) {
    const double sign = blind_sign_source >= 0.0 ? 1.0 : -1.0;
    if (target && !target->IsNull()) {
        Bnd_Box box;
        BRepBndLib::Add(*target, box);
        if (!box.IsVoid()) {
            Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
            box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
            double max_proj = 0.0;
            for (int corner = 0; corner < 8; ++corner) {
                const gp_Pnt p((corner & 1) ? xmax : xmin, (corner & 2) ? ymax : ymin,
                               (corner & 4) ? zmax : zmin);
                max_proj = std::max(max_proj, gp_Vec(origin, p).Dot(gp_Vec(ref_dir)));
            }
            const double diag = gp_Pnt(xmin, ymin, zmin).Distance(gp_Pnt(xmax, ymax, zmax));
            return sign * (std::max(max_proj, kMinValue) + 0.01 * diag + 1.0);
        }
    }
    return sign * kThroughAllFallback;
}

TopoDS_Shape make_prism(const TopoDS_Shape& profile, const gp_Dir& dir, double signed_distance,
                        std::string& err) {
    gp_Vec vec(dir.X() * signed_distance, dir.Y() * signed_distance, dir.Z() * signed_distance);
    BRepPrimAPI_MakePrism prism(profile, vec, Standard_True);
    if (prism.Shape().IsNull()) {
        err = "Extrude prism produced null shape";
        return {};
    }
    return prism.Shape();
}

}  // namespace

OpOutcome execute_extrude(OpContext& ctx, const json& op, const std::string& op_id) {
    const json params =
        (op.contains("params") && op["params"].is_object()) ? op["params"] : json::object();

    const std::string mode_str = read_str(params, "extrudeMode", "Blind");
    const std::string mode2_str = read_str(params, "extrudeMode2", "Blind");
    const bool two_dirs = params.value("twoDirections", false);
    const std::string boolean_mode_str = read_str(params, "booleanMode", "NewBody");

    // Deferred end conditions (need typed-target ladder resolution — W-WP6).
    auto unsupported_mode = [](const std::string& m) { return m == "ToFace" || m == "ToNext"; };
    if (unsupported_mode(mode_str) || (two_dirs && unsupported_mode(mode2_str))) {
        return OpOutcome::unsupported(
            "Extrude ToFace/ToNext not supported this WP (needs ladder target resolution)");
    }
    if (std::abs(read_scalar(params, "draftAngleDeg", 0.0)) > 1e-9) {
        return OpOutcome::unsupported("Extrude draft angle not supported this WP");
    }

    // --- profile face ---
    const json* sketch_params = find_sketch(ctx, params);
    if (!sketch_params) {
        return OpOutcome::fail("REF_UNRESOLVED", "Extrude: profile sketch not found in plan");
    }
    std::string perr;
    std::optional<TopoDS_Face> profile =
        build_profile_face(*sketch_params, read_str(params, "regionId"), perr);
    if (!profile) return OpOutcome::fail("OP_FAILED", perr);

    gp_Pln plane;
    gp_Dir direction(0, 0, 1);
    if (!planar_face_plane_normal(*profile, plane, direction)) {
        return OpOutcome::fail("OP_FAILED", "Extrude: only planar profile faces supported");
    }
    const gp_Pnt origin = plane.Location();

    const app::BooleanMode boolean_mode = boolean_mode_of(boolean_mode_str);

    // Resolve the boolean target body id (explicit param, else input body ref).
    // RegenerationEngine.cpp:1590-1619 resolveBooleanTargetBodyId priority chain.
    std::string target_id = read_str(params, "targetBodyId");
    if (target_id.empty()) target_id = input_body(op, 0);
    const session::BodyRecord* target_rec =
        (boolean_mode != app::BooleanMode::NewBody) ? ctx.bodies.get(target_id) : nullptr;

    const double distance = read_scalar(params, "distance", 10.0);

    // Distance-driven guard (Blind/Symmetric single-direction): a zero distance is
    // invalid; ThroughAll computes its own extent (RegenerationEngine.cpp:784-789).
    const bool distance_driven =
        !two_dirs && (mode_str == "Blind" || mode_str == "Symmetric");
    if (distance_driven && std::abs(distance) < kMinValue) {
        return OpOutcome::fail("OP_FAILED", "Extrude distance too small");
    }

    if (ctx.cancel && ctx.cancel->cancelled()) return OpOutcome::cancelled();

    // --- build the extrude tool shape ---
    const TopoDS_Shape* target_shape = target_rec ? &target_rec->geom : nullptr;
    TopoDS_Shape tool_shape;
    std::string err;
    try {
        auto effective_distance = [&](const std::string& m, double blind,
                                      const gp_Dir& ref_dir) -> std::optional<double> {
            if (m == "Blind" || m == "Symmetric") return blind;
            if (m == "ThroughAll") return through_all_distance(blind, origin, ref_dir, target_shape);
            return std::nullopt;  // ToFace/ToNext already rejected above
        };

        if (two_dirs) {
            if (mode_str == "Symmetric" || mode2_str == "Symmetric") {
                return OpOutcome::fail("OP_FAILED", "Symmetric is not valid with two directions");
            }
            const gp_Dir dir2 = direction.Reversed();
            auto d1 = effective_distance(mode_str, distance, direction);
            auto d2 = effective_distance(mode2_str, read_scalar(params, "distance2", 0.0), dir2);
            if (!d1 || !d2) return OpOutcome::fail("OP_FAILED", "Extrude: bad end condition");
            TopoDS_Shape p1 = make_prism(*profile, direction, *d1, err);
            if (p1.IsNull()) return OpOutcome::fail("OP_FAILED", err);
            TopoDS_Shape p2 = make_prism(*profile, dir2, *d2, err);
            if (p2.IsNull()) return OpOutcome::fail("OP_FAILED", err);
            BRepAlgoAPI_Fuse fuse(p1, p2);
            fuse.Build();
            if (!fuse.IsDone()) return OpOutcome::fail("OP_FAILED", "Two-direction extrude fuse failed");
            tool_shape = fuse.Shape();
        } else if (mode_str == "Symmetric") {
            const double half = distance * 0.5;
            gp_Vec fwd(direction.X() * half, direction.Y() * half, direction.Z() * half);
            gp_Vec bwd = fwd.Reversed();
            BRepPrimAPI_MakePrism fwd_prism(*profile, fwd, Standard_True);
            BRepPrimAPI_MakePrism bwd_prism(*profile, bwd, Standard_True);
            if (fwd_prism.Shape().IsNull() || bwd_prism.Shape().IsNull()) {
                return OpOutcome::fail("OP_FAILED", "Symmetric extrude prism produced null shape");
            }
            BRepAlgoAPI_Fuse fuse(fwd_prism.Shape(), bwd_prism.Shape());
            fuse.Build();
            if (!fuse.IsDone()) return OpOutcome::fail("OP_FAILED", "Symmetric extrude fuse failed");
            tool_shape = fuse.Shape();
        } else {
            auto d1 = effective_distance(mode_str, distance, direction);
            if (!d1) return OpOutcome::fail("OP_FAILED", "Extrude: bad end condition");
            tool_shape = make_prism(*profile, direction, *d1, err);
            if (tool_shape.IsNull()) return OpOutcome::fail("OP_FAILED", err);
        }
    } catch (const Standard_Failure& f) {
        return OpOutcome::fail("OP_FAILED", std::string("Extrude failed: ") +
                                               (f.GetMessageString() ? f.GetMessageString() : "OCCT"));
    } catch (...) {
        return OpOutcome::fail("OP_FAILED", "Extrude failed");
    }

    OpOutcome out;

    // --- boolean mode dispatch ---
    if (boolean_mode == app::BooleanMode::NewBody) {
        const std::string bid = "body_" + op_id;
        ctx.bodies.create(bid, op_id, tool_shape);
        out.body_events.push_back({"created", bid});
        out.body_ids.push_back(bid);
        return out;  // new body: no pre-existing partition entries → empty delta
    }

    // Add / Cut / Intersect into the target body (BodyId preserved).
    if (target_id.empty()) {
        return OpOutcome::fail("OP_FAILED", "Extrude boolean requires a target body");
    }
    if (!target_rec) {
        return OpOutcome::fail("REF_UNRESOLVED", "Extrude target body not found: " + target_id);
    }
    if (ctx.cancel && ctx.cancel->cancelled()) return OpOutcome::cancelled();

    const TopoDS_Shape old_target = target_rec->geom;
    std::shared_ptr<BRepBuilderAPI_MakeShape> builder;
    BooleanResult br = checked_boolean(old_target, tool_shape, boolean_mode, ctx.parallel,
                                       ctx.occt_options, ctx.cancel, builder);
    if (br.error_code == "CANCELLED") return OpOutcome::cancelled();
    if (!br.error_code.empty()) return OpOutcome::fail(br.error_code, br.error_message);

    // Publish the modified target (id preserved) + rebind its partition via history.
    ctx.bodies.create(target_id, op_id, br.shape);
    if (builder) {
        std::vector<std::string> unresolved;
        ctx.partition.apply_history(target_id, br.shape, *builder, out.delta, &unresolved);
        for (const std::string& id : unresolved)
            out.needs_repair.push_back(make_no_candidates_repair(id, target_id));
    }
    out.body_events.push_back({"modified", target_id});
    out.body_ids.push_back(target_id);
    return out;
}

}  // namespace onecad::ops
