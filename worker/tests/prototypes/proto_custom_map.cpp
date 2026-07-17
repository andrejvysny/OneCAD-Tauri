// Ported from OneCAD-CPP tests/prototypes/proto_custom_map.cpp @ b4ddcccc (2026-07-16)
#include <iostream>
#include <string>
#include <map>
#include <vector>
#include <functional> // For std::hash

// OCCT
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <TopExp_Explorer.hxx>
#include <TopExp.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <TopoDS_Edge.hxx>
#include <TopTools_DataMapOfShapeShape.hxx>
#include <BRepTools.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopTools_ShapeMapHasher.hxx>
#include <BRep_Tool.hxx>
#include <gp_Pnt.hxx>
#include <gp_Ax2.hxx>
#include <gp_Dir.hxx>

#include "kernel/elementmap/ElementMap.h"

using onecad::kernel::elementmap::ElementId;
using onecad::kernel::elementmap::ElementKind;
using onecad::kernel::elementmap::ElementMap;

int main() {
    std::cout << "--- Custom ElementMap Prototype ---" << std::endl;

    ElementMap emap;

    // 1. Create Box
    BRepPrimAPI_MakeBox mkBox(10.0, 10.0, 10.0);
    mkBox.Build();
    TopoDS_Shape boxShape = mkBox.Shape();
    ElementId bodyId{"body-1"};
    emap.registerElement(bodyId, ElementKind::Body, boxShape, "op-box");
    
    // 2. Assign IDs to all Faces
    TopExp_Explorer exp(boxShape, TopAbs_FACE);
    int count = 0;
    ElementId topFaceId{};
    
    for (; exp.More(); exp.Next()) {
        TopoDS_Face f = TopoDS::Face(exp.Current());
        ElementId id{"face-" + std::to_string(++count)};
        emap.registerElement(id, ElementKind::Face, f, "op-box", {bodyId});
        
        // Identify top face for tracking
        TopExp_Explorer expWire(f, TopAbs_WIRE);
        if (expWire.More()) {
            TopExp_Explorer expEdge(expWire.Current(), TopAbs_EDGE);
            if (expEdge.More()) {
                 TopoDS_Edge e = TopoDS::Edge(expEdge.Current());
                 gp_Pnt p = BRep_Tool::Pnt(TopExp::FirstVertex(e)); 
                 if (p.Z() > 9.9) topFaceId = id;
            }
        }
    }
    
    if (topFaceId.value.empty()) {
        std::cout << "Failed to find top face" << std::endl;
        return 1;
    }
    std::cout << "Registered " << count << " faces. Top Face is " << topFaceId.value << std::endl;
    
    // 3. Cut with Cylinder
    BRepPrimAPI_MakeCylinder mkCyl(gp_Ax2(gp_Pnt(5,5,0), gp_Dir(0,0,1)), 3.0, 20.0);
    BRepAlgoAPI_Cut mkCut(boxShape, mkCyl.Shape());
    mkCut.Build();
    
    if (!mkCut.IsDone()) {
        std::cout << "Cut failed" << std::endl;
        return 1;
    }
    
    // 4. Update ElementMap using History
    const std::vector<ElementId> deleted = emap.update(mkCut, "op-cut");
    for (const auto& id : deleted) {
        std::cout << "Deleted: " << id.value << std::endl;
    }
    
    // 5. Check result
    std::cout << "After Cut:" << std::endl;
    
    if (const auto* topEntry = emap.find(topFaceId)) {
        if (topEntry->shape.IsNull()) {
            std::cout << "FAILURE: Top Face shape missing." << std::endl;
            return 1;
        }
        std::cout << "SUCCESS: Top Face ID preserved! TShape: " << topEntry->shape.TShape().get() << std::endl;
    } else {
        std::cout << "FAILURE: Top Face ID lost." << std::endl;
        return 1;
    }

    return 0;
}
