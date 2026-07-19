// MeshExport.cpp — see MeshExport.h.
#include "io/MeshExport.h"

#include <array>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <string>
#include <vector>

#include <TopoDS_Shape.hxx>

#include "session/BodyStore.h"
#include "tess/Tessellate.h"

namespace onecad::io {

using nlohmann::json;
using protocol::Envelope;

namespace {

std::string get_str(const json& o, const char* key, const std::string& dflt = "") {
    if (o.is_object() && o.contains(key) && o[key].is_string()) return o[key].get<std::string>();
    return dflt;
}

// The bodies the request targets: an explicit `bodyIds` array, else every live body.
std::vector<std::string> selected_bodies(const json& args, const session::BodyStore& bodies) {
    std::vector<std::string> which;
    if (args.contains("bodyIds") && args["bodyIds"].is_array()) {
        for (const auto& b : args["bodyIds"])
            if (b.is_string()) which.push_back(b.get<std::string>());
    } else {
        which = bodies.ids();  // "all"
    }
    return which;
}

using Vec3 = std::array<float, 3>;

Vec3 vertex_at(const tess::RawMesh& m, std::uint32_t i) {
    const std::size_t o = static_cast<std::size_t>(i) * 3;
    return {m.positions[o], m.positions[o + 1], m.positions[o + 2]};
}

// Unit face normal of a triangle (geometric; degenerate → +Z, legacy parity).
Vec3 triangle_normal(const Vec3& a, const Vec3& b, const Vec3& c) {
    const float e1[3] = {b[0] - a[0], b[1] - a[1], b[2] - a[2]};
    const float e2[3] = {c[0] - a[0], c[1] - a[1], c[2] - a[2]};
    Vec3 n = {e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2],
              e1[0] * e2[1] - e1[1] * e2[0]};
    const float len = std::sqrt(n[0] * n[0] + n[1] * n[1] + n[2] * n[2]);
    if (len < 1e-12f) return {0.0f, 0.0f, 1.0f};
    return {n[0] / len, n[1] / len, n[2] / len};
}

// Little-endian raw float append (the host is asserted LE at startup — main.cpp).
void put_f32(std::vector<std::uint8_t>& buf, float v) {
    std::uint8_t bytes[4];
    std::memcpy(bytes, &v, 4);
    buf.insert(buf.end(), bytes, bytes + 4);
}
void put_u32(std::vector<std::uint8_t>& buf, std::uint32_t v) {
    std::uint8_t bytes[4];
    std::memcpy(bytes, &v, 4);
    buf.insert(buf.end(), bytes, bytes + 4);
}

Envelope fail(std::uint64_t id, const std::string& msg) {
    return Envelope::error_response(id, protocol::ErrorInfo{"OP_FAILED", msg, /*retriable=*/false});
}

// Meshes the requested bodies; returns false + fills `err` when nothing meshable.
bool collect_meshes(const json& args, const session::BodyStore& bodies, const std::string& lod,
                    std::vector<tess::RawMesh>& out, std::string& err) {
    for (const std::string& bid : selected_bodies(args, bodies)) {
        const session::BodyRecord* rec = bodies.get(bid);
        if (!rec || rec->geom.IsNull()) continue;
        tess::RawMesh m = tess::tessellate_raw(rec->geom, lod);
        if (m.triangle_count == 0) continue;
        out.push_back(std::move(m));
    }
    if (out.empty()) {
        err = "no meshable bodies to export";
        return false;
    }
    return true;
}

}  // namespace

Envelope handle_export_stl(session::Session& session, const Envelope& req) {
    const json& args = req.args;
    const std::string path = get_str(args, "path");
    if (path.empty()) return fail(req.id, "ExportStl: empty path");
    const std::string lod = get_str(args, "lod", "coarse");
    const bool binary = args.is_object() && args.contains("binary") && args["binary"].is_boolean()
                            ? args["binary"].get<bool>()
                            : true;  // §7.8 default: binary STL

    const session::BodyStore bodies = session.bodies_copy();
    std::vector<tess::RawMesh> meshes;
    std::string err;
    if (!collect_meshes(args, bodies, lod, meshes, err)) return fail(req.id, "ExportStl: " + err);

    std::uint64_t triangle_count = 0;
    for (const tess::RawMesh& m : meshes) triangle_count += m.triangle_count;

    if (binary) {
        std::vector<std::uint8_t> buf;
        buf.reserve(84 + static_cast<std::size_t>(triangle_count) * 50);
        std::array<char, 80> header{};
        std::snprintf(header.data(), header.size(), "OneCAD binary STL");
        buf.insert(buf.end(), header.begin(), header.end());
        put_u32(buf, static_cast<std::uint32_t>(triangle_count));
        for (const tess::RawMesh& m : meshes) {
            for (std::size_t t = 0; t < m.indices.size(); t += 3) {
                const Vec3 v0 = vertex_at(m, m.indices[t]);
                const Vec3 v1 = vertex_at(m, m.indices[t + 1]);
                const Vec3 v2 = vertex_at(m, m.indices[t + 2]);
                const Vec3 n = triangle_normal(v0, v1, v2);
                for (float f : n) put_f32(buf, f);
                for (float f : v0) put_f32(buf, f);
                for (float f : v1) put_f32(buf, f);
                for (float f : v2) put_f32(buf, f);
                buf.push_back(0);
                buf.push_back(0);  // 2-byte attribute count
            }
        }
        std::ofstream out(path, std::ios::binary | std::ios::trunc);
        if (!out) return fail(req.id, "ExportStl: cannot open " + path);
        out.write(reinterpret_cast<const char*>(buf.data()), static_cast<std::streamsize>(buf.size()));
        out.close();  // flush before file_size below
        if (!out) return fail(req.id, "ExportStl: write failed");
    } else {
        std::ofstream out(path, std::ios::trunc);
        if (!out) return fail(req.id, "ExportStl: cannot open " + path);
        char line[192];
        out << "solid OneCAD\n";
        for (const tess::RawMesh& m : meshes) {
            for (std::size_t t = 0; t < m.indices.size(); t += 3) {
                const Vec3 v0 = vertex_at(m, m.indices[t]);
                const Vec3 v1 = vertex_at(m, m.indices[t + 1]);
                const Vec3 v2 = vertex_at(m, m.indices[t + 2]);
                const Vec3 n = triangle_normal(v0, v1, v2);
                std::snprintf(line, sizeof(line), "  facet normal %.9g %.9g %.9g\n", n[0], n[1], n[2]);
                out << line << "    outer loop\n";
                for (const Vec3& v : {v0, v1, v2}) {
                    std::snprintf(line, sizeof(line), "      vertex %.9g %.9g %.9g\n", v[0], v[1], v[2]);
                    out << line;
                }
                out << "    endloop\n  endfacet\n";
            }
        }
        out << "endsolid OneCAD\n";
        out.close();  // flush before file_size below
        if (!out) return fail(req.id, "ExportStl: write failed");
    }

    std::error_code ec;
    const std::uintmax_t bytes = std::filesystem::file_size(path, ec);
    return Envelope::ok_response(req.id, json{{"written", true},
                                             {"bytes", ec ? 0 : static_cast<std::uint64_t>(bytes)},
                                             {"triangleCount", triangle_count}});
}

Envelope handle_export_obj(session::Session& session, const Envelope& req) {
    const json& args = req.args;
    const std::string path = get_str(args, "path");
    if (path.empty()) return fail(req.id, "ExportObj: empty path");
    const std::string lod = get_str(args, "lod", "coarse");

    const session::BodyStore bodies = session.bodies_copy();
    const std::vector<std::string> which = selected_bodies(args, bodies);

    // Mesh in body order, pairing each mesh with its body id for the `g` group.
    std::vector<std::pair<std::string, tess::RawMesh>> meshed;
    for (const std::string& bid : which) {
        const session::BodyRecord* rec = bodies.get(bid);
        if (!rec || rec->geom.IsNull()) continue;
        tess::RawMesh m = tess::tessellate_raw(rec->geom, lod);
        if (m.triangle_count == 0) continue;
        meshed.emplace_back(bid, std::move(m));
    }
    if (meshed.empty()) return fail(req.id, "ExportObj: no meshable bodies to export");

    std::ofstream out(path, std::ios::trunc);
    if (!out) return fail(req.id, "ExportObj: cannot open " + path);
    char line[192];
    out << "# OneCAD OBJ export\n# Bodies: " << meshed.size() << "\n\n";

    std::uint32_t vertex_offset = 1;  // OBJ indices are 1-based
    for (const auto& [bid, m] : meshed) {
        out << "g " << bid << "\n";
        const std::size_t vcount = m.positions.size() / 3;
        for (std::size_t i = 0; i < vcount; ++i) {
            std::snprintf(line, sizeof(line), "v %.9g %.9g %.9g\n", m.positions[i * 3],
                          m.positions[i * 3 + 1], m.positions[i * 3 + 2]);
            out << line;
        }
        for (std::size_t i = 0; i < vcount; ++i) {
            std::snprintf(line, sizeof(line), "vn %.9g %.9g %.9g\n", m.normals[i * 3],
                          m.normals[i * 3 + 1], m.normals[i * 3 + 2]);
            out << line;
        }
        for (std::size_t t = 0; t < m.indices.size(); t += 3) {
            const std::uint32_t i0 = m.indices[t] + vertex_offset;
            const std::uint32_t i1 = m.indices[t + 1] + vertex_offset;
            const std::uint32_t i2 = m.indices[t + 2] + vertex_offset;
            std::snprintf(line, sizeof(line), "f %u//%u %u//%u %u//%u\n", i0, i0, i1, i1, i2, i2);
            out << line;
        }
        vertex_offset += static_cast<std::uint32_t>(vcount);
        out << "\n";
    }
    out.close();  // flush before file_size below
    if (!out) return fail(req.id, "ExportObj: write failed");

    std::error_code ec;
    const std::uintmax_t bytes = std::filesystem::file_size(path, ec);
    return Envelope::ok_response(
        req.id, json{{"written", true}, {"bytes", ec ? 0 : static_cast<std::uint64_t>(bytes)}});
}

}  // namespace onecad::io
