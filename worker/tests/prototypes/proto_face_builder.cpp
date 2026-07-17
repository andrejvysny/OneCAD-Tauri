// Ported from OneCAD-CPP tests/prototypes/proto_face_builder.cpp @ b4ddcccc (2026-07-16)
#include "loop/FaceBuilder.h"
#include "loop/LoopDetector.h"
#include "sketch/Sketch.h"

#include <BRepBndLib.hxx>
#include <BRepCheck_Analyzer.hxx>
#include <Bnd_Box.hxx>
#include <TopoDS_Wire.hxx>
#include <TopExp_Explorer.hxx>

#include <cassert>
#include <cmath>
#include <iostream>

using namespace onecad::core;

namespace {

bool nearlyEqual(double a, double b, double tol = 1e-3) {
    return std::abs(a - b) <= tol;
}

int countWires(const TopoDS_Face& face) {
    int count = 0;
    for (TopExp_Explorer exp(face, TopAbs_WIRE); exp.More(); exp.Next()) {
        ++count;
    }
    return count;
}

} // namespace

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
        auto loops = detector.detect(sketch);

        loop::FaceBuilder builder;
        auto results = builder.buildAllFaces(loops, sketch);

        assert(results.size() == 1);
        assert(results[0].success);
        assert(BRepCheck_Analyzer(results[0].face).IsValid());
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
        auto loops = detector.detect(sketch);

        loop::FaceBuilder builder;
        auto results = builder.buildAllFaces(loops, sketch);

        assert(results.size() == 1);
        assert(results[0].success);
        assert(BRepCheck_Analyzer(results[0].face).IsValid());
        assert(countWires(results[0].face) == 2);
    }

    {
        sketch::SketchPlane plane;
        plane.origin = {0.0, 0.0, 0.0};
        plane.xAxis = {0.0, 1.0, 0.0};
        plane.yAxis = {-1.0, 0.0, 0.0};
        plane.normal = {0.0, 0.0, 1.0};

        sketch::Sketch sketch(plane);
        auto p1 = sketch.addPoint(0.0, 0.0);
        auto p2 = sketch.addPoint(10.0, 0.0);
        auto p3 = sketch.addPoint(10.0, 5.0);
        auto p4 = sketch.addPoint(0.0, 5.0);

        sketch.addLine(p1, p2);
        sketch.addLine(p2, p3);
        sketch.addLine(p3, p4);
        sketch.addLine(p4, p1);

        loop::LoopDetector detector;
        auto loops = detector.detect(sketch);

        loop::FaceBuilder builder;
        auto results = builder.buildAllFaces(loops, sketch);

        assert(results.size() == 1);
        assert(results[0].success);

        Bnd_Box box;
        BRepBndLib::Add(results[0].face, box);
        double xmin = 0.0, ymin = 0.0, zmin = 0.0;
        double xmax = 0.0, ymax = 0.0, zmax = 0.0;
        box.Get(xmin, ymin, zmin, xmax, ymax, zmax);

        assert(nearlyEqual(zmin, 0.0));
        assert(nearlyEqual(zmax, 0.0));
        assert(nearlyEqual(xmin, -5.0));
        assert(nearlyEqual(xmax, 0.0));
        assert(nearlyEqual(ymin, 0.0));
        assert(nearlyEqual(ymax, 10.0));
    }

    {
        sketch::Sketch sketch;
        auto center = sketch.addPoint(0.0, 0.0);
        auto ellipse = sketch.addEllipse(center, 6.0, 3.0, 0.25);
        assert(!ellipse.empty());

        loop::LoopDetectorConfig config;
        config.planarizeIntersections = true;
        loop::LoopDetector detector(config);
        auto loops = detector.detect(sketch);
        assert(loops.success);
        assert(loops.faces.size() == 1);

        loop::FaceBuilder builder;
        auto results = builder.buildAllFaces(loops, sketch);
        assert(results.size() == 1);
        assert(results[0].success);
        assert(BRepCheck_Analyzer(results[0].face).IsValid());
        assert(countWires(results[0].face) == 1);
    }

    std::cout << "Face builder prototype: OK" << std::endl;
    return 0;
}
