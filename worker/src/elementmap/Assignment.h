// Assignment.h — optimal (min-cost) assignment for referenced-only element sets
// (W-WP6, SCHEMA §10 "min-cost assignment … greedy is a documented counterexample").
//
// ── Why not greedy ───────────────────────────────────────────────────────────
// When a plan step references SEVERAL elements at once (e.g. a fillet over 3 edges,
// or a repair dialog resolving N stale refs against one body), each ref scores
// against every candidate sub-shape. Greedy "assign each ref its individual best"
// can double-book a candidate and force a strictly worse global outcome.
//
//   Counterexample (rows = refs, cols = candidates, values = MATCH SCORE):
//                cand_X   cand_Y
//       ref_A     0.92     0.90
//       ref_B     0.91     0.20
//   Greedy processes ref_A first, takes X (0.92); ref_B is then forced onto Y (0.20)
//   → total 1.12, and ref_B looks unresolvable. Optimal assigns A→Y (0.90), B→X
//   (0.91) → total 1.81 — the global-best assignment, and B now has a real shot.
//   Greedy's first-come choice would have destroyed ref_B's only good match. This
//   module returns the OPTIMAL assignment (maximising total score ⇔ minimising
//   total cost), so the confidence gate then sees each ref's true best AVAILABLE
//   binding, not one starved by another ref's greedy grab.
//
//   NOTE this does not by itself guarantee AutoBind: the per-ref margin gate
//   (assigned score − best OTHER candidate's score, applied after assignment —
//   SCHEMA §10) still runs. Here ref_A's assigned score (0.90 on Y) is BELOW its
//   own unassigned alternative (0.92 on X): margin = 0.90 − 0.92 = −0.02 <
//   kAutoBindMinMargin, so ref_A still routes to NeedsRepair("ambiguous") even
//   under the optimal assignment. Intended: a false negative (repair prompt) is
//   preferred over a false positive (silent mis-bind) when two candidates are
//   this close — optimal assignment fixes greedy's starvation bug, it does not
//   relax the confidence gate.
#pragma once

#include <vector>

namespace onecad::elementmap {

// Minimise Σ cost[i][assignment[i]] over injective assignments of the `rows` rows
// to distinct columns. Requires `rows ≤ cols`. Returns a vector of length `rows`
// whose i-th entry is the column assigned to row i (each column used at most once).
// Deterministic: ties broken by lowest column index. O(rows² · cols) Hungarian
// (Kuhn–Munkres, shortest-augmenting-path form). An empty matrix ⇒ empty result.
std::vector<int> min_cost_assignment(const std::vector<std::vector<double>>& cost);

}  // namespace onecad::elementmap
