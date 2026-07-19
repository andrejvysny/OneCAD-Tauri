// MeshHandle.h — the SCHEMA §7.6 inline MESH1 mesh-handle JSON, shared by the two
// tessellation producers so their field sets never drift again:
//   * the Tessellate verb           (main.cpp handle_tessellate);
//   * the ExecutePlan artifact       (PlanExecutor.cpp attach_tessellate).
// Both inline a small MESH1 blob in the resp binary tail and reference it by the
// normative inline-handle key "bin" (matches the SolverLane region handle + the
// Rust `assemble_mesh` reader). Reconciled to the §7.6 superset shape.
#pragma once

#include <cstdint>
#include <string>

#include "nlohmann/json.hpp"

namespace onecad::tess {

// One MESH1 mesh handle (SCHEMA §7.6 `result.meshes[]` + §5.2 inline-handle shape):
//   { bodyId, format:"MESH1", bin, lod, totalBytes, triangleCount, sha256, snapshotId }
// `bin_section` is the resp-tail section name (the normative inline key "bin").
nlohmann::json mesh_handle_json(const std::string& body_id, const std::string& bin_section,
                                const std::string& lod, std::uint64_t total_bytes,
                                std::uint64_t triangle_count, const std::string& sha256,
                                std::uint64_t snapshot_id);

}  // namespace onecad::tess
