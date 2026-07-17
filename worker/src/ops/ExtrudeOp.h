// ExtrudeOp.h — real OCCT Extrude executor (W-WP5).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildExtrude (:774-1059):
//   * sketch region → LoopDetector/FaceBuilder → planar profile face;
//   * profile normal → BRepPrimAPI_MakePrism;
//   * end conditions Blind / ThroughAll / Symmetric / two-direction (:816-969);
//   * booleanMode NewBody / Add / Cut / Intersect with target resolution per the
//     C++ priority chain (explicit targetBodyId → input body ref) (:1015-1049).
//
// DEFERRED this WP (documented — they need ladder resolution of a typed target
// face/body, which is W-WP6): ExtrudeMode `ToFace` and `ToNext` → UNSUPPORTED
// (recoverable §8, session intact). Draft angle is also deferred (rare; not in the
// corpus numbers) → UNSUPPORTED when a non-zero draft is requested.
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

// Execute an Extrude op into ctx.bodies/ctx.partition. `op_id` provenance-names a
// NewBody result ("body_<opId>", matching the W-WP4 convention + existing tests).
OpOutcome execute_extrude(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);

}  // namespace onecad::ops
