// ScratchJob.cpp — see ScratchJob.h.
//
// W-WP5: `ScratchJob` is now a plain aggregate. The former
// `committed_lines_on_accept()` (which spliced base + executed canonical op lines
// to recompute a SHA-256 prefix) was DELETED with the worker-side hash computation
// — the head token is opaque and Rust-minted (HistoryHash.h). The prepared token
// is `history_prefix_hash`, set directly by PlanExecutor from `prefixHashes[]`.
// This TU is intentionally empty (kept so the build/CMake source list is stable).
#include "session/ScratchJob.h"
