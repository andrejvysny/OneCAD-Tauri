# OneCAD-Tauri Migration TODO

Plan: `~/.claude/plans/act-as-senior-software-transient-popcorn.md` (approved 2026-07-16).
Tracks: W = C++ worker, R = Rust core, F = frontend. Gates in **bold**.

## M0 — Foundations
- [x] Repo bootstrap: git init, rm empty OneCAD/, tauri.conf (bun, window 1300×864 Overlay)
- [x] protocol/ contract docs: SCHEMA.md (1157) + mesh_format.md (315) + fixtures — GATE PASSED. Notable resolutions: Rust mints ElementIds (worker returns evidence only); magic BYTES normative (LE u32 read-back 0x3157434F); u16 header fields for 64B MESH1; PROTOCOL_ERROR vs UNSUPPORTED split; opType values keep PascalCase.
- [x] R-WP0 cargo workspace scaffold (3 crates, capabilities core:default, CI yml) — GATE PASSED; opener dep removed from package.json too
- [x] W-WP0 worker skeleton: CMake + TKDraw filter verbatim + OCW1 framing + dispatcher + Hello/selftest + harness, ctest 3/3, OCCT 7.9.3 — GATE PASSED (verified independently)
- [x] F-WP0 toolchain: bun deps (three 0.185.1/zustand 5.0.14/tailwind 4.3.3/vitest 4), tokens.css verbatim, alias, smoke test — GATE PASSED
- [x] Characterization corpus: 9 cases w/ provenance, 5 stdout recordings @ b4ddcccc, v1 samples, descriptor constants — GATE PASSED (STEP N/A: UI-only exporter; Symmetric/fillet volumes not-asserted in old tree)
- [x] R-WP6 protocol crate + real stub: OCW1 codec (pure/blocking/async layers), messages (`t` tag), MESH1 validator, ProtocolClient, chaos stub, 50+7 tests — GATE PASSED (verification re-run pending after R-WP1/2 lands). Remaining co-sign half: C++ side speaks to Rust client (W-WP1 build integration + cross fixtures).

## M1 — Cores (parallel per track)
- [x] R-WP1+2 ids/math + OperationRecord schema — review verdict APPROVE-WITH-FIXES
- [x] R-WP2.1 schema fixes: M1–M5 + minors all applied, SCHEMA §7.3 typed targetFace + §7.4 normative RegionId + §14 changelog, existing 14 snapshots UNCHANGED, 134 Rust tests green (verified) — GATE CLOSED. Note: Fillet/Chamfer carry dual edge_ids (bare, wire-aligned) + edges (typed refs) — commands must populate both (R-WP5/R-WP7 rule).
- [x] R-WP4 timeline+graph: cursor=appliedOpCount port, Kahn deterministic (24 perms×64), corpus case (h) reproduced, proptest ×300 — GATE PASSED
- [x] R-WP3 sketch domain: 24 snapshots, 18 constraints C++-verified (line-form H/V, Fixed non-dim, radians), RegionId=FNV-1a-64 lock-tested — GATE PASSED (RegionId must go normative into SCHEMA §7.4 → R-WP2.1)
- [x] R-WP5 Document+EditCommand/session: 21 variants w/ exact inverses, txn batching, selection port, review APPROVE-WITH-FIXES → R-WP5.1 all 12 fixes applied (txn auto-cancel, lockstep both paths, NeedsRepair seeding, producer dirty, re-derive, cycled guard), 169/169 — GATES CLOSED
- [x] R-WP7 regen executor+engine trait+FakeEngine: golden fixtures a–j, 200/200 tests, SHA-256 historyPrefixHash (normative — W-WP4 notified to match), fencing via RevisionGate — provisional PASS, independent review in flight
- [x] R-WP8 scheduler: driver-seam (policy only, no session ownership), preview>regen priority, latest-wins, 120ms debounce, cancel-timeout guard, 10/10 virtual-time tests — GATE PASSED
- [x] R-WP9 file IO: atomic v2 container, attack-surface caps (fuzz: no panics), migration registry + read-only policy, autosave/marker layout, 262/262 — GATE PASSED (verified). Decisions accepted: ops.jsonl derived (document.json authoritative), sketches inline (no sketches/ dir — plan divergence, sound).
- [ ] R-WP10 app shell DTOs (Wave A, in flight)
- [ ] **R-WP11 WorkerManager+chaos (RISKY)** (Wave B, after R-WP10)
- [x] **W-WP5-R independent review**: APPROVE-WITH-FIXES, D1 UPHELD (body_<opId> deterministic+collision-safe). Verified: atomicity/fencing, opaque tokens (no op hashing), descriptor reuse by construction (no fork), MESH1 424B byte-identical probe, determinism 2× fresh runs, stderr-only. Findings: (1) MINOR Standard_Failure not caught at Dispatcher boundary → W-WP5-F fix in flight; (2) NOTE split binds Modified().First() unscored → W-WP6 MUST close; (3) NOTE fast-mode parallel TopoKey ordering unverified → W-WP6 must diff determinism-vs-fast TopoKey tables on corpus; (4) NOTE volumes 4064/3936 bounded not pinned → W-WP5-F; (5) NOTE scratch planStep frames stamped base snapshotId not preparedSnapshotId — optional SCHEMA §3.4 tightening, deferred.
- [x] W-WP5-F fixes: Standard_Failure catch at Dispatcher boundary (GetMessageString fallback DynamicType name) + injected-throw recoverability test + Fuse/Cut volumes pinned 4064/3936 — 46/46 verified by orchestrator. **W-WP5 gate CLOSED.**
- [ ] **W-WP6 ladder+scoring+Fillet/Chamfer/Revolve+ToNext/ToFace+ExportStep (RISKY)** (Wave B, gated on W-WP5-R clean)
- [ ] F-WP8 real-backend swap (Wave C, needs R-WP10+11)
- [x] W-WP2 kernel port + PlaneGCS vendor: 12 kernel + 8 loop + 2 modeling files, ctest 8/8, elementmap byte-parity PROVEN w/ negative control — GATE PASSED (loop/modeling copied-not-compiled pending sketch stack; proto_loop_detector/face_builder deferred to W-WP3)
- [x] W-WP3a sketch stack port: 28 files/8.7k LOC Qt-stripped byte-faithful, loop+modeling compiled (BooleanMode.h adaptation), ctest 17/17, terminator-parity gates w/ negative control — GATE PASSED (verified: zero Qt tokens outside comments). Flag: ConstraintApplicability+SelectionTypes ported into worker for test parity — UI/selection layer, dedup vs Rust selection.rs later.
- [x] W-WP3b solver lane + verbs: 2-lane dispatcher, latest-wins w/ CANCELLED/superseded terminals, WireSketch translator, RegionId byte-match vs Rust (r_fbf1e34acfb51ba4), **BENCHMARK GATE PASS** (solver p95 2.50ms / rtt p95 2.66ms @200ents; busy-kernel invariant), 26 ctests — GATE PASSED. V1 limits doc'd (holes not subtracted in preview fill, arc handles, redundant-status quirk).
- [x] W-WP3c envelope alignment: SCHEMA §3 exact (t/args/result/error, u64 id, stamps, unsolicited hello), canonical Rust-authored fixtures PASS vs C++ worker, 29/29 ctests, no latency regression — GATE PASSED (verified)
- [x] W-WP4 transactional shell: Session/ScratchJob/PlanExecutor, fence-clone-execute-lockfree-swap atomicity, stub ops + test hooks, concurrent-lanes proof (solver 68ms during 500ms plan), determinism across fresh processes, 41/41 — GATE PASSED. Decisions: same-jobId re-send idempotent / different-jobId rejected (SCHEMA changelog pending); lastValidStep null=base-only.
- [x] X-WP1 + R-WP7.1: hash authority = Rust (prefixHashes[] opaque tokens), planner hash = geometry-relevant wire-op form, ALL R-WP7 review MAJORs fixed (bodyId partitions, checkpoint reseed+dedup, replay-from-0 retry, fold gating, cancel recheck, epoch gate), SCHEMA §7.2 amended, 209/209 — GATES CLOSED (R-WP7 now fully closed)
- [x] W-WP5 real OCCT Extrude+Boolean + ElementMap V2 partitions + history mapping + MESH1 tessellation + opaque-token switch: corpus volumes EXACT, 46/46 ctests (verified), determinism across fresh processes — provisional PASS; independent review HELD at user pause point. Cross-track flags: NewBody id worker-minted "body_<opId>" (Rust must adopt from bodyEvents); primary.topoKey non-SCHEMA field used for deterministic minting (W-WP6 ladder replaces); ToNext/ToFace/draft/Revolve/Fillet deferred W-WP6. solver lane verbs + **latency benchmark gate** → transactional shell → **Extrude+Boolean (RISKY)** → **ElementMap V2+ladder+calibration (RISKY)** → Fillet/Chamfer/Revolve → Tessellation+MESH1
- [x] F-WP1 primitives+icons: 9 primitives + 32 icons verbatim + DevGallery, 52/52 tests, hex gate clean — GATE PASSED (Popover radius corrected to 8px prototype value)
- [x] F-WP2 start screen: 1a faithful, 57/57 tests, 3 new tokens — GATE PASSED (Button lg=36px added for action row per prototype)
- [x] F-WP3 editor shell 1c (**flagship pixel gate**): 5 stores (document/selection/tool/viewport/settings-persisted) + keymap/useShortcuts (mode-scoped, F=fillet vs ⇧F=zoom-fit) + full floating chrome (titlebar, toolbar, tree, inspector 3-state, sketch chrome, snap popover, corner cluster, nav pill, status bar) over hatched canvas placeholder. 25 new tests (82 total), build+hex-grep clean, Playwright model/sketch/snap screenshots verified faithful to 1c. Deviations: 4 new tokens (tree-label #33383f, titlebar-text #3a3f46, warn-strong #8a5b10, shadow-sketch-pill); seed tree mirrors prototype (1 body/3 sketches) not "2/2" per pixel gate; grid default off + traffic-lights = OS overlay reservation; sketch-inspector uses 1e warn card.
- [x] F-WP4 viewport core: engine class (owns canvas — StrictMode context-loss fix), CadOrbitControls, CameraRig persp⇄ortho, adaptive grid, HtmlOverlayDriver, CSS-3D ViewCube, render-on-demand (idle=0 verified), Z-up invariant doc'd, 118/118 tests + browser verification — GATE PASSED
- [x] F-WP5 IPC+mesh+picking: MESH1 parser byte-identical to worked example, zero-copy views + lazy ID decode, registry double-buffer + leak tripwire, rAF picking + drawRange highlights, orbit hit-test gating, 169/169 — GATE PASSED (verified; .playwright-cli artifact removed)
- [x] F-WP6 sketch mode: tools (line chain/rect/circle/arc center-start-end), snapEngine + Alt suppress, AutoConstrainer port (±5°), badges + DimensionInput, Line2 px-width, plane-ortho camera flow, 233/233 + browser verified — GATE PASSED (verified)
- [x] F-WP7 tools+preview: extrude drag (auto-arm from finish-sketch, Alt symmetric, flip-through-zero), fillet radius chip, boolean chip, undo/redo, live HistoryList + dbl-click edit seed, **60fps GATE PASS** (p95 ~10ms @300ms L2 lag, epoch race-free), 278/278 — GATE PASSED (verified). F vertical slice COMPLETE; F-WP8 real-backend swap blocked on R-WP10/11.
- [ ] R-WP5.1 session review fixes (in flight: txn auto-cancel MAJOR + lockstep + repair seeding + hardening)

## M2 — **First micro-slice integration gate**
- [ ] sketch → extrude → tessellate → pick → promote ElementId → save/reopen replay → STEP export, real worker

## M3 — **Packaging gate (early)**
- [ ] bundle-dylibs.sh, signed app on clean Mac w/o Homebrew, worker --selftest

## M4 — Topology slice
- [ ] param edit → history rebind → fillet survives or deterministic NeedsRepair; repair UI

## M5 — Lifecycle + recovery
- [ ] revolve, boolean/split BodyId rules, checkpoints+envelopes, crash drills, 3-signature drift, onecad-regen CLI + CI gate, autosave/recovery, STL/OBJ

## M6 — Hardening + backlog
- [ ] Playwright e2e, perf baselines (1–5M tri bridge, solver tails), attack-surface tests, WebGPU spike, tauri-specta
- [ ] Backlog: Shell/Loft/Sweep/Patterns/Mirror, checkpoint heuristics/scrubbing, expressions, v1 importer, Channel tessellation

## Wave plan (user-approved 2026-07-17, pause resolved)
- Cadence: run Wave A→C uninterrupted to M2 gate; consolidated user review at M2.
- Wave A (parallel): W-WP5-R independent review + R-WP10 app shell.
- Wave B: W-WP6 (after W-WP5-R clean) ∥ R-WP11 (after R-WP10).
- Wave C: F-WP8 → M2 gate → /codex-implementation-review (approved for M2 AND M4) → M3 packaging.
- D1 APPROVED: worker-minted deterministic BodyIds `body_<opId>` (splits later `body_<opId>:<k>`); Rust adopts from bodyEvents + validates; SCHEMA amendment in Wave A.
- D2: ExportStep lands in W-WP6 (M2 needs it). D3: primary.topoKey removed in W-WP6.

## Execution rules
- Orchestrator: decisions/review only. WPs → Opus 4.8 subagents.
- RISKY WP = extra independent review pass.
- protocol/ or Descriptor.* or serde schema change = cross-track sign-off + fixture bump.
- Git: commit at gate boundaries (user-approved 2026-07-17). Initial commit e14774d.
