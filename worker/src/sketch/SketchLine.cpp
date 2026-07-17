// Ported from OneCAD-CPP src/core/sketch/SketchLine.cpp @ b4ddcccc (2026-07-16)
#include "SketchLine.h"


#include <algorithm>
#include <cmath>
#include <numbers>
#include <utility>

namespace onecad::core::sketch {

SketchLine::SketchLine()
    : SketchEntity() {
}

SketchLine::SketchLine(const PointID& startPointId, const PointID& endPointId)
    : SketchEntity(),
      m_startPointId(startPointId),
      m_endPointId(endPointId) {
}

double SketchLine::length(const gp_Pnt2d& startPos, const gp_Pnt2d& endPos) {
    return startPos.Distance(endPos);
}

gp_Vec2d SketchLine::direction(const gp_Pnt2d& startPos, const gp_Pnt2d& endPos) {
    gp_Vec2d vec(startPos, endPos);
    double magnitude = vec.Magnitude();
    if (magnitude <= 0.0) {
        return gp_Vec2d(0.0, 0.0);
    }
    vec /= magnitude;
    return vec;
}

gp_Pnt2d SketchLine::midpoint(const gp_Pnt2d& startPos, const gp_Pnt2d& endPos) {
    return gp_Pnt2d((startPos.X() + endPos.X()) * 0.5,
                    (startPos.Y() + endPos.Y()) * 0.5);
}

double SketchLine::angle(const gp_Pnt2d& startPos, const gp_Pnt2d& endPos) {
    return std::atan2(endPos.Y() - startPos.Y(), endPos.X() - startPos.X());
}

bool SketchLine::isHorizontal(const gp_Pnt2d& startPos, const gp_Pnt2d& endPos,
                              double tolerance) {
    double ang = angle(startPos, endPos);
    double pi = std::numbers::pi_v<double>;
    return std::abs(ang) <= tolerance || std::abs(std::abs(ang) - pi) <= tolerance;
}

bool SketchLine::isVertical(const gp_Pnt2d& startPos, const gp_Pnt2d& endPos,
                            double tolerance) {
    double ang = angle(startPos, endPos);
    double halfPi = std::numbers::pi_v<double> * 0.5;
    return std::abs(std::abs(ang) - halfPi) <= tolerance;
}

double SketchLine::distanceToPoint(const gp_Pnt2d& point,
                                   const gp_Pnt2d& startPos, const gp_Pnt2d& endPos) {
    double vx = endPos.X() - startPos.X();
    double vy = endPos.Y() - startPos.Y();
    double wx = point.X() - startPos.X();
    double wy = point.Y() - startPos.Y();

    double c1 = vx * wx + vy * wy;
    if (c1 <= 0.0) {
        return std::hypot(point.X() - startPos.X(), point.Y() - startPos.Y());
    }

    double c2 = vx * vx + vy * vy;
    if (c2 <= c1) {
        return std::hypot(point.X() - endPos.X(), point.Y() - endPos.Y());
    }

    double t = c1 / c2;
    double projX = startPos.X() + t * vx;
    double projY = startPos.Y() + t * vy;
    return std::hypot(point.X() - projX, point.Y() - projY);
}

BoundingBox2d SketchLine::bounds() const {
    return {};
}

bool SketchLine::isNear(const gp_Pnt2d&, double) const {
    return false;
}

BoundingBox2d SketchLine::boundsWithPoints(const gp_Pnt2d& startPos, const gp_Pnt2d& endPos) {
    BoundingBox2d box;
    box.minX = std::min(startPos.X(), endPos.X());
    box.maxX = std::max(startPos.X(), endPos.X());
    box.minY = std::min(startPos.Y(), endPos.Y());
    box.maxY = std::max(startPos.Y(), endPos.Y());
    return box;
}

bool SketchLine::isNearWithPoints(const gp_Pnt2d& testPoint,
                                  const gp_Pnt2d& startPos, const gp_Pnt2d& endPos,
                                  double tolerance) {
    return distanceToPoint(testPoint, startPos, endPos) <= tolerance;
}

} // namespace onecad::core::sketch
