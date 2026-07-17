# OneCAD Characterization Corpus

Captured behavior of the **existing C++ application `OneCAD-CPP`**, frozen as fixture
data so the new Tauri / Rust-core / C++-worker stack can replay identical scenarios in
its golden tests (see migration plan §"Sequencing" M0.2 and §"Verification").

- **Source repo**: `/Users/andrejvysny/workspace/CAD/OneCAD-CPP`
- **Frozen at commit**: `b4ddcccc48134531f3ff80f11ddf9f42ad5a967e`
- **Captured (UTC)**: 2026-07-16
- **OCCT**: 7.9.3, Qt6, C++20 (per OneCAD-CPP `CLAUDE.md`)

`OneCAD-CPP` is treated as **read-only**. Nothing here was produced by editing it; the
`expected-values/` recordings come from building and running its existing prototype
test binaries unchanged.

---

## 1. Purpose

The old stack is the correctness oracle for everything except its **one unsolved
defect**: topological naming dies on parametric edits (H5-B, parked — see
`OneCAD-CPP/TODO.md` lines 150-154 and `HANDOFF.md`). The corpus therefore records two
kinds of truth:

1. **Behavior to preserve** — extrude/boolean/revolve/pattern volumes, region counts,
   solver DOF and drag outcomes, rollback/timeline cursor semantics, file/format
   versions. These are the invariants the new stack MUST reproduce.
2. **Behavior to fix** — the naming-break and symmetric-ambiguity scenarios, where the
   *old* outcome is documented as the anti-goal and the *new* required outcome
   (history rebind, else `NeedsRepair`, never a silent wrong bind) is specified from
   the protocol contract (`protocol/SCHEMA.md` §9, §10, §11).

## 2. Derivation method (provenance discipline)

Every numeric expectation carries a citation to either:

- a **binary recording** in `expected-values/` (stdout of a prototype run at the frozen
  commit), or
- a **test-source line** in `OneCAD-CPP/tests/prototypes/*.cpp` where the value is
  `assert`ed, or a **kernel-source line** where the semantics are defined.

Where the C++ tests assert only booleans (e.g. "volume grew", "over-constrained",
"fallback refused"), the case records the **scenario and the meaning of the
assertion** rather than inventing a precise number. Cases explicitly flag any field
that the old stack does **not** currently assert (e.g. a `Symmetric` extrude volume).

Citations use the form `OneCAD-CPP/<path>:<line>` and all refer to the frozen commit.

## 3. Layout

```
corpus/
├── README.md                 ← this file
├── cases/                    ← one JSON per scenario (a–i), see §4
├── expected-values/          ← raw stdout of RUN prototype binaries (headed by
│                                binary name, date, git rev)
│     ├── proto_regeneration.txt
│     ├── proto_loop_detector.txt
│     └── proto_sketch_solver.txt
├── v1-format-samples/        ← legacy .onecad history fixtures + format-version note
└── step/                     ← NOTE.md (STEP export is UI-only in the old stack; N/A)
```

## 4. Cases

Each `cases/*.json` is a self-contained scenario:

```jsonc
{
  "id":      "…",
  "title":   "…",
  "source":  [ "OneCAD-CPP/…:line — what it proves" ],
  "opScript":[ /* ordered ops; field naming aligned to protocol/SCHEMA.md §7.3 */ ],
  "expected":{ /* per-step body events, face/edge/vertex counts, volumes/areas,
                  solver DOF/status — each numeric annotated with provenance */ },
  "notes":   "…"
}
```

| id | title | primary source |
|----|-------|----------------|
| `a_sketch_extrude_blind` | sketch → extrude Blind happy path | `proto_regeneration.cpp` Test 1 |
| `b_extrude_throughall_symmetric_twodir` | ThroughAll + Symmetric + two-direction | `proto_regeneration.cpp` two-dir / through-all tests |
| `c_boolean_cut_fuse_bodyid` | boolean Cut & Fuse, target-BodyId preserved | `proto_regeneration.cpp` Tests 9/10, linear-pattern fuse |
| `d_fillet_selected_edge` | fillet on selected edge(s) | `proto_regeneration.cpp` Tests 2/3, `OperationRecord.h` |
| `e_naming_break_fillet_upstream_edit` | fillet → upstream dim edit → regen (THE H5-B break) | `TODO.md` H5-B, `proto_regeneration.cpp` re-profile test |
| `f_symmetric_ambiguity` | mirror-symmetric descriptor tie ⇒ NeedsRepair | `proto_elementmap_rigorous.cpp` symmetric-twins, `ElementMap.h` |
| `g_sketch_solver_drag_constraints` | solver drags + DOF from constraints | `proto_sketch_solver.cpp` |
| `h_rollback_dirty_timeline` | rollback cursor + dirty replay | `proto_timeline_rollback_dirty.cpp` |
| `i_multiregion_loop_detection` | closed-loop region detection counts | `proto_loop_detector.cpp` |

## 5. How the new-stack golden tests consume the corpus

- **opScript → ExecutePlan.** A case's `opScript` maps 1:1 onto `protocol/SCHEMA.md`
  §7.2 `ExecutePlan.ops` (op payload schemas §7.3). A golden test compiles the plan,
  runs it against the real worker (or `onecad-worker-stub`), and asserts the
  `PlanPrepared` / per-step `planStep` events against `expected`.
- **Counts & volumes → geometry signature + assertions.** Face/edge/vertex counts and
  volumes are checked directly and also feed the `geometry` signature (SCHEMA §12).
- **Solver cases → solver lane.** DOF/status/positions map onto `SketchUpsert` /
  `SolveDrag` / `EndGesture` results (SCHEMA §7.4).
- **Naming-break & symmetric cases → ladder gate.** These are the calibration
  fixtures for the resolution ladder (SCHEMA §10): the required outcome is auto-bind
  via OCCT history **or** `NeedsRepair` state (§9) — **never** a silent wrong bind
  (Invariant 2). The old stack's orphaning/refusal behavior is recorded as the
  anti-goal, not as an expectation to reproduce.
- **Timeline/rollback → cursor semantics.** `appliedOpCount` maps to the Rust
  timeline cursor (plan: "rollback = cursor, not C++ suppression conflation").

## 6. Field-naming alignment notes (SCHEMA §7.3)

The old C++ types (`OperationRecord.h`) already use the names the protocol adopted:

- `opType` PascalCase domain names: `Sketch|Extrude|Revolve|Fillet|Chamfer|Boolean`
  (via `operationTypeName()`), `extrudeMode` ∈ `Blind|ThroughAll|Symmetric|ToNext|ToFace`,
  `booleanMode` ∈ `NewBody|Add|Cut|Intersect`, standalone Boolean `operation` ∈
  `Union|Cut|Intersect`.
- Extrude params `distance, draftAngleDeg, extrudeMode, booleanMode, targetBodyId,
  targetFaceId, twoDirections, extrudeMode2, distance2, targetFaceId2` — verbatim from
  `OperationRecord.h:91-104`.
- Fillet/Chamfer share `FilletChamferParams {mode, radius, edgeIds, chainTangentEdges}`
  (`OperationRecord.h:114-120`); the protocol splits them into two `opType`s but keeps
  the fields.
- **Non-standard sketch XY basis** (hard invariant): `xAxis=(0,1,0)`, `yAxis=(-1,0,0)`,
  `normal=(0,0,1)` — `SketchPlane::XY()` in `OneCAD-CPP/src/core/sketch/Sketch.h`,
  cross-checked by `proto_face_builder.cpp:93-131` (bbox of a sketch-XY face lands at
  x∈[-5,0], y∈[0,10]). SCHEMA §7.3 locks the same basis.

**Divergences the new stack intentionally introduces** (do not treat old values as the
target for these):

- **ElementId scheme.** Old ids are path-style `"bodyId/kind-reason-opId-hash-ordinal"`
  and embed the BodyId (`ElementMap.h` `makeChildId`, e.g.
  `face-top/face-split-op-split-<hex>-0`). The new scheme mints **globally-unique
  opaque** ids that do NOT embed BodyId; partition membership is a mapping (plan +
  SCHEMA §2, §7.5). Cases carry old ids only as illustrative TopoKeys.
- **Descriptor scoring.** Old `ElementMap::score()` is an unbounded, scale-dependent
  cost (`ElementMap.h:638-671`). The new resolver replaces it with a normalized [0,1]
  confidence + margin policy (SCHEMA §10). The 14-field descriptor itself
  (`ElementMap.h:58-70`, quantization `1e-6`, FNV-1a offset `14695981039346656037`,
  prime `1099511628211`) is ported verbatim.

## 7. Binaries run & recorded

Run unchanged from `OneCAD-CPP/build/tests/`; stdout+stderr captured into
`expected-values/*.txt`, each headed by binary name, UTC date, and git rev:

| binary | result | recording |
|--------|--------|-----------|
| `proto_regeneration` | `=== All tests passed! ===`, exit 0 | `expected-values/proto_regeneration.txt` |
| `proto_loop_detector` | `Loop detector prototype: OK`, exit 0 | `expected-values/proto_loop_detector.txt` |
| `proto_sketch_solver` | `Sketch solver adapter prototype: OK`, exit 0 | `expected-values/proto_sketch_solver.txt` |
| `proto_timeline_rollback_dirty` | `Timeline rollback/dirty prototype passed` (`succeeded=3 failed=0`), exit 0 | `expected-values/proto_timeline_rollback_dirty.txt` |
| `proto_elementmap_rigorous` | `All ElementMap tests passed.`, exit 0 | `expected-values/proto_elementmap_rigorous.txt` |

The prototypes emit heavy `qDebug` logging to stdout; numeric DOF/positions/counts are
`assert`ed in the C++ sources (not printed), so those cases cite source lines while
the `OK`/`passed` terminator + `exit_code: 0` in the recording proves the asserts held.

### Build-cache caveat (important, verified)

The pre-existing `OneCAD-CPP/build/` has a **stale `CMakeCache.txt`** whose recorded
source dir is an old relocated path (`/Users/andrejvysny/workspace/OneCAD`, which no
longer exists — the repo now lives at `.../CAD/OneCAD-CPP`). Because of this,
`cmake --build build` fails at the `cmake_check_build_system` step (CMake refuses the
path mismatch) and recompiles **nothing**. Rebuilding cleanly would need a reconfigure
(`make init` / fresh `cmake`) — deliberately **not** done, to avoid mutating the
read-only repo's build config.

This does **not** compromise provenance: the target binaries were already built
(Jul 16 2026 15:34–15:35) and are **newer than every one of their source files**
(sources dated Mar–Jul 13 2026), with **`git status` clean at HEAD** — so the binaries
reflect the current committed source. Verified by comparing `stat` mtimes of each
`build/tests/<binary>` against `tests/prototypes/<binary>.cpp`. The recordings are
therefore HEAD-current behavior.
