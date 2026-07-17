// ElementMapPartition.cpp — see ElementMapPartition.h.
#include "elementmap/ElementMapPartition.h"

#include <TopAbs_ShapeEnum.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>

namespace onecad::elementmap {

namespace {

TopAbs_ShapeEnum topabs_of(km::ElementKind kind) {
    switch (kind) {
        case km::ElementKind::Face: return TopAbs_FACE;
        case km::ElementKind::Edge: return TopAbs_EDGE;
        case km::ElementKind::Vertex: return TopAbs_VERTEX;
        default: return TopAbs_SHAPE;
    }
}

char topokey_prefix(km::ElementKind kind) {
    switch (kind) {
        case km::ElementKind::Face: return 'f';
        case km::ElementKind::Edge: return 'e';
        case km::ElementKind::Vertex: return 'v';
        case km::ElementKind::Body: return 'b';
        default: return '?';
    }
}

// Parse "<prefix>:<index>" → {prefix, 1-based index}. Returns false on garbage.
bool parse_topokey(const std::string& tk, char& prefix, int& index) {
    const std::size_t colon = tk.find(':');
    if (colon == std::string::npos || colon == 0) return false;
    prefix = tk[0];
    try {
        index = std::stoi(tk.substr(colon + 1));
    } catch (...) {
        return false;
    }
    return index >= 1;
}

km::ElementKind kind_of_prefix(char p) {
    switch (p) {
        case 'f': return km::ElementKind::Face;
        case 'e': return km::ElementKind::Edge;
        case 'v': return km::ElementKind::Vertex;
        case 'b': return km::ElementKind::Body;
        default: return km::ElementKind::Unknown;
    }
}

}  // namespace

// --- statics ---------------------------------------------------------------

std::string ElementMapPartition::kind_name(km::ElementKind kind) {
    switch (kind) {
        case km::ElementKind::Face: return "face";
        case km::ElementKind::Edge: return "edge";
        case km::ElementKind::Vertex: return "vertex";
        case km::ElementKind::Body: return "body";
        default: return "unknown";
    }
}

km::ElementKind ElementMapPartition::kind_from_name(const std::string& s) {
    if (s == "face") return km::ElementKind::Face;
    if (s == "edge") return km::ElementKind::Edge;
    if (s == "vertex") return km::ElementKind::Vertex;
    if (s == "body") return km::ElementKind::Body;
    return km::ElementKind::Unknown;
}

km::ElementDescriptor ElementMapPartition::describe(const TopoDS_Shape& shape) {
    // REUSE the kernel descriptor verbatim: register into a throwaway ElementMap
    // (which runs the exact private computeDescriptor + quantization constants),
    // then read the stored descriptor back. This forks NO constant and never
    // touches the header the parity gate pins.
    km::ElementMap tmp;
    const km::ElementId id = km::ElementId::From("__describe__");
    tmp.registerElement(id, km::ElementKind::Unknown, shape);
    if (const km::Entry* e = tmp.find(id)) return e->descriptor;
    return km::ElementDescriptor{};
}

std::string ElementMapPartition::topokey_for_shape(const TopoDS_Shape& body_shape,
                                                   const TopoDS_Shape& sub_shape,
                                                   km::ElementKind kind) {
    if (body_shape.IsNull() || sub_shape.IsNull()) return "";
    const TopAbs_ShapeEnum type = topabs_of(kind);
    if (type == TopAbs_SHAPE) return "";
    TopTools_IndexedMapOfShape map;
    TopExp::MapShapes(body_shape, type, map);
    const int idx = map.FindIndex(sub_shape);  // 1-based; 0 if absent (IsSame match)
    if (idx <= 0) return "";
    return std::string(1, topokey_prefix(kind)) + ":" + std::to_string(idx);
}

TopoDS_Shape ElementMapPartition::shape_for_topokey(const TopoDS_Shape& body_shape,
                                                    const std::string& topo_key) {
    char prefix = 0;
    int index = 0;
    if (body_shape.IsNull() || !parse_topokey(topo_key, prefix, index)) return TopoDS_Shape();
    const km::ElementKind kind = kind_of_prefix(prefix);
    const TopAbs_ShapeEnum type = topabs_of(kind);
    if (type == TopAbs_SHAPE) return TopoDS_Shape();
    TopTools_IndexedMapOfShape map;
    TopExp::MapShapes(body_shape, type, map);
    if (index < 1 || index > map.Extent()) return TopoDS_Shape();
    return map(index);
}

TopoDS_Shape ElementMapPartition::nearest_subshape(const TopoDS_Shape& body_shape,
                                                   km::ElementKind kind, double wx, double wy,
                                                   double wz) {
    if (body_shape.IsNull()) return TopoDS_Shape();
    const TopAbs_ShapeEnum type = topabs_of(kind);
    if (type == TopAbs_SHAPE) return TopoDS_Shape();
    TopTools_IndexedMapOfShape map;
    TopExp::MapShapes(body_shape, type, map);
    TopoDS_Shape best;
    double best_d2 = -1.0;
    for (int i = 1; i <= map.Extent(); ++i) {
        const km::ElementDescriptor d = describe(map(i));
        const double dx = d.center.X() - wx, dy = d.center.Y() - wy, dz = d.center.Z() - wz;
        const double d2 = dx * dx + dy * dy + dz * dz;
        if (best_d2 < 0.0 || d2 < best_d2) {
            best_d2 = d2;
            best = map(i);
        }
    }
    return best;
}

nlohmann::json ElementMapPartition::descriptor_to_json(const km::ElementDescriptor& d) {
    return nlohmann::json{
        {"shapeType", static_cast<int>(d.shapeType)},
        {"center", {d.center.X(), d.center.Y(), d.center.Z()}},
        {"size", d.size},
        {"magnitude", d.magnitude},
        {"surfaceType", static_cast<int>(d.surfaceType)},
        {"curveType", static_cast<int>(d.curveType)},
        {"normal", {d.normal.X(), d.normal.Y(), d.normal.Z()}},
        {"tangent", {d.tangent.X(), d.tangent.Y(), d.tangent.Z()}},
        {"hasNormal", d.hasNormal},
        {"hasTangent", d.hasTangent},
        // adjacencyHash is a 64-bit value → hex string (SCHEMA §2 hash wire form).
        {"adjacencyHash", [&] {
             char buf[17];
             std::snprintf(buf, sizeof(buf), "%016llx",
                           static_cast<unsigned long long>(d.adjacencyHash));
             return std::string(buf);
         }()},
    };
}

// --- queries ---------------------------------------------------------------

const PartitionEntry* ElementMapPartition::find(const std::string& element_id) const {
    auto it = entries_.find(element_id);
    return it != entries_.end() ? &it->second : nullptr;
}

bool ElementMapPartition::contains(const std::string& element_id) const {
    return entries_.count(element_id) != 0;
}

std::vector<const PartitionEntry*> ElementMapPartition::entries_for_body(
    const std::string& body_id) const {
    std::vector<const PartitionEntry*> out;
    for (const auto& [id, e] : entries_) {
        if (e.body_id == body_id) out.push_back(&e);
    }
    return out;
}

// --- minting ---------------------------------------------------------------

DeltaEntry ElementMapPartition::mint(const std::string& body_id, const std::string& element_id,
                                     km::ElementKind kind, const TopoDS_Shape& sub_shape,
                                     const TopoDS_Shape& body_shape, nlohmann::json anchor) {
    PartitionEntry& e = entries_[element_id];
    e.element_id = element_id;
    e.body_id = body_id;
    e.kind = kind;
    e.shape = sub_shape;
    e.topo_key = topokey_for_shape(body_shape, sub_shape, kind);
    e.descriptor = describe(sub_shape);
    if (!anchor.is_null()) e.anchor = std::move(anchor);
    return DeltaEntry{element_id, e.topo_key, kind_name(kind), body_id};
}

// --- history application ---------------------------------------------------

void ElementMapPartition::apply_history(const std::string& body_id,
                                        const TopoDS_Shape& new_body_shape,
                                        BRepBuilderAPI_MakeShape& hist, ElementMapDelta& delta,
                                        std::vector<std::string>* unresolved_out) {
    // Collect the entries of this body up front (we mutate the map below).
    std::vector<std::string> ids;
    for (const auto& [id, e] : entries_) {
        if (e.body_id == body_id) ids.push_back(id);
    }

    for (const std::string& id : ids) {
        PartitionEntry& e = entries_[id];
        const TopoDS_Shape old = e.shape;

        // Deleted by the operation → the element no longer exists (definitive).
        if (!old.IsNull() && hist.IsDeleted(old)) {
            delta.removed.push_back(id);
            entries_.erase(id);
            continue;
        }

        // Determine the element's image in the new body (SCHEMA §10 ladder level 1):
        // OCCT Modified() first. A UNIQUE image auto-binds; a SPLIT (many images) is
        // valid lineage (no forced 1:1) → bind best-effort to the first image (W-WP6
        // scores which successor). If Modified is empty, the old shape may survive.
        TopoDS_Shape image;
        if (!old.IsNull()) {
            const TopTools_ListOfShape& modified = hist.Modified(old);
            if (!modified.IsEmpty()) image = modified.First();
        }
        bool from_modified = !image.IsNull();

        std::string new_key;
        if (from_modified) {
            new_key = topokey_for_shape(new_body_shape, image, e.kind);
        } else {
            // Not deleted, not modified: does the old shape survive verbatim?
            new_key = topokey_for_shape(new_body_shape, old, e.kind);
            image = old;
        }

        if (new_key.empty()) {
            // History gave no usable image and the old shape is not in the new body:
            // the element vanished with no identifiable successor. This is the
            // genuinely-ambiguous case (ladderFailed "history") → NeedsRepair
            // "no-candidates" (W-WP5 placeholder; W-WP6 scores candidates). The
            // entry is dropped from this snapshot's partition.
            if (unresolved_out) unresolved_out->push_back(id);
            entries_.erase(id);
            continue;
        }

        const bool changed = (new_key != e.topo_key) || !image.IsSame(e.shape);
        e.shape = image;
        e.topo_key = new_key;
        e.descriptor = describe(image);
        if (changed) {
            delta.relabeled.push_back(DeltaEntry{id, new_key, kind_name(e.kind), body_id});
        }
    }
}

void ElementMapPartition::remove_body(const std::string& body_id, ElementMapDelta& delta) {
    for (auto it = entries_.begin(); it != entries_.end();) {
        if (it->second.body_id == body_id) {
            delta.removed.push_back(it->first);
            it = entries_.erase(it);
        } else {
            ++it;
        }
    }
}

}  // namespace onecad::elementmap
