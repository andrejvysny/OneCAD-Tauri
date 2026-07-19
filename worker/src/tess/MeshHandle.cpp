// MeshHandle.cpp — see MeshHandle.h.
#include "tess/MeshHandle.h"

namespace onecad::tess {

nlohmann::json mesh_handle_json(const std::string& body_id, const std::string& bin_section,
                                const std::string& lod, std::uint64_t total_bytes,
                                std::uint64_t triangle_count, const std::string& sha256,
                                std::uint64_t snapshot_id) {
    return nlohmann::json{
        {"bodyId", body_id},
        {"format", "MESH1"},
        {"bin", bin_section},
        {"lod", lod},
        {"totalBytes", total_bytes},
        {"triangleCount", triangle_count},
        {"sha256", sha256},
        {"snapshotId", snapshot_id},
    };
}

}  // namespace onecad::tess
