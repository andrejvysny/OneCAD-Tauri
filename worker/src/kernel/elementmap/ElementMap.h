// Ported from OneCAD-CPP src/kernel/elementmap/ElementMap.h @ b4ddcccc (2026-07-16)
#pragma once

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <iomanip>
#include <iosfwd>
#include <limits>
#include <sstream>
#include <string>
#include <unordered_map>
#include <utility>
#include <vector>

#include <BRepAlgoAPI_BooleanOperation.hxx>
#include <BRepAdaptor_Curve.hxx>
#include <BRepAdaptor_Surface.hxx>
#include <BRepBndLib.hxx>
#include <BRepGProp.hxx>
#include <BRep_Tool.hxx>
#include <BRepTools.hxx>
#include <Bnd_Box.hxx>
#include <GProp_GProps.hxx>
#include <GeomAbs_CurveType.hxx>
#include <GeomAbs_SurfaceType.hxx>
#include <NCollection_DataMap.hxx>
#include <TopAbs_ShapeEnum.hxx>
#include <TopAbs_Orientation.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Shape.hxx>
#include <TopoDS_Edge.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Vertex.hxx>
#include <TopExp_Explorer.hxx>
#include <TopTools_ListOfShape.hxx>
#include <TopTools_ListIteratorOfListOfShape.hxx>
#include <TopTools_ShapeMapHasher.hxx>
#include <gp_Pnt.hxx>
#include <gp_Vec.hxx>

namespace onecad::kernel::elementmap {

enum class ElementKind {
    Body,
    Face,
    Edge,
    Vertex,
    Unknown
};

struct ElementId {
    std::string value;

    std::string toString() const { return value; }
    static ElementId From(const std::string& v) { return ElementId{v}; }
};

struct ElementDescriptor {
    TopAbs_ShapeEnum shapeType{TopAbs_SHAPE};
    gp_Pnt center{0.0, 0.0, 0.0};
    double size{0.0};      // Diagonal length of the bounding box
    double magnitude{0.0}; // Area/length/volume approximation
    GeomAbs_SurfaceType surfaceType{GeomAbs_OtherSurface};
    GeomAbs_CurveType curveType{GeomAbs_OtherCurve};
    gp_Dir normal{0.0, 0.0, 1.0};
    gp_Dir tangent{1.0, 0.0, 0.0};
    bool hasNormal{false};
    bool hasTangent{false};
    std::uint64_t adjacencyHash{0};
};

struct Entry {
    ElementId id;
    ElementKind kind{ElementKind::Unknown};
    TopoDS_Shape shape;
    ElementDescriptor descriptor;
    std::string opId;
    std::vector<ElementId> sources;
};

class ElementMap {
public:
    void registerElement(const ElementId& id, ElementKind kind, const TopoDS_Shape& shape,
                         const std::string& opId = {}, std::vector<ElementId> sources = {});
    void registerEntry(const ElementId& id, ElementKind kind, const ElementDescriptor& descriptor,
                       const std::string& opId = {}, std::vector<ElementId> sources = {});
    bool attachShape(const ElementId& id, const TopoDS_Shape& shape, const std::string& opId = {});

    const Entry* find(const ElementId& id) const;
    Entry* find(const ElementId& id);
    bool contains(const ElementId& id) const;
    std::vector<ElementId> ids() const;
    std::vector<ElementId> findIdsByShape(const TopoDS_Shape& shape) const;
    void clear();
    void clearShape(const ElementId& id);
    void removeElementsForBody(const std::string& bodyId);
    void rebindBody(const std::string& bodyId, const TopoDS_Shape& shape,
                    const std::string& opId = {});

    // Updates tracked shapes using the history from a boolean operation. Returns IDs that were deleted.
    std::vector<ElementId> update(BRepAlgoAPI_BooleanOperation& algo, const std::string& opId);

    // Resolve a (possibly stale) id to a live shape. If the literal id still has a bound
    // shape, returns it (scoreOut = 0). Otherwise, if the id is known, re-matches by stored
    // descriptor against same-owner entries that still have shapes, returning the best match
    // when its score is within `maxScore`. Returns null shape if no acceptable match exists.
    // This is the "broken reference" recovery used after a face/edge id is remapped by an edit.
    TopoDS_Shape resolveWithFallback(const ElementId& id, double maxScore, double& scoreOut) const;

    bool write(std::ostream& os) const;
    bool read(std::istream& is);
    std::string toString() const;
    bool fromString(const std::string& data);

private:
    enum class ChildReason {
        Split,
        Generated
    };

    struct DescriptorKey {
        int shapeType{0};
        int surfaceType{0};
        int curveType{0};
        std::int64_t cx{0};
        std::int64_t cy{0};
        std::int64_t cz{0};
        std::int64_t nx{0};
        std::int64_t ny{0};
        std::int64_t nz{0};
        std::int64_t tx{0};
        std::int64_t ty{0};
        std::int64_t tz{0};
        std::int64_t size{0};
        std::int64_t magnitude{0};
        std::uint64_t adjacencyHash{0};

        bool operator<(const DescriptorKey& other) const;
    };

    ElementDescriptor computeDescriptor(const TopoDS_Shape& shape) const;
    double score(const ElementDescriptor& target, const ElementDescriptor& candidate) const;
    TopoDS_Shape pickBestShape(const TopTools_ListOfShape& list, const ElementDescriptor& target) const;
    ElementKind inferKind(const TopoDS_Shape& shape, ElementKind fallback) const;
    DescriptorKey makeKey(const ElementDescriptor& descriptor) const;
    std::uint64_t stableHash(const DescriptorKey& key) const;
    ElementId makeChildId(const ElementId& parent, ElementKind kind, const ElementDescriptor& descriptor,
                          const std::string& opId, ChildReason reason, std::size_t ordinal) const;
    void upsertEntry(const ElementId& id, ElementKind kind, const TopoDS_Shape& shape,
                     const ElementDescriptor& descriptor, const std::string& opId,
                     std::vector<ElementId> sources);
    void bindShape(const TopoDS_Shape& shape, const ElementId& id);
    void unbindShape(const TopoDS_Shape& shape, const ElementId& id);
    void unbindShape(const TopoDS_Shape& shape);
    static TopoDS_Shape normalizeShape(const TopoDS_Shape& shape);
    std::string kindToString(ElementKind kind) const;
    ElementKind kindFromString(const std::string& value) const;

    std::unordered_map<std::string, Entry> entries_;
    NCollection_DataMap<TopoDS_Shape, std::vector<std::string>, TopTools_ShapeMapHasher> shapeToIds_;
};

// --- Inline implementation -------------------------------------------------

inline void ElementMap::registerElement(const ElementId& id, ElementKind kind, const TopoDS_Shape& shape,
                                        const std::string& opId, std::vector<ElementId> sources) {
    if (id.value.empty()) return;
    ElementDescriptor descriptor = computeDescriptor(shape);
    upsertEntry(id, kind, shape, descriptor, opId, std::move(sources));
}

inline void ElementMap::registerEntry(const ElementId& id, ElementKind kind, const ElementDescriptor& descriptor,
                                      const std::string& opId, std::vector<ElementId> sources) {
    if (id.value.empty()) return;
    upsertEntry(id, kind, TopoDS_Shape(), descriptor, opId, std::move(sources));
}

inline bool ElementMap::attachShape(const ElementId& id, const TopoDS_Shape& shape, const std::string& opId) {
    auto it = entries_.find(id.value);
    if (it == entries_.end()) return false;

    Entry& entry = it->second;
    if (!entry.shape.IsNull()) {
        unbindShape(entry.shape, entry.id);
    }

    entry.shape = shape;
    entry.descriptor = computeDescriptor(shape);
    if (!opId.empty()) {
        entry.opId = opId;
    }
    bindShape(entry.shape, entry.id);
    return true;
}

inline const Entry* ElementMap::find(const ElementId& id) const {
    auto it = entries_.find(id.value);
    if (it == entries_.end()) return nullptr;
    return &it->second;
}

inline Entry* ElementMap::find(const ElementId& id) {
    auto it = entries_.find(id.value);
    if (it == entries_.end()) return nullptr;
    return &it->second;
}

inline bool ElementMap::contains(const ElementId& id) const {
    return entries_.find(id.value) != entries_.end();
}

inline TopoDS_Shape ElementMap::resolveWithFallback(const ElementId& id, double maxScore,
                                                    double& scoreOut) const {
    scoreOut = std::numeric_limits<double>::max();
    auto it = entries_.find(id.value);
    if (it != entries_.end() && !it->second.shape.IsNull()) {
        scoreOut = 0.0;
        return it->second.shape;  // literal id still resolves
    }
    if (it == entries_.end()) {
        return TopoDS_Shape();  // unknown id: no stored descriptor to re-match against
    }

    const Entry& stale = it->second;
    auto ownerOf = [](const std::string& value) -> std::string {
        const auto pos = value.find('/');
        return pos == std::string::npos ? std::string{} : value.substr(0, pos);
    };
    const std::string owner = ownerOf(id.value);

    const Entry* best = nullptr;
    double bestScore = std::numeric_limits<double>::max();
    double secondBestScore = std::numeric_limits<double>::max();
    for (const auto& [key, cand] : entries_) {
        if (cand.kind != stale.kind || cand.shape.IsNull()) {
            continue;
        }
        if (cand.id.value == id.value) {
            continue;
        }
        if (!owner.empty() && ownerOf(cand.id.value) != owner) {
            continue;  // restrict the search to the same owning body
        }
        const double candidateScore = score(stale.descriptor, cand.descriptor);
        if (candidateScore < bestScore) {
            secondBestScore = bestScore;
            bestScore = candidateScore;
            best = &cand;
        } else if (candidateScore < secondBestScore) {
            secondBestScore = candidateScore;
        }
    }
    if (!best || bestScore > maxScore) {
        return TopoDS_Shape();
    }
    // Uniqueness margin: with two near-equidistant candidates (symmetric
    // twins) a re-match is a coin flip — refuse rather than guess.
    if (secondBestScore != std::numeric_limits<double>::max() &&
        secondBestScore - bestScore < std::max(0.5, bestScore)) {
        return TopoDS_Shape();
    }
    scoreOut = bestScore;
    return best->shape;
}

inline std::vector<ElementId> ElementMap::ids() const {
    std::vector<ElementId> out;
    out.reserve(entries_.size());
    for (auto const& [key, entry] : entries_) {
        out.push_back(entry.id);
    }
    return out;
}

inline std::vector<ElementId> ElementMap::findIdsByShape(const TopoDS_Shape& shape) const {
    std::vector<ElementId> out;
    TopoDS_Shape normalized = normalizeShape(shape);
    if (normalized.IsNull()) return out;
    if (!shapeToIds_.IsBound(normalized)) return out;
    const std::vector<std::string>& ids = shapeToIds_.Find(normalized);
    out.reserve(ids.size());
    for (const auto& idValue : ids) {
        out.push_back(ElementId{idValue});
    }
    return out;
}

inline void ElementMap::clear() {
    entries_.clear();
    shapeToIds_.Clear();
}

inline void ElementMap::clearShape(const ElementId& id) {
    auto it = entries_.find(id.value);
    if (it == entries_.end()) {
        return;
    }
    if (!it->second.shape.IsNull()) {
        unbindShape(it->second.shape, it->second.id);
    }
    it->second.shape.Nullify();
}

inline void ElementMap::removeElementsForBody(const std::string& bodyId) {
    if (bodyId.empty()) {
        return;
    }
    const std::string prefix = bodyId + "/";
    for (auto it = entries_.begin(); it != entries_.end();) {
        const std::string& id = it->first;
        if (id == bodyId || id.rfind(prefix, 0) == 0) {
            if (!it->second.shape.IsNull()) {
                unbindShape(it->second.shape, it->second.id);
            }
            it = entries_.erase(it);
        } else {
            ++it;
        }
    }
}

inline void ElementMap::rebindBody(const std::string& bodyId, const TopoDS_Shape& shape,
                                   const std::string& opId) {
    if (bodyId.empty() || shape.IsNull()) {
        return;
    }

    // Rigid-translation compensation: when the whole body moved (its bbox
    // center shifted), stale descriptors are compared as if translated by the
    // same delta, so a pure move keeps every ID without inflating scores.
    // Capture the OLD body center before attachShape() recomputes it.
    gp_Vec bodyCenterDelta(0.0, 0.0, 0.0);
    const ElementDescriptor newBodyDescriptor = computeDescriptor(shape);
    ElementId bodyElem = ElementId::From(bodyId);
    if (const Entry* bodyEntry = find(bodyElem)) {
        if (bodyEntry->descriptor.shapeType != TopAbs_SHAPE) {
            bodyCenterDelta = gp_Vec(bodyEntry->descriptor.center, newBodyDescriptor.center);
        }
    }
    // Accept a rebind only when it is geometrically close: beyond this the
    // "match" would silently land on unrelated geometry — losing the shape
    // (explicit downstream failure) beats resolving to the wrong face.
    const double maxAcceptScore = std::max(1.0, 0.5 * newBodyDescriptor.size);

    if (contains(bodyElem)) {
        attachShape(bodyElem, shape, opId);
    } else {
        registerElement(bodyElem, ElementKind::Body, shape, opId);
    }

    struct Candidate {
        TopoDS_Shape shape;
        ElementDescriptor descriptor;
        DescriptorKey key;
        bool assigned{false};
    };

    auto collectEntries = [&](ElementKind kind) {
        std::vector<Entry*> entries;
        const std::string prefix = bodyId + "/";
        for (auto& [key, entry] : entries_) {
            if (entry.kind != kind) {
                continue;
            }
            if (entry.id.value == bodyId || entry.id.value.rfind(prefix, 0) == 0) {
                entries.push_back(&entry);
            }
        }
        std::sort(entries.begin(), entries.end(),
                  [](const Entry* a, const Entry* b) {
                      return a->id.value < b->id.value;
                  });
        return entries;
    };

    auto collectCandidates = [&](TopAbs_ShapeEnum kind) {
        std::vector<Candidate> candidates;
        TopTools_IndexedMapOfShape map;
        TopExp::MapShapes(shape, kind, map);
        candidates.reserve(static_cast<std::size_t>(map.Extent()));
        for (int i = 1; i <= map.Extent(); ++i) {
            TopoDS_Shape candidateShape = map(i);
            ElementDescriptor descriptor = computeDescriptor(candidateShape);
            candidates.push_back(Candidate{
                candidateShape,
                descriptor,
                makeKey(descriptor),
                false
            });
        }
        std::sort(candidates.begin(), candidates.end(),
                  [](const Candidate& a, const Candidate& b) {
                      return a.key < b.key;
                  });
        return candidates;
    };

    auto matchKind = [&](ElementKind kind, TopAbs_ShapeEnum shapeKind) {
        auto entries = collectEntries(kind);
        auto candidates = collectCandidates(shapeKind);

        // Identity pre-pass: an entry whose stored shape survives IsSame() in
        // the new body keeps it outright. This makes exact assignments (e.g.
        // from OCCT-history update()) sticky — the greedy scoring pass below
        // can no longer steal them in entry-id order.
        std::vector<Entry*> unresolved;
        unresolved.reserve(entries.size());
        for (auto* entry : entries) {
            bool identity = false;
            if (!entry->shape.IsNull()) {
                for (auto& candidate : candidates) {
                    if (!candidate.assigned && candidate.shape.IsSame(entry->shape)) {
                        attachShape(entry->id, candidate.shape, opId);
                        candidate.assigned = true;
                        identity = true;
                        break;
                    }
                }
            }
            if (!identity) {
                unresolved.push_back(entry);
            }
        }

        // Global best-first assignment: score every viable (entry, candidate)
        // pair and assign in ascending-score order with deterministic
        // tie-breaks. Per-entry greedy matching in entry-id order let a stale
        // entry steal another entry's exact candidate just because its id
        // sorted first.
        struct ScoredPair {
            double value;
            std::size_t entryIndex;
            std::size_t candidateIndex;
        };
        std::vector<ScoredPair> pairs;
        for (std::size_t e = 0; e < unresolved.size(); ++e) {
            // Compare as if the stale descriptor moved with the body.
            ElementDescriptor shifted = unresolved[e]->descriptor;
            shifted.center.Translate(bodyCenterDelta);
            for (std::size_t c = 0; c < candidates.size(); ++c) {
                if (candidates[c].assigned) {
                    continue;
                }
                // Hard-skip cross-type candidates: the soft score penalties
                // allowed a face id to silently migrate onto a face of a
                // different surface type (or edge curve type) when the real
                // geometry disappeared.
                if (candidates[c].descriptor.surfaceType != shifted.surfaceType ||
                    candidates[c].descriptor.curveType != shifted.curveType) {
                    continue;
                }
                const double value = score(shifted, candidates[c].descriptor);
                if (value <= maxAcceptScore) {
                    pairs.push_back(ScoredPair{value, e, c});
                }
            }
        }
        std::sort(pairs.begin(), pairs.end(),
                  [&](const ScoredPair& a, const ScoredPair& b) {
                      if (a.value != b.value) {
                          return a.value < b.value;
                      }
                      // entries_ is unordered: ties must not be resolved by
                      // hash iteration order.
                      if (unresolved[a.entryIndex]->id.value != unresolved[b.entryIndex]->id.value) {
                          return unresolved[a.entryIndex]->id.value < unresolved[b.entryIndex]->id.value;
                      }
                      return a.candidateIndex < b.candidateIndex;
                  });

        std::vector<bool> entryAssigned(unresolved.size(), false);
        for (const auto& pair : pairs) {
            if (entryAssigned[pair.entryIndex] || candidates[pair.candidateIndex].assigned) {
                continue;
            }
            attachShape(unresolved[pair.entryIndex]->id,
                        candidates[pair.candidateIndex].shape, opId);
            candidates[pair.candidateIndex].assigned = true;
            entryAssigned[pair.entryIndex] = true;
        }
        for (std::size_t e = 0; e < unresolved.size(); ++e) {
            if (!entryAssigned[e]) {
                clearShape(unresolved[e]->id);
            }
        }

        std::size_t ordinal = 0;
        for (auto& candidate : candidates) {
            if (candidate.assigned) {
                continue;
            }
            ElementId childId = makeChildId(bodyElem, kind, candidate.descriptor, opId,
                                            ChildReason::Generated, ordinal++);
            upsertEntry(childId, kind, candidate.shape, candidate.descriptor, opId, {bodyElem});
        }
    };

    matchKind(ElementKind::Face, TopAbs_FACE);
    matchKind(ElementKind::Edge, TopAbs_EDGE);
    matchKind(ElementKind::Vertex, TopAbs_VERTEX);
}

inline bool ElementMap::DescriptorKey::operator<(const DescriptorKey& other) const {
    if (shapeType != other.shapeType) return shapeType < other.shapeType;
    if (surfaceType != other.surfaceType) return surfaceType < other.surfaceType;
    if (curveType != other.curveType) return curveType < other.curveType;
    if (cx != other.cx) return cx < other.cx;
    if (cy != other.cy) return cy < other.cy;
    if (cz != other.cz) return cz < other.cz;
    if (nx != other.nx) return nx < other.nx;
    if (ny != other.ny) return ny < other.ny;
    if (nz != other.nz) return nz < other.nz;
    if (tx != other.tx) return tx < other.tx;
    if (ty != other.ty) return ty < other.ty;
    if (tz != other.tz) return tz < other.tz;
    if (size != other.size) return size < other.size;
    if (magnitude != other.magnitude) return magnitude < other.magnitude;
    return adjacencyHash < other.adjacencyHash;
}

inline ElementDescriptor ElementMap::computeDescriptor(const TopoDS_Shape& shape) const {
    ElementDescriptor desc;
    if (shape.IsNull()) return desc;

    desc.shapeType = shape.ShapeType();

    Bnd_Box box;
    BRepBndLib::Add(shape, box);
    if (!box.IsVoid()) {
        Standard_Real xmin, ymin, zmin, xmax, ymax, zmax;
        box.Get(xmin, ymin, zmin, xmax, ymax, zmax);
        desc.center = gp_Pnt((xmin + xmax) * 0.5, (ymin + ymax) * 0.5, (zmin + zmax) * 0.5);
        const double dx = xmax - xmin;
        const double dy = ymax - ymin;
        const double dz = zmax - zmin;
        desc.size = std::sqrt(dx * dx + dy * dy + dz * dz);
    }

    GProp_GProps props;
    switch (desc.shapeType) {
    case TopAbs_FACE:
    {
        const TopoDS_Face face = TopoDS::Face(shape);
        BRepGProp::SurfaceProperties(face, props);
        desc.magnitude = props.Mass();
        BRepAdaptor_Surface surface(face, true);
        desc.surfaceType = surface.GetType();

        double uMin = 0.0;
        double uMax = 0.0;
        double vMin = 0.0;
        double vMax = 0.0;
        BRepTools::UVBounds(face, uMin, uMax, vMin, vMax);
        const double uMid = 0.5 * (uMin + uMax);
        const double vMid = 0.5 * (vMin + vMax);
        gp_Pnt midPoint;
        gp_Vec dU;
        gp_Vec dV;
        surface.D1(uMid, vMid, midPoint, dU, dV);
        const gp_Vec normalVec = dU.Crossed(dV);
        if (normalVec.Magnitude() > 0.0) {
            desc.normal = gp_Dir(normalVec);
            desc.hasNormal = true;
        }

        std::vector<std::int64_t> edgeKeys;
        TopExp_Explorer edgeExp(face, TopAbs_EDGE);
        for (; edgeExp.More(); edgeExp.Next()) {
            const TopoDS_Edge edge = TopoDS::Edge(edgeExp.Current());
            GProp_GProps edgeProps;
            BRepGProp::LinearProperties(edge, edgeProps);
            const double edgeLength = edgeProps.Mass();
            edgeKeys.push_back(static_cast<std::int64_t>(std::llround(edgeLength * 1e6)));
        }
        std::sort(edgeKeys.begin(), edgeKeys.end());
        std::uint64_t hash = 14695981039346656037ull;
        for (const auto key : edgeKeys) {
            hash ^= static_cast<std::uint64_t>(key);
            hash *= 1099511628211ull;
        }
        desc.adjacencyHash = hash;
        break;
    }
    case TopAbs_EDGE:
    {
        const TopoDS_Edge edge = TopoDS::Edge(shape);
        BRepGProp::LinearProperties(edge, props);
        desc.magnitude = props.Mass();
        BRepAdaptor_Curve curve(edge);
        desc.curveType = curve.GetType();
        const double first = curve.FirstParameter();
        const double last = curve.LastParameter();
        if (first != last) {
            const double mid = 0.5 * (first + last);
            gp_Pnt midPoint;
            gp_Vec d1;
            curve.D1(mid, midPoint, d1);
            if (d1.Magnitude() > 0.0) {
                desc.tangent = gp_Dir(d1);
                desc.hasTangent = true;
            }
        }
        std::uint64_t hash = 14695981039346656037ull;
        hash ^= static_cast<std::uint64_t>(std::llround(desc.magnitude * 1e6));
        hash *= 1099511628211ull;
        int vertexCount = 0;
        TopExp_Explorer vExp(edge, TopAbs_VERTEX);
        for (; vExp.More(); vExp.Next()) {
            const TopoDS_Vertex vertex = TopoDS::Vertex(vExp.Current());
            const gp_Pnt pnt = BRep_Tool::Pnt(vertex);
            hash ^= static_cast<std::uint64_t>(std::llround((pnt.X() - desc.center.X()) * 1e6));
            hash *= 1099511628211ull;
            hash ^= static_cast<std::uint64_t>(std::llround((pnt.Y() - desc.center.Y()) * 1e6));
            hash *= 1099511628211ull;
            hash ^= static_cast<std::uint64_t>(std::llround((pnt.Z() - desc.center.Z()) * 1e6));
            hash *= 1099511628211ull;
            ++vertexCount;
        }
        hash ^= static_cast<std::uint64_t>(vertexCount);
        hash *= 1099511628211ull;
        desc.adjacencyHash = hash;
        break;
    }
    case TopAbs_SOLID:
    case TopAbs_COMPOUND:
    case TopAbs_COMPSOLID:
        BRepGProp::VolumeProperties(shape, props);
        desc.magnitude = props.Mass();
        break;
    default:
        desc.magnitude = 0.0;
        break;
    }

    return desc;
}

inline double ElementMap::score(const ElementDescriptor& target, const ElementDescriptor& candidate) const {
    const double centerDistance = target.center.Distance(candidate.center);
    const double sizeDiff = std::abs(target.size - candidate.size);
    const double magDiff = std::abs(target.magnitude - candidate.magnitude);
    double scoreValue = centerDistance + 0.1 * sizeDiff + 0.01 * magDiff;

    if (target.shapeType != candidate.shapeType) {
        scoreValue += 1000.0;
    }
    if (target.surfaceType != candidate.surfaceType) {
        scoreValue += 10.0;
    }
    if (target.curveType != candidate.curveType) {
        scoreValue += 5.0;
    }
    if (target.hasNormal && candidate.hasNormal) {
        const double dot = target.normal.Dot(candidate.normal);
        scoreValue += (1.0 - std::clamp(dot, -1.0, 1.0)) * 2.0;
    } else if (target.hasNormal != candidate.hasNormal) {
        scoreValue += 1.0;
    }
    if (target.hasTangent && candidate.hasTangent) {
        const double dot = target.tangent.Dot(candidate.tangent);
        scoreValue += (1.0 - std::clamp(dot, -1.0, 1.0));
    } else if (target.hasTangent != candidate.hasTangent) {
        scoreValue += 0.5;
    }
    if (target.adjacencyHash != 0 && candidate.adjacencyHash != 0 &&
        target.adjacencyHash != candidate.adjacencyHash) {
        scoreValue += 1.0;
    }

    return scoreValue;
}

inline TopoDS_Shape ElementMap::pickBestShape(const TopTools_ListOfShape& list, const ElementDescriptor& target) const {
    if (list.IsEmpty()) return TopoDS_Shape();
    if (list.Extent() == 1) return list.First();

    TopoDS_Shape best;
    double bestScore = std::numeric_limits<double>::max();

    for (TopTools_ListIteratorOfListOfShape it(list); it.More(); it.Next()) {
        const TopoDS_Shape& candidate = it.Value();
        const ElementDescriptor candDesc = computeDescriptor(candidate);
        const double s = score(target, candDesc);
        if (s < bestScore) {
            bestScore = s;
            best = candidate;
        }
    }

    return best;
}

inline ElementKind ElementMap::inferKind(const TopoDS_Shape& shape, ElementKind fallback) const {
    switch (shape.ShapeType()) {
    case TopAbs_FACE: return ElementKind::Face;
    case TopAbs_EDGE: return ElementKind::Edge;
    case TopAbs_VERTEX: return ElementKind::Vertex;
    case TopAbs_SOLID: return ElementKind::Body;
    default: return fallback;
    }
}

inline ElementMap::DescriptorKey ElementMap::makeKey(const ElementDescriptor& descriptor) const {
    constexpr double kPositionQuant = 1e-6;
    constexpr double kSizeQuant = 1e-6;
    constexpr double kMagnitudeQuant = 1e-6;
    constexpr double kDirQuant = 1e-6;

    const auto quantize = [](double value, double step) -> std::int64_t {
        return static_cast<std::int64_t>(std::llround(value / step));
    };

    DescriptorKey key;
    key.shapeType = static_cast<int>(descriptor.shapeType);
    key.surfaceType = static_cast<int>(descriptor.surfaceType);
    key.curveType = static_cast<int>(descriptor.curveType);
    key.cx = quantize(descriptor.center.X(), kPositionQuant);
    key.cy = quantize(descriptor.center.Y(), kPositionQuant);
    key.cz = quantize(descriptor.center.Z(), kPositionQuant);
    if (descriptor.hasNormal) {
        key.nx = quantize(descriptor.normal.X(), kDirQuant);
        key.ny = quantize(descriptor.normal.Y(), kDirQuant);
        key.nz = quantize(descriptor.normal.Z(), kDirQuant);
    }
    if (descriptor.hasTangent) {
        key.tx = quantize(descriptor.tangent.X(), kDirQuant);
        key.ty = quantize(descriptor.tangent.Y(), kDirQuant);
        key.tz = quantize(descriptor.tangent.Z(), kDirQuant);
    }
    key.size = quantize(descriptor.size, kSizeQuant);
    key.magnitude = quantize(descriptor.magnitude, kMagnitudeQuant);
    key.adjacencyHash = descriptor.adjacencyHash;
    return key;
}

inline std::uint64_t ElementMap::stableHash(const DescriptorKey& key) const {
    auto hashStep = [](std::uint64_t h, std::uint64_t value) {
        h ^= value;
        h *= 1099511628211ull;
        return h;
    };

    std::uint64_t h = 14695981039346656037ull;
    h = hashStep(h, static_cast<std::uint64_t>(key.shapeType));
    h = hashStep(h, static_cast<std::uint64_t>(key.surfaceType));
    h = hashStep(h, static_cast<std::uint64_t>(key.curveType));
    h = hashStep(h, static_cast<std::uint64_t>(key.cx));
    h = hashStep(h, static_cast<std::uint64_t>(key.cy));
    h = hashStep(h, static_cast<std::uint64_t>(key.cz));
    h = hashStep(h, static_cast<std::uint64_t>(key.nx));
    h = hashStep(h, static_cast<std::uint64_t>(key.ny));
    h = hashStep(h, static_cast<std::uint64_t>(key.nz));
    h = hashStep(h, static_cast<std::uint64_t>(key.tx));
    h = hashStep(h, static_cast<std::uint64_t>(key.ty));
    h = hashStep(h, static_cast<std::uint64_t>(key.tz));
    h = hashStep(h, static_cast<std::uint64_t>(key.size));
    h = hashStep(h, static_cast<std::uint64_t>(key.magnitude));
    h = hashStep(h, key.adjacencyHash);
    return h;
}

inline ElementId ElementMap::makeChildId(const ElementId& parent, ElementKind kind,
                                         const ElementDescriptor& descriptor, const std::string& opId,
                                         ChildReason reason, std::size_t ordinal) const {
    std::string suffix;
    switch (kind) {
    case ElementKind::Face: suffix = "face"; break;
    case ElementKind::Edge: suffix = "edge"; break;
    case ElementKind::Vertex: suffix = "vertex"; break;
    case ElementKind::Body: suffix = "body"; break;
    default: suffix = "elem"; break;
    }

    std::string reasonTag = (reason == ChildReason::Split) ? "split" : "gen";
    const DescriptorKey key = makeKey(descriptor);
    const std::uint64_t hash = stableHash(key);
    std::ostringstream oss;
    if (!parent.value.empty()) {
        oss << parent.value << "/";
    }
    oss << suffix << "-" << reasonTag;
    if (!opId.empty()) {
        oss << "-" << opId;
    }
    oss << "-" << std::hex << hash << std::dec << "-" << ordinal;
    return ElementId{oss.str()};
}

inline void ElementMap::upsertEntry(const ElementId& id, ElementKind kind, const TopoDS_Shape& shape,
                                    const ElementDescriptor& descriptor, const std::string& opId,
                                    std::vector<ElementId> sources) {
    auto it = entries_.find(id.value);
    if (it != entries_.end() && !it->second.shape.IsNull()) {
        unbindShape(it->second.shape, id);
    }

    Entry entry{ id, kind, shape, descriptor, opId, std::move(sources) };
    entries_[id.value] = entry;
    bindShape(shape, id);
}

inline void ElementMap::bindShape(const TopoDS_Shape& shape, const ElementId& id) {
    TopoDS_Shape normalized = normalizeShape(shape);
    if (normalized.IsNull()) return;
    if (!shapeToIds_.IsBound(normalized)) {
        shapeToIds_.Bind(normalized, {});
    }
    std::vector<std::string>& ids = shapeToIds_.ChangeFind(normalized);
    if (std::find(ids.begin(), ids.end(), id.value) == ids.end()) {
        ids.push_back(id.value);
    }
}

inline void ElementMap::unbindShape(const TopoDS_Shape& shape, const ElementId& id) {
    TopoDS_Shape normalized = normalizeShape(shape);
    if (normalized.IsNull()) return;
    if (!shapeToIds_.IsBound(normalized)) return;
    std::vector<std::string>& ids = shapeToIds_.ChangeFind(normalized);
    ids.erase(std::remove(ids.begin(), ids.end(), id.value), ids.end());
    if (ids.empty()) {
        shapeToIds_.UnBind(normalized);
    }
}

inline void ElementMap::unbindShape(const TopoDS_Shape& shape) {
    TopoDS_Shape normalized = normalizeShape(shape);
    if (normalized.IsNull()) return;
    if (shapeToIds_.IsBound(normalized)) {
        shapeToIds_.UnBind(normalized);
    }
}

inline TopoDS_Shape ElementMap::normalizeShape(const TopoDS_Shape& shape) {
    if (shape.IsNull()) {
        return shape;
    }
    if (shape.Orientation() == TopAbs_FORWARD) {
        return shape;
    }
    return shape.Oriented(TopAbs_FORWARD);
}

inline std::string ElementMap::kindToString(ElementKind kind) const {
    switch (kind) {
    case ElementKind::Body: return "body";
    case ElementKind::Face: return "face";
    case ElementKind::Edge: return "edge";
    case ElementKind::Vertex: return "vertex";
    default: return "unknown";
    }
}

inline ElementKind ElementMap::kindFromString(const std::string& value) const {
    if (value == "body") return ElementKind::Body;
    if (value == "face") return ElementKind::Face;
    if (value == "edge") return ElementKind::Edge;
    if (value == "vertex") return ElementKind::Vertex;
    return ElementKind::Unknown;
}

inline std::vector<ElementId> ElementMap::update(BRepAlgoAPI_BooleanOperation& algo, const std::string& opId) {
    std::vector<ElementId> deleted;
    struct PendingEntry {
        ElementId id;
        ElementKind kind{ElementKind::Unknown};
        TopoDS_Shape shape;
        ElementDescriptor descriptor;
        std::string opId;
        std::vector<ElementId> sources;
    };

    std::vector<PendingEntry> pending;
    std::unordered_map<std::string, std::size_t> pendingById;
    NCollection_DataMap<TopoDS_Shape, std::vector<std::string>, TopTools_ShapeMapHasher> pendingShapeToIds;

    // Track shapes to erase after iteration to avoid invalidating iterators.
    std::vector<std::pair<std::string, TopoDS_Shape>> toErase;

    auto appendSource = [](std::vector<ElementId>& sources, const ElementId& source) {
        const bool exists = std::any_of(sources.begin(), sources.end(),
                                        [&](const ElementId& existing) {
                                            return existing.value == source.value;
                                        });
        if (!exists) {
            sources.push_back(source);
        }
    };

    auto collectIdsByShape = [&](const TopoDS_Shape& shape) {
        std::vector<ElementId> ids = findIdsByShape(shape);
        if (pendingShapeToIds.IsBound(shape)) {
            const std::vector<std::string>& pendingIds = pendingShapeToIds.Find(shape);
            ids.reserve(ids.size() + pendingIds.size());
            for (const auto& idValue : pendingIds) {
                ids.push_back(ElementId{idValue});
            }
        }
        return ids;
    };

    auto registerPending = [&](PendingEntry entry) {
        const auto index = pending.size();
        pendingById[entry.id.value] = index;
        if (!entry.shape.IsNull()) {
            if (!pendingShapeToIds.IsBound(entry.shape)) {
                pendingShapeToIds.Bind(entry.shape, {});
            }
            std::vector<std::string>& ids = pendingShapeToIds.ChangeFind(entry.shape);
            if (std::find(ids.begin(), ids.end(), entry.id.value) == ids.end()) {
                ids.push_back(entry.id.value);
            }
        }
        pending.push_back(std::move(entry));
    };

    auto addSourceToId = [&](const ElementId& id, const ElementId& source) {
        if (Entry* existing = find(id)) {
            appendSource(existing->sources, source);
            return;
        }
        auto it = pendingById.find(id.value);
        if (it != pendingById.end()) {
            appendSource(pending[it->second].sources, source);
        }
    };

    for (auto& [key, entry] : entries_) {
        const TopoDS_Shape oldShape = entry.shape;
        if (oldShape.IsNull()) {
            continue;
        }

        if (algo.IsDeleted(oldShape)) {
            deleted.push_back(entry.id);
            toErase.emplace_back(key, oldShape);
            continue;
        }

        const TopTools_ListOfShape& modified = algo.Modified(oldShape);
        if (!modified.IsEmpty()) {
            if (modified.Extent() == 1) {
                const TopoDS_Shape best = modified.First();
                if (!best.IsNull()) {
                    unbindShape(oldShape, entry.id);
                    entry.shape = best;
                    entry.descriptor = computeDescriptor(best);
                    entry.opId = opId;
                    bindShape(entry.shape, entry.id);
                }
            } else {
                struct Candidate {
                    TopoDS_Shape shape;
                    ElementDescriptor descriptor;
                    DescriptorKey key;
                    double score{0.0};
                };

                std::vector<Candidate> candidates;
                candidates.reserve(modified.Extent());
                for (TopTools_ListIteratorOfListOfShape it(modified); it.More(); it.Next()) {
                    const TopoDS_Shape& candidateShape = it.Value();
                    ElementDescriptor descriptor = computeDescriptor(candidateShape);
                    candidates.push_back(Candidate{
                        candidateShape,
                        descriptor,
                        makeKey(descriptor),
                        score(entry.descriptor, descriptor)
                    });
                }

                auto bestIt = std::min_element(candidates.begin(), candidates.end(),
                                               [](const Candidate& a, const Candidate& b) {
                                                   if (a.score != b.score) {
                                                       return a.score < b.score;
                                                   }
                                                   return a.key < b.key;
                                               });

                if (bestIt != candidates.end()) {
                    unbindShape(oldShape, entry.id);
                    entry.shape = bestIt->shape;
                    entry.descriptor = bestIt->descriptor;
                    entry.opId = opId;
                    bindShape(entry.shape, entry.id);

                    std::sort(candidates.begin(), candidates.end(),
                              [](const Candidate& a, const Candidate& b) {
                                  return a.key < b.key;
                              });

                    for (std::size_t index = 0; index < candidates.size(); ++index) {
                        const Candidate& candidate = candidates[index];
                        if (candidate.shape.IsSame(entry.shape)) {
                            continue;
                        }

                        const auto existingIds = collectIdsByShape(candidate.shape);
                        if (!existingIds.empty()) {
                            for (const auto& existingId : existingIds) {
                                addSourceToId(existingId, entry.id);
                            }
                            continue;
                        }

                        ElementId childId = makeChildId(entry.id, entry.kind, candidate.descriptor, opId,
                                                        ChildReason::Split, index);
                        registerPending(PendingEntry{
                            childId,
                            entry.kind,
                            candidate.shape,
                            candidate.descriptor,
                            opId,
                            {entry.id}
                        });
                    }
                }
            }
        }

        const TopTools_ListOfShape& generated = algo.Generated(oldShape);
        if (!generated.IsEmpty()) {
            struct GeneratedCandidate {
                TopoDS_Shape shape;
                ElementDescriptor descriptor;
                DescriptorKey key;
            };

            std::vector<GeneratedCandidate> candidates;
            candidates.reserve(generated.Extent());
            for (TopTools_ListIteratorOfListOfShape git(generated); git.More(); git.Next()) {
                const TopoDS_Shape& child = git.Value();
                ElementDescriptor descriptor = computeDescriptor(child);
                candidates.push_back(GeneratedCandidate{child, descriptor, makeKey(descriptor)});
            }

            std::sort(candidates.begin(), candidates.end(),
                      [](const GeneratedCandidate& a, const GeneratedCandidate& b) {
                          return a.key < b.key;
                      });

            for (std::size_t index = 0; index < candidates.size(); ++index) {
                const GeneratedCandidate& candidate = candidates[index];
                const auto existingIds = collectIdsByShape(candidate.shape);
                if (!existingIds.empty()) {
                    for (const auto& existingId : existingIds) {
                        addSourceToId(existingId, entry.id);
                    }
                    continue;
                }

                ElementKind childKind = inferKind(candidate.shape, entry.kind);
                ElementId childId = makeChildId(entry.id, childKind, candidate.descriptor, opId,
                                                ChildReason::Generated, index);
                registerPending(PendingEntry{
                    childId,
                    childKind,
                    candidate.shape,
                    candidate.descriptor,
                    opId,
                    {entry.id}
                });
            }
        }
    }

    for (auto const& [idKey, shape] : toErase) {
        if (auto it = entries_.find(idKey); it != entries_.end()) {
            unbindShape(shape, it->second.id);
        } else {
            unbindShape(shape);
        }
        entries_.erase(idKey);
    }

    for (auto& entry : pending) {
        upsertEntry(entry.id, entry.kind, entry.shape, entry.descriptor, entry.opId, std::move(entry.sources));
    }

    return deleted;
}

inline bool ElementMap::write(std::ostream& os) const {
    os << "ElementMap v1\n";
    std::vector<const Entry*> ordered;
    ordered.reserve(entries_.size());
    for (auto const& [key, entry] : entries_) {
        ordered.push_back(&entry);
    }
    std::sort(ordered.begin(), ordered.end(),
              [](const Entry* a, const Entry* b) { return a->id.value < b->id.value; });

    os << ordered.size() << "\n";
    os << std::setprecision(17);
    for (const Entry* entry : ordered) {
        os << "entry\n";
        os << std::quoted(entry->id.value) << "\n";
        os << kindToString(entry->kind) << "\n";
        os << std::quoted(entry->opId) << "\n";
        os << entry->sources.size() << "\n";
        for (const auto& source : entry->sources) {
            os << std::quoted(source.value) << "\n";
        }
        const ElementDescriptor& d = entry->descriptor;
        os << static_cast<int>(d.shapeType) << " "
           << static_cast<int>(d.surfaceType) << " "
           << static_cast<int>(d.curveType) << "\n";
        os << d.center.X() << " " << d.center.Y() << " " << d.center.Z() << "\n";
        os << d.size << " " << d.magnitude << "\n";
        os << static_cast<int>(d.hasNormal) << " "
           << d.normal.X() << " " << d.normal.Y() << " " << d.normal.Z() << "\n";
        os << static_cast<int>(d.hasTangent) << " "
           << d.tangent.X() << " " << d.tangent.Y() << " " << d.tangent.Z() << "\n";
        os << d.adjacencyHash << "\n";
        os << "end\n";
    }

    return os.good();
}

inline bool ElementMap::read(std::istream& is) {
    clear();

    std::string header;
    std::getline(is, header);
    if (header != "ElementMap v1") {
        return false;
    }

    std::size_t count = 0;
    if (!(is >> count)) {
        return false;
    }

    for (std::size_t i = 0; i < count; ++i) {
        std::string marker;
        if (!(is >> marker) || marker != "entry") {
            return false;
        }

        std::string idValue;
        std::string kindValue;
        std::string opIdValue;
        if (!(is >> std::quoted(idValue))) {
            return false;
        }
        if (!(is >> kindValue)) {
            return false;
        }
        if (!(is >> std::quoted(opIdValue))) {
            return false;
        }

        std::size_t sourceCount = 0;
        if (!(is >> sourceCount)) {
            return false;
        }
        std::vector<ElementId> sources;
        sources.reserve(sourceCount);
        for (std::size_t s = 0; s < sourceCount; ++s) {
            std::string sourceValue;
            if (!(is >> std::quoted(sourceValue))) {
                return false;
            }
            sources.push_back(ElementId{sourceValue});
        }

        int shapeType = 0;
        int surfaceType = 0;
        int curveType = 0;
        if (!(is >> shapeType >> surfaceType >> curveType)) {
            return false;
        }

        double cx = 0.0;
        double cy = 0.0;
        double cz = 0.0;
        if (!(is >> cx >> cy >> cz)) {
            return false;
        }

        double size = 0.0;
        double magnitude = 0.0;
        if (!(is >> size >> magnitude)) {
            return false;
        }

        int hasNormal = 0;
        double nx = 0.0;
        double ny = 0.0;
        double nz = 0.0;
        if (!(is >> hasNormal >> nx >> ny >> nz)) {
            return false;
        }

        int hasTangent = 0;
        double tx = 0.0;
        double ty = 0.0;
        double tz = 0.0;
        if (!(is >> hasTangent >> tx >> ty >> tz)) {
            return false;
        }

        std::uint64_t adjacencyHash = 0;
        if (!(is >> adjacencyHash)) {
            return false;
        }

        if (!(is >> marker) || marker != "end") {
            return false;
        }

        ElementDescriptor descriptor;
        descriptor.shapeType = static_cast<TopAbs_ShapeEnum>(shapeType);
        descriptor.surfaceType = static_cast<GeomAbs_SurfaceType>(surfaceType);
        descriptor.curveType = static_cast<GeomAbs_CurveType>(curveType);
        descriptor.center = gp_Pnt(cx, cy, cz);
        descriptor.size = size;
        descriptor.magnitude = magnitude;
        descriptor.hasNormal = (hasNormal != 0);
        if (descriptor.hasNormal) {
            descriptor.normal = gp_Dir(nx, ny, nz);
        }
        descriptor.hasTangent = (hasTangent != 0);
        if (descriptor.hasTangent) {
            descriptor.tangent = gp_Dir(tx, ty, tz);
        }
        descriptor.adjacencyHash = adjacencyHash;

        registerEntry(ElementId{idValue}, kindFromString(kindValue), descriptor, opIdValue, std::move(sources));
    }

    return true;
}

inline std::string ElementMap::toString() const {
    std::ostringstream oss;
    write(oss);
    return oss.str();
}

inline bool ElementMap::fromString(const std::string& data) {
    std::istringstream iss(data);
    return read(iss);
}

} // namespace onecad::kernel::elementmap
