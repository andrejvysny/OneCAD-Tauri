// Ported from OneCAD-CPP src/core/sketch/SketchPoint.cpp @ b4ddcccc (2026-07-16)
#include "SketchPoint.h"


#include <algorithm>
#include <cmath>
#include <utility>

namespace onecad::core::sketch {

SketchPoint::SketchPoint()
    : SketchEntity(),
      m_position(0.0, 0.0) {
}

SketchPoint::SketchPoint(const gp_Pnt2d& position)
    : SketchEntity(),
      m_position(position) {
}

SketchPoint::SketchPoint(double x, double y)
    : SketchEntity(),
      m_position(x, y) {
}

void SketchPoint::addConnectedEntity(const EntityID& entityId) {
    if (std::find(m_connectedEntities.begin(), m_connectedEntities.end(), entityId) != m_connectedEntities.end()) {
        return;
    }
    m_connectedEntities.push_back(entityId);
}

void SketchPoint::removeConnectedEntity(const EntityID& entityId) {
    auto it = std::remove(m_connectedEntities.begin(), m_connectedEntities.end(), entityId);
    if (it != m_connectedEntities.end()) {
        m_connectedEntities.erase(it, m_connectedEntities.end());
    }
}

BoundingBox2d SketchPoint::bounds() const {
    BoundingBox2d box;
    box.minX = m_position.X();
    box.maxX = m_position.X();
    box.minY = m_position.Y();
    box.maxY = m_position.Y();
    return box;
}

bool SketchPoint::isNear(const gp_Pnt2d& point, double tolerance) const {
    return m_position.Distance(point) <= tolerance;
}

double SketchPoint::distanceTo(const SketchPoint& other) const {
    return distanceTo(other.m_position);
}

double SketchPoint::distanceTo(const gp_Pnt2d& point) const {
    return m_position.Distance(point);
}

bool SketchPoint::coincidentWith(const SketchPoint& other, double tolerance) const {
    return distanceTo(other) <= tolerance;
}

} // namespace onecad::core::sketch
