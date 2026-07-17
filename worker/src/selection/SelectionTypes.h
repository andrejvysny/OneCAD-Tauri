// Ported from OneCAD-CPP src/app/selection/SelectionTypes.h @ b4ddcccc (2026-07-16)
// Qt-free already; carried verbatim. Only the subset consumed by the ported
// sketch constraint-applicability logic is exercised by the worker.
#ifndef ONECAD_APP_SELECTION_SELECTIONTYPES_H
#define ONECAD_APP_SELECTION_SELECTIONTYPES_H

#include <functional>
#include <optional>
#include <string>
#include <unordered_set>
#include <vector>

namespace onecad::app::selection {

enum class SelectionMode {
    Sketch,
    Model
};

enum class SelectionKind {
    None,
    SketchPoint,
    SketchEdge,
    SketchRegion,
    SketchConstraint,
    Vertex,
    Edge,
    Face,
    Body
};

struct SelectionId {
    std::string ownerId;
    std::string elementId;

    bool operator==(const SelectionId& other) const {
        return ownerId == other.ownerId && elementId == other.elementId;
    }
};

struct SelectionPoint3d {
    double x = 0.0;
    double y = 0.0;
    double z = 0.0;
};

struct SelectionItem {
    SelectionKind kind = SelectionKind::None;
    SelectionId id;
    double depth = 0.0;
    double screenDistance = 0.0; // pixels
    int priority = 0;
    SelectionPoint3d worldPos{};
    SelectionPoint3d normal{};
    bool isConstruction = false;
};

struct PickResult {
    std::vector<SelectionItem> hits;

    bool empty() const { return hits.empty(); }
};

struct SelectionFilter {
    std::unordered_set<SelectionKind> allowedKinds;

    bool allows(SelectionKind kind) const {
        return allowedKinds.empty() || allowedKinds.find(kind) != allowedKinds.end();
    }
};

struct ClickModifiers {
    bool shift = false;
    bool toggle = false;
};

struct ClickAction {
    bool needsDeepSelect = false;
    bool selectionChanged = false;
    std::vector<SelectionItem> candidates;
};

struct SelectionKey {
    SelectionKind kind = SelectionKind::None;
    SelectionId id;

    bool operator==(const SelectionKey& other) const {
        return kind == other.kind && id == other.id;
    }
};

} // namespace onecad::app::selection

namespace std {
template <>
struct hash<onecad::app::selection::SelectionId> {
    size_t operator()(const onecad::app::selection::SelectionId& id) const noexcept {
        size_t seed = std::hash<std::string>{}(id.ownerId);
        seed ^= std::hash<std::string>{}(id.elementId) + 0x9e3779b9 + (seed << 6) + (seed >> 2);
        return seed;
    }
};

template <>
struct hash<onecad::app::selection::SelectionKey> {
    size_t operator()(const onecad::app::selection::SelectionKey& key) const noexcept {
        size_t seed = std::hash<int>{}(static_cast<int>(key.kind));
        seed ^= std::hash<onecad::app::selection::SelectionId>{}(key.id) + 0x9e3779b9 +
                (seed << 6) + (seed >> 2);
        return seed;
    }
};
} // namespace std

#endif // ONECAD_APP_SELECTION_SELECTIONTYPES_H
