// Ported from OneCAD-CPP src/core/sketch/SketchCircle.cpp @ b4ddcccc (2026-07-16)
#include "SketchCircle.h"


#include <algorithm>
#include <cmath>
#include <numbers>
#include <utility>
#include "util/Log.h"

namespace onecad::core::sketch {

SketchCircle::SketchCircle()
    : SketchEntity() {
}

SketchCircle::SketchCircle(const PointID& centerPointId, double radius)
    : SketchEntity(),
      m_centerPointId(centerPointId),
      m_radius(std::max(0.0, radius)) {
    if (radius < 0.0) {
        WLOG_WARN("%s", "ctor:negative-radius-clamped");
    }
}

gp_Pnt2d SketchCircle::pointAtAngle(const gp_Pnt2d& centerPos, double angle) const {
    return gp_Pnt2d(centerPos.X() + m_radius * std::cos(angle),
                    centerPos.Y() + m_radius * std::sin(angle));
}

gp_Vec2d SketchCircle::tangentAtAngle(double angle) const {
    return gp_Vec2d(-std::sin(angle), std::cos(angle));
}

bool SketchCircle::containsPoint(const gp_Pnt2d& centerPos, const gp_Pnt2d& point) const {
    return centerPos.Distance(point) < m_radius;
}

double SketchCircle::distanceToEdge(const gp_Pnt2d& centerPos, const gp_Pnt2d& point) const {
    return centerPos.Distance(point) - m_radius;
}

BoundingBox2d SketchCircle::bounds() const {
    return {};
}

bool SketchCircle::isNear(const gp_Pnt2d&, double) const {
    return false;
}

BoundingBox2d SketchCircle::boundsWithCenter(const gp_Pnt2d& centerPos) const {
    BoundingBox2d box;
    box.minX = centerPos.X() - m_radius;
    box.maxX = centerPos.X() + m_radius;
    box.minY = centerPos.Y() - m_radius;
    box.maxY = centerPos.Y() + m_radius;
    return box;
}

bool SketchCircle::isNearWithCenter(const gp_Pnt2d& testPoint, const gp_Pnt2d& centerPos,
                                    double tolerance) const {
    return std::abs(centerPos.Distance(testPoint) - m_radius) <= tolerance;
}

} // namespace onecad::core::sketch
