// FilletChamferOp.h — real OCCT Fillet / Chamfer executors (W-WP6).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildFillet (:1285-1349) and buildChamfer
// (:1351-1429):
//   * target body from the op's semantic refs (a BodyRef in the C++ engine; here
//     the edge refs' shared primary.bodyId);
//   * radius guard `< kMinValue (1e-3)` → "…radius/distance too small" (OP_FAILED);
//   * per-edge: fillet `Add(radius, edge)`; chamfer `Add(radius, radius, edge,
//     refFace)` (equal-leg, refFace = first ancestor face via MapShapesAndAncestors);
//   * Build + IsDone; a radius-too-large / OCCT failure → recoverable OP_FAILED /
//     GEOMETRY_INVALID (SCHEMA §8; session intact).
//
// EDGE RESOLUTION (SCHEMA §10 / Invariant 3): each edge is resolved through the
// resolution ladder against the EXACT predecessor snapshot (the target body BEFORE
// the fillet), using the per-edge semantic ref's descriptor + anchor evidence. An
// edge ref that no longer resolves ⇒ NeedsRepair STATE (never a wrong bind, never
// an Err) — the op does not run and the step prepares m−1. `chainTangentEdges` is
// metadata: the tangent-chain is already expanded into explicit refs upstream
// (OneCAD-CPP FilletChamferTool.cpp), so the engine iterates the given refs verbatim.
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

OpOutcome execute_fillet(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);
OpOutcome execute_chamfer(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);

}  // namespace onecad::ops
