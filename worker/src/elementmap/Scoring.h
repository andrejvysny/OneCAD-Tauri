// Scoring.h — normalized [0,1] descriptor-match confidence (W-WP6, resolverVersion 1).
//
// ── Why a new score (not the OneCAD-CPP one) ─────────────────────────────────
// OneCAD-CPP `ElementMap::score()` (kernel/elementmap/ElementMap.h:639-672) is an
// UNBOUNDED, scale-dependent COST: it sums a raw center distance (mm) with fixed
// per-mismatch penalties (1000, 10, 5, …). Its magnitude depends on the model's
// absolute size and units, so it cannot express the locked confidence policy
// (auto-bind iff score ≥ 0.85 AND margin ≥ 0.10 — SCHEMA §10). This module is the
// redesign: a normalized [0,1] CONFIDENCE (higher = better) built from per-feature
// similarities in [0,1], each with a fixed weight. The 14-field DESCRIPTOR itself
// is ported verbatim (it is EVIDENCE, never identity — Invariant 2); only the
// scoring on top of it is new and versioned (`kResolverVersion`).
//
// ── Feature model ────────────────────────────────────────────────────────────
// Per candidate we compute a weighted sum of feature similarities, renormalized by
// the weight of the features whose evidence is actually present (so an anchor-only
// ref still yields a bounded [0,1] score). Weights (faces / edges):
//   type       0.20  surfaceType (face) / curveType (edge) exact match → 1 else 0
//   magnitude  0.25  area (face) / length (edge) relative closeness
//   direction  0.20  |normal·normal| (face) / |tangent·tangent| (edge)
//   anchor     0.25  world-point proximity to the candidate centre (narrowing)
//   adjacency  0.10  adjacencyHash exact match → 1 else 0
// `adjacency` is deliberately LOW-weight: the ported adjacency hash is all-or-nothing
// (ANY dimensional edit flips it to 0), so a higher weight would sink a
// topology-preserving small edit below the 0.85 gate and defeat the whole point of
// the ladder (a fillet edge surviving an upstream parameter change). Ambiguity is
// caught by the MARGIN gate (a symmetric twin ties regardless of weights), not by
// the absolute score, so lowering adjacency loses no safety. A vertex ref (or a ref
// with no frozen descriptor) scores on `anchor` alone.
#pragma once

#include <cstdint>
#include <map>
#include <string>

#include <gp_Pnt.hxx>

#include "kernel/elementmap/ElementMap.h"

namespace onecad::elementmap {

namespace km = onecad::kernel::elementmap;

// resolverVersion (SCHEMA §10 / handshake §13). Bump on any scoring change; it is
// stamped into every NeedsRepair evidence payload (`scoringVersion`).
inline constexpr int kResolverVersion = 1;

// Locked confidence policy (SCHEMA §10). A false positive (silent wrong bind) is
// strictly worse than a false negative (asking the user), so BOTH must hold.
inline constexpr double kAutoBindMinScore = 0.85;   // best candidate confidence
inline constexpr double kAutoBindMinMargin = 0.10;  // best − runner-up

// Anchor evidence used to narrow a descriptor tie (SCHEMA §10 "anchor narrowing").
struct AnchorEvidence {
    bool has_world_point = false;
    gp_Pnt world_point{0.0, 0.0, 0.0};
};

// A normalized [0,1] confidence + the per-feature contributions that produced it
// (they sum to `score`). Reported verbatim as NeedsRepair `featureContributions`
// so a repair UI — and, later, a Rust-side policy — can inspect the evidence.
struct ScoreResult {
    double score = 0.0;
    std::map<std::string, double> contributions;  // named; Σ == score
};

// Score one candidate against a frozen intent descriptor + anchor. When
// `has_intent_descriptor` is false only the `anchor` feature is available
// (anchor-only resolution). `body_diag` scales the anchor proximity feature
// (bbox diagonal of the body the candidate lives in). Deterministic; pure.
ScoreResult score_candidate(const km::ElementDescriptor& intent, bool has_intent_descriptor,
                            const AnchorEvidence& anchor, const km::ElementDescriptor& candidate,
                            double body_diag);

// --- test-only instrumentation ------------------------------------------------
// Number of score_candidate() evaluations since the last reset. The W-WP6
// calibration corpus asserts this stays 0 in the "history resolves everything"
// case — proving the descriptor stage is NEVER consulted when OCCT history alone
// rebinds every tracked element (SCHEMA §10 ladder level 1). Thread-safe.
std::uint64_t scoring_call_count();
void reset_scoring_call_count();

}  // namespace onecad::elementmap
