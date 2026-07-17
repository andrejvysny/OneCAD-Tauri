// Ported from OneCAD-CPP tests/prototypes/proto_tnaming.cpp @ b4ddcccc (2026-07-16)
#include <iostream>
#include <string>

// OCCT
#include <TDocStd_Document.hxx>
#include <TDocStd_Application.hxx>
#include <TNaming_Builder.hxx>
#include <TNaming_NamedShape.hxx>
#include <TNaming_Tool.hxx>
#include <TDF_Label.hxx>
#include <TDF_Tool.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <BRepPrimAPI_MakeCylinder.hxx>
#include <BRepAlgoAPI_Cut.hxx>
#include <TopExp_Explorer.hxx>
#include <TopoDS.hxx>
#include <TopoDS_Face.hxx>
#include <BRep_Tool.hxx>
#include <TNaming_Iterator.hxx>
#include <TopTools_ShapeMapHasher.hxx>

// BinDrivers for saving (optional, for debug)
#include <BinDrivers.hxx>

void printShapeInfo(const TopoDS_Shape& shape, const std::string& label) {
    if (shape.IsNull()) {
        std::cout << label << ": NULL" << std::endl;
        return;
    }
    std::cout << label << ": Type=" << shape.ShapeType() 
              << " TShape=" << shape.TShape().get() << " Orient=" << (int)shape.Orientation() << std::endl;
}

int main() {
    std::cout << "--- OCAF TNaming Prototype ---" << std::endl;

    // 1. Setup OCAF Document
    Handle(TDocStd_Application) app = new TDocStd_Application();
    BinDrivers::DefineFormat(app);
    Handle(TDocStd_Document) doc;
    app->NewDocument("BinOcaf", doc);

    // 2. Create Initial Geometry (Box)
    TDF_Label mainLabel = doc->Main();
    TDF_Label boxLabel = mainLabel.FindChild(1);
    
    BRepPrimAPI_MakeBox mkBox(10.0, 10.0, 10.0);
    mkBox.Build();
    TopoDS_Shape boxShape = mkBox.Shape();
    
    // Register Box in OCAF
    TNaming_Builder(boxLabel).Generated(boxShape);
    std::cout << "Created Box" << std::endl;

    // 3. Identify a specific Face (e.g., Top Face)
    // In a real app, we would pick this via selection. 
    // Here we just find the face with highest Z.
    TopoDS_Face topFace;
    double maxZ = -1e9;
    
    TopExp_Explorer exp(boxShape, TopAbs_FACE);
    for (; exp.More(); exp.Next()) {
        TopoDS_Face f = TopoDS::Face(exp.Current());
        // Simple heuristic for demo
        TopExp_Explorer expWire(f, TopAbs_WIRE);
        if (expWire.More()) {
            TopExp_Explorer expEdge(expWire.Current(), TopAbs_EDGE);
            if (expEdge.More()) {
                TopoDS_Edge e = TopoDS::Edge(expEdge.Current());
                gp_Pnt p = BRep_Tool::Pnt(TopExp::FirstVertex(e)); 
                 if (p.Z() > maxZ) {
                    maxZ = p.Z();
                    topFace = f;
                }
            }
        }
    }
    
    // Create a label for this specific face to "Watch" it
    TDF_Label faceLabel = mainLabel.FindChild(2);
    TNaming_Builder(faceLabel).Generated(topFace); // This might be wrong usage, usually we use Selected
    
    // Correct usage: Use TNaming_Selector or just rely on history?
    // Let's try to simulate history tracking manually first via NamedShape.
    
    // 4. Modify Geometry (Cut with Cylinder)
    TDF_Label cutLabel = mainLabel.FindChild(3);
    BRepPrimAPI_MakeCylinder mkCyl(gp_Ax2(gp_Pnt(5,5,0), gp_Dir(0,0,1)), 3.0, 20.0);
    
    BRepAlgoAPI_Cut mkCut(boxShape, mkCyl.Shape());
    mkCut.Build();
    TopoDS_Shape cutShape = mkCut.Shape();
    
    // Register evolution: Box -> CutShape
    TNaming_Builder(cutLabel).Modify(boxShape, cutShape);
    std::cout << "Performed Cut" << std::endl;

    // 5. Query: Where is my "Top Face" now?
    // OCAF should be able to resolve "Top Face" in the new shape if we track it correctly.
    
    // This part is the tricky one. TNaming relies on "NamedShape" which stores the evolution.
    // If we want to find "what happened to topFace", we iterate the NamedShape attribute at cutLabel.
    
    Handle(TNaming_NamedShape) ns;
    if (cutLabel.FindAttribute(TNaming_NamedShape::GetID(), ns)) {
        // Iterate evolution
        TNaming_Iterator it(ns);
        for (; it.More(); it.Next()) {
            if (it.OldShape().IsSame(topFace)) {
                std::cout << "FOUND! Top Face evolved to a new shape." << std::endl;
                printShapeInfo(it.NewShape(), "New Top Face");
            }
        }
    } else {
        std::cout << "Could not find NamedShape at cutLabel" << std::endl;
    }

    return 0;
}
