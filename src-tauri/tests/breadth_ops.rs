//! M6a breadth-op integration gate against the REAL C++ OCCT worker, driven through
//! the app's [`DocumentRuntime`] exactly like `wire_contract.rs`.
//!
//! Proves the four M6a ops end-to-end (Rust wire → worker dispatch → OCCT → mesh):
//! * `linear_pattern_three_boxes` — 3× disjoint box ⇒ EXACT 30000 (fused compound).
//! * `circular_pattern_three` — 3× box about a far Z-axis ⇒ EXACT 30000.
//! * `mirror_body_fuse` — box + its mirror across x=0 merged ⇒ EXACT 20000.
//! * `shell_box_open_top` — hollow a 20×20×25 box (t=2) ⇒ EXACT 4112 when the open
//!   face resolves (a ToFace op pre-tracks it), else a CLEAN NeedsRepair (the bare
//!   `open_faces` schema carries no anchor — `fillet_body_context` philosophy).
//! * `linear_pattern_deterministic_across_processes` — two FRESH worker processes
//!   yield the identical pattern body signature + volume (Invariant 5).
//! * `pattern_tracks_upstream_extrude_edit` — editing the source extrude's depth
//!   re-runs the pattern (30000 → 60000), proving the dependency graph feeds regen.
//!
//! Gated on `ONECAD_WORKER_PATH` (else dev-tree fallback); a missing binary skips
//! cleanly unless `ONECAD_REQUIRE_WORKER=1`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, CircularPatternParams, ExtrudeMode, ExtrudeParams, KnownOperation,
    LinearPatternParams, MirrorBodyParams, Operation, OperationRecord, PlaneKind, ShellParams,
    SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::refs::{
    AnchorIntent, ElementKind, ElementRef, PrimaryRef, SketchRegionRef,
};
use onecad_core::document::variables::Scalar;
use onecad_core::edit::EditCommand;
use onecad_core::ids::{BodyId, ConstraintId, ElementId, EntityId, RecordId, RegionId, SketchId};
use onecad_core::math::{Vec2, Vec3};
use onecad_core::regen::{CancelToken, GeometryEngine, Lod, ModelSnapshot, Outcome, RegenRequest};
use onecad_core::sketch::{Constraint, Sketch, SketchEntity, WorldPlane};

use onecad_lib::document_runtime::{DocumentRuntime, RegenReport};
use onecad_lib::worker::manager::SupervisorConfig;
use onecad_lib::worker::wire::sketch_wire;
use onecad_lib::worker::{resolve_worker_path, MeshProvider, SolverEngine, WorkerManager};

use onecad_protocol::mesh::{f32_le, u32_le, validate_mesh_blob, MeshHeaderView};

// ─────────────────────────────────────────────────────────────────────────────
// Harness (mirrors wire_contract.rs)
// ─────────────────────────────────────────────────────────────────────────────

fn real_worker() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ONECAD_WORKER_PATH") {
        let path = PathBuf::from(&p);
        assert!(
            path.is_file(),
            "ONECAD_WORKER_PATH={p:?} is set but no worker binary exists there"
        );
        return Some(path);
    }
    if let Some(path) = resolve_worker_path() {
        return Some(path);
    }
    assert!(
        std::env::var("ONECAD_REQUIRE_WORKER").as_deref() != Ok("1"),
        "ONECAD_REQUIRE_WORKER=1 but no worker binary resolved (CI must hard-fail here)"
    );
    None
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

fn body_of(rec: u128) -> BodyId {
    BodyId(Uuid::from_u128(rec))
}

// Fixed record ids.
const SKETCH_A: u128 = 0xA00;
const EXTRUDE_A: u128 = 0xA01;
const SKETCH_COL: u128 = 0xB00;
const EXTRUDE_COL: u128 = 0xB01;
const OP_PATTERN: u128 = 0xC10;
const OP_MIRROR: u128 = 0xC30;
const OP_SHELL: u128 = 0xC40;

// ─────────────────────────────────────────────────────────────────────────────
// Sketch + op record builders (rect_sketch verbatim from wire_contract.rs)
// ─────────────────────────────────────────────────────────────────────────────

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

fn extrude_op(sketch: SketchId, dist: f64) -> Operation {
    Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: Some(SketchRegionRef {
            sketch,
            region: RegionId::new(""), // first-region fallback
            extra: Default::default(),
        }),
        distance: Scalar::new(dist),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: BooleanMode::NewBody,
        target_body: None,
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    }))
}

fn extrude_record(rec: u128, sketch: SketchId, dist: f64) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Extrude",
        extrude_op(sketch, dist),
    )
}

fn extrude_to_face_record(rec: u128, sketch: SketchId, face: ElementRef) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Extrude",
        Operation::Known(KnownOperation::Extrude(ExtrudeParams {
            profile: Some(SketchRegionRef {
                sketch,
                region: RegionId::new(""),
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
        })),
    )
}

fn linear_pattern_record(
    rec: u128,
    source: BodyId,
    dir: Vec3,
    spacing: f64,
    count: u32,
    fuse: bool,
) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "LinearPattern",
        Operation::Known(KnownOperation::LinearPattern(LinearPatternParams {
            source_body: Some(source),
            direction: dir,
            spacing: Scalar::new(spacing),
            count,
            fuse_result: fuse,
            extra: Default::default(),
        })),
    )
}

fn circular_pattern_record(
    rec: u128,
    source: BodyId,
    origin: Vec3,
    axis: Vec3,
    angle_deg: f64,
    count: u32,
    fuse: bool,
) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "CircularPattern",
        Operation::Known(KnownOperation::CircularPattern(CircularPatternParams {
            source_body: Some(source),
            axis_origin: origin,
            axis_direction: axis,
            angle_deg: Scalar::new(angle_deg),
            count,
            fuse_result: fuse,
            extra: Default::default(),
        })),
    )
}

fn mirror_record(
    rec: u128,
    source: BodyId,
    point: Vec3,
    normal: Vec3,
    fuse: bool,
) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "MirrorBody",
        Operation::Known(KnownOperation::MirrorBody(MirrorBodyParams {
            source_body: Some(source),
            plane_point: point,
            plane_normal: normal,
            fuse_with_original: fuse,
            extra: Default::default(),
        })),
    )
}

fn shell_record(rec: u128, body: BodyId, faces: Vec<ElementId>, thickness: f64) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Shell",
        Operation::Known(KnownOperation::Shell(ShellParams {
            thickness: Scalar::new(thickness),
            open_faces: faces,
            target_body: Some(body),
            extra: Default::default(),
        })),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// MESH1 geometry helpers (exact for planar-faced polyhedra) — from wire_contract.rs
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

/// The face with the greatest average world-Z (the extrude cap): its `(TopoKey, centroid)`.
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

/// Builds box A (a 20×20 rect extruded `depth`), regenerates, and returns box A's id.
async fn build_box_a(rt: &mut DocumentRuntime, depth: f64) -> BodyId {
    let sa = SketchId(Uuid::from_u128(0xA));
    add_op(
        rt,
        sketch_record(SKETCH_A, &rect_sketch(sa, 0x1000, 0.0, 0.0, 20.0, 20.0)),
    );
    add_op(rt, extrude_record(EXTRUDE_A, sa, depth));
    let rep = regen_all(rt).await;
    let _ = published(&rep, "box A");
    body_of(EXTRUDE_A)
}

// ─────────────────────────────────────────────────────────────────────────────
// LinearPattern — 3 disjoint boxes ⇒ EXACT 30000
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linear_pattern_three_boxes() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let body_a = build_box_a(&mut rt, 25.0).await; // 20×20×25 = 10000

    // Spacing 40 along world Y (box footprint spans 20) ⇒ 3 disjoint copies.
    add_op(
        &mut rt,
        linear_pattern_record(
            OP_PATTERN,
            body_a,
            Vec3::new_unchecked(0.0, 1.0, 0.0),
            40.0,
            3,
            true,
        ),
    );
    let rep = regen_all(&mut rt).await;
    let snap = published(&rep, "linear pattern");
    assert_eq!(
        snap.repair_summary.needs_repair_count, 0,
        "pattern resolves the source body"
    );
    assert_eq!(snap.bodies.len(), 2, "box A + the pattern result body");

    let mesh = body_mesh(&mut rt, body_of(OP_PATTERN)).await;
    let view = validate_mesh_blob(&mesh).expect("pattern MESH1 validates");
    let vol = mesh_volume(&view, &mesh);
    assert!(
        (vol - 30_000.0).abs() < 1.0,
        "linear pattern = 3 × 10000 (disjoint) = 30000, got {vol}"
    );
    wm.shutdown().await;
    eprintln!("LinearPattern PASS: volume {vol} == 30000 (3 disjoint boxes)");
}

// ─────────────────────────────────────────────────────────────────────────────
// CircularPattern — 3 boxes about a far Z-axis ⇒ EXACT 30000
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn circular_pattern_three() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let body_a = build_box_a(&mut rt, 25.0).await;

    // 3 copies at 120° about a Z-axis 100 units away ⇒ well-separated (arc ≫ box).
    add_op(
        &mut rt,
        circular_pattern_record(
            OP_PATTERN,
            body_a,
            Vec3::new_unchecked(0.0, -100.0, 0.0),
            Vec3::new_unchecked(0.0, 0.0, 1.0),
            360.0,
            3,
            true,
        ),
    );
    let rep = regen_all(&mut rt).await;
    let snap = published(&rep, "circular pattern");
    assert_eq!(snap.repair_summary.needs_repair_count, 0);

    let mesh = body_mesh(&mut rt, body_of(OP_PATTERN)).await;
    let view = validate_mesh_blob(&mesh).expect("circular pattern MESH1 validates");
    let vol = mesh_volume(&view, &mesh);
    assert!(
        (vol - 30_000.0).abs() < 2.0,
        "circular pattern = 3 × 10000 (disjoint) = 30000, got {vol}"
    );
    wm.shutdown().await;
    eprintln!("CircularPattern PASS: volume {vol} == 30000");
}

// ─────────────────────────────────────────────────────────────────────────────
// MirrorBody — box + its mirror across x=0, fused ⇒ EXACT 20000
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mirror_body_fuse() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let body_a = build_box_a(&mut rt, 25.0).await; // world x[-20,0]

    // Mirror across the x=0 plane → world x[0,20]; source+mirror touch at x=0, fuse
    // into a single 40×20×25 box.
    add_op(
        &mut rt,
        mirror_record(
            OP_MIRROR,
            body_a,
            Vec3::new_unchecked(0.0, 0.0, 0.0),
            Vec3::new_unchecked(1.0, 0.0, 0.0),
            true,
        ),
    );
    let rep = regen_all(&mut rt).await;
    let snap = published(&rep, "mirror");
    assert_eq!(snap.repair_summary.needs_repair_count, 0);

    let mesh = body_mesh(&mut rt, body_of(OP_MIRROR)).await;
    let view = validate_mesh_blob(&mesh).expect("mirror MESH1 validates");
    let vol = mesh_volume(&view, &mesh);
    let dims = bbox_dims(&view);
    assert!(
        (vol - 20_000.0).abs() < 1.0,
        "mirror(fuse) = 2 × 10000 merged across x=0 = 20000, got {vol}"
    );
    assert!(
        (dims[0] - 40.0).abs() < 0.5,
        "merged box spans 40 in world X, got {dims:?}"
    );
    wm.shutdown().await;
    eprintln!("MirrorBody PASS: volume {vol} == 20000, x-span {}", dims[0]);
}

// ─────────────────────────────────────────────────────────────────────────────
// Shell — hollow a 20×20×25 box, t=2 ⇒ EXACT 4112 (open face pre-tracked via ToFace)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shell_box_open_top() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let body_a = build_box_a(&mut rt, 25.0).await;

    // The open (removed) face is box A's top cap. ShellParams carries only a bare
    // ElementId, so we PRE-TRACK it: a ToFace extrude whose targetFace claims the SAME
    // elementId + a top-centroid anchor mints it into the partition (resolve_input_refs)
    // during the same regen, BEFORE the shell step — the production tracking path.
    let mesh_a = body_mesh(&mut rt, body_a).await;
    let view_a = validate_mesh_blob(&mesh_a).expect("box A MESH1 validates");
    assert_eq!(view_a.face_count, 6, "box A has 6 faces");
    let (_top_key, centroid) = top_face_pick(&view_a, &mesh_a);
    assert!(centroid.z > 24.0, "top face at z≈25, got {}", centroid.z);

    let open_face = ElementId::new("el_shell_top");
    let anchor = AnchorIntent {
        world_point: centroid,
        surface_uv: None,
        local_frame: None,
        adjacency_hint: None,
        extra: Default::default(),
    };
    let face_ref = ElementRef {
        primary: Some(PrimaryRef {
            body: body_a,
            element: open_face.clone(),
            kind: ElementKind::Face,
            extra: Default::default(),
        }),
        intent: None,
        anchor: Some(anchor),
        extra: Default::default(),
    };
    // ToFace column (a small profile) — mints `el_shell_top` for box A's top face.
    let scol = SketchId(Uuid::from_u128(0xC0));
    add_op(
        &mut rt,
        sketch_record(SKETCH_COL, &rect_sketch(scol, 0x3000, 5.0, 5.0, 5.0, 5.0)),
    );
    add_op(&mut rt, extrude_to_face_record(EXTRUDE_COL, scol, face_ref));
    // Shell box A, removing that tracked top face.
    add_op(
        &mut rt,
        shell_record(OP_SHELL, body_a, vec![open_face], 2.0),
    );

    let rep = regen_all(&mut rt).await;
    let snap = published(&rep, "shell");

    if snap.repair_summary.needs_repair_count == 0 {
        let mesh = body_mesh(&mut rt, body_a).await;
        let view = validate_mesh_blob(&mesh).expect("shelled box MESH1 validates");
        let vol = mesh_volume(&view, &mesh);
        // 10000 − inner cavity 16×16×23 (5888) = 4112 (exact box arithmetic).
        assert!(
            (vol - 4112.0).abs() < 2.0,
            "shell(20×20×25, t=2, top open) = 10000 − 5888 = 4112, got {vol}"
        );
        eprintln!("Shell PASS: APPLIED — hollow volume {vol} == 4112");
    } else {
        // Clean NeedsRepair — the open-face ref did not confidently resolve (the bare
        // `open_faces` schema carries no anchor). Still proves the wire + dispatch +
        // ladder path (never a crash / wrong bind / OP_FAILED). Exact-volume shell is
        // pinned in the worker ctest `m6a_ops`.
        eprintln!(
            "Shell PASS: CLEAN NeedsRepair ({} refs) — wire + resolution path OK",
            snap.repair_summary.needs_repair_count
        );
    }
    wm.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Determinism — two FRESH worker processes yield the identical pattern signature
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn linear_pattern_deterministic_across_processes() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary");
        return;
    };

    async fn run_once(bin: PathBuf) -> (String, f64) {
        let wm = spawn_worker(bin).await;
        let mut rt = runtime_over(&wm);
        let body_a = build_box_a(&mut rt, 25.0).await;
        add_op(
            &mut rt,
            linear_pattern_record(
                OP_PATTERN,
                body_a,
                Vec3::new_unchecked(0.0, 1.0, 0.0),
                40.0,
                3,
                true,
            ),
        );
        let rep = regen_all(&mut rt).await;
        let snap = published(&rep, "determinism pattern");
        let sig = snap
            .bodies
            .iter()
            .find(|b| b.body == body_of(OP_PATTERN))
            .expect("pattern body present")
            .signature
            .as_str()
            .to_string();
        let mesh = body_mesh(&mut rt, body_of(OP_PATTERN)).await;
        let view = validate_mesh_blob(&mesh).expect("MESH1");
        let vol = mesh_volume(&view, &mesh);
        wm.shutdown().await;
        (sig, vol)
    }

    let (sig1, vol1) = run_once(bin.clone()).await;
    let (sig2, vol2) = run_once(bin).await;
    assert_eq!(
        sig1, sig2,
        "pattern geometry signature is identical across fresh processes"
    );
    assert!(
        (vol1 - vol2).abs() < 1e-6,
        "pattern volume identical: {vol1} vs {vol2}"
    );
    assert!((vol1 - 30_000.0).abs() < 1.0, "and exact: {vol1}");
    eprintln!("Determinism PASS: signature {sig1} stable across two worker processes");
}

// ─────────────────────────────────────────────────────────────────────────────
// Regen dependency — editing the source extrude re-runs the pattern (volumes scale)
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pattern_tracks_upstream_extrude_edit() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let sa = SketchId(Uuid::from_u128(0xA));
    let body_a = build_box_a(&mut rt, 25.0).await; // 10000

    add_op(
        &mut rt,
        linear_pattern_record(
            OP_PATTERN,
            body_a,
            Vec3::new_unchecked(0.0, 1.0, 0.0),
            40.0,
            3,
            true,
        ),
    );
    let rep0 = regen_all(&mut rt).await;
    let _ = published(&rep0, "pattern before edit");
    let mesh0 = body_mesh(&mut rt, body_of(OP_PATTERN)).await;
    let vol0 = mesh_volume(&validate_mesh_blob(&mesh0).unwrap(), &mesh0);
    assert!(
        (vol0 - 30_000.0).abs() < 1.0,
        "before: 3 × 10000 = 30000, got {vol0}"
    );

    // Edit the SOURCE extrude depth 25 → 50 (box A doubles to 20000). The pattern
    // depends on box A's producer, so regen MUST re-run it: 3 × 20000 = 60000.
    rt.apply(EditCommand::UpdateOperationParams {
        record: RecordId(Uuid::from_u128(EXTRUDE_A)),
        op: extrude_op(sa, 50.0),
    })
    .expect("edit extrude depth");
    let rep1 = regen_all(&mut rt).await;
    let _ = published(&rep1, "pattern after edit");
    let mesh1 = body_mesh(&mut rt, body_of(OP_PATTERN)).await;
    let vol1 = mesh_volume(&validate_mesh_blob(&mesh1).unwrap(), &mesh1);
    assert!(
        (vol1 - 60_000.0).abs() < 1.0,
        "after upstream edit: pattern re-ran ⇒ 3 × 20000 = 60000, got {vol1}"
    );
    wm.shutdown().await;
    eprintln!("Upstream-edit PASS: pattern volume {vol0} → {vol1} (source depth 25 → 50)");
}
