// ShellOp.cpp — see ShellOp.h. Ports RegenerationEngine.cpp buildShell.
#include "ops/ShellOp.h"

#include <memory>
#include <string>
#include <vector>

#include <BRepCheck_Analyzer.hxx>
#include <BRepOffsetAPI_MakeThickSolid.hxx>
#include <BRepOffset_Mode.hxx>
#include <GeomAbs_JoinType.hxx>
#include <Standard_Failure.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>

#include "elementmap/ElementMapPartition.h"
#include "elementmap/Ladder.h"
#include "ops/OpCommon.h"

namespace onecad::ops {

using nlohmann::json;
namespace em = onecad::elementmap;

namespace {

constexpr double kMinValue = 1e-3;  // RegenerationEngine.cpp:61 kMinValue

// The shared target body id of the op: params.targetBodyId, else the first primary
// bodyId carried by any open-face ref (all open faces live on one body).
std::string target_body_of(const json& op, const json& params) {
    const std::string pid = read_str(params, "targetBodyId");
    if (!pid.empty()) return pid;
    if (op.contains("inputs") && op["inputs"].is_array()) {
        for (const json& in : op["inputs"]) {
            if (in.is_object() && in.contains("primary") && in["primary"].is_object()) {
                const std::string bid = read_str(in["primary"], "bodyId");
                if (!bid.empty()) return bid;
            }
        }
    }
    return "";
}

}  // namespace

OpOutcome execute_shell(OpContext& ctx, const json& op, const std::string& op_id) {
    const json params =
        (op.contains("params") && op["params"].is_object()) ? op["params"] : json::object();

    // --- target body (predecessor snapshot — Invariant 3) ---
    const std::string target_id = target_body_of(op, params);
    if (target_id.empty()) {
        return OpOutcome::fail("OP_FAILED", "Shell requires body input");
    }
    const session::BodyRecord* target_rec = ctx.bodies.get(target_id);
    if (!target_rec) {
        return OpOutcome::fail("REF_UNRESOLVED", "Shell target body not found: " + target_id);
    }
    const TopoDS_Shape target_shape = target_rec->geom;

    // --- thickness guard (signed '<' per RegenerationEngine.cpp:1455) ---
    const double thickness = read_scalar(params, "thickness", 0.0);
    if (thickness < kMinValue) {
        return OpOutcome::fail("OP_FAILED", "Shell thickness too small");
    }

    // --- resolve each open-face ref (partition-tracked, else the ladder) ---
    OpOutcome out;
    TopTools_ListOfShape faces_to_remove;
    std::size_t face_ref_count = 0;
    std::vector<em::LadderRef> ladder_refs;  // refs that fall through to the descriptor stage

    if (op.contains("inputs") && op["inputs"].is_array()) {
        std::size_t i = 0;
        for (const json& in : op["inputs"]) {
            em::LadderRef r = em::ladder_ref_from_input(in, op_id + ".input" + std::to_string(i));
            ++i;
            if (r.kind != em::km::ElementKind::Face) continue;  // Shell removes faces only
            ++face_ref_count;

            // (1) Partition-tracked: the elementId was minted against this body on the
            // predecessor snapshot; its TopoKey maps straight to the current sub-face.
            if (!r.element_id.empty()) {
                if (const em::PartitionEntry* e = ctx.partition.find(r.element_id)) {
                    const TopoDS_Shape sub =
                        em::ElementMapPartition::shape_for_topokey(target_shape, e->topo_key);
                    if (!sub.IsNull() && sub.ShapeType() == TopAbs_FACE) {
                        faces_to_remove.Append(TopoDS::Face(sub));
                        continue;
                    }
                }
            }
            // (2) Fall through to the descriptor+anchor ladder (the Fillet path).
            ladder_refs.push_back(std::move(r));
        }
    }

    if (face_ref_count == 0) {
        return OpOutcome::fail("OP_FAILED", "No valid faces for shell");
    }

    if (!ladder_refs.empty()) {
        const std::vector<em::LadderResolution> res =
            em::resolve_descriptor_stage(target_shape, target_id, ladder_refs);
        for (const em::LadderResolution& r : res) {
            if (r.outcome == em::LadderOutcome::AutoBind && !r.bound_shape.IsNull() &&
                r.bound_shape.ShapeType() == TopAbs_FACE) {
                faces_to_remove.Append(TopoDS::Face(r.bound_shape));
            } else {
                // Open-face ref no longer resolves ⇒ NeedsRepair STATE (never a wrong bind).
                out.needs_repair.push_back(r.to_needs_repair_json());
            }
        }
    }
    if (!out.needs_repair.empty()) {
        return out;  // status Ok + needsRepair ⇒ PlanExecutor prepares m−1, op not applied
    }
    if (faces_to_remove.IsEmpty()) {
        return OpOutcome::fail("OP_FAILED", "No valid faces for shell");
    }

    if (ctx.cancel && ctx.cancel->cancelled()) return OpOutcome::cancelled();

    // --- build the thick solid, keeping the builder alive for history ---
    TopoDS_Shape result;
    std::shared_ptr<BRepOffsetAPI_MakeThickSolid> builder;
    try {
        builder = std::make_shared<BRepOffsetAPI_MakeThickSolid>();
        // NEGATIVE offset hollows inward (legacy `-params.thickness`). Skin mode,
        // Arc joins, 1e-3 tolerance — verbatim from RegenerationEngine.cpp:1482-1485.
        builder->MakeThickSolidByJoin(target_shape, faces_to_remove, -thickness, 1e-3,
                                      BRepOffset_Skin, Standard_False, Standard_False, GeomAbs_Arc,
                                      Standard_False);
        builder->Build();
        if (!builder->IsDone()) {
            return OpOutcome::fail("OP_FAILED", "Shell operation failed");
        }
        result = builder->Shape();
    } catch (const Standard_Failure& f) {
        return OpOutcome::fail("OP_FAILED",
                               std::string("Shell operation failed: ") +
                                   (f.GetMessageString() ? f.GetMessageString() : "OCCT"));
    } catch (...) {
        return OpOutcome::fail("OP_FAILED", "Shell operation failed");
    }

    if (result.IsNull()) {
        return OpOutcome::fail("GEOMETRY_INVALID", "Shell produced null shape");
    }
    if (!BRepCheck_Analyzer(result).IsValid()) {
        return OpOutcome::fail("GEOMETRY_INVALID", "Shell produced invalid shape");
    }

    // --- publish the modified body (id preserved) + rebind partition via history ---
    ctx.bodies.create(target_id, op_id, result);
    ctx.partition.apply_history(target_id, result, *builder, out.delta, &out.needs_repair);
    out.body_events.push_back({"modified", target_id});
    out.body_ids.push_back(target_id);
    return out;
}

}  // namespace onecad::ops
