// Ported from OneCAD-CPP tests/prototypes/proto_sketch_fixed_and_move.cpp @ b4ddcccc (2026-07-16)
/**
 * Prototype: Fixed constraint and translatePlaneInSketch
 *
 * Tests:
 * - addFixed on point succeeds and constraint is stored
 * - addFixed on non-point (line ID) returns empty
 * - Fixed point does not move when solveWithDrag is called
 * - translatePlaneInSketch moves world position, sketch-local coords unchanged
 * - translatePlaneInSketch does not mutate Fixed constraint x/y values
 */

#include "sketch/Sketch.h"
#include "sketch/SketchPoint.h"
#include "sketch/SketchLine.h"
#include "sketch/constraints/Constraints.h"

#include <cassert>
#include <cmath>
#include <iostream>

using namespace onecad::core::sketch;
using namespace onecad::core::sketch::constraints;

namespace {

bool approx(double a, double b, double tol = 1e-6) {
    double diff = std::abs(a - b);
    double scale = std::max(std::abs(a), std::abs(b));
    return diff <= tol || diff <= tol * scale;
}

} // namespace

int main() {
    // ----- addFixed on point succeeds -----
    {
        Sketch sketch(SketchPlane::XY());
        EntityID pId = sketch.addPoint(3.0, 4.0);
        assert(!pId.empty());

        ConstraintID fixedId = sketch.addFixed(pId);
        assert(!fixedId.empty());

        assert(sketch.hasFixedConstraint(pId));

        const auto* constraint = sketch.getConstraint(fixedId);
        assert(constraint);
        assert(constraint->type() == ConstraintType::Fixed);

        auto* fixedConstraint = dynamic_cast<const FixedConstraint*>(constraint);
        assert(fixedConstraint);
        assert(approx(fixedConstraint->fixedX(), 3.0));
        assert(approx(fixedConstraint->fixedY(), 4.0));
    }

    // ----- addFixed on non-point (line) returns empty -----
    {
        Sketch sketch(SketchPlane::XY());
        EntityID p1 = sketch.addPoint(0.0, 0.0);
        EntityID p2 = sketch.addPoint(1.0, 0.0);
        EntityID lineId = sketch.addLine(p1, p2);
        assert(!lineId.empty());

        ConstraintID fixedId = sketch.addFixed(lineId);
        assert(fixedId.empty());
        assert(!sketch.hasFixedConstraint(lineId));
    }

    // ----- Fixed point does not move with solveWithDrag -----
    {
        Sketch sketch(SketchPlane::XY());
        EntityID pId = sketch.addPoint(5.0, 6.0);
        sketch.addFixed(pId);

        Vec2d tryDragTo{100.0, 200.0};
        SolveResult result = sketch.solveWithDrag(pId, tryDragTo);

        auto* point = sketch.getEntityAs<SketchPoint>(pId);
        assert(point);
        assert(approx(point->x(), 5.0));
        assert(approx(point->y(), 6.0));
    }

    // ----- translatePlaneInSketch: world position changes, local unchanged -----
    {
        Sketch sketch(SketchPlane::XY());
        EntityID pId = sketch.addPoint(10.0, 20.0);  // sketch-local
        auto* point = sketch.getEntityAs<SketchPoint>(pId);
        assert(point);

        Vec3d worldBefore = sketch.toWorld(Vec2d{point->x(), point->y()});
        sketch.translatePlaneInSketch(Vec2d{1.0, 2.0});
        Vec3d worldAfter = sketch.toWorld(Vec2d{point->x(), point->y()});

        assert(approx(point->x(), 10.0));
        assert(approx(point->y(), 20.0));

        assert(approx(worldAfter.x - worldBefore.x, 1.0 * sketch.getPlane().xAxis.x + 2.0 * sketch.getPlane().yAxis.x));
        assert(approx(worldAfter.y - worldBefore.y, 1.0 * sketch.getPlane().xAxis.y + 2.0 * sketch.getPlane().yAxis.y));
        assert(approx(worldAfter.z - worldBefore.z, 1.0 * sketch.getPlane().xAxis.z + 2.0 * sketch.getPlane().yAxis.z));
    }

    // ----- translatePlaneInSketch: Fixed constraint x/y unchanged -----
    {
        Sketch sketch(SketchPlane::XY());
        EntityID pId = sketch.addPoint(7.0, 8.0);
        ConstraintID fixedId = sketch.addFixed(pId);
        assert(!fixedId.empty());

        const auto* constraint = sketch.getConstraint(fixedId);
        auto* fixedConstraint = dynamic_cast<const FixedConstraint*>(constraint);
        assert(fixedConstraint);
        double xBefore = fixedConstraint->fixedX();
        double yBefore = fixedConstraint->fixedY();

        sketch.translatePlaneInSketch(Vec2d{-3.0, 5.0});

        constraint = sketch.getConstraint(fixedId);
        fixedConstraint = dynamic_cast<const FixedConstraint*>(constraint);
        assert(fixedConstraint);
        assert(approx(fixedConstraint->fixedX(), xBefore));
        assert(approx(fixedConstraint->fixedY(), yBefore));
    }

    std::cout << "proto_sketch_fixed_and_move: OK" << std::endl;
    return 0;
}
