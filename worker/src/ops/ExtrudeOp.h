// ExtrudeOp.h — real OCCT Extrude executor (W-WP5).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildExtrude (:774-1059):
//   * sketch region → LoopDetector/FaceBuilder → planar profile face;
//   * profile normal → BRepPrimAPI_MakePrism;
//   * end conditions Blind / ThroughAll / Symmetric / two-direction (:816-969),
//     plus ToFace (typed targetFace ref resolved via the ladder → projection
//     distance, :858-876) and ToNext (nearest planar face of the target body,
//     :877-894 / distanceToNextPlanarFace :223-241) — W-WP6;
//   * draft angle via BRepOffsetAPI_DraftAngle on the side faces (:977-1013) — W-WP6;
//   * booleanMode NewBody / Add / Cut / Intersect with target resolution per the
//     C++ priority chain (explicit targetBodyId → input body ref) (:1015-1049).
//
// A `ToFace` targetFace ref that does not resolve on the predecessor snapshot ⇒
// NeedsRepair STATE (SCHEMA §7.3 / Invariants 2/3) — never a wrong bind, never Err.
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

// Execute an Extrude op into ctx.bodies/ctx.partition. `op_id` provenance-names a
// NewBody result ("body_<opId>", matching the W-WP4 convention + existing tests).
OpOutcome execute_extrude(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);

}  // namespace onecad::ops
