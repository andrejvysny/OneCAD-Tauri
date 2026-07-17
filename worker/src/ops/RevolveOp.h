// RevolveOp.h — real OCCT Revolve executor (W-WP6).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildRevolve (:1061-1283):
//   * sketch region → planar profile face (buildFaceFromSketchRegion);
//   * axis from a sketch line (2D endpoints mapped through the sketch plane) OR a
//     straight body edge OR none (→ fail "Could not resolve revolution axis");
//   * angleRad = angleDeg·π/180 (no 360 special-case); guard |angleDeg| < 1e-3;
//   * BRepPrimAPI_MakeRevol(profileFace, axis, angleRad, Copy=true);
//   * booleanMode NewBody / Add / Cut / Intersect with the C++ target priority.
//
// NewBody id follows the worker convention "body_<opId>" (D1). A profile touching
// the axis raises Standard_ConstructionError from the MakeRevol ctor → guarded into
// a recoverable OP_FAILED (SCHEMA §8; session intact).
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

OpOutcome execute_revolve(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);

}  // namespace onecad::ops
