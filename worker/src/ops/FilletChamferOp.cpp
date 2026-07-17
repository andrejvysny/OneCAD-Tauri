// FilletChamferOp.cpp — see FilletChamferOp.h. Ports buildFillet / buildChamfer.
#include "ops/FilletChamferOp.h"

#include <memory>
#include <string>
#include <vector>

#include <BRepCheck_Analyzer.hxx>
#include <BRepFilletAPI_MakeChamfer.hxx>
#include <BRepFilletAPI_MakeFillet.hxx>
#include <Standard_Failure.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedDataMapOfShapeListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>

#include "elementmap/Ladder.h"
#include "ops/OpCommon.h"

namespace onecad::ops {

using nlohmann::json;
namespace em = onecad::elementmap;

namespace {

constexpr double kMinValue = 1e-3;  // RegenerationEngine.cpp:61 kMinValue

enum class Mode { Fillet, Chamfer };

// The shared target body id of the op's edge refs (all edges live on one body).
std::string target_body_of(const json& op) {
    if (!op.contains("inputs") || !op["inputs"].is_array()) return "";
    for (const json& in : op["inputs"]) {
        if (in.is_object() && in.contains("primary") && in["primary"].is_object()) {
            const std::string bid = read_str(in["primary"], "bodyId");
            if (!bid.empty()) return bid;
        }
    }
    return "";
}

OpOutcome run(OpContext& ctx, const json& op, const std::string& op_id, Mode mode) {
    const char* op_name = (mode == Mode::Fillet) ? "Fillet" : "Chamfer";
    const json params =
        (op.contains("params") && op["params"].is_object()) ? op["params"] : json::object();

    // --- target body (predecessor snapshot — Invariant 3) ---
    const std::string target_id = target_body_of(op);
    if (target_id.empty()) {
        return OpOutcome::fail("OP_FAILED", std::string(op_name) + " requires body input");
    }
    const session::BodyRecord* target_rec = ctx.bodies.get(target_id);
    if (!target_rec) {
        return OpOutcome::fail("REF_UNRESOLVED",
                               std::string(op_name) + " target body not found: " + target_id);
    }
    const TopoDS_Shape target_shape = target_rec->geom;

    // --- radius / distance guard (signed '<' per RegenerationEngine.cpp:1314) ---
    const double radius = read_scalar(params, "radius", 0.0);
    if (radius < kMinValue) {
        return OpOutcome::fail("OP_FAILED", (mode == Mode::Fillet)
                                                ? "Fillet radius too small"
                                                : "Chamfer distance too small");
    }

    // --- resolve each edge ref through the ladder (descriptor + anchor) ---
    std::vector<em::LadderRef> refs;
    if (op.contains("inputs") && op["inputs"].is_array()) {
        std::size_t i = 0;
        for (const json& in : op["inputs"]) {
            em::LadderRef r = em::ladder_ref_from_input(in, op_id + ".input" + std::to_string(i));
            if (r.kind == em::km::ElementKind::Edge) refs.push_back(std::move(r));
            ++i;
        }
    }
    if (refs.empty()) {
        return OpOutcome::fail("OP_FAILED", std::string("No edges for ") + op_name);
    }

    OpOutcome out;
    const std::vector<em::LadderResolution> res =
        em::resolve_descriptor_stage(target_shape, target_id, refs);
    std::vector<TopoDS_Edge> edges;
    for (const em::LadderResolution& r : res) {
        if (r.outcome == em::LadderOutcome::AutoBind && !r.bound_shape.IsNull() &&
            r.bound_shape.ShapeType() == TopAbs_EDGE) {
            edges.push_back(TopoDS::Edge(r.bound_shape));
        } else {
            // Edge ref no longer resolves ⇒ NeedsRepair STATE (never a wrong bind).
            out.needs_repair.push_back(r.to_needs_repair_json());
        }
    }
    if (!out.needs_repair.empty()) {
        return out;  // status Ok + needsRepair ⇒ PlanExecutor prepares m−1, op not applied
    }

    if (ctx.cancel && ctx.cancel->cancelled()) return OpOutcome::cancelled();

    // --- build the fillet / chamfer, keeping the builder alive for history ---
    TopoDS_Shape result;
    std::shared_ptr<BRepBuilderAPI_MakeShape> builder;
    try {
        if (mode == Mode::Fillet) {
            auto f = std::make_shared<BRepFilletAPI_MakeFillet>(target_shape);
            for (const TopoDS_Edge& e : edges) f->Add(radius, e);
            f->Build();
            if (!f->IsDone()) {
                return OpOutcome::fail("OP_FAILED", "Fillet operation failed (radius too large?)");
            }
            result = f->Shape();
            builder = f;
        } else {
            TopTools_IndexedDataMapOfShapeListOfShape edge_face_map;
            TopExp::MapShapesAndAncestors(target_shape, TopAbs_EDGE, TopAbs_FACE, edge_face_map);
            auto ch = std::make_shared<BRepFilletAPI_MakeChamfer>(target_shape);
            std::size_t added = 0;
            for (const TopoDS_Edge& e : edges) {
                const int idx = edge_face_map.FindIndex(e);
                if (idx == 0) continue;
                const TopTools_ListOfShape& faces = edge_face_map(idx);
                if (faces.IsEmpty()) continue;
                ch->Add(radius, radius, e, TopoDS::Face(faces.First()));  // equal-leg
                ++added;
            }
            if (added == 0) return OpOutcome::fail("OP_FAILED", "No valid edges for chamfer");
            ch->Build();
            if (!ch->IsDone()) return OpOutcome::fail("OP_FAILED", "Chamfer operation failed");
            result = ch->Shape();
            builder = ch;
        }
    } catch (const Standard_Failure& f) {
        return OpOutcome::fail("OP_FAILED",
                               std::string(op_name) + " operation failed (radius too large?): " +
                                   (f.GetMessageString() ? f.GetMessageString() : "OCCT"));
    } catch (...) {
        return OpOutcome::fail("OP_FAILED", std::string(op_name) + " operation failed");
    }

    if (result.IsNull()) {
        return OpOutcome::fail("GEOMETRY_INVALID", std::string(op_name) + " produced null shape");
    }
    if (!BRepCheck_Analyzer(result).IsValid()) {
        return OpOutcome::fail("GEOMETRY_INVALID", std::string(op_name) + " produced invalid shape");
    }

    // --- publish the modified body (id preserved) + rebind partition via history ---
    ctx.bodies.create(target_id, op_id, result);
    if (builder) {
        ctx.partition.apply_history(target_id, result, *builder, out.delta, &out.needs_repair);
    }
    out.body_events.push_back({"modified", target_id});
    out.body_ids.push_back(target_id);
    return out;
}

}  // namespace

OpOutcome execute_fillet(OpContext& ctx, const json& op, const std::string& op_id) {
    return run(ctx, op, op_id, Mode::Fillet);
}

OpOutcome execute_chamfer(OpContext& ctx, const json& op, const std::string& op_id) {
    return run(ctx, op, op_id, Mode::Chamfer);
}

}  // namespace onecad::ops
