# Solver-lane latency benchmark (W-WP3b GATE)

Round-trip = steady_clock at request write -> response read. solveMicros is the worker-reported PlaneGCS solve time; transport = round-trip - solveMicros. Each row: small-move drags (+ large jumps) over one gesture.

| scenario | entities | samples | status | rtt p50 (ms) | rtt p95 (ms) | rtt p99 (ms) | solve p50 | solve p95 | solve p99 | transport p95 (ms) |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|
| chain 10 | 9 | 1050 | success | 0.070 | 0.091 | 0.114 | 0.045 | 0.057 | 0.071 | 0.034 |
| chain 50 | 51 | 1050 | success | 0.239 | 0.298 | 0.349 | 0.189 | 0.238 | 0.275 | 0.060 |
| chain 200 | 201 | 1050 | success | 2.282 | 2.660 | 2.806 | 2.142 | 2.498 | 2.642 | 0.166 |
| chain 500 | 501 | 1050 | success | 23.340 | 26.917 | 29.849 | 22.977 | 26.546 | 29.481 | 0.392 |
| pathological near-singular | 5 | 204 | success | 0.054 | 0.070 | 0.097 | 0.030 | 0.040 | 0.071 | 0.029 |
| pathological redundant | 3 | 204 | conflicting | 0.055 | 0.085 | 0.114 | 0.033 | 0.063 | 0.069 | 0.025 |
| pathological conflicting | 2 | 204 | conflicting | 0.048 | 0.056 | 0.059 | 0.028 | 0.034 | 0.037 | 0.023 |
| chain 200 (kernel BUSY) | 201 | 500 | success | 2.410 | 2.674 | 2.766 | 2.257 | 2.492 | 2.566 | 0.175 |

## GATE verdict (@200 entities)

- solver-only p95 = **2.498 ms** (target <= 2-3 ms)
- round-trip p95 = **2.660 ms** (target <= 6 ms; fallback <= 12-16 ms)

**VERDICT: PASS (120Hz-exact target met)**
