// BooleanOp.h — real OCCT standalone body-body Boolean executor (W-WP5).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildBoolean (:1499-1562):
//   * operation ∈ Union | Cut | Intersect → BRepAlgoAPI_Fuse/Cut/Common;
//   * target + tool resolved by BodyId; the TARGET BodyId is PRESERVED through the
//     boolean (result reuses the target's id — the invariant corpus c asserts);
//   * the TOOL body is consumed (deleted) by the operation.
//
// The builder is kept alive so OCCT history rebinds the target's tracked elements
// (SCHEMA §10 ladder level 1) and the tool's entries are removed with the body.
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

OpOutcome execute_boolean(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);

}  // namespace onecad::ops
