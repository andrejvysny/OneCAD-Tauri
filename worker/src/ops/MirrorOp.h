// MirrorOp.h — real OCCT Mirror-body executor (M6a).
//
// Ports OneCAD-CPP RegenerationEngine.cpp buildMirrorBody (:1994-2042):
//   * source body from params.sourceBodyId (C++ BodyRef; here a predecessor body in
//     the scratch BodyStore);
//   * a plane mirror `gp_Trsf::SetMirror(gp_Ax2(planePoint, planeNormal))` — the Ax2
//     main direction is `planeNormal`, so the transform reflects about the plane
//     through `planePoint` perpendicular to `planeNormal` — applied via
//     `BRepBuilderAPI_Transform`;
//   * `fuseWithOriginal` (default false): `true` ⇒ the source + its mirror image are
//     FUSED into one solid (`BRepAlgoAPI_Fuse`); `false` ⇒ the result is the mirror
//     image alone. EITHER way the op produces ONE new body `body_<opId>` (NewBody
//     lineage — the source body is preserved; legacy applyBodyResult adds a fresh
//     result body).
//   * a transform / fuse failure → recoverable OP_FAILED (SCHEMA §8; session intact).
//
// LINEAGE: first-seen body (ID-on-demand) → EMPTY elementMapDelta (as Extrude
// NewBody); a mirror face is minted on demand when first referenced.
#pragma once

#include <string>

#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"

namespace onecad::ops {

OpOutcome execute_mirror_body(OpContext& ctx, const nlohmann::json& op, const std::string& op_id);

}  // namespace onecad::ops
