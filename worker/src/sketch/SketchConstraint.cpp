// Ported from OneCAD-CPP src/core/sketch/SketchConstraint.cpp @ b4ddcccc (2026-07-16)
#include "SketchConstraint.h"
#include "constraints/Constraints.h"
#include "util/Uuid.h"

#include <algorithm>
#include <utility>

namespace onecad::core::sketch {

SketchConstraint::SketchConstraint()
    : m_id(generateId()) {
}

SketchConstraint::SketchConstraint(const ConstraintID& id)
    : m_id(id.empty() ? generateId() : id) {
}

ConstraintID SketchConstraint::generateId() {
    // W-WP3a: QUuid::createUuid() -> Qt-free v4 UUID (see util/Uuid.h).
    return onecad::util::generate_uuid_v4();
}

bool SketchConstraint::references(const EntityID& entityId) const {
    const auto entities = referencedEntities();
    return std::any_of(entities.begin(), entities.end(),
                       [&](const EntityID& id) { return id == entityId; });
}

gp_Pnt2d SketchConstraint::getDimensionTextPosition(const Sketch& sketch) const {
    return getIconPosition(sketch);
}

DimensionalConstraint::DimensionalConstraint(double value)
    : SketchConstraint(),
      m_value(value) {
}

DimensionalConstraint::DimensionalConstraint(const ConstraintID& id, double value)
    : SketchConstraint(id),
      m_value(value) {
}

// W-WP3a: ConstraintFactory / JSON deserialization (registerBuiltins,
// serializeBase/deserializeBase, fromJson, detail::constraintRegistry) removed —
// serialization is Rust-owned. Constraints are constructed programmatically.

} // namespace onecad::core::sketch
