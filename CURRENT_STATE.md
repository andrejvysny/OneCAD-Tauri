# OneCAD-Tauri — Current State (2026-07-18)

Non-destructive migration of OneCAD-CPP (~69k LOC C++20 Qt6+OCCT) into a 4-layer
Tauri app per NEW_SPEC.md. Plan: `~/.claude/plans/act-as-senior-software-transient-popcorn.md`.
Tracker: `TODO.md` (per-WP gates + flags). OneCAD-CPP stays untouched.

## Milestone status

| Milestone | Status |
|---|---|
| M0 foundations (protocol contract, scaffolds, corpus) | DONE |
| M1 cores (Rust document/history/regen/io · C++ worker OCCT+solver · frontend slice) | DONE, all gates closed |
| **M2 first micro-slice integration gate** | **PASS** (`src-tauri/tests/m2_gate.rs` vs real worker) |
| M3 packaging gate | NEXT |
| M4 topology slice (param edit → rebind/NeedsRepair, repair UI) | pending |
| M5 lifecycle (checkpoints, crash drills, regen CLI, autosave) | pending |
| M6 hardening + backlog | pending |

## What works end-to-end (real worker, single automated test)
Sketch (PlaneGCS dof=0) → region → extrude → regen (ExecutePlan prepare/accept,
fenced) → MESH1 tessellation → face pick → ElementId promotion (stable ids,
Invariant 1) → fillet via resolution ladder → save v2 container → reopen →
deterministic replay in a fresh worker (identical hash chain/signatures,
byte-stable document.json) → STEP export → undo.

## Suites (all green, orchestrator-verified)
- Worker: 52/52 ctests, zero warnings (OCCT 7.9.3, ladder scoring resolverVersion 1)
- Rust: 330 tests, clippy -D warnings clean (incl. chaos 14, real-worker 5, m2_gate 2)
- Frontend: 320 vitest, build green, hex-token gate 0

## Architecture decisions log (D-series)
- D1: NewBody BodyIds worker-minted `body_<opId>`, Rust adopts+fences (SCHEMA §2/§7.2)
- D2: STEP export in worker (done, W-WP6)
- D3: `primary.topoKey` interim field removed (ladder replaces)
- D4: ExecutePlan fences workerEpoch+expectedBaseHash only; documentRevision is
  Rust-owned, adopted at AcceptPrepared
- D5: from-0 plans (empty anchor) always base-valid; accept replaces head wholesale
- Hash authority: Rust sole (prefixHashes opaque tokens, rename-safe planner hash)
- ElementId never embeds BodyId; Rust mints, worker returns evidence
- Ladder policy: auto-bind ≥0.85 AND margin ≥0.10; symmetric tie ⇒ NeedsRepair

## Key flags / known gaps (carried in TODO.md per-WP entries)
- Extrude profile binding: worker `last_sketch_id` + first-region fallback —
  multi-region/multi-sketch selection = M4 gap
- Sketch re-entry returns [] constraints; solvedPositions reverse map missing (M4)
- L2 exact preview stays local (no backend preview verb yet)
- SaveCheckpoint/RestoreCheckpoint unwired (V1 replays from 0; checkpoints = M5)
- resolve_refs DTO lossy; worker autoBind returns topoKey vs SCHEMA elementId
  (repair-UI WP fixes both)
- STEP writer stderr chatter — confirm hygiene under M3 packaging
- Solver drag holds writer lock (~ms); lock-free only if measured tails fail

## Next steps
1. M3 packaging gate: scripts/bundle-dylibs.sh (otool closure → @rpath),
   externalBin worker path, signed app on clean Mac, worker --selftest
2. /codex-implementation-review at M2 (user-approved; also at M4)
3. M4 topology slice, M5 lifecycle, M6 hardening per plan

## Conventions
- Orchestrator delegates WPs to subagents; RISKY WPs get independent review agents
- Commits at gate boundaries only; protocol/SCHEMA changes need sign-off + changelog §14
- Worker binary for tests: `ONECAD_WORKER_PATH=worker/build/onecad-worker`
