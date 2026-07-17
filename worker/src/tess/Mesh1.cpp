// Mesh1.cpp — see Mesh1.h. MESH1 encoder per protocol/mesh_format.md.
#include "tess/Mesh1.h"

#include <cstring>

namespace onecad::tess {

namespace {

// MESH1 section type codes (mesh_format.md §4).
enum SectionType : std::uint32_t {
    POSITIONS = 1,
    NORMALS = 2,
    INDICES = 3,
    FACE_RANGES = 4,
    FACE_ID_OFFS = 5,
    FACE_ID_CHARS = 6,
    EDGE_RANGES = 7,
    EDGE_POSITIONS = 8,
    EDGE_ID_OFFS = 9,
    EDGE_ID_CHARS = 10,
};

void put_u16(std::vector<std::uint8_t>& b, std::size_t at, std::uint16_t v) {
    b[at] = static_cast<std::uint8_t>(v & 0xff);
    b[at + 1] = static_cast<std::uint8_t>((v >> 8) & 0xff);
}
void put_u32(std::vector<std::uint8_t>& b, std::size_t at, std::uint32_t v) {
    for (int i = 0; i < 4; ++i) b[at + i] = static_cast<std::uint8_t>((v >> (i * 8)) & 0xff);
}
void put_f32(std::vector<std::uint8_t>& b, std::size_t at, float f) {
    std::uint32_t u;
    std::memcpy(&u, &f, 4);  // host is LE (asserted at startup)
    put_u32(b, at, u);
}

void append_u32(std::vector<std::uint8_t>& b, std::uint32_t v) {
    for (int i = 0; i < 4; ++i) b.push_back(static_cast<std::uint8_t>((v >> (i * 8)) & 0xff));
}
void append_f32(std::vector<std::uint8_t>& b, float f) {
    std::uint32_t u;
    std::memcpy(&u, &f, 4);
    append_u32(b, u);
}

// One section's serialized body + its type, prior to placement.
struct Section {
    std::uint32_t type = 0;
    std::vector<std::uint8_t> data;
};

void pad4(std::vector<std::uint8_t>& b) {
    while (b.size() % 4 != 0) b.push_back(0);
}

// Build id table sections (offsets prefix-sum + concatenated UTF-8 chars).
void build_id_table(const std::vector<std::string>& ids, std::uint32_t offs_type,
                    std::uint32_t chars_type, std::vector<Section>& out) {
    Section offs;
    offs.type = offs_type;
    Section chars;
    chars.type = chars_type;
    std::uint32_t acc = 0;
    append_u32(offs.data, acc);  // offs[0] = 0
    for (const std::string& id : ids) {
        for (char c : id) chars.data.push_back(static_cast<std::uint8_t>(c));
        acc += static_cast<std::uint32_t>(id.size());
        append_u32(offs.data, acc);  // prefix sum
    }
    out.push_back(std::move(offs));
    out.push_back(std::move(chars));
}

}  // namespace

std::vector<std::uint8_t> encode_mesh1(const Mesh1Input& in) {
    const std::uint32_t V = static_cast<std::uint32_t>(in.positions.size() / 3);
    const std::uint32_t T = static_cast<std::uint32_t>(in.indices.size() / 3);
    const std::uint32_t F = static_cast<std::uint32_t>(in.face_ranges.size());
    const std::uint32_t E = static_cast<std::uint32_t>(in.edge_ranges.size());
    const std::uint32_t P = static_cast<std::uint32_t>(in.edge_positions.size() / 3);

    // --- assemble section bodies in ascending type order (== offset order) ---
    std::vector<Section> sections;

    {  // POSITIONS
        Section s;
        s.type = POSITIONS;
        s.data.reserve(in.positions.size() * 4);
        for (float f : in.positions) append_f32(s.data, f);
        sections.push_back(std::move(s));
    }
    if (in.has_normals && !in.normals.empty()) {  // NORMALS
        Section s;
        s.type = NORMALS;
        for (float f : in.normals) append_f32(s.data, f);
        sections.push_back(std::move(s));
    }
    {  // INDICES
        Section s;
        s.type = INDICES;
        for (std::uint32_t i : in.indices) append_u32(s.data, i);
        sections.push_back(std::move(s));
    }
    {  // FACE_RANGES
        Section s;
        s.type = FACE_RANGES;
        for (const auto& r : in.face_ranges) {
            append_u32(s.data, r.first);
            append_u32(s.data, r.second);
        }
        sections.push_back(std::move(s));
    }
    build_id_table(in.face_ids, FACE_ID_OFFS, FACE_ID_CHARS, sections);

    if (in.has_edges) {
        {  // EDGE_RANGES
            Section s;
            s.type = EDGE_RANGES;
            for (const auto& r : in.edge_ranges) {
                append_u32(s.data, r.first);
                append_u32(s.data, r.second);
            }
            sections.push_back(std::move(s));
        }
        {  // EDGE_POSITIONS
            Section s;
            s.type = EDGE_POSITIONS;
            for (float f : in.edge_positions) append_f32(s.data, f);
            sections.push_back(std::move(s));
        }
        build_id_table(in.edge_ids, EDGE_ID_OFFS, EDGE_ID_CHARS, sections);
    }

    const std::uint16_t section_count = static_cast<std::uint16_t>(sections.size());

    // --- header + table skeleton ---
    const std::size_t table_off = 0x40;
    const std::size_t table_len = static_cast<std::size_t>(section_count) * 16;
    std::vector<std::uint8_t> blob(table_off + table_len, 0);

    // Place section data (4-byte aligned) and fill the table.
    for (std::size_t i = 0; i < sections.size(); ++i) {
        pad4(blob);
        const std::uint32_t off = static_cast<std::uint32_t>(blob.size());
        const std::uint32_t len = static_cast<std::uint32_t>(sections[i].data.size());
        blob.insert(blob.end(), sections[i].data.begin(), sections[i].data.end());

        const std::size_t entry = table_off + i * 16;
        put_u32(blob, entry + 0x00, sections[i].type);
        put_u32(blob, entry + 0x04, 0);  // pad
        put_u32(blob, entry + 0x08, off);
        put_u32(blob, entry + 0x0C, len);
    }
    pad4(blob);  // trailing pad to 4-byte boundary (mesh_format.md §5.1)

    // --- header ---
    std::uint16_t flags = 0;
    if (in.has_normals && !in.normals.empty()) flags |= 0x0001;  // HAS_NORMALS
    if (in.has_edges) flags |= 0x0002;                           // HAS_EDGES
    if (in.ids_have_elementids) flags |= 0x0008;                 // IDS_HAVE_ELEMENTIDS

    put_u32(blob, 0x00, 0x4D455348);  // magic "MESH" (LE u32)
    put_u16(blob, 0x04, 1);           // version
    put_u16(blob, 0x06, flags);
    put_u32(blob, 0x08, V);
    put_u32(blob, 0x0C, T);
    put_u32(blob, 0x10, F);
    put_u32(blob, 0x14, E);
    put_u32(blob, 0x18, P);
    put_u16(blob, 0x1C, in.lod);
    put_u16(blob, 0x1E, section_count);
    put_f32(blob, 0x20, in.bbox_min[0]);
    put_f32(blob, 0x24, in.bbox_min[1]);
    put_f32(blob, 0x28, in.bbox_min[2]);
    put_f32(blob, 0x2C, in.bbox_max[0]);
    put_f32(blob, 0x30, in.bbox_max[1]);
    put_f32(blob, 0x34, in.bbox_max[2]);
    put_u32(blob, 0x38, 0);  // reserved0
    put_u32(blob, 0x3C, 0);  // reserved1

    return blob;
}

}  // namespace onecad::tess
