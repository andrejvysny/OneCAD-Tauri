// ElementMapPartition.cpp — see ElementMapPartition.h.
#include "elementmap/ElementMapPartition.h"

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <vector>

#include <BRepBndLib.hxx>
#include <Bnd_Box.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <gp_Dir.hxx>
#include <gp_XYZ.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ListOfShape.hxx>

#include "elementmap/Scoring.h"

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

km::ElementDescriptor ElementMapPartition::descriptor_from_json(const nlohmann::json& j) {
    km::ElementDescriptor d;
    if (!j.is_object()) return d;
    auto num = [](const nlohmann::json& v, double dflt) {
        return v.is_number() ? v.get<double>() : dflt;
    };
    auto vec3 = [&](const char* key, double dx, double dy, double dz) -> gp_XYZ {
        if (j.contains(key) && j[key].is_array() && j[key].size() >= 3) {
            const nlohmann::json& a = j[key];
            return gp_XYZ(num(a[0], dx), num(a[1], dy), num(a[2], dz));
        }
        return gp_XYZ(dx, dy, dz);
    };
    if (j.contains("shapeType") && j["shapeType"].is_number())
        d.shapeType = static_cast<TopAbs_ShapeEnum>(j["shapeType"].get<int>());
    if (j.contains("surfaceType") && j["surfaceType"].is_number())
        d.surfaceType = static_cast<GeomAbs_SurfaceType>(j["surfaceType"].get<int>());
    if (j.contains("curveType") && j["curveType"].is_number())
        d.curveType = static_cast<GeomAbs_CurveType>(j["curveType"].get<int>());
    const gp_XYZ c = vec3("center", 0, 0, 0);
    d.center = gp_Pnt(c.X(), c.Y(), c.Z());
    if (j.contains("size")) d.size = num(j["size"], 0.0);
    if (j.contains("magnitude")) d.magnitude = num(j["magnitude"], 0.0);
    d.hasNormal = j.value("hasNormal", false);
    d.hasTangent = j.value("hasTangent", false);
    if (d.hasNormal) {
        const gp_XYZ n = vec3("normal", 0, 0, 1);
        if (n.Modulus() > 1e-12) d.normal = gp_Dir(n);
    }
    if (d.hasTangent) {
        const gp_XYZ t = vec3("tangent", 1, 0, 0);
        if (t.Modulus() > 1e-12) d.tangent = gp_Dir(t);
    }
    if (j.contains("adjacencyHash") && j["adjacencyHash"].is_string()) {
        d.adjacencyHash =
            std::strtoull(j["adjacencyHash"].get<std::string>().c_str(), nullptr, 16);
    }
    return d;
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

namespace {

double body_diag_of(const TopoDS_Shape& shape) {
    if (shape.IsNull()) return 1.0;
    Bnd_Box box;
    BRepBndLib::Add(shape, box);
    if (box.IsVoid()) return 1.0;
    Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
    box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
    const double dx = xmax - xmin, dy = ymax - ymin, dz = zmax - zmin;
    const double diag = std::sqrt(dx * dx + dy * dy + dz * dz);
    return diag > 1e-9 ? diag : 1.0;
}

// A world-point AnchorEvidence parsed from an entry's stored anchor echo (if any).
AnchorEvidence anchor_of(const nlohmann::json& anchor) {
    AnchorEvidence a;
    if (anchor.is_object() && anchor.contains("worldPoint") && anchor["worldPoint"].is_array() &&
        anchor["worldPoint"].size() >= 3) {
        const nlohmann::json& wp = anchor["worldPoint"];
        if (wp[0].is_number() && wp[1].is_number() && wp[2].is_number()) {
            a.has_world_point = true;
            a.world_point = gp_Pnt(wp[0].get<double>(), wp[1].get<double>(), wp[2].get<double>());
        }
    }
    return a;
}

}  // namespace

void ElementMapPartition::apply_history(const std::string& body_id,
                                        const TopoDS_Shape& new_body_shape,
                                        BRepBuilderAPI_MakeShape& hist, ElementMapDelta& delta,
                                        std::vector<nlohmann::json>* needs_repair_out) {
    const double body_diag = body_diag_of(new_body_shape);

    // Collect the entries of this body up front (we mutate the map below).
    std::vector<std::string> ids;
    for (const auto& [id, e] : entries_) {
        if (e.body_id == body_id) ids.push_back(id);
    }

    auto emit_no_candidates = [&](const std::string& id) {
        if (needs_repair_out) {
            needs_repair_out->push_back(nlohmann::json{
                {"refId", id},
                {"elementId", id},
                {"ladderFailed", "history"},
                {"reason", "no-candidates"},
                {"scoringVersion", kResolverVersion},
                {"candidates", nlohmann::json::array()},
                {"anchor", nlohmann::json::object()},
                {"uiLabel", "unresolved element on " + body_id}});
        }
    };

    for (const std::string& id : ids) {
        PartitionEntry& e = entries_[id];
        const TopoDS_Shape old = e.shape;

        // Deleted by the operation → the element no longer exists (definitive).
        if (!old.IsNull() && hist.IsDeleted(old)) {
            delta.removed.push_back(id);
            entries_.erase(id);
            continue;
        }

        // Ladder level 1 (SCHEMA §10): consult OCCT Modified().
        TopTools_ListOfShape modified;
        if (!old.IsNull()) modified = hist.Modified(old);

        TopoDS_Shape image;
        if (modified.IsEmpty()) {
            // Not deleted, not modified: does the old shape survive verbatim?
            image = old;
        } else if (modified.Extent() == 1) {
            // UNIQUE image → auto-bind (the fillet-survives-edit path).
            image = modified.First();
        } else {
            // SPLIT: >1 images. EXPLICIT LINEAGE — score every image candidate
            // against the entry's frozen descriptor + anchor and gate on confidence
            // (W-WP6, closes review finding 2 — no forced Modified().First()).
            const AnchorEvidence anchor = anchor_of(e.anchor);
            std::vector<TopoDS_Shape> imgs;
            std::vector<double> scores;
            std::vector<km::ElementDescriptor> descs;
            for (TopTools_ListIteratorOfListOfShape it(modified); it.More(); it.Next()) {
                const km::ElementDescriptor cd = describe(it.Value());
                imgs.push_back(it.Value());
                descs.push_back(cd);
                scores.push_back(
                    score_candidate(e.descriptor, /*has_intent_descriptor=*/true, anchor, cd, body_diag)
                        .score);
            }
            // best / runner-up (deterministic tie-break by list order).
            int best = 0;
            for (int i = 1; i < static_cast<int>(scores.size()); ++i)
                if (scores[i] > scores[best]) best = i;
            double runner_up = 0.0;
            for (int i = 0; i < static_cast<int>(scores.size()); ++i)
                if (i != best) runner_up = std::max(runner_up, scores[i]);
            const double margin = scores[best] - runner_up;

            if (scores[best] >= kAutoBindMinScore && margin >= kAutoBindMinMargin) {
                image = imgs[best];  // confident unique successor
            } else {
                // Ambiguous / symmetric split ⇒ NeedsRepair (never a guess).
                if (needs_repair_out) {
                    nlohmann::json cands = nlohmann::json::array();
                    // Rank images desc by score for the evidence payload.
                    std::vector<int> order(imgs.size());
                    for (int i = 0; i < static_cast<int>(order.size()); ++i) order[i] = i;
                    std::sort(order.begin(), order.end(), [&](int a, int b) {
                        if (scores[a] != scores[b]) return scores[a] > scores[b];
                        return a < b;
                    });
                    for (int k = 0; k < static_cast<int>(order.size()); ++k) {
                        const int i = order[k];
                        const std::string tk = topokey_for_shape(new_body_shape, imgs[i], e.kind);
                        const double next = (k + 1 < static_cast<int>(order.size()))
                                                ? scores[order[k + 1]]
                                                : scores[i];
                        cands.push_back(nlohmann::json{
                            {"topoKey", tk},
                            {"score", scores[i]},
                            {"margin", scores[i] - next},
                            {"worldPos", {descs[i].center.X(), descs[i].center.Y(), descs[i].center.Z()}},
                            {"summary", "split image of " + id}});
                    }
                    needs_repair_out->push_back(nlohmann::json{
                        {"refId", id},
                        {"elementId", id},
                        {"ladderFailed", "history"},
                        {"reason", "ambiguous"},
                        {"scoringVersion", kResolverVersion},
                        {"candidates", std::move(cands)},
                        {"anchor", e.anchor.is_null() ? nlohmann::json::object() : e.anchor},
                        {"uiLabel", "ambiguous split of element on " + body_id}});
                }
                entries_.erase(id);  // cannot confidently rebind
                continue;
            }
        }

        const std::string new_key = topokey_for_shape(new_body_shape, image, e.kind);
        if (new_key.empty()) {
            // No identifiable successor in the new body → NeedsRepair "no-candidates".
            emit_no_candidates(id);
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
