// Ported from OneCAD-CPP tests/prototypes/proto_sketch_constraint_applicability.cpp @ b4ddcccc (2026-07-16)
#include "sketch/Sketch.h"
#include "sketch/ConstraintApplicability.h"

#include <cassert>
#include <iostream>
#include <string>

using onecad::app::selection::SelectionItem;
using onecad::app::selection::SelectionKind;
using onecad::core::sketch::ConstraintType;
using onecad::core::sketch::Sketch;

namespace {

SelectionItem selection(SelectionKind kind, const std::string& entityId) {
    SelectionItem item;
    item.kind = kind;
    item.id.elementId = entityId;
    return item;
}

} // namespace

int main() {
    Sketch sketch;

    const auto p1 = sketch.addPoint(0.0, 0.0);
    const auto p2 = sketch.addPoint(10.0, 0.0);
    const auto line = sketch.addLine(p1, p2);
    const auto p3 = sketch.addPoint(0.0, 4.0);
    const auto p4 = sketch.addPoint(10.0, 4.0);
    const auto line2 = sketch.addLine(p3, p4);
    const auto circleCenter = sketch.addPoint(5.0, 5.0);
    const auto circle = sketch.addCircle(circleCenter, 2.5);
    const auto arcCenter = sketch.addPoint(12.0, 5.0);
    const auto arc = sketch.addArc(arcCenter, 2.5, 0.0, 1.0);
    const auto ellipseCenter = sketch.addPoint(20.0, 5.0);
    const auto ellipse = sketch.addEllipse(ellipseCenter, 5.0, 2.0);

    assert(!p1.empty() && !p2.empty() && !line.empty());
    assert(!line2.empty() && !circle.empty() && !arc.empty() && !ellipse.empty());

    auto pointPoint = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchPoint, p1), selection(SelectionKind::SketchPoint, p2)});
    assert(pointPoint.isApplicable(ConstraintType::HorizontalDistance));
    assert(pointPoint.isApplicable(ConstraintType::VerticalDistance));

    auto pointLine = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchPoint, p1), selection(SelectionKind::SketchEdge, line)});
    assert(pointLine.isApplicable(ConstraintType::Distance));
    assert(pointLine.isApplicable(ConstraintType::OnCurve));

    auto pointCircle = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchPoint, p1), selection(SelectionKind::SketchEdge, circle)});
    assert(!pointCircle.isApplicable(ConstraintType::Distance));
    assert(pointCircle.isApplicable(ConstraintType::OnCurve));

    auto pointEllipse = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchPoint, p1), selection(SelectionKind::SketchEdge, ellipse)});
    assert(!pointEllipse.isApplicable(ConstraintType::Distance));
    assert(!pointEllipse.isApplicable(ConstraintType::OnCurve));
    assert(!pointEllipse.hasApplicableConstraints());

    auto lineCircle = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchEdge, line), selection(SelectionKind::SketchEdge, circle)});
    assert(!lineCircle.isApplicable(ConstraintType::Distance));
    assert(!lineCircle.hasApplicableConstraints());

    auto circleArc = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchEdge, circle), selection(SelectionKind::SketchEdge, arc)});
    assert(circleArc.isApplicable(ConstraintType::Concentric));

    auto lineLine = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchEdge, line), selection(SelectionKind::SketchEdge, line2)});
    assert(lineLine.isApplicable(ConstraintType::Distance));
    assert(lineLine.isApplicable(ConstraintType::Parallel));
    assert(lineLine.isApplicable(ConstraintType::Perpendicular));
    assert(lineLine.isApplicable(ConstraintType::Angle));

    auto symmetric = onecad::ui::evaluateConstraintApplicability(
        &sketch,
        {selection(SelectionKind::SketchPoint, p1),
         selection(SelectionKind::SketchPoint, p3),
         selection(SelectionKind::SketchEdge, line2)});
    assert(symmetric.isApplicable(ConstraintType::Symmetric));

    std::cout << "Sketch constraint applicability prototype: OK" << std::endl;
    return 0;
}
