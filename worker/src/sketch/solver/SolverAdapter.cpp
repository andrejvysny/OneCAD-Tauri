// Ported from OneCAD-CPP src/core/sketch/solver/SolverAdapter.cpp @ b4ddcccc (2026-07-16)
#include "SolverAdapter.h"

#include "../Sketch.h"
#include "ConstraintSolver.h"
#include "util/Log.h"


namespace onecad::core::sketch {

bool SolverAdapter::populateSolver(Sketch& sketch, ConstraintSolver& solver) {
    solver.clear();

    for (const auto& entity : sketch.getAllEntities()) {
        if (entity && entity->type() == EntityType::Point) {
            solver.addPoint(dynamic_cast<SketchPoint*>(entity.get()));
        }
    }

    for (const auto& entity : sketch.getAllEntities()) {
        if (!entity) {
            continue;
        }

        switch (entity->type()) {
            case EntityType::Line:
                solver.addLine(dynamic_cast<SketchLine*>(entity.get()));
                break;
            case EntityType::Arc:
                solver.addArc(dynamic_cast<SketchArc*>(entity.get()));
                break;
            case EntityType::Circle:
                solver.addCircle(dynamic_cast<SketchCircle*>(entity.get()));
                break;
            default:
                break;
        }
    }

    bool ok = true;
    for (const auto& constraint : sketch.getAllConstraints()) {
        if (!addConstraintToSolver(constraint.get(), solver)) {
            ok = false;
            WLOG_WARN("%s", "populateSolver:constraint-translation-failed");
        }
    }

    return ok;
}

bool SolverAdapter::addConstraintToSolver(SketchConstraint* constraint, ConstraintSolver& solver) {
    if (!constraint) {
        return false;
    }
    return solver.addConstraint(constraint);
}

} // namespace onecad::core::sketch
