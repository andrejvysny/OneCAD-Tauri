// PatternOp.h — real OCCT Linear / Circular pattern executors (M6a).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildLinearPattern (:1687-1757) and
// buildCircularPattern (:1920-1992):
//   * source body from params.sourceBodyId (the C++ engine's BodyRef, resolved via
//     the dependency graph's producesBefore guard; here the source must already be a
//     predecessor body in the scratch BodyStore);
//   * `count` copies of the source transformed by a per-instance gp_Trsf —
//     translation `n·spacing·i` (linear) or rotation `(angleDeg/count)·i` about a
//     gp_Ax1 (circular), i ∈ [1, count) — via `BRepBuilderAPI_Transform`;
//   * `fuseResult` (default true): the source + all instances are FUSED into one
//     solid (`BRepAlgoAPI_Fuse` chained). `false`: they are gathered into one
//     `TopoDS_Compound`. EITHER way the op produces ONE new body `body_<opId>`
//     (NewBody lineage — the source body is preserved as its own body; legacy
//     applyBodyResult adds a fresh result body). Legacy semantics: the source
//     geometry IS included in the result (result initialised to the source).
//   * guards: `count < 2`, `|spacing| < 1e-9` (linear) / zero direction, a
//     transform/fuse failure → recoverable OP_FAILED (SCHEMA §8; session intact).
//
// LINEAGE: the result is a first-seen body (ID-on-demand, SCHEMA §7.5) — no
// pre-existing tracked elements, so the elementMapDelta is EMPTY (as Extrude
// NewBody). Legacy `rebindBody` re-descriptored every face generically (no
// per-instance ordinal naming); the V2 partition mints a pattern face on demand
// when first referenced, its descriptor (center/normal) naturally distinguishing
// the instance — so no eager per-instance naming is required.
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

OpOutcome execute_linear_pattern(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);
OpOutcome execute_circular_pattern(OpContext& ctx, const nlohmann::json& op,
                                   const std::string& op_id);

}  // namespace onecad::ops
