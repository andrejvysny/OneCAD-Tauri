// test_wp6_ladder.cpp — W-WP6 resolution-ladder CALIBRATION CORPUS (scope A).
// White-box, in-process, real OCCT. Proves the locked scoring policy (SCHEMA §10):
//   (1) SYMMETRIC TIE  ⇒ NeedsRepair — two candidates tie, margin 0, never a guess.
//   (2) CONFIDENT      ⇒ AutoBind — a clear winner (score ≥ 0.85, margin ≥ 0.10).
//   (3) HISTORY RESOLVES EVERYTHING ⇒ the descriptor stage is NEVER consulted
//       (scoring_call_count() stays 0 when every tracked element rebinds via a
//       unique OCCT-history image).
//   (4) MIN-COST ASSIGNMENT beats greedy on the documented counterexample.
//   (5) SCORED SPLIT LINEAGE (closes review finding 2): a symmetric split of a
//       tracked face ⇒ NeedsRepair "ambiguous" (was an unscored Modified().First()).
// No framework: exit code == failure count.
#include <cstdio>
#include <string>
#include <vector>

#include <BRepAlgoAPI_Cut.hxx>
#include <BRepAlgoAPI_Fuse.hxx>
#include <BRepPrimAPI_MakeBox.hxx>
#include <TopExp.hxx>
#include <TopTools_IndexedMapOfShape.hxx>
#include <TopoDS_Shape.hxx>
#include <gp_Pnt.hxx>

#include "elementmap/Assignment.h"
#include "elementmap/ElementMapPartition.h"
#include "elementmap/Ladder.h"
#include "elementmap/Scoring.h"

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

// The face of `shape` whose bbox centre is nearest (cx,cy,cz).
TopoDS_Shape face_by_center(const TopoDS_Shape& shape, double cx, double cy, double cz) {
    TopTools_IndexedMapOfShape faces;
    TopExp::MapShapes(shape, TopAbs_FACE, faces);
    TopoDS_Shape best;
    double best_d2 = -1.0;
    for (int i = 1; i <= faces.Extent(); ++i) {
        const km::ElementDescriptor d = em::ElementMapPartition::describe(faces(i));
        const double dx = d.center.X() - cx, dy = d.center.Y() - cy, dz = d.center.Z() - cz;
        const double d2 = dx * dx + dy * dy + dz * dz;
        if (best_d2 < 0.0 || d2 < best_d2) { best_d2 = d2; best = faces(i); }
    }
    return best;
}

TopoDS_Shape edge_by_center(const TopoDS_Shape& shape, double cx, double cy, double cz) {
    TopTools_IndexedMapOfShape edges;
    TopExp::MapShapes(shape, TopAbs_EDGE, edges);
    TopoDS_Shape best;
    double best_d2 = -1.0;
    for (int i = 1; i <= edges.Extent(); ++i) {
        const km::ElementDescriptor d = em::ElementMapPartition::describe(edges(i));
        const double dx = d.center.X() - cx, dy = d.center.Y() - cy, dz = d.center.Z() - cz;
        const double d2 = dx * dx + dy * dy + dz * dz;
        if (best_d2 < 0.0 || d2 < best_d2) { best_d2 = d2; best = edges(i); }
    }
    return best;
}

em::LadderRef face_ref(const std::string& id, const TopoDS_Shape& intent_face, double ax, double ay,
                       double az) {
    em::LadderRef r;
    r.ref_id = id;
    r.element_id = id;
    r.kind = km::ElementKind::Face;
    r.has_descriptor = true;
    r.descriptor = em::ElementMapPartition::describe(intent_face);
    r.anchor.has_world_point = true;
    r.anchor.world_point = gp_Pnt(ax, ay, az);
    r.anchor_json = {{"worldPoint", {ax, ay, az}}};
    return r;
}

// (1) SYMMETRIC TIE — a 20×10×10-tall box; the two 10×10 end faces (z=±10) are
// descriptor twins and the anchor sits equidistant between them ⇒ tie ⇒ NeedsRepair.
void test_symmetric_tie() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(gp_Pnt(-5, -5, -10), 10.0, 10.0, 20.0).Shape();
    const TopoDS_Shape top = face_by_center(box, 0, 0, 10);  // z=+10 end face
    // Anchor at the body centre → equidistant to the +Z and −Z end faces.
    std::vector<em::LadderRef> refs{face_ref("el_sym", top, 0, 0, 0)};

    const auto res = em::resolve_descriptor_stage(box, "body", refs);
    check(res.size() == 1, "sym: one resolution");
    check(res[0].outcome == em::LadderOutcome::NeedsRepair, "sym: NeedsRepair (never a guess)");
    check(res[0].reason == "ambiguous", "sym: reason ambiguous");
    check(res[0].candidates.size() >= 2, "sym: >=2 candidates surfaced");
    if (res[0].candidates.size() >= 2) {
        // The load-bearing guarantee: the two best candidates TIE (margin < 0.10).
        const double s1 = res[0].candidates[0].score, s2 = res[0].candidates[1].score;
        check(s1 == s2, "sym: top-2 candidate scores are equal (a tie)");
        check((s1 - s2) < em::kAutoBindMinMargin, "sym: tie margin below policy margin");
    }
    // Evidence payload carries scoringVersion (SCHEMA §9).
    check(res[0].to_needs_repair_json().value("scoringVersion", -1) == em::kResolverVersion,
          "sym: NeedsRepair payload stamps scoringVersion");
}

// (2) CONFIDENT — a 10×20×30 box (all three face-pairs distinct areas); the anchor
// sits ON the z=0 face, far from its z=30 twin ⇒ a clear unique winner ⇒ AutoBind.
void test_confident_autobind() {
    const TopoDS_Shape box = BRepPrimAPI_MakeBox(10.0, 20.0, 30.0).Shape();
    const TopoDS_Shape z0 = face_by_center(box, 5, 10, 0);
    std::vector<em::LadderRef> refs{face_ref("el_z0", z0, 5, 10, 0)};

    const auto res = em::resolve_descriptor_stage(box, "body", refs);
    check(res.size() == 1, "confident: one resolution");
    check(res[0].outcome == em::LadderOutcome::AutoBind, "confident: AutoBind");
    check(res[0].score >= em::kAutoBindMinScore, "confident: score >= 0.85");
    check(res[0].margin >= em::kAutoBindMinMargin, "confident: margin >= 0.10");
    // It bound the actual z=0 face (its snapshot TopoKey resolves back to that face).
    const std::string tk =
        em::ElementMapPartition::topokey_for_shape(box, z0, km::ElementKind::Face);
    check(res[0].bound_topo_key == tk, "confident: bound the correct face topoKey");
}

// (3) HISTORY RESOLVES EVERYTHING — track a face that survives a fuse with a UNIQUE
// image; apply_history rebinds it WITHOUT ever consulting the descriptor scorer.
void test_history_resolves_no_scoring() {
    const TopoDS_Shape a = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();          // 0..10
    const TopoDS_Shape b = BRepPrimAPI_MakeBox(gp_Pnt(6, 6, 6), 10.0, 10.0, 10.0).Shape();

    em::ElementMapPartition part;
    // Track the x=0 face — the corner box `b` does not touch it, so the fuse maps it
    // to a single image (unique history).
    const TopoDS_Shape x0 = face_by_center(a, 0, 5, 5);
    part.mint("A", "A_x0", km::ElementKind::Face, x0, a);

    BRepAlgoAPI_Fuse fuse(a, b);
    fuse.Build();
    const TopoDS_Shape result = fuse.Shape();

    em::reset_scoring_call_count();
    em::ElementMapDelta delta;
    std::vector<nlohmann::json> nr;
    part.apply_history("A", result, fuse, delta, &nr);

    check(em::scoring_call_count() == 0,
          "history: descriptor scorer NEVER consulted (unique image auto-binds)");
    check(nr.empty(), "history: no NeedsRepair (history resolves the element)");
    check(part.contains("A_x0"), "history: tracked face still bound");
}

// (4) MIN-COST ASSIGNMENT — the documented greedy counterexample (Assignment.h).
void test_assignment_beats_greedy() {
    // cost = 1 − matchScore. Greedy gives ref_A its best (X, 0.08) and forces ref_B
    // onto Y (0.80) → total 0.88; optimal is A→Y (0.10) + B→X (0.09) = 0.19.
    const std::vector<std::vector<double>> cost = {{0.08, 0.10}, {0.09, 0.80}};
    const std::vector<int> a = em::min_cost_assignment(cost);
    check(a.size() == 2, "assign: two rows");
    check(a[0] == 1 && a[1] == 0, "assign: optimal A->Y, B->X (not greedy A->X)");
    const double total = cost[0][a[0]] + cost[1][a[1]];
    check(total < 0.20, "assign: optimal total cost < 0.20 (greedy would be 0.88)");
}

// (5) SCORED SPLIT LINEAGE (closes review finding 2) — a symmetric split of a
// tracked top face ⇒ NeedsRepair "ambiguous", with the scorer CONSULTED.
void test_symmetric_split_needs_repair() {
    const TopoDS_Shape base = BRepPrimAPI_MakeBox(20.0, 10.0, 10.0).Shape();  // top z=10, 20×10
    em::ElementMapPartition part;
    const TopoDS_Shape top = face_by_center(base, 10, 5, 10);
    // Mint WITH an anchor at the original centre so the split halves tie.
    part.mint("A", "A_top", km::ElementKind::Face, top, base,
              nlohmann::json{{"worldPoint", {10.0, 5.0, 10.0}}});

    // Cut a central slot across the top → splits it into two equal 9×10 halves.
    const TopoDS_Shape slot = BRepPrimAPI_MakeBox(gp_Pnt(9, 0, 8), 2.0, 10.0, 4.0).Shape();
    BRepAlgoAPI_Cut cut(base, slot);
    cut.Build();
    const TopoDS_Shape result = cut.Shape();

    em::reset_scoring_call_count();
    em::ElementMapDelta delta;
    std::vector<nlohmann::json> nr;
    part.apply_history("A", result, cut, delta, &nr);

    check(em::scoring_call_count() > 0, "split: descriptor scorer WAS consulted (finding 2)");
    check(nr.size() == 1, "split: exactly one NeedsRepair for the ambiguous split");
    if (nr.size() == 1) {
        check(nr[0].value("reason", "") == "ambiguous", "split: reason ambiguous");
        check(nr[0].value("ladderFailed", "") == "history", "split: ladderFailed history");
        check(nr[0]["candidates"].size() >= 2, "split: both split images surfaced as candidates");
    }
    check(!part.contains("A_top"), "split: ambiguous entry dropped (never a wrong bind)");
}

// (6) THE MIGRATION GOAL — a fillet edge reference (frozen on box A, width 10)
// SURVIVES a small upstream edit (width 10.5) via descriptor+anchor, but a large
// ambiguous change (width 30) ⇒ NeedsRepair (never a silent wrong bind). This is
// the H5-B naming-break fix expressed at the ladder level (corpus case e).
void test_survives_small_edit_else_needs_repair() {
    const TopoDS_Shape a = BRepPrimAPI_MakeBox(10.0, 10.0, 10.0).Shape();
    const TopoDS_Shape edge_a = edge_by_center(a, 5, 0, 10);  // top-front edge, len 10
    // The ref as authored on A (frozen edge descriptor + anchor at its midpoint).
    em::LadderRef ref;
    ref.ref_id = "op_fillet.input0";
    ref.element_id = "el_rim";
    ref.kind = km::ElementKind::Edge;
    ref.has_descriptor = true;
    ref.descriptor = em::ElementMapPartition::describe(edge_a);
    ref.anchor.has_world_point = true;
    ref.anchor.world_point = gp_Pnt(5, 0, 10);
    ref.anchor_json = {{"worldPoint", {5.0, 0.0, 10.0}}};

    // Small edit: width 10 → 10.5. The frozen ref auto-binds to the moved rim edge.
    const TopoDS_Shape b = BRepPrimAPI_MakeBox(10.5, 10.0, 10.0).Shape();
    auto rb = em::resolve_descriptor_stage(b, "body", {ref});
    check(rb.size() == 1 && rb[0].outcome == em::LadderOutcome::AutoBind,
          "edit: fillet edge SURVIVES a small upstream edit (auto-bind)");
    if (!rb.empty() && rb[0].outcome == em::LadderOutcome::AutoBind) {
        const TopoDS_Shape want = edge_by_center(b, 5.25, 0, 10);
        check(rb[0].bound_topo_key ==
                  em::ElementMapPartition::topokey_for_shape(b, want, km::ElementKind::Edge),
              "edit: bound the corresponding moved rim edge");
    }

    // Large edit: width 10 → 30. The rim edge changed too much ⇒ NeedsRepair.
    const TopoDS_Shape c = BRepPrimAPI_MakeBox(30.0, 10.0, 10.0).Shape();
    auto rc = em::resolve_descriptor_stage(c, "body", {ref});
    check(rc.size() == 1 && rc[0].outcome == em::LadderOutcome::NeedsRepair,
          "edit: a large ambiguous change ⇒ NeedsRepair (never a wrong bind)");
}

}  // namespace

int main() {
    test_symmetric_tie();
    test_confident_autobind();
    test_history_resolves_no_scoring();
    test_assignment_beats_greedy();
    test_symmetric_split_needs_repair();
    test_survives_small_edit_else_needs_repair();
    if (g_failures == 0) std::fprintf(stderr, "wp6_ladder: OK\n");
    return g_failures;
}
