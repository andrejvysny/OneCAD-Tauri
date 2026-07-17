// Assignment.cpp — see Assignment.h. Kuhn–Munkres (Hungarian) O(n²·m), the
// shortest-augmenting-path form with row/column potentials (n rows ≤ m cols).
#include "elementmap/Assignment.h"

#include <limits>

namespace onecad::elementmap {

std::vector<int> min_cost_assignment(const std::vector<std::vector<double>>& cost) {
    const int n = static_cast<int>(cost.size());
    if (n == 0) return {};
    const int m = static_cast<int>(cost[0].size());
    if (m == 0 || m < n) return {};  // requires rows ≤ cols

    constexpr double kInf = std::numeric_limits<double>::max() / 4.0;

    // 1-based potentials + column→row match (p[0] is a scratch slot).
    std::vector<double> u(n + 1, 0.0), v(m + 1, 0.0);
    std::vector<int> p(m + 1, 0), way(m + 1, 0);

    for (int i = 1; i <= n; ++i) {
        p[0] = i;
        int j0 = 0;
        std::vector<double> minv(m + 1, kInf);
        std::vector<char> used(m + 1, 0);
        do {
            used[j0] = 1;
            const int i0 = p[j0];
            double delta = kInf;
            int j1 = -1;
            for (int j = 1; j <= m; ++j) {
                if (used[j]) continue;
                const double cur = cost[i0 - 1][j - 1] - u[i0] - v[j];
                if (cur < minv[j]) {
                    minv[j] = cur;
                    way[j] = j0;
                }
                // Strict '<' keeps the lowest column index on ties (deterministic).
                if (minv[j] < delta) {
                    delta = minv[j];
                    j1 = j;
                }
            }
            for (int j = 0; j <= m; ++j) {
                if (used[j]) {
                    u[p[j]] += delta;
                    v[j] -= delta;
                } else {
                    minv[j] -= delta;
                }
            }
            j0 = j1;
        } while (p[j0] != 0);
        // Augment along the recorded path.
        do {
            const int j1 = way[j0];
            p[j0] = p[j1];
            j0 = j1;
        } while (j0 != 0);
    }

    std::vector<int> assignment(n, -1);
    for (int j = 1; j <= m; ++j) {
        if (p[j] >= 1 && p[j] <= n) assignment[p[j] - 1] = j - 1;
    }
    return assignment;
}

}  // namespace onecad::elementmap
