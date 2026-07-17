// Ported from OneCAD-CPP tests/prototypes/proto_planegcs_integration.cpp @ b4ddcccc (2026-07-16)
#include "GCS.h"
#include "Geo.h"

#include <cassert>
#include <cmath>
#include <iostream>

int main() {
    double x1 = 0.0;
    double y1 = 0.0;
    double x2 = 10.0;
    double y2 = 0.0;

    double fixedX1 = 0.0;
    double fixedY1 = 0.0;
    double fixedX2 = 10.0;
    double fixedY2 = 0.0;

    GCS::Point p1(&x1, &y1);
    GCS::Point p2(&x2, &y2);

    GCS::System system;
    system.addConstraintCoordinateX(p1, &fixedX1);
    system.addConstraintCoordinateY(p1, &fixedY1);
    system.addConstraintCoordinateX(p2, &fixedX2);
    system.addConstraintCoordinateY(p2, &fixedY2);

    GCS::VEC_pD params;
    int status = system.solve(params);
    assert(status == GCS::Success || status == GCS::Converged);

    double dx = x2 - x1;
    double dy = y2 - y1;
    double dist = std::sqrt(dx * dx + dy * dy);
    assert(std::abs(dist - 10.0) < 1e-6);

    std::cout << "PlaneGCS integration prototype: OK" << std::endl;
    return 0;
}
