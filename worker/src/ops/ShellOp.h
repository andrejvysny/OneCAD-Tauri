// ShellOp.h — real OCCT Shell (hollow) executor (M6a).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildShell (:1431-1497):
//   * target body from the op's semantic refs (a BodyRef in the C++ engine; here
//     params.targetBodyId, else the open-face refs' shared primary.bodyId);
//   * thickness guard `< kMinValue (1e-3)` → "Shell thickness too small" (OP_FAILED);
//   * open faces resolved to real sub-faces of the target body, then removed by
//     `BRepOffsetAPI_MakeThickSolid::MakeThickSolidByJoin(target, removed, -thickness,
//     1e-3, BRepOffset_Skin, false, false, GeomAbs_Arc, false)` — the NEGATIVE offset
//     hollows INWARD (legacy parity: `-params.thickness`);
//   * IsDone + validity; a failure → recoverable OP_FAILED / GEOMETRY_INVALID
//     (SCHEMA §8; session intact). Result REPLACES the body (Modify lineage — id
//     preserved, OCCT history folded into the partition).
//
// OPEN-FACE RESOLUTION (SCHEMA §10 / Invariant 3): each open face is resolved on the
// EXACT predecessor snapshot (the target body BEFORE the shell). Because ShellParams
// carries only bare ElementIds (no per-face typed ref / anchor — the frozen v2
// schema), resolution is:
//   1. partition-tracked — the ref's elementId was minted by PlanExecutor's
//      resolve_input_refs (or a prior accepted op) against this body; its snapshot
//      TopoKey maps straight to the current sub-face. This is the production path.
//   2. else the descriptor+anchor ladder (`resolve_descriptor_stage`), the SAME path
//      Fillet uses — the in-process / evidence-bearing path.
// A face that resolves via NEITHER ⇒ NeedsRepair STATE (never a wrong bind, never an
// Err) — the op does not run and the step prepares m−1.
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

OpOutcome execute_shell(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);

}  // namespace onecad::ops
