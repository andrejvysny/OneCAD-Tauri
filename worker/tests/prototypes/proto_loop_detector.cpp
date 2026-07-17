// Ported from OneCAD-CPP tests/prototypes/proto_loop_detector.cpp @ b4ddcccc (2026-07-16)
#include "loop/LoopDetector.h"
#include "sketch/Sketch.h"

#include <cassert>
#include <iostream>
#include <numbers>

using namespace onecad::core;

int main() {
    {
        sketch::Sketch sketch;

        auto p1 = sketch.addPoint(0.0, 0.0);
        auto p2 = sketch.addPoint(10.0, 0.0);
        auto p3 = sketch.addPoint(10.0, 5.0);
        auto p4 = sketch.addPoint(0.0, 5.0);

        sketch.addLine(p1, p2);
        sketch.addLine(p2, p3);
        sketch.addLine(p3, p4);
        sketch.addLine(p4, p1);

        loop::LoopDetector detector;
        auto result = detector.detect(sketch);

        assert(result.success);
        assert(result.faces.size() == 1);
        assert(result.faces.front().innerLoops.empty());
    }

    {
        sketch::Sketch sketch;

        auto p1 = sketch.addPoint(0.0, 0.0);
        auto p2 = sketch.addPoint(10.0, 0.0);
        auto p3 = sketch.addPoint(10.0, 10.0);
        auto p4 = sketch.addPoint(0.0, 10.0);

        sketch.addLine(p1, p2);
        sketch.addLine(p2, p3);
        sketch.addLine(p3, p4);
        sketch.addLine(p4, p1);

        auto h1 = sketch.addPoint(3.0, 3.0);
        auto h2 = sketch.addPoint(7.0, 3.0);
        auto h3 = sketch.addPoint(7.0, 7.0);
        auto h4 = sketch.addPoint(3.0, 7.0);

        sketch.addLine(h1, h2);
        sketch.addLine(h2, h3);
        sketch.addLine(h3, h4);
        sketch.addLine(h4, h1);

        loop::LoopDetector detector;
        auto result = detector.detect(sketch);

        assert(result.success);
        assert(result.faces.size() == 1);
        assert(result.faces.front().innerLoops.size() == 1);
    }

    {
        sketch::Sketch sketch;

        auto center = sketch.addPoint(0.0, 0.0);
        sketch.addArc(center, 5.0, 0.0, std::numbers::pi_v<double>);

        auto p1 = sketch.addPoint(5.0, 0.0);
        auto p2 = sketch.addPoint(-5.0, 0.0);
        sketch.addLine(p1, p2);

        loop::LoopDetector detector;
        auto result = detector.detect(sketch);

        assert(result.success);
        assert(!result.faces.empty());
    }

    {
        sketch::Sketch sketch;
        auto center = sketch.addPoint(0.0, 0.0);
        auto ellipse = sketch.addEllipse(center, 6.0, 3.0, 0.25);
        assert(!ellipse.empty());

        loop::LoopDetectorConfig config;
        config.planarizeIntersections = true;
        loop::LoopDetector detector(config);
        auto result = detector.detect(sketch);

        assert(result.success);
        assert(result.faces.size() == 1);
        assert(result.faces.front().outerLoop.area() > 50.0);
    }

    std::cout << "Loop detector prototype: OK" << std::endl;
    return 0;
}
