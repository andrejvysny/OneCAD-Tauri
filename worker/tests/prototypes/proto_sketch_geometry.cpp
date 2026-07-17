// Ported from OneCAD-CPP tests/prototypes/proto_sketch_geometry.cpp @ b4ddcccc (2026-07-16)
#include "sketch/SketchPoint.h"
#include "sketch/SketchLine.h"
#include "sketch/SketchArc.h"
#include "sketch/SketchCircle.h"

#include <cassert>
#include <cmath>
#include <algorithm>
#include <iostream>
#include <numbers>

using namespace onecad::core::sketch;

namespace {

bool approx(double a, double b, double tol = 1e-6) {
    double diff = std::abs(a - b);
    double scale = std::max(std::abs(a), std::abs(b));
    return diff <= tol || diff <= tol * scale;
}

} // namespace

int main() {
    {
        SketchPoint point(1.0, 2.0);
        assert(approx(point.x(), 1.0));
        assert(approx(point.y(), 2.0));

        BoundingBox2d box = point.bounds();
        assert(approx(box.minX, 1.0));
        assert(approx(box.maxX, 1.0));
        assert(approx(box.minY, 2.0));
        assert(approx(box.maxY, 2.0));

        assert(point.isNear(gp_Pnt2d(1.0, 2.0), 1e-6));
        assert(!point.isNear(gp_Pnt2d(10.0, 10.0), 0.1));
    }

    {
        gp_Pnt2d start(0.0, 0.0);
        gp_Pnt2d end(3.0, 4.0);

        assert(approx(SketchLine::length(start, end), 5.0));

        gp_Vec2d dir = SketchLine::direction(start, end);
        assert(approx(dir.X(), 0.6));
        assert(approx(dir.Y(), 0.8));

        gp_Pnt2d mid = SketchLine::midpoint(start, end);
        assert(approx(mid.X(), 1.5));
        assert(approx(mid.Y(), 2.0));

        assert(SketchLine::isHorizontal(gp_Pnt2d(0.0, 1.0), gp_Pnt2d(5.0, 1.0)));
        assert(SketchLine::isVertical(gp_Pnt2d(2.0, -1.0), gp_Pnt2d(2.0, 3.0)));
    }

    {
        SketchArc arc("center", 10.0, 0.0, std::numbers::pi_v<double> * 0.5);
        gp_Pnt2d center(0.0, 0.0);

        assert(approx(arc.sweepAngle(), std::numbers::pi_v<double> * 0.5));
        assert(approx(arc.arcLength(), 10.0 * std::numbers::pi_v<double> * 0.5));

        gp_Pnt2d start = arc.startPoint(center);
        gp_Pnt2d end = arc.endPoint(center);
        assert(approx(start.X(), 10.0));
        assert(approx(start.Y(), 0.0));
        assert(approx(end.X(), 0.0));
        assert(approx(end.Y(), 10.0));

        assert(arc.containsAngle(std::numbers::pi_v<double> * 0.25));
        assert(!arc.containsAngle(std::numbers::pi_v<double>));

        BoundingBox2d box = arc.boundsWithCenter(center);
        assert(approx(box.minX, 0.0));
        assert(approx(box.minY, 0.0));
        assert(approx(box.maxX, 10.0));
        assert(approx(box.maxY, 10.0));
    }

    {
        SketchCircle circle("center", 5.0);
        gp_Pnt2d center(2.0, 3.0);

        assert(approx(circle.circumference(), 2.0 * std::numbers::pi_v<double> * 5.0));
        gp_Pnt2d onCircle = circle.pointAtAngle(center, 0.0);
        assert(approx(onCircle.X(), 7.0));
        assert(approx(onCircle.Y(), 3.0));

        BoundingBox2d box = circle.boundsWithCenter(center);
        assert(approx(box.minX, -3.0));
        assert(approx(box.maxX, 7.0));
        assert(approx(box.minY, -2.0));
        assert(approx(box.maxY, 8.0));
    }

    std::cout << "Sketch geometry prototype: OK" << std::endl;
    return 0;
}
