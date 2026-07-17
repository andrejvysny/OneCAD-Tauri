// Scoring.cpp — see Scoring.h.
#include "elementmap/Scoring.h"

#include <algorithm>
#include <atomic>
#include <cmath>
#include <vector>

#include <TopAbs_ShapeEnum.hxx>

namespace onecad::elementmap {

namespace {

std::atomic<std::uint64_t> g_scoring_calls{0};

// Relative closeness of two non-negative magnitudes → [0,1]. Equal ⇒ 1; both ~0
// (e.g. vertices) ⇒ 1; wildly different ⇒ →0. Bounded and symmetric.
double magnitude_similarity(double a, double b) {
    const double denom = std::abs(a) + std::abs(b);
    if (denom < 1e-12) return 1.0;
    return std::max(0.0, 1.0 - std::abs(a - b) / denom);
}

// One weighted feature contributing to the score.
struct Feature {
    const char* name;
    double weight;
    double similarity;  // [0,1]
};

}  // namespace

std::uint64_t scoring_call_count() { return g_scoring_calls.load(std::memory_order_relaxed); }
void reset_scoring_call_count() { g_scoring_calls.store(0, std::memory_order_relaxed); }

ScoreResult score_candidate(const km::ElementDescriptor& intent, bool has_intent_descriptor,
                            const AnchorEvidence& anchor, const km::ElementDescriptor& candidate,
                            double body_diag) {
    g_scoring_calls.fetch_add(1, std::memory_order_relaxed);

    const bool is_face = candidate.shapeType == TopAbs_FACE;
    const bool is_edge = candidate.shapeType == TopAbs_EDGE;

    std::vector<Feature> feats;

    // Descriptor-derived features are available only with a frozen intent
    // descriptor (real semantic ref). Anchor-only refs skip straight to `anchor`.
    if (has_intent_descriptor && (is_face || is_edge)) {
        // type: surfaceType (face) or curveType (edge), exact categorical match.
        const bool type_match = is_face ? (intent.surfaceType == candidate.surfaceType)
                                        : (intent.curveType == candidate.curveType);
        feats.push_back({"type", 0.20, type_match ? 1.0 : 0.0});

        // magnitude: area (face) or length (edge) relative closeness.
        feats.push_back({is_face ? "area" : "length", 0.25,
                         magnitude_similarity(intent.magnitude, candidate.magnitude)});

        // direction: |normal·normal| (face) / |tangent·tangent| (edge). Absolute
        // value so an orientation flip (a mirror twin) still aligns — that keeps a
        // symmetric split a genuine descriptor TIE (→ NeedsRepair) rather than
        // silently favouring one twin.
        if (is_face && intent.hasNormal && candidate.hasNormal) {
            const double dot = std::abs(intent.normal.Dot(candidate.normal));
            feats.push_back({"normal", 0.20, std::clamp(dot, 0.0, 1.0)});
        } else if (is_edge && intent.hasTangent && candidate.hasTangent) {
            const double dot = std::abs(intent.tangent.Dot(candidate.tangent));
            feats.push_back({"tangent", 0.20, std::clamp(dot, 0.0, 1.0)});
        }

        // adjacency: 64-bit adjacency hash exact match (LOW weight — see Scoring.h;
        // the hash is all-or-nothing, so a topology-preserving edit must not be sunk
        // by it below the auto-bind gate).
        if (intent.adjacencyHash != 0 && candidate.adjacencyHash != 0) {
            feats.push_back(
                {"adjacency", 0.10, intent.adjacencyHash == candidate.adjacencyHash ? 1.0 : 0.0});
        }
    }

    // anchor: world-point proximity to the candidate centre. Scale by half the
    // body diagonal so proximity is unit-independent; a coincident anchor ⇒ 1.
    if (anchor.has_world_point) {
        const double scale = std::max(0.5 * body_diag, 1.0);
        const double dist = anchor.world_point.Distance(candidate.center);
        feats.push_back({"anchor", 0.25, std::max(0.0, 1.0 - dist / scale)});
    }

    double total_weight = 0.0;
    double weighted = 0.0;
    for (const Feature& f : feats) {
        total_weight += f.weight;
        weighted += f.weight * f.similarity;
    }

    ScoreResult out;
    if (total_weight <= 0.0) return out;  // no evidence → score 0
    out.score = weighted / total_weight;
    // Contributions are renormalized so they SUM to the reported score (matching
    // the SCHEMA §9 example where the featureContributions add up to `score`).
    for (const Feature& f : feats) {
        out.contributions[f.name] = (f.weight * f.similarity) / total_weight;
    }
    return out;
}

}  // namespace onecad::elementmap
