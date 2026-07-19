// PatternOp.cpp — see PatternOp.h. Ports buildLinearPattern / buildCircularPattern.
#include "ops/PatternOp.h"

#include <cmath>
#include <functional>

#include <BRep_Builder.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepBuilderAPI_Transform.hxx>
#include <Standard_Failure.hxx>
#include <TopoDS_Compound.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Ax1.hxx>
#include <gp_Dir.hxx>
#include <gp_Pnt.hxx>
#include <gp_Trsf.hxx>
#include <gp_Vec.hxx>

#include "ops/OpCommon.h"

namespace onecad::ops {

using nlohmann::json;

namespace {

// Read a Vec3 param serialized as the JSON array `[x, y, z]` (core `Vec3` wire form).
bool read_vec3(const json& params, const char* key, double& x, double& y, double& z) {
    if (!params.is_object() || !params.contains(key)) return false;
    const json& v = params[key];
    if (!v.is_array() || v.size() < 3) return false;
    if (!v[0].is_number() || !v[1].is_number() || !v[2].is_number()) return false;
    x = v[0].get<double>();
    y = v[1].get<double>();
    z = v[2].get<double>();
    return true;
}

// The shared pattern replay: source ⊕ (count−1) transformed instances, fused into one
// solid or gathered into a compound. `xform(i)` is the gp_Trsf for instance i∈[1,count).
OpOutcome build_pattern(OpContext& ctx, const json& op, const std::string& op_id,
                        const char* op_name, int count, bool fuse_result,
                        const std::function<gp_Trsf(int)>& xform) {
    const json params =
        (op.contains("params") && op["params"].is_object()) ? op["params"] : json::object();

    if (count < 2) {
        return OpOutcome::fail("OP_FAILED", std::string(op_name) + " count must be >= 2");
    }

    const std::string source_id = read_str(params, "sourceBodyId");
    if (source_id.empty()) {
        return OpOutcome::fail("OP_FAILED", std::string(op_name) + " requires a source body");
    }
    const session::BodyRecord* source_rec = ctx.bodies.get(source_id);
    if (!source_rec || source_rec->geom.IsNull()) {
        return OpOutcome::fail("REF_UNRESOLVED",
                               std::string(op_name) + " source body not found: " + source_id);
    }
    const TopoDS_Shape source = source_rec->geom;

    if (ctx.cancel && ctx.cancel->cancelled()) return OpOutcome::cancelled();

    TopoDS_Shape result;
    try {
        result = source;  // legacy: the result INCLUDES the source geometry
        TopoDS_Compound compound;
        BRep_Builder cbuilder;
        if (!fuse_result) {
            cbuilder.MakeCompound(compound);
            cbuilder.Add(compound, source);
        }
        for (int i = 1; i < count; ++i) {
            BRepBuilderAPI_Transform xf(source, xform(i), Standard_True);
            if (!xf.IsDone() || xf.Shape().IsNull()) {
                return OpOutcome::fail("OP_FAILED", std::string(op_name) +
                                                        " transform failed at instance " +
                                                        std::to_string(i));
            }
            if (fuse_result) {
                BRepAlgoAPI_Fuse fuse(result, xf.Shape());
                fuse.Build();
                if (!fuse.IsDone() || fuse.Shape().IsNull()) {
                    return OpOutcome::fail("OP_FAILED", std::string(op_name) +
                                                           " fuse failed at instance " +
                                                           std::to_string(i));
                }
                result = fuse.Shape();
            } else {
                cbuilder.Add(compound, xf.Shape());
            }
        }
        if (!fuse_result) result = compound;
    } catch (const Standard_Failure& f) {
        return OpOutcome::fail("OP_FAILED",
                               std::string(op_name) + " raised: " +
                                   (f.GetMessageString() ? f.GetMessageString() : "OCCT"));
    } catch (...) {
        return OpOutcome::fail("OP_FAILED", std::string(op_name) + " raised an unknown exception");
    }

    if (result.IsNull()) {
        return OpOutcome::fail("GEOMETRY_INVALID", std::string(op_name) + " produced null shape");
    }

    // NewBody lineage: a fresh body `body_<opId>` (D1); the source body is preserved.
    OpOutcome out;
    const std::string bid = "body_" + op_id;
    ctx.bodies.create(bid, op_id, result);
    out.body_events.push_back({"created", bid});
    out.body_ids.push_back(bid);
    return out;  // no pre-existing tracked elements → empty delta
}

}  // namespace

OpOutcome execute_linear_pattern(OpContext& ctx, const json& op, const std::string& op_id) {
    const json params =
        (op.contains("params") && op["params"].is_object()) ? op["params"] : json::object();

    const int count = static_cast<int>(read_scalar(params, "count", 0.0));
    const double spacing = read_scalar(params, "spacing", 0.0);
    const bool fuse_result = params.value("fuseResult", true);

    double dx = 0.0, dy = 0.0, dz = 0.0;
    if (!read_vec3(params, "direction", dx, dy, dz)) {
        return OpOutcome::fail("OP_FAILED", "LinearPattern: missing direction vector");
    }
    if (std::abs(spacing) < 1e-9) {
        return OpOutcome::fail("OP_FAILED", "LinearPattern spacing must be non-zero");
    }
    const double len = std::sqrt(dx * dx + dy * dy + dz * dz);
    if (len < 1e-10) {
        return OpOutcome::fail("OP_FAILED", "LinearPattern direction vector is zero");
    }
    const double nx = dx / len, ny = dy / len, nz = dz / len;

    return build_pattern(ctx, op, op_id, "LinearPattern", count, fuse_result, [&](int i) {
        gp_Trsf t;
        t.SetTranslation(gp_Vec(nx * spacing * i, ny * spacing * i, nz * spacing * i));
        return t;
    });
}

OpOutcome execute_circular_pattern(OpContext& ctx, const json& op, const std::string& op_id) {
    const json params =
        (op.contains("params") && op["params"].is_object()) ? op["params"] : json::object();

    const int count = static_cast<int>(read_scalar(params, "count", 0.0));
    const double angle_deg = read_scalar(params, "angleDeg", 360.0);
    const bool fuse_result = params.value("fuseResult", true);

    double ox = 0.0, oy = 0.0, oz = 0.0;
    double ax = 0.0, ay = 0.0, az = 1.0;
    if (!read_vec3(params, "axisOrigin", ox, oy, oz) ||
        !read_vec3(params, "axisDirection", ax, ay, az)) {
        return OpOutcome::fail("OP_FAILED", "CircularPattern: missing axis origin/direction");
    }
    if (ax * ax + ay * ay + az * az < 1e-20) {
        return OpOutcome::fail("OP_FAILED", "CircularPattern axis direction is zero");
    }
    if (count < 2) {
        return OpOutcome::fail("OP_FAILED", "CircularPattern count must be >= 2");
    }

    gp_Ax1 axis(gp_Pnt(ox, oy, oz), gp_Dir(ax, ay, az));
    // Legacy stepAngle = (angleDeg / count) — divides by count (not count−1); parity.
    const double step_rad = (angle_deg / count) * M_PI / 180.0;

    return build_pattern(ctx, op, op_id, "CircularPattern", count, fuse_result, [&](int i) {
        gp_Trsf t;
        t.SetRotation(axis, step_rad * i);
        return t;
    });
}

}  // namespace onecad::ops
