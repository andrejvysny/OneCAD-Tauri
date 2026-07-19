// RevolveOp.cpp — see RevolveOp.h. Ports RegenerationEngine.cpp buildRevolve.
#include "ops/RevolveOp.h"

#include <cmath>
#include <memory>
#include <optional>

#include <BRepAdaptor_Curve.hxx>
#include <BRepBuilderAPI_MakeShape.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <BRepPrimAPI_MakeRevol.hxx>
#include <GeomAbs_CurveType.hxx>
#include <Standard_Failure.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <gp_Ax1.hxx>
#include <gp_Dir.hxx>
#include <gp_Lin.hxx>
#include <gp_Pnt.hxx>

#include "elementmap/ElementMapPartition.h"
#include "modeling/BooleanMode.h"
#include "ops/OpCommon.h"
#include "sketch/Sketch.h"
#include "sketch/SketchLine.h"
#include "sketch/SketchPoint.h"
#include "sketch/WireSketch.h"

namespace onecad::ops {

using nlohmann::json;

namespace {

constexpr double kMinAngleDeg = 1e-3;  // RegenerationEngine.cpp:62 kMinAngleDeg

app::BooleanMode boolean_mode_of(const std::string& s) {
    if (s == "Add") return app::BooleanMode::Add;
    if (s == "Cut") return app::BooleanMode::Cut;
    if (s == "Intersect") return app::BooleanMode::Intersect;
    return app::BooleanMode::NewBody;
}

const json* find_sketch(const OpContext& ctx, const std::string& sid_in,
                        const std::string& fallback_last) {
    std::string sid = sid_in;
    if (sid.empty()) sid = fallback_last;
    if (!ctx.sketches) return nullptr;
    for (const auto& [id, p] : *ctx.sketches) {
        if (id == sid) return &p;
    }
    return nullptr;
}

std::string input_body(const json& op, std::size_t index) {
    if (!op.contains("inputs") || !op["inputs"].is_array() || op["inputs"].size() <= index) return "";
    const json& in = op["inputs"][index];
    if (in.is_object() && in.contains("primary") && in["primary"].is_object()) {
        // Only a whole-BODY ref is a valid boolean-target fallback — a face/edge ref
        // must never be mistaken for the operated body (M2 review hazard 6; mirrors
        // ExtrudeOp::input_body).
        if (read_str(in["primary"], "kind") != "body") return "";
        return read_str(in["primary"], "bodyId");
    }
    return "";
}

// Axis from a sketch line: map the line's 2D endpoints through the sketch plane
// into world space (RegenerationEngine.cpp:1134-1172). Returns false + fills `err`.
bool axis_from_sketch_line(const OpContext& ctx, const std::string& sketch_id,
                           const std::string& line_id, gp_Ax1& axis_out, std::string& err) {
    const json* sk_params = find_sketch(ctx, sketch_id, ctx.last_sketch_id ? *ctx.last_sketch_id : "");
    if (!sk_params) {
        err = "Revolve: axis sketch not found in plan";
        return false;
    }
    wire::TranslateResult tr = wire::translate(*sk_params);
    if (!tr.ok) {
        err = "Revolve: axis sketch translate failed: " + tr.error;
        return false;
    }
    tr.sketch->solve();
    auto it = tr.index.wire_to_internal.find(line_id);
    if (it == tr.index.wire_to_internal.end()) {
        err = "Revolve: axis line '" + line_id + "' not found in sketch";
        return false;
    }
    const auto* line = tr.sketch->getEntityAs<core::sketch::SketchLine>(it->second);
    if (!line) {
        err = "Revolve: axis reference '" + line_id + "' is not a line";
        return false;
    }
    const auto* sp = tr.sketch->getEntityAs<core::sketch::SketchPoint>(line->startPointId());
    const auto* ep = tr.sketch->getEntityAs<core::sketch::SketchPoint>(line->endPointId());
    if (!sp || !ep) {
        err = "Revolve: axis line has no endpoints";
        return false;
    }
    const core::sketch::Vec3d ws = tr.sketch->toWorld({sp->position().X(), sp->position().Y()});
    const core::sketch::Vec3d we = tr.sketch->toWorld({ep->position().X(), ep->position().Y()});
    const gp_Pnt origin(ws.x, ws.y, ws.z);
    const gp_Vec dir(we.x - ws.x, we.y - ws.y, we.z - ws.z);
    if (dir.Magnitude() < 1e-6) {
        err = "Revolve: degenerate axis line";
        return false;
    }
    axis_out = gp_Ax1(origin, gp_Dir(dir));
    return true;
}

// Axis from a straight body edge (RegenerationEngine.cpp:1173-1191).
bool axis_from_edge(const OpContext& ctx, const std::string& body_id, const std::string& edge_id,
                    gp_Ax1& axis_out, std::string& err) {
    const session::BodyRecord* rec = ctx.bodies.get(body_id);
    if (!rec) {
        err = "Revolve: axis edge body not found: " + body_id;
        return false;
    }
    const TopoDS_Shape sub = elementmap::ElementMapPartition::shape_for_topokey(rec->geom, edge_id);
    if (sub.IsNull() || sub.ShapeType() != TopAbs_EDGE) {
        err = "Revolve: axis edge not resolved: " + edge_id;
        return false;
    }
    BRepAdaptor_Curve curve(TopoDS::Edge(sub));
    if (curve.GetType() != GeomAbs_Line) {
        err = "Revolve: axis edge must be a straight line";
        return false;
    }
    const gp_Lin lin = curve.Line();
    axis_out = gp_Ax1(lin.Location(), lin.Direction());
    return true;
}

}  // namespace

OpOutcome execute_revolve(OpContext& ctx, const json& op, const std::string& op_id) {
    const json params =
        (op.contains("params") && op["params"].is_object()) ? op["params"] : json::object();

    const double angle_deg = read_scalar(params, "angleDeg", 360.0);
    if (std::abs(angle_deg) < kMinAngleDeg) {
        return OpOutcome::fail("OP_FAILED", "Revolve angle too small");
    }
    const double angle_rad = angle_deg * M_PI / 180.0;  // no 360 special-case (parity)

    // --- profile face ---
    const json* sketch_params = find_sketch(ctx, read_str(params, "sketchId"),
                                            ctx.last_sketch_id ? *ctx.last_sketch_id : "");
    if (!sketch_params) {
        return OpOutcome::fail("REF_UNRESOLVED", "Revolve: profile sketch not found in plan");
    }
    std::string perr;
    std::optional<TopoDS_Face> profile =
        build_profile_face(*sketch_params, read_str(params, "regionId"), perr);
    if (!profile) return OpOutcome::fail("OP_FAILED", perr);

    // --- axis ---
    gp_Ax1 axis;
    std::string aerr;
    bool axis_ok = false;
    if (params.contains("axis") && params["axis"].is_object()) {
        const json& ax = params["axis"];
        const std::string kind = read_str(ax, "kind", "none");
        if (kind == "sketchLine") {
            axis_ok = axis_from_sketch_line(ctx, read_str(ax, "sketchId"), read_str(ax, "lineId"),
                                            axis, aerr);
        } else if (kind == "edge") {
            axis_ok = axis_from_edge(ctx, read_str(ax, "bodyId"), read_str(ax, "edgeId"), axis, aerr);
        }
    }
    if (!axis_ok) {
        return OpOutcome::fail("OP_FAILED",
                               aerr.empty() ? "Revolve: could not resolve revolution axis" : aerr);
    }

    if (ctx.cancel && ctx.cancel->cancelled()) return OpOutcome::cancelled();

    // --- build the revolved tool shape ---
    TopoDS_Shape tool_shape;
    try {
        // A profile touching the axis raises Standard_ConstructionError here.
        BRepPrimAPI_MakeRevol revol(*profile, axis, angle_rad, Standard_True);
        if (!revol.IsDone() || revol.Shape().IsNull()) {
            return OpOutcome::fail("OP_FAILED", "Revolve operation failed");
        }
        tool_shape = revol.Shape();
    } catch (const Standard_Failure& f) {
        return OpOutcome::fail("OP_FAILED", std::string("Revolve failed: ") +
                                               (f.GetMessageString() ? f.GetMessageString() : "OCCT"));
    } catch (...) {
        return OpOutcome::fail("OP_FAILED", "Revolve failed");
    }

    const app::BooleanMode boolean_mode = boolean_mode_of(read_str(params, "booleanMode", "NewBody"));

    OpOutcome out;
    if (boolean_mode == app::BooleanMode::NewBody) {
        const std::string bid = "body_" + op_id;  // D1 worker-minted NewBody id
        ctx.bodies.create(bid, op_id, tool_shape);
        out.body_events.push_back({"created", bid});
        out.body_ids.push_back(bid);
        return out;
    }

    // Add / Cut / Intersect into a target body (id preserved).
    std::string target_id = read_str(params, "targetBodyId");
    if (target_id.empty()) target_id = input_body(op, 0);
    if (target_id.empty()) return OpOutcome::fail("OP_FAILED", "Revolve boolean requires a target body");
    const session::BodyRecord* target_rec = ctx.bodies.get(target_id);
    if (!target_rec) return OpOutcome::fail("REF_UNRESOLVED", "Revolve target body not found: " + target_id);
    if (ctx.cancel && ctx.cancel->cancelled()) return OpOutcome::cancelled();

    std::shared_ptr<BRepBuilderAPI_MakeShape> builder;
    BooleanResult br = checked_boolean(target_rec->geom, tool_shape, boolean_mode, ctx.parallel,
                                       ctx.occt_options, ctx.cancel, builder);
    if (br.error_code == "CANCELLED") return OpOutcome::cancelled();
    if (!br.error_code.empty()) return OpOutcome::fail(br.error_code, br.error_message);

    ctx.bodies.create(target_id, op_id, br.shape);
    if (builder) {
        ctx.partition.apply_history(target_id, br.shape, *builder, out.delta, &out.needs_repair);
    }
    out.body_events.push_back({"modified", target_id});
    out.body_ids.push_back(target_id);
    return out;
}

}  // namespace onecad::ops
