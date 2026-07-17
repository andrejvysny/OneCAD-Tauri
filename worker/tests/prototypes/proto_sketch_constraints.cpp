// Ported from OneCAD-CPP tests/prototypes/proto_sketch_constraints.cpp @ b4ddcccc (2026-07-16)
#include "sketch/Sketch.h"
#include "sketch/constraints/Constraints.h"

#include <cassert>
#include <cmath>
#include <iostream>
#include <memory>
#include <numbers>

using namespace onecad::core::sketch;
using namespace onecad::core::sketch::constraints;

int main() {
    Sketch sketch;

    auto p1 = sketch.addPoint(0.0, 0.0);
    auto p2 = sketch.addPoint(0.0, 0.0);
    CoincidentConstraint coincident(p1, p2);
    assert(coincident.isSatisfied(sketch, 1e-6));

    auto* p2Entity = sketch.getEntityAs<SketchPoint>(p2);
    assert(p2Entity);
    p2Entity->setPosition(1.0, 0.0);
    assert(!coincident.isSatisfied(sketch, 1e-6));

    auto hStart = sketch.addPoint(0.0, 2.0);
    auto hEnd = sketch.addPoint(5.0, 2.0);
    auto hLine = sketch.addLine(hStart, hEnd);
    HorizontalConstraint horizontal(hLine);
    assert(horizontal.isSatisfied(sketch, 1e-6));

    auto vStart = sketch.addPoint(3.0, -1.0);
    auto vEnd = sketch.addPoint(3.0, 4.0);
    auto vLine = sketch.addLine(vStart, vEnd);
    VerticalConstraint vertical(vLine);
    assert(vertical.isSatisfied(sketch, 1e-6));

    auto p3 = sketch.addPoint(0.0, 4.0);
    auto p4 = sketch.addPoint(5.0, 4.0);
    auto hLine2 = sketch.addLine(p3, p4);
    ParallelConstraint parallel(hLine, hLine2);
    assert(parallel.isSatisfied(sketch, 1e-6));

    PerpendicularConstraint perpendicular(hLine, vLine);
    assert(perpendicular.isSatisfied(sketch, 1e-6));

    auto npStart = sketch.addPoint(0.0, 0.0);
    auto npEnd = sketch.addPoint(5.0, 5.0);
    auto nonParallelLine = sketch.addLine(npStart, npEnd);
    ParallelConstraint notParallel(hLine, nonParallelLine);
    assert(!notParallel.isSatisfied(sketch, 1e-6));

    auto circleCenter = sketch.addPoint(0.0, 0.0);
    auto circle = sketch.addCircle(circleCenter, 5.0);
    auto tStart = sketch.addPoint(-10.0, 5.0);
    auto tEnd = sketch.addPoint(10.0, 5.0);
    auto tangentLine = sketch.addLine(tStart, tEnd);
    TangentConstraint tangent(tangentLine, circle);
    assert(tangent.isSatisfied(sketch, 1e-6));

    auto eqStart = sketch.addPoint(0.0, -3.0);
    auto eqEnd = sketch.addPoint(5.0, -3.0);
    auto eqLine = sketch.addLine(eqStart, eqEnd);
    EqualConstraint equal(hLine, eqLine);
    assert(equal.isSatisfied(sketch, 1e-6));

    DistanceConstraint distance(p1, p2, 1.0);
    assert(distance.isSatisfied(sketch, 1e-6));

    DistanceConstraint pointLineDistance(p1, hLine, 2.0);
    assert(pointLineDistance.isSatisfied(sketch, 1e-6));

    AngleConstraint angle(hLine, vLine, std::numbers::pi_v<double> * 0.5);
    assert(angle.isSatisfied(sketch, 1e-6));

    RadiusConstraint radius(circle, 5.0);
    assert(radius.isSatisfied(sketch, 1e-6));

    DiameterConstraint diameter(circle, 10.0);
    assert(diameter.isSatisfied(sketch, 1e-6));

    HorizontalDistanceConstraint hDistance(p1, p2, 1.0);
    assert(hDistance.isSatisfied(sketch, 1e-6));

    VerticalDistanceConstraint vDistance(p1, p2, 0.0);
    assert(vDistance.isSatisfied(sketch, 1e-6));

    auto onLinePoint = sketch.addPoint(2.5, 2.0);
    assert(!sketch.addPointOnCurve(onLinePoint, hLine).empty());

    auto ellipseCenter = sketch.addPoint(20.0, 0.0);
    auto ellipse = sketch.addEllipse(ellipseCenter, 5.0, 2.0);
    assert(!ellipse.empty());
    assert(sketch.addPointOnCurve(onLinePoint, ellipse).empty());

    auto arcCenter = sketch.addPoint(20.0, 20.0);
    auto arc = sketch.addArc(arcCenter, 3.0, 0.0, std::numbers::pi_v<double> * 0.5);
    assert(!arc.empty());
    auto onArcPoint = sketch.addPoint(20.0 + 3.0 / std::sqrt(2.0),
                                      20.0 + 3.0 / std::sqrt(2.0));
    assert(!sketch.addPointOnCurve(onArcPoint, arc).empty());
    auto arcStartPoint = sketch.addPoint(23.0, 20.0);
    assert(sketch.addPointOnCurve(arcStartPoint, arc).empty());

    auto onCirclePoint = sketch.addPoint(5.0, 0.0);
    assert(!sketch.addPointOnCurve(onCirclePoint, circle).empty());

    assert(!sketch.addConstraint(std::make_unique<DiameterConstraint>(circle, 10.0)).empty());
    assert(!sketch.addConstraint(std::make_unique<ConcentricConstraint>(circle, arc)).empty());

    auto symA = sketch.addPoint(-1.0, 0.0);
    auto symB = sketch.addPoint(1.0, 0.0);
    auto axisStart = sketch.addPoint(0.0, -2.0);
    auto axisEnd = sketch.addPoint(0.0, 2.0);
    auto axis = sketch.addLine(axisStart, axisEnd);
    SymmetricConstraint symmetric(symA, symB, axis);
    assert(symmetric.isSatisfied(sketch, 1e-6));
    assert(!sketch.addSymmetric(symA, symB, axis).empty());

    // W-WP3a: the JSON round-trip test (QJsonObject + DistanceConstraint::serialize
    // + ConstraintFactory::fromJson) is stripped — serialization is Rust-owned and
    // removed from the worker sketch stack. Programmatic construction/solve paths
    // above remain fully exercised.

    std::cout << "Sketch constraints prototype: OK" << std::endl;
    return 0;
}
