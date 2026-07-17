// HistoryHash.h — the worker's opaque `historyPrefixHash` head token (SCHEMA §7.2).
//
// ══════════════════════════════════════════════════════════════════════════════
// OPAQUE-TOKEN CONTRACT (amended 2026-07-17 — "Rust is the sole hash authority"):
// ══════════════════════════════════════════════════════════════════════════════
//
// The worker NO LONGER computes any history hash. `expectedBaseHash` and every
// entry of `ExecutePlan.prefixHashes[]` are OPAQUE tokens minted by Rust (SHA-256
// over the geometry-relevant canonical wire-op form — a shape the worker cannot
// see). The worker only:
//   * STORES the current head token (a plain string),
//   * COMPARES `expectedBaseHash` against the head by string equality (fencing),
//   * ECHOES the token for the last executed op (`prefixHashes[lastExecutedIdx]`,
//     or `expectedBaseHash` when only the base is valid) on `PlanPrepared`,
//   * ADOPTS that echoed token as the new head on `AcceptPrepared`.
//
// The pre-amendment worker-side SHA-256 computation (`canonical_op_line`,
// `history_prefix_hash`) was DELETED: it hashed the wire op payload, which diverges
// from Rust's record-level canonical form after the first accepted plan and would
// raise false `PROTOCOL_ERROR`s. Making the token opaque removes that divergence
// class and lets a rename/cosmetic edit reuse a checkpoint (the Rust canonical form
// excludes record cosmetics).
//
// The only value the worker still owns is the empty-prefix anchor, shared verbatim
// with the Rust core (`onecad-core HistoryPrefixHash::empty()`): a fresh session
// head starts here.
#pragma once

#include <string>

namespace onecad::session {

// SHA-256("") — the empty-prefix history token. HARDCODED (the worker does not
// compute SHA-256 anymore). Byte-identical to the Rust core's
// `HistoryPrefixHash::empty()` — the shared cross-track anchor a fresh session
// head adopts (Session::open / Session::reset).
inline constexpr const char* kEmptyPrefixHash =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

}  // namespace onecad::session
