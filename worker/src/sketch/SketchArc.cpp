// Ported from OneCAD-CPP src/core/sketch/SketchArc.cpp @ b4ddcccc (2026-07-16)
#include "SketchArc.h"


#include <algorithm>
#include <cmath>
#include <numbers>
#include <utility>
#include "util/Log.h"

namespace onecad::core::sketch {

namespace {

double normalizeAnglePositive(double angle) {
    double twoPi = 2.0 * std::numbers::pi_v<double>;
    angle = std::fmod(angle, twoPi);
    if (angle < 0.0) {
        angle += twoPi;
    }
    return angle;
}

} // namespace

SketchArc::SketchArc()
    : SketchEntity() {
}

SketchArc::SketchArc(const PointID& centerPointId, double radius,
                     double startAngle, double endAngle)
    : SketchEntity(),
      m_centerPointId(centerPointId),
      m_radius(std::max(0.0, radius)),
      m_startAngle(normalizeAngle(startAngle)),
      m_endAngle(normalizeAngle(endAngle)) {
    if (radius < 0.0) {
        WLOG_WARN("%s", "ctor:negative-radius-clamped");
    }
}

double SketchArc::sweepAngle() const {
    double sweep = m_endAngle - m_startAngle;
    if (sweep < 0.0) {
        sweep += 2.0 * std::numbers::pi_v<double>;
    }
    return sweep;
}

double SketchArc::arcLength() const {
    return m_radius * sweepAngle();
}

gp_Pnt2d SketchArc::startPoint(const gp_Pnt2d& centerPos) const {
    return gp_Pnt2d(centerPos.X() + m_radius * std::cos(m_startAngle),
                    centerPos.Y() + m_radius * std::sin(m_startAngle));
}

gp_Pnt2d SketchArc::endPoint(const gp_Pnt2d& centerPos) const {
    return gp_Pnt2d(centerPos.X() + m_radius * std::cos(m_endAngle),
                    centerPos.Y() + m_radius * std::sin(m_endAngle));
}

gp_Pnt2d SketchArc::midpoint(const gp_Pnt2d& centerPos) const {
    double midAngle = m_startAngle + sweepAngle() * 0.5;
    return gp_Pnt2d(centerPos.X() + m_radius * std::cos(midAngle),
                    centerPos.Y() + m_radius * std::sin(midAngle));
}

gp_Vec2d SketchArc::startTangent() const {
    return gp_Vec2d(-std::sin(m_startAngle), std::cos(m_startAngle));
}

gp_Vec2d SketchArc::endTangent() const {
    return gp_Vec2d(-std::sin(m_endAngle), std::cos(m_endAngle));
}

bool SketchArc::containsAngle(double angle) const {
    double start = normalizeAnglePositive(m_startAngle);
    double end = normalizeAnglePositive(m_endAngle);
    double test = normalizeAnglePositive(angle);

    if (end >= start) {
        return test >= start && test <= end;
    }
    return test >= start || test <= end;
}

gp_Pnt2d SketchArc::pointAtAngle(const gp_Pnt2d& centerPos, double angle) const {
    return gp_Pnt2d(centerPos.X() + m_radius * std::cos(angle),
                    centerPos.Y() + m_radius * std::sin(angle));
}

BoundingBox2d SketchArc::bounds() const {
    return {};
}

bool SketchArc::isNear(const gp_Pnt2d&, double) const {
    return false;
}

BoundingBox2d SketchArc::boundsWithCenter(const gp_Pnt2d& centerPos) const {
    BoundingBox2d box;
    gp_Pnt2d start = startPoint(centerPos);
    gp_Pnt2d end = endPoint(centerPos);

    box.minX = std::min(start.X(), end.X());
    box.maxX = std::max(start.X(), end.X());
    box.minY = std::min(start.Y(), end.Y());
    box.maxY = std::max(start.Y(), end.Y());

    const double cardinalAngles[] = {0.0, std::numbers::pi_v<double> * 0.5,
                                     std::numbers::pi_v<double>, std::numbers::pi_v<double> * 1.5};

    for (double angle : cardinalAngles) {
        if (!containsAngle(angle)) {
            continue;
        }
        gp_Pnt2d p = pointAtAngle(centerPos, angle);
        box.minX = std::min(box.minX, p.X());
        box.maxX = std::max(box.maxX, p.X());
        box.minY = std::min(box.minY, p.Y());
        box.maxY = std::max(box.maxY, p.Y());
    }

    return box;
}

bool SketchArc::isNearWithCenter(const gp_Pnt2d& testPoint, const gp_Pnt2d& centerPos,
                                 double tolerance) const {
    double dx = testPoint.X() - centerPos.X();
    double dy = testPoint.Y() - centerPos.Y();
    double distance = std::hypot(dx, dy);
    if (std::abs(distance - m_radius) > tolerance) {
        return false;
    }

    double angle = std::atan2(dy, dx);
    return containsAngle(angle);
}

void SketchArc::dragEndpoint(const gp_Pnt2d& centerPos, bool isDraggingStart,
                             const gp_Pnt2d& newPos) {
    double angle = std::atan2(newPos.Y() - centerPos.Y(), newPos.X() - centerPos.X());
    if (isDraggingStart) {
        m_startAngle = normalizeAngle(angle);
    } else {
        m_endAngle = normalizeAngle(angle);
    }
}

double SketchArc::normalizeAngle(double angle) {
    double twoPi = 2.0 * std::numbers::pi_v<double>;
    angle = std::fmod(angle, twoPi);
    if (angle <= -std::numbers::pi_v<double>) {
        angle += twoPi;
    } else if (angle > std::numbers::pi_v<double>) {
        angle -= twoPi;
    }
    return angle;
}

} // namespace onecad::core::sketch
