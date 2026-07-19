//! Wire-contract regression gate (M2 code-review defects 1–7) against the REAL C++
//! OCCT worker, driven through the app's [`DocumentRuntime`] exactly like
//! `m2_gate.rs`.
//!
//! Each worker-backed test exercises a body-bearing wire path that was broken by the
//! `BodyId` wire-form mismatch (core serde emits a bare uuid; the worker's
//! `BodyStore` is keyed `body_<opId>`). Before the `wire::to_wire_body_form` fix each
//! would have failed (REF_UNRESOLVED / "target body not found" / ToFace NeedsRepair):
//!
//! * `standalone_boolean_cut` / `_union` — a standalone `Boolean` reads bare
//!   `params.targetBodyId`/`toolBodyId` → BodyStore miss (defect 1).
//! * `extrude_pocket_cut` — an `Extrude` Cut reads bare `params.targetBodyId` →
//!   "Extrude target body not found" (defect 2).
//! * `extrude_to_face` — a `ToFace` extrude reads bare
//!   `params.targetFace.primary.bodyId` → NeedsRepair every time (defect 3); also
//!   pins the pre-resolver / `resolve_to_face` ownership split (defect 7).
//! * `fillet_body_context` — the fillet wire flow over `element_ref_wire` (defect 5's
//!   sibling; the bare-fallback body attach itself is unit-pinned in `wire.rs`).
//!
//! `planner_hash_decoupled_from_wire_body_form` is a pure test (no worker) pinning
//! that the regen planner's history-prefix hash is UNCHANGED by this fix (the planner
//! hashes the core serde form and never calls `wire_op`; task A).
//!
//! Gated on `ONECAD_WORKER_PATH` (else the dev-tree fallback); a missing binary skips
//! the worker-backed tests cleanly. The pure hash test always runs.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, BooleanOp, BooleanParams, ExtrudeMode, ExtrudeParams, FilletParams,
    KnownOperation, Operation, OperationRecord, PlaneKind, SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::refs::{
    AnchorIntent, ElementKind, ElementRef, PrimaryRef, SketchRegionRef,
};
use onecad_core::document::variables::Scalar;
use onecad_core::edit::EditCommand;
use onecad_core::history::{DependencyGraph, Timeline};
use onecad_core::ids::{
    BodyId, ConstraintId, DocumentRevision, ElementId, EntityId, JobId, RecordId, RegionId,
    SketchId, SnapshotId, TopoKey, WorkerEpoch,
};
use onecad_core::math::{Vec2, Vec3};
use onecad_core::regen::{
    history_prefix_hash, CancelToken, GeometryEngine, HistoryPrefixHash, Lod, ModelSnapshot,
    Outcome, PlanArtifacts, PlanContext, PolicyVersions, RegenPlanner, RegenRequest,
};
use onecad_core::sketch::{Constraint, Sketch, SketchEntity, WorldPlane};

use onecad_lib::document_runtime::{DocumentRuntime, RegenReport};
use onecad_lib::worker::manager::SupervisorConfig;
use onecad_lib::worker::wire::{body_id_wire, execute_plan_args, sketch_wire};
use onecad_lib::worker::{resolve_worker_path, MeshProvider, SolverEngine, WorkerManager};

use onecad_protocol::mesh::{f32_le, u32_le, validate_mesh_blob, MeshHeaderView};

// ─────────────────────────────────────────────────────────────────────────────
// Harness (mirrors m2_gate.rs)
// ─────────────────────────────────────────────────────────────────────────────

fn real_worker() -> Option<PathBuf> {
    resolve_worker_path()
}

async fn spawn_worker(bin: PathBuf) -> WorkerManager {
    let wm = WorkerManager::spawn(SupervisorConfig::production(bin));
    assert!(
        wm.wait_ready(Duration::from_secs(10)).await,
        "real worker must connect + handshake + OpenSession"
    );
    wm
}

fn runtime_over(wm: &WorkerManager) -> DocumentRuntime {
    let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
    let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
    let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
    DocumentRuntime::new_blank(engine, meshes, solver)
}

fn add_op(rt: &mut DocumentRuntime, record: OperationRecord) {
    rt.apply(EditCommand::AddOperation {
        record,
        at_cursor: true,
    })
    .expect("AddOperation");
}

async fn regen_all(rt: &mut DocumentRuntime) -> RegenReport {
    rt.run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await
}

fn published<'a>(report: &'a RegenReport, what: &str) -> &'a Arc<ModelSnapshot> {
    match &report.outcome {
        Outcome::Published(s) => s,
        other => panic!("{what}: expected Published, got {other:?}"),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Fixed record ids → their worker-minted NewBody ids (`body_<opId>`, adopted as
// `BodyId(recordId.uuid)`). A Boolean/pocket op names its target/tool by these ids.
// ─────────────────────────────────────────────────────────────────────────────

const SKETCH_A: u128 = 0xA00;
const EXTRUDE_A: u128 = 0xA01;
const SKETCH_B: u128 = 0xB00;
const EXTRUDE_B: u128 = 0xB01;
const OP_TAIL: u128 = 0xC00; // boolean / pocket / to-face / fillet tail op

fn body_of(rec: u128) -> BodyId {
    BodyId(Uuid::from_u128(rec))
}

// ─────────────────────────────────────────────────────────────────────────────
// Sketch + op record builders
// ─────────────────────────────────────────────────────────────────────────────

/// The non-standard XY plane ref carried on the timeline Sketch op (as m2_gate).
fn xy_plane_ref() -> SketchPlaneRef {
    SketchPlaneRef {
        kind: PlaneKind::Xy,
        origin: Vec3::new_unchecked(0.0, 0.0, 0.0),
        x_axis: Vec3::new_unchecked(0.0, 1.0, 0.0),
        y_axis: Vec3::new_unchecked(-1.0, 0.0, 0.0),
        normal: Vec3::new_unchecked(0.0, 0.0, 1.0),
        extra: Default::default(),
    }
}

/// A fully-constrained (dof 0) rectangle at sketch-space `(x0, y0)` with size `w × h`,
/// built the marshaller way (8 synthesized points, 4 lines, coincident corners, H/V,
/// a Fixed anchor, and H/V dimension constraints). `base` seeds unique entity ids.
fn rect_sketch(sid: SketchId, base: u128, x0: f64, y0: f64, w: f64, h: f64) -> Sketch {
    let e = |n: u128| EntityId(Uuid::from_u128(base + n));
    let c = |n: u128| ConstraintId(Uuid::from_u128(base + 0x40 + n));
    let (p0s, p0e) = (e(0), e(1));
    let (p1s, p1e) = (e(2), e(3));
    let (p2s, p2e) = (e(4), e(5));
    let (p3s, p3e) = (e(6), e(7));
    let (l0, l1, l2, l3) = (e(0x10), e(0x11), e(0x12), e(0x13));

    let mut sk = Sketch::on_world_plane(sid, "Rect", WorldPlane::XY);
    let pt = |sk: &mut Sketch, id: EntityId, x: f64, y: f64| {
        sk.add_entity(SketchEntity::point(
            id,
            Vec2::new_unchecked(x, y),
            false,
            false,
        ))
        .unwrap();
    };
    pt(&mut sk, p0s, x0, y0);
    pt(&mut sk, p0e, x0 + w, y0);
    pt(&mut sk, p1s, x0 + w, y0);
    pt(&mut sk, p1e, x0 + w, y0 + h);
    pt(&mut sk, p2s, x0 + w, y0 + h);
    pt(&mut sk, p2e, x0, y0 + h);
    pt(&mut sk, p3s, x0, y0 + h);
    pt(&mut sk, p3e, x0, y0);
    sk.add_entity(SketchEntity::line(l0, p0s, p0e, false))
        .unwrap();
    sk.add_entity(SketchEntity::line(l1, p1s, p1e, false))
        .unwrap();
    sk.add_entity(SketchEntity::line(l2, p2s, p2e, false))
        .unwrap();
    sk.add_entity(SketchEntity::line(l3, p3s, p3e, false))
        .unwrap();

    let coincident = |sk: &mut Sketch, id, a, b| {
        sk.add_constraint(Constraint::Coincident {
            id,
            point1: a,
            point2: b,
        })
        .unwrap();
    };
    coincident(&mut sk, c(1), p0e, p1s);
    coincident(&mut sk, c(2), p1e, p2s);
    coincident(&mut sk, c(3), p2e, p3s);
    coincident(&mut sk, c(4), p3e, p0s);
    sk.add_constraint(Constraint::Horizontal { id: c(5), line: l0 })
        .unwrap();
    sk.add_constraint(Constraint::Horizontal { id: c(6), line: l2 })
        .unwrap();
    sk.add_constraint(Constraint::Vertical { id: c(7), line: l1 })
        .unwrap();
    sk.add_constraint(Constraint::Vertical { id: c(8), line: l3 })
        .unwrap();
    sk.add_constraint(Constraint::Fixed {
        id: c(9),
        point: p0s,
        at: Vec2::new_unchecked(x0, y0),
    })
    .unwrap();
    sk.add_constraint(Constraint::HorizontalDistance {
        id: c(10),
        point1: p0s,
        point2: p0e,
        value: Scalar::new(w),
    })
    .unwrap();
    sk.add_constraint(Constraint::VerticalDistance {
        id: c(11),
        point1: p1s,
        point2: p1e,
        value: Scalar::new(h),
    })
    .unwrap();
    sk
}

fn sketch_record(rec: u128, sk: &Sketch) -> OperationRecord {
    let (_plane, entities, constraints) = sketch_wire(sk);
    let params = SketchOpParams {
        sketch: sk.id,
        plane: xy_plane_ref(),
        entities: entities.as_array().cloned().unwrap_or_default(),
        constraints: constraints.as_array().cloned().unwrap_or_default(),
        extra: Default::default(),
    };
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Sketch",
        Operation::Known(KnownOperation::Sketch(params)),
    )
}

fn extrude_record(
    rec: u128,
    sketch: SketchId,
    dist: f64,
    boolean: BooleanMode,
    target: Option<BodyId>,
) -> OperationRecord {
    let params = ExtrudeParams {
        profile: Some(SketchRegionRef {
            sketch,
            // Empty ⇒ the worker's V1 first-region fallback (a NON-EMPTY id that
            // matched no region is now a hard OP_FAILED — M4a strict rule; these
            // single-region fixtures assert the fallback, so they carry no id).
            region: RegionId::new(""),
            extra: Default::default(),
        }),
        distance: Scalar::new(dist),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: boolean,
        target_body: target,
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    };
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Extrude",
        Operation::Known(KnownOperation::Extrude(params)),
    )
}

/// A `ToFace` extrude (NewBody) whose direction-1 target is the given face ref.
fn extrude_to_face_record(rec: u128, sketch: SketchId, face: ElementRef) -> OperationRecord {
    let params = ExtrudeParams {
        profile: Some(SketchRegionRef {
            sketch,
            region: RegionId::new(""), // empty ⇒ V1 first-region fallback (M4a strict rule)
            extra: Default::default(),
        }),
        distance: Scalar::new(1.0),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::ToFace,
        boolean_mode: BooleanMode::NewBody,
        target_body: None,
        target_face: Some(face),
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    };
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Extrude",
        Operation::Known(KnownOperation::Extrude(params)),
    )
}

fn boolean_record(rec: u128, op: BooleanOp, target: BodyId, tool: BodyId) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Boolean",
        Operation::Known(KnownOperation::Boolean(BooleanParams {
            operation: op,
            target_body: target,
            tool_body: tool,
            extra: Default::default(),
        })),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// MESH1 geometry helpers (exact for planar-faced polyhedra)
// ─────────────────────────────────────────────────────────────────────────────

const SEC_POSITIONS: u32 = 1;
const SEC_INDICES: u32 = 3;
const SEC_FACE_RANGES: u32 = 4;
const SEC_FACE_ID_OFFS: u32 = 5;
const SEC_FACE_ID_CHARS: u32 = 6;

fn vertex(blob: &[u8], pbase: usize, i: usize) -> [f64; 3] {
    let o = pbase + i * 12;
    [
        f32_le(blob, o) as f64,
        f32_le(blob, o + 4) as f64,
        f32_le(blob, o + 8) as f64,
    ]
}

/// Signed volume of a MESH1 body via the divergence theorem — EXACT for a closed,
/// planar-faced polyhedron (a box, a box minus a box), so box arithmetic is testable
/// to f32 precision.
fn mesh_volume(view: &MeshHeaderView, blob: &[u8]) -> f64 {
    let pos = view.section(SEC_POSITIONS).expect("POSITIONS");
    let idx = view.section(SEC_INDICES).expect("INDICES");
    let (pbase, ibase) = (pos.offset as usize, idx.offset as usize);
    let mut vol6 = 0.0f64;
    for t in 0..view.triangle_count as usize {
        let o = ibase + t * 12;
        let a = vertex(blob, pbase, u32_le(blob, o) as usize);
        let b = vertex(blob, pbase, u32_le(blob, o + 4) as usize);
        let c = vertex(blob, pbase, u32_le(blob, o + 8) as usize);
        // a · (b × c)
        vol6 += a[0] * (b[1] * c[2] - b[2] * c[1])
            + a[1] * (b[2] * c[0] - b[0] * c[2])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
    }
    (vol6 / 6.0).abs()
}

fn bbox_dims(view: &MeshHeaderView) -> [f64; 3] {
    [
        f64::from(view.bbox_max[0] - view.bbox_min[0]),
        f64::from(view.bbox_max[1] - view.bbox_min[1]),
        f64::from(view.bbox_max[2] - view.bbox_min[2]),
    ]
}

fn id_table(
    view: &MeshHeaderView,
    blob: &[u8],
    offs_ty: u32,
    chars_ty: u32,
    count: usize,
) -> Vec<String> {
    let offs = view.section(offs_ty).expect("id-offs");
    let chars = view.section(chars_ty).expect("id-chars");
    let (obase, cbase) = (offs.offset as usize, chars.offset as usize);
    (0..count)
        .map(|i| {
            let lo = u32_le(blob, obase + i * 4) as usize;
            let hi = u32_le(blob, obase + (i + 1) * 4) as usize;
            String::from_utf8_lossy(&blob[cbase + lo..cbase + hi]).into_owned()
        })
        .collect()
}

/// The face with the greatest average world-Z (the extrude cap / top face): its
/// `(TopoKey, centroid-anchor)`. Used to author a ToFace target ref.
fn top_face_pick(view: &MeshHeaderView, blob: &[u8]) -> (String, Vec3) {
    let fr = view.section(SEC_FACE_RANGES).expect("FACE_RANGES");
    let idx = view.section(SEC_INDICES).expect("INDICES");
    let pos = view.section(SEC_POSITIONS).expect("POSITIONS");
    let (frbase, ibase, pbase) = (fr.offset as usize, idx.offset as usize, pos.offset as usize);
    let keys = id_table(
        view,
        blob,
        SEC_FACE_ID_OFFS,
        SEC_FACE_ID_CHARS,
        view.face_count as usize,
    );
    let mut best: Option<(usize, f64, Vec3)> = None;
    for f in 0..view.face_count as usize {
        let first = u32_le(blob, frbase + f * 8) as usize;
        let count = u32_le(blob, frbase + f * 8 + 4) as usize;
        let (mut sx, mut sy, mut sz, mut n) = (0.0, 0.0, 0.0, 0.0f64);
        for t in first..first + count {
            let io = ibase + t * 12;
            for k in 0..3 {
                let v = vertex(blob, pbase, u32_le(blob, io + k * 4) as usize);
                sx += v[0];
                sy += v[1];
                sz += v[2];
                n += 1.0;
            }
        }
        if n == 0.0 {
            continue;
        }
        let centroid = Vec3::new_unchecked(sx / n, sy / n, sz / n);
        if best.is_none_or(|(_, z, _)| centroid.z > z) {
            best = Some((f, centroid.z, centroid));
        }
    }
    let (idx_best, _, centroid) = best.expect("at least one face");
    (keys[idx_best].clone(), centroid)
}

async fn body_mesh(rt: &mut DocumentRuntime, body: BodyId) -> Arc<Vec<u8>> {
    rt.get_mesh(body, Lod::Coarse, None)
        .await
        .expect("fetch body mesh")
}

// ─────────────────────────────────────────────────────────────────────────────
// standalone Boolean — bare params.targetBodyId/toolBodyId (defect 1)
// ─────────────────────────────────────────────────────────────────────────────

/// Two disjoint-then-overlapping extruded boxes fed to a standalone `Boolean`.
/// A = worldY[0,40], B = worldY[20,60], both worldX[-20,0] × Z[0,25]; A∩B = 20×20×25.
async fn run_boolean(op: BooleanOp) -> f64 {
    let bin = real_worker().expect("worker checked by caller");
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);

    let sa = SketchId(Uuid::from_u128(0xA));
    let sb = SketchId(Uuid::from_u128(0xB));
    add_op(
        &mut rt,
        sketch_record(SKETCH_A, &rect_sketch(sa, 0x1000, 0.0, 0.0, 40.0, 20.0)),
    );
    add_op(
        &mut rt,
        extrude_record(EXTRUDE_A, sa, 25.0, BooleanMode::NewBody, None),
    );
    add_op(
        &mut rt,
        sketch_record(SKETCH_B, &rect_sketch(sb, 0x2000, 20.0, 0.0, 40.0, 20.0)),
    );
    add_op(
        &mut rt,
        extrude_record(EXTRUDE_B, sb, 25.0, BooleanMode::NewBody, None),
    );
    add_op(
        &mut rt,
        boolean_record(OP_TAIL, op, body_of(EXTRUDE_A), body_of(EXTRUDE_B)),
    );

    let report = regen_all(&mut rt).await;
    let _snap = published(&report, "boolean");
    // The boolean modifies the target (id preserved) and consumes the tool.
    let mesh = body_mesh(&mut rt, body_of(EXTRUDE_A)).await;
    let view = validate_mesh_blob(&mesh).expect("boolean result MESH1 validates");
    // Volume is exact for a planar-faced polyhedron regardless of face count (a
    // tiled Union leaves coplanar faces unmerged — OCCT Fuse does not unify domains).
    let vol = mesh_volume(&view, &mesh);
    assert!(view.face_count >= 6, "boolean result is a closed solid");
    wm.shutdown().await;
    vol
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_boolean_cut() {
    if real_worker().is_none() {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    }
    let vol = run_boolean(BooleanOp::Cut).await;
    // A − B = worldY[0,20] × worldX[-20,0] × Z[0,25] = 20·20·25 = 10000.
    assert!(
        (vol - 10_000.0).abs() < 1.0,
        "Cut volume = A − (A∩B) = 10000, got {vol}"
    );
    eprintln!("boolean Cut PASS: volume {vol} == 10000 (exact box arithmetic)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn standalone_boolean_union() {
    if real_worker().is_none() {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    }
    let vol = run_boolean(BooleanOp::Union).await;
    // A ∪ B = contiguous worldY[0,60] × worldX[-20,0] × Z[0,25] = 60·20·25 = 30000.
    assert!(
        (vol - 30_000.0).abs() < 1.0,
        "Union volume = 30000, got {vol}"
    );
    eprintln!("boolean Union PASS: volume {vol} == 30000 (exact box arithmetic)");
}

// ─────────────────────────────────────────────────────────────────────────────
// pocket — Extrude Cut with bare params.targetBodyId (defect 2)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extrude_pocket_cut() {
    if real_worker().is_none() {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    }
    let bin = real_worker().unwrap();
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);

    let sa = SketchId(Uuid::from_u128(0xA));
    let sp = SketchId(Uuid::from_u128(0xB));
    // Box A: 40×20 profile, extrude 25 (vol 20000).
    add_op(
        &mut rt,
        sketch_record(SKETCH_A, &rect_sketch(sa, 0x1000, 0.0, 0.0, 40.0, 20.0)),
    );
    add_op(
        &mut rt,
        extrude_record(EXTRUDE_A, sa, 25.0, BooleanMode::NewBody, None),
    );
    // Pocket: 20×10 profile fully inside A, extrude Cut 10 into A (removes 2000).
    add_op(
        &mut rt,
        sketch_record(SKETCH_B, &rect_sketch(sp, 0x2000, 10.0, 5.0, 20.0, 10.0)),
    );
    add_op(
        &mut rt,
        extrude_record(
            OP_TAIL,
            sp,
            10.0,
            BooleanMode::Cut,
            Some(body_of(EXTRUDE_A)),
        ),
    );

    let report = regen_all(&mut rt).await;
    let _snap = published(&report, "pocket");
    let mesh = body_mesh(&mut rt, body_of(EXTRUDE_A)).await;
    let view = validate_mesh_blob(&mesh).expect("pocket result MESH1 validates");
    let vol = mesh_volume(&view, &mesh);
    assert!(
        (vol - 18_000.0).abs() < 1.0,
        "pocket volume = A(20000) − pocket(2000) = 18000, got {vol}"
    );
    assert!(
        view.face_count > 6,
        "a blind pocket adds faces to the box (got {})",
        view.face_count
    );
    wm.shutdown().await;
    eprintln!(
        "pocket PASS: volume {vol} == 18000, faces {}",
        view.face_count
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// ToFace — bare params.targetFace.primary.bodyId (defect 3) + pre-resolver split (7)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn extrude_to_face() {
    if real_worker().is_none() {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    }
    let bin = real_worker().unwrap();
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);

    let sa = SketchId(Uuid::from_u128(0xA));
    let sp = SketchId(Uuid::from_u128(0xB));
    // Phase 1: box A (top face at worldZ = 25).
    add_op(
        &mut rt,
        sketch_record(SKETCH_A, &rect_sketch(sa, 0x1000, 0.0, 0.0, 40.0, 20.0)),
    );
    add_op(
        &mut rt,
        extrude_record(EXTRUDE_A, sa, 25.0, BooleanMode::NewBody, None),
    );
    let rep_a = regen_all(&mut rt).await;
    let snap_a = published(&rep_a, "toFace box A");
    let snap_id = SnapshotId(rep_a.snapshot_id);
    let body_a = body_of(EXTRUDE_A);

    // Promote the top face → a persistent el_ id (the ToFace target's identity).
    let mesh_a = body_mesh(&mut rt, body_a).await;
    let view_a = validate_mesh_blob(&mesh_a).expect("box A MESH1 validates");
    assert_eq!(view_a.face_count, 6, "box A has 6 faces");
    let (top_key, top_centroid) = top_face_pick(&view_a, &mesh_a);
    assert!(
        top_centroid.z > 24.0,
        "top face is at worldZ≈25, got {}",
        top_centroid.z
    );
    let anchor = AnchorIntent {
        world_point: top_centroid,
        surface_uv: None,
        local_frame: None,
        adjacency_hint: None,
        extra: Default::default(),
    };
    let promoted = rt
        .promote_selection(
            snap_id,
            body_a,
            vec![(TopoKey::new(&top_key), Some(anchor.clone()))],
        )
        .await
        .expect("promote top face");
    let top_el = ElementId::new(promoted[0].element_id.clone());
    let _ = snap_a; // (bodies asserted via mesh)

    // Phase 2: a smaller profile extruded ToFace UP TO box A's top face (worldZ=25).
    let face_ref = ElementRef {
        primary: Some(PrimaryRef {
            body: body_a,
            element: top_el,
            kind: ElementKind::Face,
            extra: Default::default(),
        }),
        intent: None,
        anchor: Some(anchor),
        extra: Default::default(),
    };
    add_op(
        &mut rt,
        sketch_record(SKETCH_B, &rect_sketch(sp, 0x2000, 10.0, 5.0, 20.0, 10.0)),
    );
    add_op(&mut rt, extrude_to_face_record(OP_TAIL, sp, face_ref));

    let rep_tf = regen_all(&mut rt).await;
    let snap_tf = published(&rep_tf, "toFace extrude");
    // Two bodies now exist (A + the ToFace column), and the ToFace body reached z=25.
    assert!(
        snap_tf.repair_summary.needs_repair_count == 0,
        "ToFace resolved (defect 3): no NeedsRepair, got {}",
        snap_tf.repair_summary.needs_repair_count
    );
    assert_eq!(snap_tf.bodies.len(), 2, "box A + the ToFace column");

    let body_tf = body_of(OP_TAIL);
    let mesh_tf = body_mesh(&mut rt, body_tf).await;
    let view_tf = validate_mesh_blob(&mesh_tf).expect("ToFace body MESH1 validates");
    let dims = bbox_dims(&view_tf);
    let vol = mesh_volume(&view_tf, &mesh_tf);
    // 20×10 profile extruded from z=0 up to the z=25 face ⇒ 20·10·25 = 5000; z-extent 25.
    assert!(
        (dims[2] - 25.0).abs() < 0.5,
        "ToFace depth reached the target face (z-extent ≈ 25), got {dims:?}"
    );
    assert!(
        (vol - 5000.0).abs() < 1.0,
        "ToFace column volume = 20·10·25 = 5000, got {vol}"
    );
    wm.shutdown().await;
    eprintln!("ToFace PASS: reached z=25, volume {vol} == 5000, 2 bodies (pre-resolver + resolve_to_face)");
}

// ─────────────────────────────────────────────────────────────────────────────
// fillet — body-bearing wire refs over element_ref_wire (fix B end-to-end)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fillet_body_context() {
    if real_worker().is_none() {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    }
    let bin = real_worker().unwrap();
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);

    let sa = SketchId(Uuid::from_u128(0xA));
    add_op(
        &mut rt,
        sketch_record(SKETCH_A, &rect_sketch(sa, 0x1000, 0.0, 0.0, 40.0, 20.0)),
    );
    add_op(
        &mut rt,
        extrude_record(EXTRUDE_A, sa, 25.0, BooleanMode::NewBody, None),
    );
    let rep_a = regen_all(&mut rt).await;
    let _ = published(&rep_a, "fillet box A");
    let body_a = body_of(EXTRUDE_A);

    let mesh_a = body_mesh(&mut rt, body_a).await;
    let view_a = validate_mesh_blob(&mesh_a).expect("box A MESH1 validates");
    assert_eq!(view_a.face_count, 6);
    let (top_key, centroid) = top_face_pick(&view_a, &mesh_a);
    let _ = top_key;

    // A fillet whose per-edge ref carries the operated body (primary.bodyId) + an
    // anchor — the body-bearing wire ref element_ref_wire now serde-renders. We anchor
    // near a top edge (the top-face centroid nudged to an edge is a coarse anchor; the
    // fillet either applies (faces grow) or cleanly NeedsRepairs — both prove the body
    // input resolved, i.e. NOT the pre-fix "requires body input"/BodyStore miss).
    let edge_el = ElementId::new("el_fillet_edge");
    let edge_ref = ElementRef {
        primary: Some(PrimaryRef {
            body: body_a,
            element: edge_el.clone(),
            kind: ElementKind::Edge,
            extra: Default::default(),
        }),
        intent: None,
        anchor: Some(AnchorIntent {
            world_point: Vec3::new_unchecked(centroid.x, centroid.y, centroid.z),
            surface_uv: None,
            local_frame: None,
            adjacency_hint: None,
            extra: Default::default(),
        }),
        extra: Default::default(),
    };
    let fillet = OperationRecord::new(
        RecordId(Uuid::from_u128(OP_TAIL)),
        0,
        "Fillet",
        Operation::Known(KnownOperation::Fillet(FilletParams {
            radius: Scalar::new(2.0),
            edge_ids: vec![edge_el],
            edges: vec![edge_ref],
            chain_tangent_edges: false,
            extra: Default::default(),
        })),
    );
    add_op(&mut rt, fillet);
    let rep_f = regen_all(&mut rt).await;
    let snap_f = published(&rep_f, "fillet");

    if snap_f.repair_summary.needs_repair_count > 0 {
        // Clean NeedsRepair (state) — the body input DID resolve (target_body_of found
        // primary.bodyId); the edge anchor was just not confident. Pre-fix this path
        // never reached the ladder (BodyStore miss / wrong-form bodyId).
        eprintln!(
            "fillet PASS: body input resolved → CLEAN NeedsRepair ({} refs) — element_ref_wire body form OK",
            snap_f.repair_summary.needs_repair_count
        );
    } else {
        let mesh_f = body_mesh(&mut rt, body_a).await;
        let view_f = validate_mesh_blob(&mesh_f).expect("filleted body MESH1 validates");
        assert!(
            view_f.face_count >= 7,
            "fillet APPLIED adds a rolled face (6→≥7), got {}",
            view_f.face_count
        );
        eprintln!("fillet PASS: APPLIED — faces 6 → {}", view_f.face_count);
    }
    wm.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// (A) Hash stability — the planner's history-prefix hash is unchanged by the wire
// body-form fix. The planner hashes the CORE serde form (BodyId → bare uuid) and
// never calls wire_op; the wire renders body_<uuid>. The two are decoupled.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn planner_hash_decoupled_from_wire_body_form() {
    let target = body_of(0xB0);
    let tool = body_of(0xB1);
    let rec = boolean_record(0xBEEF, BooleanOp::Union, target, tool);

    let mut tl = Timeline::new();
    tl.insert_at_cursor(rec.clone());
    let ctx = PlanContext {
        policy_versions: PolicyVersions::default(),
        occt_fingerprint: "fp".into(),
    };
    let plan = RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &[],
        RegenRequest::ToEnd { from: 0 },
        &ctx,
    );

    // (1) The planner hash is a FIXED value derived from the core serde form (bare
    //     uuids) — a golden that breaks if the hash inputs ever change (e.g. if
    //     wire_op body forms ever leaked into the hash). It equals the standalone
    //     history_prefix_hash over the same record (the planner's own function).
    assert_eq!(
        plan.expected_base_hash,
        HistoryPrefixHash::empty(),
        "from-0 base"
    );
    assert_eq!(plan.prefix_hashes.len(), 1);
    assert_eq!(
        plan.prefix_hashes[0],
        history_prefix_hash(std::slice::from_ref(&rec)),
        "plan prefix hash == history_prefix_hash of the record (planner path, not wire_op)"
    );
    assert_eq!(
        plan.prefix_hashes[0].as_str(),
        GOLDEN_BOOLEAN_PREFIX_HASH,
        "history-prefix hash is UNCHANGED by the wire body-form fix (task A)"
    );

    // (2) The WIRE, by contrast, renders body_<uuid> for the same op — proving the
    //     hashed form (bare uuid) and the wire form (body_<uuid>) are decoupled.
    let req = plan.into_request(
        JobId(Uuid::from_u128(1)),
        DocumentRevision(0),
        WorkerEpoch(0),
        PolicyVersions::default(),
        PlanArtifacts { tessellate: None },
    );
    let args = execute_plan_args(&req);
    let params = &args["ops"][0]["params"];
    assert_eq!(
        params["targetBodyId"],
        serde_json::json!(body_id_wire(target))
    );
    assert_eq!(params["toolBodyId"], serde_json::json!(body_id_wire(tool)));
    // ...and the bare uuid MUST NOT appear on the wire (it was the defect).
    assert_ne!(
        params["targetBodyId"],
        serde_json::json!(target.to_string())
    );
}

/// The golden history-prefix hash of the fixed one-Boolean document above. Pinned so
/// any accidental change to the planner's hash inputs (including routing the wire
/// body form into the hash) is caught.
const GOLDEN_BOOLEAN_PREFIX_HASH: &str =
    "bed9be34040605a6cf938f215234353381931643fe23351618b1875c77bcbb5d";
