// Ported from OneCAD-CPP src/ui/sketch/ConstraintApplicability.cpp @ b4ddcccc (2026-07-16)
// Qt-free. W-WP3a: include paths repointed for the worker layout
// (../../core/sketch -> local).
#include "ConstraintApplicability.h"

#include "Sketch.h"
#include "SketchLine.h"

#include <unordered_set>

namespace onecad::ui {

namespace {

using onecad::app::selection::SelectionItem;
using onecad::app::selection::SelectionKind;
using onecad::core::sketch::ConstraintType;
using onecad::core::sketch::EntityID;
using onecad::core::sketch::EntityType;
using onecad::core::sketch::Sketch;

bool isCurveType(EntityType type) {
    return type == EntityType::Line ||
           type == EntityType::Arc ||
           type == EntityType::Circle;
}

bool isCircularType(EntityType type) {
    return type == EntityType::Arc || type == EntityType::Circle;
}

bool hasLineBetweenPoints(const Sketch* sketch, const EntityID& pointA, const EntityID& pointB) {
    if (!sketch) {
        return false;
    }

    for (const auto& entityPtr : sketch->getAllEntities()) {
        auto* line = dynamic_cast<const onecad::core::sketch::SketchLine*>(entityPtr.get());
        if (!line) {
            continue;
        }
        const bool matches = (line->startPointId() == pointA && line->endPointId() == pointB) ||
                             (line->startPointId() == pointB && line->endPointId() == pointA);
        if (matches) {
            return true;
        }
    }

    return false;
}

} // namespace

ConstraintApplicabilityResult evaluateConstraintApplicability(
    const Sketch* sketch,
    const std::vector<SelectionItem>& selection) {
    ConstraintApplicabilityResult result;
    if (!sketch) {
        return result;
    }

    std::vector<EntityID> selectedEntityIds;
    selectedEntityIds.reserve(selection.size());
    std::unordered_set<EntityID> seen;
    for (const auto& item : selection) {
        if (item.kind != SelectionKind::SketchPoint && item.kind != SelectionKind::SketchEdge) {
            continue;
        }
        if (item.id.elementId.empty()) {
            continue;
        }
        if (seen.insert(item.id.elementId).second) {
            selectedEntityIds.push_back(item.id.elementId);
        }
    }

    const size_t selectedCount = selectedEntityIds.size();
    if (selectedCount == 0) {
        return result;
    }

    std::vector<const onecad::core::sketch::SketchEntity*> selectedEntities;
    selectedEntities.reserve(selectedCount);
    for (const auto& id : selectedEntityIds) {
        const auto* entity = sketch->getEntity(id);
        if (!entity) {
            return result;
        }
        selectedEntities.push_back(entity);
    }

    if (selectedCount == 1) {
        const auto* entity = selectedEntities[0];
        const EntityType type = entity->type();
        if (type == EntityType::Line) {
            result.applicableConstraints.insert(ConstraintType::Horizontal);
            result.applicableConstraints.insert(ConstraintType::Vertical);
        }
        if (type == EntityType::Point) {
            result.applicableConstraints.insert(ConstraintType::Fixed);
        }
        if (type == EntityType::Arc || type == EntityType::Circle) {
            result.applicableConstraints.insert(ConstraintType::Radius);
            result.applicableConstraints.insert(ConstraintType::Diameter);
        }
        return result;
    }

    if (selectedCount == 3) {
        int pointCount = 0;
        int lineCount = 0;
        for (const auto* entity : selectedEntities) {
            if (entity->type() == EntityType::Point) {
                ++pointCount;
            } else if (entity->type() == EntityType::Line) {
                ++lineCount;
            }
        }
        if (pointCount == 2 && lineCount == 1) {
            result.applicableConstraints.insert(ConstraintType::Symmetric);
        }
        return result;
    }

    if (selectedCount != 2) {
        return result;
    }

    const auto* entityA = selectedEntities[0];
    const auto* entityB = selectedEntities[1];
    const EntityType typeA = entityA->type();
    const EntityType typeB = entityB->type();

    const bool aPoint = typeA == EntityType::Point;
    const bool bPoint = typeB == EntityType::Point;
    const bool aLine = typeA == EntityType::Line;
    const bool bLine = typeB == EntityType::Line;
    const bool aCurve = isCurveType(typeA);
    const bool bCurve = isCurveType(typeB);

    if ((aPoint || aLine) && (bPoint || bLine)) {
        result.applicableConstraints.insert(ConstraintType::Distance);
    }

    if (aPoint && bPoint) {
        result.applicableConstraints.insert(ConstraintType::Coincident);
        result.applicableConstraints.insert(ConstraintType::HorizontalDistance);
        result.applicableConstraints.insert(ConstraintType::VerticalDistance);
        if (hasLineBetweenPoints(sketch, selectedEntityIds[0], selectedEntityIds[1])) {
            result.applicableConstraints.insert(ConstraintType::Horizontal);
            result.applicableConstraints.insert(ConstraintType::Vertical);
        }
    }

    if (isCircularType(typeA) && isCircularType(typeB)) {
        result.applicableConstraints.insert(ConstraintType::Concentric);
    }

    if (aLine && bLine) {
        result.applicableConstraints.insert(ConstraintType::Parallel);
        result.applicableConstraints.insert(ConstraintType::Perpendicular);
        result.applicableConstraints.insert(ConstraintType::Angle);
    }

    if ((aPoint && bCurve) || (bPoint && aCurve)) {
        result.applicableConstraints.insert(ConstraintType::OnCurve);
    }

    return result;
}

} // namespace onecad::ui
