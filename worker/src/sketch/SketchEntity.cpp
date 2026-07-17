// Ported from OneCAD-CPP src/core/sketch/SketchEntity.cpp @ b4ddcccc (2026-07-16)
#include "SketchEntity.h"
#include "util/Uuid.h"


namespace onecad::core::sketch {

SketchEntity::SketchEntity()
    : m_id(generateId()) {
}

SketchEntity::SketchEntity(const EntityID& id)
    : m_id(id.empty() ? generateId() : id) {
}

EntityID SketchEntity::generateId() {
    // W-WP3a: QUuid::createUuid() -> Qt-free v4 UUID (see util/Uuid.h).
    return onecad::util::generate_uuid_v4();
}

} // namespace onecad::core::sketch
