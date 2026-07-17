// Ported from OneCAD-CPP tests/prototypes/proto_sketch_solver.cpp @ b4ddcccc (2026-07-16)
#include "sketch/Sketch.h"
#include "sketch/constraints/Constraints.h"
#include "sketch/solver/ConstraintSolver.h"
#include "sketch/solver/SolverAdapter.h"
#include "loop/RegionUtils.h"

#include <cassert>
#include <cmath>
#include <algorithm>
#include <iostream>
#include <numbers>
#include <unordered_map>
#include <unordered_set>

using namespace onecad::core::sketch;
using namespace onecad::core::sketch::constraints;

namespace {

bool approx(double a, double b, double tol = 1e-6) {
    double diff = std::abs(a - b);
    double scale = std::max(std::abs(a), std::abs(b));
    return diff <= tol || diff <= tol * scale;
}

Vec2d pointPosition(const Sketch& sketch, EntityID pointId) {
    if (const auto* point = sketch.getEntityAs<SketchPoint>(pointId)) {
        return Vec2d{.x = point->position().X(), .y = point->position().Y()};
    }
    return Vec2d{.x = 0.0, .y = 0.0};
}

void testVerticalConstraintDrag() {
    Sketch sketch;
    auto top = sketch.addPoint(0.0, 6.0);
    auto bottom = sketch.addPoint(0.0, 0.0);
    auto line = sketch.addLine(top, bottom);
    assert(!line.empty());
    assert(!sketch.addVertical(line).empty());

    const Vec2d target{0.0, 9.0};
    sketch.beginPointDrag(bottom);
    SolveResult dragResult = sketch.solveWithDrag(bottom, target);
    sketch.endPointDrag();

    auto* bottomAfter = sketch.getEntityAs<SketchPoint>(bottom);
    auto* topAfter = sketch.getEntityAs<SketchPoint>(top);
    assert(bottomAfter && topAfter);
    assert(approx(bottomAfter->x(), topAfter->x(), 1e-4));
    assert(approx(topAfter->x(), 0.0, 1e-4));
    assert(approx(bottomAfter->y(), target.y, 1e-4));
}

void testHorizontalConstraintDrag() {
    Sketch sketch;
    auto east = sketch.addPoint(0.0, 0.0);
    auto west = sketch.addPoint(12.0, 0.0);
    auto line = sketch.addLine(east, west);
    assert(!line.empty());
    assert(!sketch.addHorizontal(line).empty());

    const Vec2d target{7.25, 0.0};
    sketch.beginPointDrag(east);
    SolveResult dragResult = sketch.solveWithDrag(east, target);
    sketch.endPointDrag();
    assert(dragResult.success);

    auto* eastAfter = sketch.getEntityAs<SketchPoint>(east);
    auto* westAfter = sketch.getEntityAs<SketchPoint>(west);
    assert(eastAfter && westAfter);
    assert(approx(eastAfter->y(), westAfter->y(), 1e-4));
    assert(approx(eastAfter->y(), 0.0, 1e-4));
    assert(approx(eastAfter->x(), target.x, 1e-4));
}

void testCoincidentFixedDrag() {
    Sketch sketch;
    auto fixed = sketch.addPoint(1.0, 1.0);
    auto slave = sketch.addPoint(2.0, 1.5);
    assert(!sketch.addCoincident(fixed, slave).empty());
    assert(!sketch.addFixed(fixed).empty());

    const Vec2d fixedPos = pointPosition(sketch, fixed);
    const Vec2d target{fixedPos.x + 4.0, fixedPos.y + 3.0};

    sketch.beginPointDrag(slave);
    SolveResult dragResult = sketch.solveWithDrag(slave, target);
    sketch.endPointDrag();
    assert(dragResult.success);

    const Vec2d fixedAfter = pointPosition(sketch, fixed);
    const Vec2d slaveAfter = pointPosition(sketch, slave);
    assert(approx(fixedAfter.x, fixedPos.x, 1e-6));
    assert(approx(fixedAfter.y, fixedPos.y, 1e-6));
    assert(approx(slaveAfter.x, fixedPos.x, 1e-6));
    assert(approx(slaveAfter.y, fixedPos.y, 1e-6));
}

void testFixedPointDragNoMovement() {
    Sketch sketch;
    auto fixed = sketch.addPoint(-3.2, 5.3);
    assert(!sketch.addFixed(fixed).empty());

    const Vec2d start = pointPosition(sketch, fixed);
    const Vec2d target{start.x + 6.0, start.y - 2.5};

    sketch.beginPointDrag(fixed);
    SolveResult dragResult = sketch.solveWithDrag(fixed, target);
    sketch.endPointDrag();
    assert(dragResult.success);

    const Vec2d end = pointPosition(sketch, fixed);
    assert(approx(end.x, start.x, 1e-6));
    assert(approx(end.y, start.y, 1e-6));
}

void testPointOnCurveSolverTranslation() {
    Sketch sketch;
    auto lineStart = sketch.addPoint(0.0, 0.0);
    auto lineEnd = sketch.addPoint(10.0, 0.0);
    auto line = sketch.addLine(lineStart, lineEnd);
    auto linePoint = sketch.addPoint(5.0, 0.0);
    assert(!line.empty());
    assert(!sketch.addPointOnCurve(linePoint, line).empty());

    auto circleCenter = sketch.addPoint(20.0, 0.0);
    auto circle = sketch.addCircle(circleCenter, 5.0);
    auto circlePoint = sketch.addPoint(25.0, 0.0);
    assert(!circle.empty());
    assert(!sketch.addPointOnCurve(circlePoint, circle).empty());

    auto arcCenter = sketch.addPoint(40.0, 0.0);
    auto arc = sketch.addArc(arcCenter, 5.0, 0.0, std::numbers::pi_v<double> * 0.5);
    const double arcCoord = 40.0 + 5.0 / std::sqrt(2.0);
    auto arcPoint = sketch.addPoint(arcCoord, 5.0 / std::sqrt(2.0));
    assert(!arc.empty());
    assert(!sketch.addPointOnCurve(arcPoint, arc).empty());

    SolveResult result = sketch.solve();
    assert(result.success);
}

void testNewConstraintTranslations() {
    Sketch sketch;
    auto p1 = sketch.addPoint(0.0, 0.0);
    auto p2 = sketch.addPoint(7.0, -3.0);
    auto axisA = sketch.addPoint(0.0, -5.0);
    auto axisB = sketch.addPoint(0.0, 5.0);
    auto axis = sketch.addLine(axisA, axisB);
    assert(!sketch.addHorizontalDistance(p1, p2, 7.0).empty());
    assert(!sketch.addVerticalDistance(p1, p2, -3.0).empty());

    auto c1 = sketch.addPoint(20.0, 0.0);
    auto c2 = sketch.addPoint(20.0, 0.0);
    auto circle = sketch.addCircle(c1, 3.0);
    auto arc = sketch.addArc(c2, 4.0, 0.0, std::numbers::pi_v<double>);
    assert(!sketch.addDiameter(circle, 6.0).empty());
    assert(!sketch.addConcentric(circle, arc).empty());

    auto sp1 = sketch.addPoint(-2.0, 1.0);
    auto sp2 = sketch.addPoint(2.0, 1.0);
    assert(!sketch.addSymmetric(sp1, sp2, axis).empty());

    SolveResult result = sketch.solve();
    assert(result.success);
}

void testDofDiagnosisWithRedundantConstraint() {
    // A duplicate Horizontal is redundant, not conflicting: the naive count
    // used to subtract it twice and misreport the sketch as tighter than it
    // is; PlaneGCS diagnosis must ignore the redundancy.
    Sketch sketch;
    auto p1 = sketch.addPoint(0.0, 0.0);
    auto p2 = sketch.addPoint(10.0, 0.0);
    auto line = sketch.addLine(p1, p2);

    const int dofBefore = sketch.getDegreesOfFreedom();
    assert(dofBefore == 4);  // two free points

    assert(!sketch.addHorizontal(line).empty());
    const int dofOne = sketch.getDegreesOfFreedom();
    assert(dofOne == 3);

    assert(!sketch.addHorizontal(line).empty());  // redundant duplicate
    const int dofTwo = sketch.getDegreesOfFreedom();
    assert(dofTwo == 3);  // unchanged — redundancy removes nothing
    assert(!sketch.isOverConstrained());
}

void testConflictingConstraintsDetected() {
    // Two fixed points cannot satisfy an incompatible distance: a genuine
    // conflict (unlike H+V on one line, which a degenerate line satisfies).
    Sketch sketch;
    auto p1 = sketch.addPoint(0.0, 0.0);
    auto p2 = sketch.addPoint(10.0, 0.0);

    assert(!sketch.addFixed(p1).empty());
    assert(!sketch.addFixed(p2).empty());
    assert(!sketch.addHorizontalDistance(p1, p2, 25.0).empty());
    assert(sketch.isOverConstrained());
}

} // namespace

int main() {
    Sketch sketch;

    auto p1 = sketch.addPoint(0.0, 0.0);
    auto p2 = sketch.addPoint(10.0, 0.0);
    auto line = sketch.addLine(p1, p2);
    (void)line;

    auto circleCenter = sketch.addPoint(5.0, 5.0);
    auto circle = sketch.addCircle(circleCenter, 2.5);
    auto arcCenter = sketch.addPoint(-2.0, 2.0);
    auto arc = sketch.addArc(arcCenter, 3.0, 0.0, std::numbers::pi_v<double> * 0.5);
    (void)arc;

    assert(sketch.addDistance(p1, circle, 5.0).empty());
    assert(sketch.addDistance(line, circle, 5.0).empty());

    auto distanceId = sketch.addDistance(p1, p2, 10.0);
    auto radiusId = sketch.addRadius(circle, 2.5);
    assert(!distanceId.empty());
    assert(!radiusId.empty());

    ConstraintSolver solver;
    SolverAdapter::populateSolver(sketch, solver);

    SolveResult solveResult = sketch.solve();
    assert(solveResult.success);

    // Keep target consistent with the fixed-distance constraint to p2.
    Vec2d target{10.0, 10.0};
    SolveResult dragResult = sketch.solveWithDrag(p1, target);
    assert(dragResult.success);

    auto* p1Entity = sketch.getEntityAs<SketchPoint>(p1);
    assert(p1Entity);
    assert(approx(p1Entity->x(), target.x));
    assert(approx(p1Entity->y(), target.y));

    Sketch projectedDrag;
    auto hp1 = projectedDrag.addPoint(0.0, 0.0);
    auto hp2 = projectedDrag.addPoint(10.0, 0.0);
    auto hLine = projectedDrag.addLine(hp1, hp2);
    assert(!hLine.empty());
    assert(!projectedDrag.addHorizontal(hLine).empty());
    assert(!projectedDrag.addFixed(hp2).empty());

    SolveResult projectedInitSolve = projectedDrag.solve();
    assert(projectedInitSolve.success);

    Vec2d projectedTarget{5.0, 7.0};
    SolveResult projectedDragResult = projectedDrag.solveWithDrag(hp1, projectedTarget);
    assert(projectedDragResult.success);

    auto* hp1Entity = projectedDrag.getEntityAs<SketchPoint>(hp1);
    auto* hp2Entity = projectedDrag.getEntityAs<SketchPoint>(hp2);
    assert(hp1Entity && hp2Entity);
    assert(std::abs(hp1Entity->x() - projectedTarget.x) < 1e-3);
    assert(std::abs(hp1Entity->y() - hp2Entity->y()) < 1e-4);
    assert(!approx(hp1Entity->y(), projectedTarget.y, 1e-3));

    // Rectangle drag regression:
    // dragging one corner should keep the opposite corner fixed.
    Sketch rectangle;
    auto rp1 = rectangle.addPoint(0.0, 0.0);
    auto rp2 = rectangle.addPoint(10.0, 0.0);
    auto rp3 = rectangle.addPoint(10.0, 6.0);
    auto rp4 = rectangle.addPoint(0.0, 6.0);
    assert(!rp1.empty() && !rp2.empty() && !rp3.empty() && !rp4.empty());

    auto bottom = rectangle.addLine(rp1, rp2);
    auto right = rectangle.addLine(rp2, rp3);
    auto top = rectangle.addLine(rp3, rp4);
    auto left = rectangle.addLine(rp4, rp1);
    assert(!bottom.empty() && !right.empty() && !top.empty() && !left.empty());

    assert(!rectangle.addHorizontal(bottom).empty());
    assert(!rectangle.addHorizontal(top).empty());
    assert(!rectangle.addVertical(left).empty());
    assert(!rectangle.addVertical(right).empty());

    auto regionId = onecad::core::loop::getRegionIdContainingEntity(rectangle, rp1);
    assert(regionId.has_value());
    auto face = onecad::core::loop::resolveRegionFace(rectangle, *regionId);
    assert(face.has_value());
    auto boundaryPoints = onecad::core::loop::getOrderedBoundaryPointIds(rectangle, face->outerLoop);
    assert(boundaryPoints.size() == 4);
    assert(std::find(boundaryPoints.begin(), boundaryPoints.end(), rp1) != boundaryPoints.end());

    auto* rp3Before = rectangle.getEntityAs<SketchPoint>(rp3);
    assert(rp3Before);
    double oppositeX = rp3Before->x();
    double oppositeY = rp3Before->y();

    rectangle.beginPointDrag(rp1);
    SolveResult rectangleDrag = rectangle.solveWithDrag(rp1, Vec2d{-2.0, -1.0});
    rectangle.endPointDrag();
    assert(rectangleDrag.success);

    auto* rp1After = rectangle.getEntityAs<SketchPoint>(rp1);
    auto* rp2After = rectangle.getEntityAs<SketchPoint>(rp2);
    auto* rp3After = rectangle.getEntityAs<SketchPoint>(rp3);
    auto* rp4After = rectangle.getEntityAs<SketchPoint>(rp4);
    assert(rp1After && rp2After && rp3After && rp4After);

    assert(approx(rp3After->x(), oppositeX));
    assert(approx(rp3After->y(), oppositeY));
    assert(approx(rp2After->y(), rp1After->y()));
    assert(approx(rp4After->x(), rp1After->x()));
    assert(approx(rp3After->x(), rp2After->x()));
    assert(approx(rp3After->y(), rp4After->y()));

    // Drag rollback determinism regression:
    // if a drag session has at least one failed solve after a successful move,
    // endPointDrag() should rollback to drag-start pose.
    Sketch dragRollback;
    auto d1 = dragRollback.addPoint(0.0, 0.0);
    auto d2 = dragRollback.addPoint(10.0, 0.0);
    auto d3 = dragRollback.addPoint(10.0, 6.0);
    auto d4 = dragRollback.addPoint(0.0, 6.0);
    assert(!d1.empty() && !d2.empty() && !d3.empty() && !d4.empty());
    assert(!dragRollback.addLine(d1, d2).empty());
    assert(!dragRollback.addLine(d2, d3).empty());
    assert(!dragRollback.addLine(d3, d4).empty());
    assert(!dragRollback.addLine(d4, d1).empty());
    assert(!dragRollback.addHorizontal(d1, d2).empty());
    assert(!dragRollback.addHorizontal(d3, d4).empty());
    assert(!dragRollback.addVertical(d2, d3).empty());
    assert(!dragRollback.addVertical(d4, d1).empty());

    dragRollback.beginPointDrag(d1);
    SolveResult moveOk = dragRollback.solveWithDrag(d1, Vec2d{-2.0, -1.0});
    assert(moveOk.success);

    auto* d1AfterFirstMove = dragRollback.getEntityAs<SketchPoint>(d1);
    assert(d1AfterFirstMove);
    const double firstMoveX = d1AfterFirstMove->x();
    const double firstMoveY = d1AfterFirstMove->y();

    // Inject a hard lock after a successful move so next drag target is unsolved.
    assert(!dragRollback.addFixed(d1).empty());
    SolveResult moveFail = dragRollback.solveWithDrag(d1, Vec2d{-4.0, -3.0});
    assert(moveFail.success);

    auto* d1AfterFixedDrag = dragRollback.getEntityAs<SketchPoint>(d1);
    assert(d1AfterFixedDrag);
    assert(approx(d1AfterFixedDrag->x(), firstMoveX));
    assert(approx(d1AfterFixedDrag->y(), firstMoveY));

    dragRollback.endPointDrag();

    auto* d1Final = dragRollback.getEntityAs<SketchPoint>(d1);
    assert(d1Final);
    assert(approx(d1Final->x(), firstMoveX));
    assert(approx(d1Final->y(), firstMoveY));

    testVerticalConstraintDrag();
    testHorizontalConstraintDrag();
    testCoincidentFixedDrag();
    testFixedPointDragNoMovement();
    testPointOnCurveSolverTranslation();
    testNewConstraintTranslations();
    testDofDiagnosisWithRedundantConstraint();
    testConflictingConstraintsDetected();

    Sketch largeDrag;
    auto ld1 = largeDrag.addPoint(0.0, 0.0);
    auto ld2 = largeDrag.addPoint(200.0, 0.0);
    auto ldLine = largeDrag.addLine(ld1, ld2);
    assert(!ldLine.empty());
    assert(!largeDrag.addHorizontal(ldLine).empty());
    assert(!largeDrag.addFixed(ld2).empty());
    assert(largeDrag.solve().success);

    Vec2d largeTarget{500.0, 300.0};
    SolveResult largeDragResult = largeDrag.solveWithDrag(ld1, largeTarget);
    assert(largeDragResult.success);

    auto* ld1Entity = largeDrag.getEntityAs<SketchPoint>(ld1);
    auto* ld2Entity = largeDrag.getEntityAs<SketchPoint>(ld2);
    assert(ld1Entity && ld2Entity);
    assert(std::abs(ld1Entity->x() - largeTarget.x) < 1e-3);
    assert(std::abs(ld1Entity->y() - ld2Entity->y()) < 1e-4);

    Sketch largeGroup;
    auto lg1 = largeGroup.addPoint(0.0, 0.0);
    auto lg2 = largeGroup.addPoint(40.0, 0.0);
    auto lgLine = largeGroup.addLine(lg1, lg2);
    assert(!lgLine.empty());
    assert(!largeGroup.addHorizontal(lgLine).empty());
    assert(largeGroup.solve().success);

    std::unordered_set<EntityID> largeSelection{lg1, lg2};
    largeGroup.beginGroupDrag(largeSelection);
    auto* lg1Before = largeGroup.getEntityAs<SketchPoint>(lg1);
    auto* lg2Before = largeGroup.getEntityAs<SketchPoint>(lg2);
    assert(lg1Before && lg2Before);
    Vec2d largeGroupDelta{350.0, -275.0};
    std::unordered_map<EntityID, Vec2d> largeGroupTargets;
    largeGroupTargets[lg1] = Vec2d{.x = lg1Before->x() + largeGroupDelta.x, .y = lg1Before->y() + largeGroupDelta.y};
    largeGroupTargets[lg2] = Vec2d{.x = lg2Before->x() + largeGroupDelta.x, .y = lg2Before->y() + largeGroupDelta.y};

    SolveResult largeGroupResult = largeGroup.solveWithGroupDrag(largeGroupTargets);
    largeGroup.endGroupDrag();
    assert(largeGroupResult.success);

    auto* lg1After = largeGroup.getEntityAs<SketchPoint>(lg1);
    auto* lg2After = largeGroup.getEntityAs<SketchPoint>(lg2);
    assert(lg1After && lg2After);
    assert(approx(lg1After->x(), largeGroupTargets[lg1].x, 1e-5));
    assert(approx(lg1After->y(), largeGroupTargets[lg1].y, 1e-5));
    assert(approx(lg2After->x(), largeGroupTargets[lg2].x, 1e-5));
    assert(approx(lg2After->y(), largeGroupTargets[lg2].y, 1e-5));

    std::cout << "Sketch solver adapter prototype: OK" << std::endl;
    return 0;
}
