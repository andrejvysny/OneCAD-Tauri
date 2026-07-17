// test_wp5_partition_history.cpp — white-box test of ElementMap V2 history
// rebinding against REAL OCCT builder history. Demonstrates: a minted target face
// rebinds via Modified (relabeled), a consumed face is dropped via IsDeleted
// (removed), and a tool body's entries are removed. No framework: exit == failures.
#include <cstdio>
#include <string>
#include <vector>

#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Pnt.hxx>

#include "elementmap/ElementMapPartition.h"

namespace em = onecad::elementmap;
namespace km = onecad::kernel::elementmap;

namespace {
int g_failures = 0;
void check(bool cond, const std::string& msg) {
    if (!cond) {
        std::fprintf(stderr, "FAIL: %s\n", msg.c_str());
        ++g_failures;
    }
}

// The MapShapes face index whose bbox center has the greatest Z (the +Z "top").
int top_face_index(const TopoDS_Shape& shape) {
    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(shape, TopAbs_FACE, faces);
    int best = 1;
    double best_z = -1e18;
    for (int i = 1; i <= faces.Extent(); ++i) {
        const km::ElementDescriptor d = em::ElementMapPartition::describe(faces(i));
        if (d.center.Z() > best_z) {
            best_z = d.center.Z();
            best = i;
        }
    }
    return best;
}

// --- rebind-via-Modified + survivor integrity ---
void test_rebind_via_modified() {
    const TopoDS_Shape box1 = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
    const TopoDS_Shape box2 = BRepPrimAPI_MakeBox(gp_Pnt(5, 5, 5), 10.0, 10.0, 10.0).Shape();

    em::ElementMapPartition part;
    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(box1, TopAbs_FACE, faces);
    for (int i = 1; i <= faces.Extent(); ++i) {
        part.mint("A", "A_f" + std::to_string(i), km::ElementKind::Face, faces(i), box1);
    }
    const std::size_t minted = part.entries_for_body("A").size();
    check(minted == 6, "mint: 6 faces tracked on body A");

    BRepAlgoAPI_Fuse fuse(box1, box2);
    fuse.Build();
    check(fuse.IsDone(), "fuse built");
    const TopoDS_Shape result = fuse.Shape();

    em::ElementMapDelta delta;
    std::vector<nlohmann::json> needs_repair;
    part.apply_history("A", result, fuse, delta, &needs_repair);

    // Every relabeled entry carries bodyId "A" + a topoKey resolving to a real face
    // in the fused result.
    for (const auto& e : delta.relabeled) {
        check(e.body_id == "A", "relabeled bodyId is A");
        check(e.kind == "face", "relabeled kind face");
        check(!em::ElementMapPartition::shape_for_topokey(result, e.topo_key).IsNull(),
              "relabeled topoKey resolves in the new snapshot");
    }
    // Survivors still resolve; removed are gone.
    for (const em::PartitionEntry* e : part.entries_for_body("A")) {
        check(!em::ElementMapPartition::shape_for_topokey(result, e->topo_key).IsNull(),
              "surviving entry binds to a real face");
    }
    for (const std::string& id : delta.removed) check(!part.contains(id), "removed entry dropped");

    // Accounting: relabeled + removed + still-unchanged-present == 6.
    const std::size_t survivors = part.entries_for_body("A").size();
    check(survivors + delta.removed.size() == 6, "all 6 minted faces accounted for");
    // A box fused with an overlapping box modifies (reindexes) at least one face →
    // proves rebind via Modified actually fires.
    check(!delta.relabeled.empty(), "at least one face rebound via Modified (relabeled)");
}

// --- consumed face → removed via IsDeleted (same-footprint taller fuse) ---
void test_removed_via_deleted() {
    const TopoDS_Shape base = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();   // z 0..10
    const TopoDS_Shape tall = BRepPrimAPI_MakeBox(10.0, 10.0, 20.0).Shape();   // z 0..20, same xy

    em::ElementMapPartition part;
    const int top = top_face_index(base);
    part.mint("A", "A_top", km::ElementKind::Face, [&] {
        TopTools_IndexedMapOfShape faces;
        TopExp::MapShapes(base, TopAbs_FACE, faces);
        return faces(top);
    }(), base);

    BRepAlgoAPI_Fuse fuse(base, tall);
    fuse.Build();
    const TopoDS_Shape result = fuse.Shape();

    em::ElementMapDelta delta;
    part.apply_history("A", result, fuse, delta);
    // The base's top face (z=10) becomes an interior seam of the union → consumed.
    bool removed = false;
    for (const std::string& id : delta.removed)
        if (id == "A_top") removed = true;
    check(removed, "consumed top face → delta.removed");
    check(!part.contains("A_top"), "consumed face dropped from partition");
}

// --- tool body entries removed by remove_body ---
void test_remove_body() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(4.0, 4.0, 4.0).Shape();
    em::ElementMapPartition part;
    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(box, TopAbs_FACE, faces);
    part.mint("B", "B_f1", km::ElementKind::Face, faces(1), box);
    part.mint("B", "B_f2", km::ElementKind::Face, faces(2), box);

    em::ElementMapDelta delta;
    part.remove_body("B", delta);
    check(delta.removed.size() == 2, "remove_body: both tool entries removed");
    check(!part.contains("B_f1") && !part.contains("B_f2"), "tool entries dropped");
}

}  // namespace

int main() {
    test_rebind_via_modified();
    test_removed_via_deleted();
    test_remove_body();
    if (g_failures == 0) std::fprintf(stderr, "wp5_partition_history: OK\n");
    return g_failures;
}
