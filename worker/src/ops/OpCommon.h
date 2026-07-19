// OpCommon.h — shared helpers for the real OCCT op executors (W-WP5).
//
// Ports the reusable pieces of OneCAD-CPP RegenerationEngine.cpp:
//   * profile build: sketch params → LoopDetector/FaceBuilder → TopoDS_Face
//     (RegenerationEngine.cpp:1639-1667 buildFaceFromSketchRegion);
//   * planar face plane+normal (RegenerationEngine.cpp:201-219
//     planarFacePlaneAndNormal);
//   * checked boolean (Fuse/Cut/Common with IsDone + validity + cancellation)
//     (RegenerationEngine.cpp:144-199 checkedBooleanResult);
//   * scalar reader (bare number OR {value, expr?}, SCHEMA §7.3).
#pragma once

#include <memory>
#include <optional>
#include <string>
#include <utility>
#include <vector>

#include <BRepBuilderAPI_MakeShape.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Dir.hxx>
#include <gp_Pln.hxx>

#include "modeling/BooleanMode.h"
#include "nlohmann/json.hpp"
#include "ops/OpTypes.h"  // OpContext / OpOutcome / session::BodyEvent
#include "util/Cancel.h"

namespace onecad::ops {

// A dimensional param: bare number OR {value, expr?} object (SCHEMA §7.3).
double read_scalar(const nlohmann::json& params, const char* key, double dflt);

// params.<key> as a string, or `dflt`.
std::string read_str(const nlohmann::json& o, const char* key, const std::string& dflt = "");

// Build one planar profile face from a Sketch op's params (plane + entities +
// constraints). A non-empty `region_id` (SCHEMA §7.4 normative FNV `r_<hash>`) MUST
// match a detected region — no match is a HARD FAILURE (nullopt + `err` naming the
// requested id and the available ids), never a silent fallback to a different
// region. An empty/absent `region_id` uses the FIRST detected region (V1 fallback).
// Returns nullopt + fills `err` on any failure.
std::optional<TopoDS_Face> build_profile_face(const nlohmann::json& sketch_params,
                                              const std::string& region_id, std::string& err);

// Plane + outward normal of a planar face (normal reversed for REVERSED faces).
// false when the face is null / non-planar.
bool planar_face_plane_normal(const TopoDS_Face& face, gp_Pln& plane_out, gp_Dir& normal_out);

// Result of a checked boolean: the shape (null on failure) + the §8 error code to
// surface. `hist_out` receives the builder so the caller can apply OCCT history to
// the ElementMap partition (SCHEMA §10 ladder level 1 — builder kept alive).
struct BooleanResult {
    TopoDS_Shape shape;           // null ⇒ failed / cancelled
    std::string error_code;       // "" on success; OP_FAILED / GEOMETRY_INVALID / CANCELLED
    std::string error_message;
};

// Fuse/Cut/Common of target ⊕ tool, honoring determinism (SetRunParallel) +
// occtOptions (fuzzyValue/useOBB) + the cancel token (via CancelProgress). The
// builder is heap-owned and returned in `builder_out` (kept alive for history).
// Mirrors RegenerationEngine.cpp checkedBooleanResult semantics (IsDone → fail,
// invalid shape → fail), plus cancellation.
BooleanResult checked_boolean(const TopoDS_Shape& target, const TopoDS_Shape& tool,
                              app::BooleanMode mode, bool parallel,
                              const nlohmann::json& occt_options, const onecad::CancelToken* cancel,
                              std::shared_ptr<BRepBuilderAPI_MakeShape>& builder_out);

// The solids of `shape` in DETERMINISTIC order — the ordinal a split's child ids
// (`body_<opId>:<k>`) are numbered by (SCHEMA §2, D1). Ordered by a quantized
// geometric key: (volume, centroid x, y, z, face count) at 1e-6 quantization, so a
// symmetric bisection (equal volumes) is disambiguated by centroid. NEVER unordered
// TopExp iteration. Empty when `shape` carries no solid.
std::vector<TopoDS_Shape> ordered_solids(const TopoDS_Shape& shape);

// Publish a boolean / boolean-mode-Cut result into the scratch as the successor of
// `target_id`. A SINGLE-solid result MODIFIES `target_id` in place (BodyId preserved
// — corpus invariant — + OCCT history applied to its partition). A MULTI-solid
// result SPLITS: `target_id` is Deleted and each solid becomes a deterministic child
// `body_<opId>:<k>` (Created, ordered by `ordered_solids`; SCHEMA §2 split minting,
// D1). Fills `out` (body_events, body_ids, delta). `builder` may be null (no history
// applied). On a split, the parent's referenced-element partition entries are dropped
// (a rebuildable ID-on-demand cache; a later ref re-mints against a child or
// NeedsRepairs) — no confident 1:1 child assignment exists.
void publish_boolean_result(OpContext& ctx, const std::string& op_id,
                            const std::string& target_id, const TopoDS_Shape& result,
                            BRepBuilderAPI_MakeShape* builder, OpOutcome& out);

}  // namespace onecad::ops
