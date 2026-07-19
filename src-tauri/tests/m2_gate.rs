//! M2 — first micro-slice **integration gate** against the REAL C++ OCCT worker.
//!
//! Drives the whole vertical slice through the app's own single-writer
//! [`DocumentRuntime`] (NOT hand-built wire frames): sketch → constrained rectangle
//! → extrude → tessellate → pick TopoKey → promote ElementId → fillet → save/reopen
//! replay across a FRESH worker process → STEP export → undo. Every numbered step is
//! genuinely asserted (plan "M2 — First micro-slice" + "Verification").
//!
//! Gated on the worker binary (`ONECAD_WORKER_PATH`, else the dev-tree fallback via
//! [`resolve_worker_path`]); a missing binary skips cleanly. The gate binary at
//! `worker/build/onecad-worker` exists, so under `cargo test` with
//! `ONECAD_WORKER_PATH` set these tests actually run.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, FilletParams, KnownOperation, Operation,
    OperationRecord, PlaneKind, SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::refs::{
    AnchorIntent, ElementKind, ElementRef, PrimaryRef, SketchRegionRef,
};
use onecad_core::document::variables::Scalar;
use onecad_core::edit::{EditCommand, SketchEditOp};
use onecad_core::ids::{
    BodyId, ConstraintId, ElementId, EntityId, RecordId, RegionId, SketchId, SnapshotId, TopoKey,
};
use onecad_core::io::container::SaveMeta;
use onecad_core::math::{Vec2, Vec3};
use onecad_core::regen::{CancelToken, GeometryEngine, Lod, ModelSnapshot, Outcome, RegenRequest};
use onecad_core::sketch::{Constraint, Sketch, SketchEntity, WorldPlane};

use onecad_lib::document_runtime::{DocumentRuntime, RegenReport};
use onecad_lib::worker::manager::SupervisorConfig;
use onecad_lib::worker::wire::{body_id_wire, sketch_wire};
use onecad_lib::worker::{resolve_worker_path, MeshProvider, SolverEngine, WorkerManager};

use onecad_protocol::mesh::{f32_le, u32_le, validate_mesh_blob, MeshHeaderView};

// ─────────────────────────────────────────────────────────────────────────────
// Harness
// ─────────────────────────────────────────────────────────────────────────────

/// The real worker binary (`ONECAD_WORKER_PATH` → dev fallback). `None` skips.
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

/// A [`DocumentRuntime`] whose engine / mesh / solver lanes all speak to `wm`.
fn runtime_over(wm: &WorkerManager) -> DocumentRuntime {
    let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
    let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
    let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
    DocumentRuntime::new_blank(engine, meshes, solver)
}

fn open_over(wm: &WorkerManager, path: &Path) -> DocumentRuntime {
    let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
    let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
    let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
    DocumentRuntime::open(path, engine, meshes, solver).expect("reopen saved container")
}

fn save_meta() -> SaveMeta {
    SaveMeta {
        app_version: "m2-gate".into(),
        occt_fingerprint: None,
        created: "2026-07-18T00:00:00Z".into(),
        modified: "2026-07-18T00:00:00Z".into(),
    }
}

/// Append an operation record at the timeline cursor (the repeated apply-expect).
fn add_op(rt: &mut DocumentRuntime, record: OperationRecord) {
    rt.apply(EditCommand::AddOperation {
        record,
        at_cursor: true,
    })
    .expect("AddOperation");
}

/// Drive a full replay-from-0 regen against the real worker (the repeated
/// `run_regen(ToEnd { from: 0 })` incantation).
async fn regen_all(rt: &mut DocumentRuntime) -> RegenReport {
    rt.run_regen(RegenRequest::ToEnd { from: 0 }, CancelToken::new())
        .await
}

// ─────────────────────────────────────────────────────────────────────────────
// Sketch construction (the SAME typed SketchEditOps the frontend marshaller emits)
// ─────────────────────────────────────────────────────────────────────────────

const SKETCH_ID: u128 = 0x5c;
const SKETCH_REC: u128 = 0x5c_00;
const EXTRUDE_REC: u128 = 0xe0;
const FILLET_REC: u128 = 0xf0;

const RECT_W: f64 = 40.0;
const RECT_H: f64 = 20.0;
const EXTRUDE_DIST: f64 = 25.0;

/// A fully-constrained rectangle built the way the frontend marshaller
/// (`sketchWireMap.ts`) emits: **four lines**, each with its OWN two synthesized
/// `Point` endpoints, the shared corners tied by **Coincident**, plus H/V on the
/// sides, one **Fixed** anchor, and horizontal/vertical **dimension** constraints —
/// exactly dof 0. Returns the populated [`Sketch`] (the single source of truth for
/// both the solver-lane edit ops and the timeline Sketch op params).
fn rectangle_sketch() -> Sketch {
    let e = |n: u128| EntityId(Uuid::from_u128(n));
    let c = |n: u128| ConstraintId(Uuid::from_u128(n));

    // Eight synthesized points — two per line (marshaller shape).
    let (p0s, p0e) = (e(0x100), e(0x101)); // line 0: (0,0)→(W,0)
    let (p1s, p1e) = (e(0x110), e(0x111)); // line 1: (W,0)→(W,H)
    let (p2s, p2e) = (e(0x120), e(0x121)); // line 2: (W,H)→(0,H)
    let (p3s, p3e) = (e(0x130), e(0x131)); // line 3: (0,H)→(0,0)
    let (l0, l1, l2, l3) = (e(0x200), e(0x201), e(0x202), e(0x203));

    let mut sk =
        Sketch::on_world_plane(SketchId(Uuid::from_u128(SKETCH_ID)), "Rect", WorldPlane::XY);
    let pt = |sk: &mut Sketch, id: EntityId, x: f64, y: f64| {
        sk.add_entity(SketchEntity::point(
            id,
            Vec2::new_unchecked(x, y),
            false,
            false,
        ))
        .unwrap();
    };
    // Points first (validation requires referents to exist before lines/constraints).
    pt(&mut sk, p0s, 0.0, 0.0);
    pt(&mut sk, p0e, RECT_W, 0.0);
    pt(&mut sk, p1s, RECT_W, 0.0);
    pt(&mut sk, p1e, RECT_W, RECT_H);
    pt(&mut sk, p2s, RECT_W, RECT_H);
    pt(&mut sk, p2e, 0.0, RECT_H);
    pt(&mut sk, p3s, 0.0, RECT_H);
    pt(&mut sk, p3e, 0.0, 0.0);
    sk.add_entity(SketchEntity::line(l0, p0s, p0e, false))
        .unwrap();
    sk.add_entity(SketchEntity::line(l1, p1s, p1e, false))
        .unwrap();
    sk.add_entity(SketchEntity::line(l2, p2s, p2e, false))
        .unwrap();
    sk.add_entity(SketchEntity::line(l3, p3s, p3e, false))
        .unwrap();

    // Coincident ties the four shared corners (marshaller synthesizes these).
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
    // H on the two horizontals, V on the two verticals.
    sk.add_constraint(Constraint::Horizontal { id: c(5), line: l0 })
        .unwrap();
    sk.add_constraint(Constraint::Horizontal { id: c(6), line: l2 })
        .unwrap();
    sk.add_constraint(Constraint::Vertical { id: c(7), line: l1 })
        .unwrap();
    sk.add_constraint(Constraint::Vertical { id: c(8), line: l3 })
        .unwrap();
    // Pin one corner + two dimensions → fully constrained (dof 0).
    sk.add_constraint(Constraint::Fixed {
        id: c(9),
        point: p0s,
        at: Vec2::new_unchecked(0.0, 0.0),
    })
    .unwrap();
    sk.add_constraint(Constraint::HorizontalDistance {
        id: c(10),
        point1: p0s,
        point2: p0e,
        value: Scalar::new(RECT_W),
    })
    .unwrap();
    sk.add_constraint(Constraint::VerticalDistance {
        id: c(11),
        point1: p1s,
        point2: p1e,
        value: Scalar::new(RECT_H),
    })
    .unwrap();
    sk
}

/// Derive the ordered [`SketchEditOp`] batch (AddEntity points+lines, then
/// AddConstraint) from a populated sketch — the ops the runtime commits and
/// re-solves on the worker's PlaneGCS lane.
fn edit_ops(sk: &Sketch) -> Vec<SketchEditOp> {
    let mut ops = Vec::new();
    for ent in sk.entities() {
        ops.push(SketchEditOp::AddEntity {
            entity: ent.clone(),
        });
    }
    for con in sk.constraints() {
        ops.push(SketchEditOp::AddConstraint {
            constraint: con.clone(),
        });
    }
    ops
}

/// The non-standard XY plane ref carried on the timeline Sketch op (the worker
/// derives the profile basis from `kind`; the vectors ride along for the record).
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

/// A timeline **Sketch** op record carrying the sketch's §7.3 wire entities /
/// constraints — the profile source the worker's `build_profile_face` consumes
/// during regen (the extrude references it by `sketchId`).
fn sketch_op_record(sk: &Sketch) -> OperationRecord {
    let (_plane, entities, constraints) = sketch_wire(sk);
    let params = SketchOpParams {
        sketch: sk.id,
        plane: xy_plane_ref(),
        entities: entities.as_array().cloned().unwrap_or_default(),
        constraints: constraints.as_array().cloned().unwrap_or_default(),
        extra: Default::default(),
    };
    OperationRecord::new(
        RecordId(Uuid::from_u128(SKETCH_REC)),
        0,
        "Sketch",
        Operation::Known(KnownOperation::Sketch(params)),
    )
}

/// A Blind / NewBody extrude of the sketch region.
fn extrude_op_record(sketch: SketchId, region: RegionId) -> OperationRecord {
    let params = ExtrudeParams {
        profile: Some(SketchRegionRef {
            sketch,
            region,
            extra: Default::default(),
        }),
        distance: Scalar::new(EXTRUDE_DIST),
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
    };
    OperationRecord::new(
        RecordId(Uuid::from_u128(EXTRUDE_REC)),
        1,
        "Extrude",
        Operation::Known(KnownOperation::Extrude(params)),
    )
}

/// A single-edge fillet, the edge resolved by its **anchor** (world midpoint) — the
/// SCHEMA §7.3 `inputs[]` semantic ref shape (`{primary:{bodyId,elementId,kind},
/// anchor:{worldPoint}}`) the worker's ladder binds (matches the worker's own
/// determinism corpus).
fn fillet_op_record(body: BodyId, edge_anchor: Vec3) -> OperationRecord {
    // R-WP2.1 F2 lockstep: `edges[i].primary.element` MUST equal `edge_ids[i]`.
    let edge_el = ElementId::new("el_edge");
    let edge_ref = ElementRef {
        primary: Some(PrimaryRef {
            body,
            element: edge_el.clone(),
            kind: ElementKind::Edge,
            extra: Default::default(),
        }),
        intent: None,
        anchor: Some(AnchorIntent {
            world_point: edge_anchor,
            surface_uv: None,
            local_frame: None,
            adjacency_hint: None,
            extra: Default::default(),
        }),
        extra: Default::default(),
    };
    let params = FilletParams {
        radius: Scalar::new(2.0),
        edge_ids: vec![edge_el],
        edges: vec![edge_ref],
        chain_tangent_edges: false,
        extra: Default::default(),
    };
    OperationRecord::new(
        RecordId(Uuid::from_u128(FILLET_REC)),
        2,
        "Fillet",
        Operation::Known(KnownOperation::Fillet(params)),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// MESH1 section parsing (Rust-side pick support)
// ─────────────────────────────────────────────────────────────────────────────

const SEC_FACE_RANGES: u32 = 4;
const SEC_FACE_ID_OFFS: u32 = 5;
const SEC_FACE_ID_CHARS: u32 = 6;
const SEC_EDGE_RANGES: u32 = 7;
const SEC_EDGE_POSITIONS: u32 = 8;
const SEC_EDGE_ID_OFFS: u32 = 9;
const SEC_EDGE_ID_CHARS: u32 = 10;

/// Read the (`count`+1) prefix-sum offsets + concatenated UTF-8 id chars of an id
/// table (FACE_* / EDGE_*) into the per-element id strings.
fn id_table(
    view: &MeshHeaderView,
    blob: &[u8],
    offs_ty: u32,
    chars_ty: u32,
    count: usize,
) -> Vec<String> {
    let offs = view.section(offs_ty).expect("id-offs section");
    let chars = view.section(chars_ty).expect("id-chars section");
    let obase = offs.offset as usize;
    let cbase = chars.offset as usize;
    (0..count)
        .map(|i| {
            let lo = u32_le(blob, obase + i * 4) as usize;
            let hi = u32_le(blob, obase + (i + 1) * 4) as usize;
            String::from_utf8_lossy(&blob[cbase + lo..cbase + hi]).into_owned()
        })
        .collect()
}

/// The face TopoKeys (`"f:N"`) in face order.
fn face_topokeys(view: &MeshHeaderView, blob: &[u8]) -> Vec<String> {
    id_table(
        view,
        blob,
        SEC_FACE_ID_OFFS,
        SEC_FACE_ID_CHARS,
        view.face_count as usize,
    )
}

/// Assert FACE_RANGES tiles `[0, triangleCount)` contiguously with no gap/overlap.
fn assert_face_ranges_tile(view: &MeshHeaderView, blob: &[u8]) {
    let fr = view.section(SEC_FACE_RANGES).expect("FACE_RANGES");
    let base = fr.offset as usize;
    let mut covered = 0u32;
    for i in 0..view.face_count as usize {
        let first = u32_le(blob, base + i * 8);
        let count = u32_le(blob, base + i * 8 + 4);
        assert_eq!(first, covered, "FACE_RANGES face {i} starts contiguously");
        covered += count;
    }
    assert_eq!(
        covered, view.triangle_count,
        "FACE_RANGES covers exactly all {} triangles",
        view.triangle_count
    );
}

/// Pick the edge with the greatest world-Z extent (a vertical box edge, always
/// safely fillettable) and return `(topoKey, centroid-anchor)`.
fn vertical_edge_pick(view: &MeshHeaderView, blob: &[u8]) -> (String, Vec3) {
    assert!(
        view.has_edges(),
        "MESH1 must carry edges for the fillet pick"
    );
    let er = view.section(SEC_EDGE_RANGES).expect("EDGE_RANGES");
    let ep = view.section(SEC_EDGE_POSITIONS).expect("EDGE_POSITIONS");
    let keys = id_table(
        view,
        blob,
        SEC_EDGE_ID_OFFS,
        SEC_EDGE_ID_CHARS,
        view.edge_count as usize,
    );
    let erbase = er.offset as usize;
    let epbase = ep.offset as usize;

    let mut best: Option<(usize, f64, Vec3)> = None;
    for i in 0..view.edge_count as usize {
        let first = u32_le(blob, erbase + i * 8) as usize;
        let count = u32_le(blob, erbase + i * 8 + 4) as usize;
        if count == 0 {
            continue;
        }
        let (mut zmin, mut zmax) = (f64::INFINITY, f64::NEG_INFINITY);
        let (mut sx, mut sy, mut sz) = (0.0f64, 0.0f64, 0.0f64);
        for p in 0..count {
            let o = epbase + (first + p) * 12;
            let (x, y, z) = (
                f32_le(blob, o) as f64,
                f32_le(blob, o + 4) as f64,
                f32_le(blob, o + 8) as f64,
            );
            zmin = zmin.min(z);
            zmax = zmax.max(z);
            sx += x;
            sy += y;
            sz += z;
        }
        let span = zmax - zmin;
        let centroid = Vec3::new_unchecked(sx / count as f64, sy / count as f64, sz / count as f64);
        if best.is_none_or(|(_, s, _)| span > s) {
            best = Some((i, span, centroid));
        }
    }
    let (idx, span, centroid) = best.expect("at least one edge");
    assert!(
        span > EXTRUDE_DIST * 0.5,
        "picked a vertical edge (z-span {span})"
    );
    (keys[idx].clone(), centroid)
}

/// The world bbox centre of a validated mesh (a rough face anchor).
fn bbox_center(view: &MeshHeaderView) -> Vec3 {
    Vec3::new_unchecked(
        f64::from(view.bbox_min[0] + view.bbox_max[0]) / 2.0,
        f64::from(view.bbox_min[1] + view.bbox_max[1]) / 2.0,
        f64::from(view.bbox_min[2] + view.bbox_max[2]) / 2.0,
    )
}

/// Sorted `{bbox dimension}` set of a validated mesh.
fn bbox_dims(view: &MeshHeaderView) -> [f64; 3] {
    let mut d = [
        f64::from(view.bbox_max[0] - view.bbox_min[0]),
        f64::from(view.bbox_max[1] - view.bbox_min[1]),
        f64::from(view.bbox_max[2] - view.bbox_min[2]),
    ];
    d.sort_by(|a, b| a.partial_cmp(b).unwrap());
    d
}

// ─────────────────────────────────────────────────────────────────────────────
// Determinism helpers
// ─────────────────────────────────────────────────────────────────────────────

fn published<'a>(report: &'a RegenReport, what: &str) -> &'a Arc<ModelSnapshot> {
    match &report.outcome {
        Outcome::Published(s) => s,
        other => panic!("{what}: expected Published, got {other:?}"),
    }
}

/// The `(bodyId, geometrySignature)` set of a snapshot — process-independent
/// (worker-minted `body_<opId>` + quantized signatures), so it is comparable across
/// two fresh worker processes (Invariant: same plan ⇒ identical quantized signatures).
fn body_sig_set(snap: &ModelSnapshot) -> BTreeSet<(String, String)> {
    snap.bodies
        .iter()
        .map(|b| (b.body.to_string(), b.signature.as_str().to_string()))
        .collect()
}

fn body_id_set(snap: &ModelSnapshot) -> BTreeSet<String> {
    snap.bodies.iter().map(|b| b.body.to_string()).collect()
}

/// Extract `document.json` bytes from a saved v2 container (byte-stable-save proof).
fn read_document_json(path: &Path) -> Vec<u8> {
    let file = std::fs::File::open(path).expect("open container");
    let mut zip = zip::ZipArchive::new(file).expect("v2 container is a zip");
    let mut entry = zip.by_name("document.json").expect("document.json entry");
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut bytes).expect("read document.json");
    bytes
}

// ─────────────────────────────────────────────────────────────────────────────
// The gate: steps 1–8 on one document, across two fresh worker processes.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn m2_gate_full_slice() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: real worker binary not found (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin.clone()).await;
    let mut rt = runtime_over(&wm);

    // ── Step 1: sketch on XY → fully-constrained rectangle → dof 0 → region ─────
    let sketch = rectangle_sketch();
    let sid = sketch.id;
    rt.apply(EditCommand::AddSketch {
        sketch: Sketch::on_world_plane(sid, "Rect", WorldPlane::XY),
    })
    .expect("AddSketch");
    let session = rt.enter_sketch(sid).await.expect("enter_sketch");
    assert_eq!(session.sketch_id, sid.to_string());

    let solved = rt
        .sketch_upsert(sid, edit_ops(&sketch))
        .await
        .expect("sketch_upsert (build rectangle)");
    assert_eq!(
        solved.dof, 0,
        "STEP 1: fully-constrained rectangle reaches dof 0 (status {:?})",
        solved.status
    );

    let finished = rt.finish_sketch(sid).await.expect("finish_sketch");
    assert!(
        !finished.regions.is_empty(),
        "STEP 1: closed region detected"
    );
    let region_id = finished.regions[0].region_id.clone();
    assert!(!region_id.is_empty(), "STEP 1: region has a RegionId");
    eprintln!(
        "STEP 1 PASS: dof=0, regions={}, regionId={region_id}",
        finished.regions.len()
    );

    // ── Step 2: extrude the region → regen on the real worker → published body ──
    add_op(&mut rt, sketch_op_record(&sketch));
    add_op(
        &mut rt,
        extrude_op_record(sid, RegionId::new(region_id.clone())),
    );

    let ext_report = regen_all(&mut rt).await;
    let ext_snap = published(&ext_report, "STEP 2 extrude").clone();
    assert_eq!(ext_report.changed.len(), 1, "STEP 2: one extruded body");
    let body = ext_report.changed[0].0;
    let ext_snapshot_id = ext_report.snapshot_id;
    assert!(ext_snapshot_id > 0, "STEP 2: published a real snapshot id");
    let ext_sig = body_sig_set(&ext_snap);

    // ── Step 8 pre-capture / Step 2 volume: mesh bbox sanity (box 40×20×25) ─────
    let mesh = rt
        .get_mesh(body, Lod::Coarse, None)
        .await
        .expect("STEP 2/3: fetch extrude mesh");
    let view = validate_mesh_blob(&mesh).expect("STEP 3: MESH1 validates");
    assert_eq!(view.face_count, 6, "STEP 2: a box has 6 faces");
    let dims = bbox_dims(&view);
    assert!(
        (dims[0] - RECT_H).abs() < 0.5
            && (dims[1] - EXTRUDE_DIST).abs() < 0.5
            && (dims[2] - RECT_W).abs() < 0.5,
        "STEP 2: box bbox dims ≈ {{20,25,40}}, got {dims:?}"
    );
    eprintln!(
        "STEP 2 PASS: body={}, snapshot={ext_snapshot_id}, faces={}, dims={dims:?}",
        body_id_wire(body),
        view.face_count
    );

    // ── F-WP9 gap 8: authoritative projection reflects the published body ───────
    let proj = rt.projection();
    assert!(
        proj.bodies.contains_key(&body.to_string()),
        "gap 8: store-visible projection includes the published body"
    );
    assert!(
        proj.bodies[&body.to_string()].visible,
        "gap 8: published body is visible in the authoritative projection"
    );

    // ── Step 3: tessellate → parse MESH1 → face TopoKey + FACE_RANGES sanity ────
    assert_face_ranges_tile(&view, &mesh);
    let faces = face_topokeys(&view, &mesh);
    assert_eq!(faces.len(), 6, "STEP 3: 6 face TopoKeys");
    let face_key = faces[0].clone();
    assert!(
        face_key.starts_with("f:"),
        "STEP 3: face TopoKey shape 'f:N', got {face_key:?}"
    );
    eprintln!("STEP 3 PASS: face TopoKeys={faces:?}, FACE_RANGES tiles all triangles");

    // ── Step 4: promote a face pick → el_ id; re-pick ⇒ SAME id (Invariant 1) ───
    let snap = SnapshotId(ext_snapshot_id);
    let anchor = AnchorIntent {
        world_point: bbox_center(&view),
        surface_uv: None,
        local_frame: None,
        adjacency_hint: None,
        extra: Default::default(),
    };
    let promoted = rt
        .promote_selection(snap, body, vec![(TopoKey::new(&face_key), Some(anchor))])
        .await
        .expect("STEP 4: promote_selection");
    assert_eq!(promoted.len(), 1, "STEP 4: one promoted element");
    assert!(
        promoted[0].element_id.starts_with("el_"),
        "STEP 4: Rust-minted ElementId, got {}",
        promoted[0].element_id
    );
    assert_eq!(promoted[0].kind, "face", "STEP 4: promoted a face");
    let again = rt
        .promote_selection(snap, body, vec![(TopoKey::new(&face_key), None)])
        .await
        .expect("STEP 4: re-promote");
    assert_eq!(
        again[0].element_id, promoted[0].element_id,
        "STEP 4 (Invariant 1): re-picking the same (body,topoKey) mints the SAME id"
    );
    eprintln!(
        "STEP 4 PASS: elementId={} stable across re-pick",
        promoted[0].element_id
    );

    // ── Step 5: fillet one edge (anchor-resolved) → regen → applied | NeedsRepair ─
    let (edge_key, edge_anchor) = vertical_edge_pick(&view, &mesh);
    add_op(&mut rt, fillet_op_record(body, edge_anchor));
    let fil_report = regen_all(&mut rt).await;
    let fil_snap = published(&fil_report, "STEP 5 fillet").clone();
    let fillet_applied;
    if fil_snap.repair_summary.needs_repair_count > 0 {
        // A clean NeedsRepair is an ACCEPTABLE outcome (state, never a wrong bind):
        // the plan publishes ≤ m−1 (pre-fillet). Report it loudly.
        fillet_applied = false;
        assert_eq!(
            body_sig_set(&fil_snap),
            ext_sig,
            "STEP 5: NeedsRepair publishes the pre-fillet snapshot (m−1)"
        );
        eprintln!(
            "STEP 5 RESULT: fillet ⇒ CLEAN NeedsRepair ({} refs) on edge {edge_key} — body reverted to m−1 (acceptable)",
            fil_snap.repair_summary.needs_repair_count
        );
    } else {
        // Fillet applied: the rolled fillet face is added (6 → ≥7) on the same body.
        fillet_applied = true;
        assert_eq!(fil_report.changed.len(), 1, "STEP 5: the filleted body");
        let fbody = fil_report.changed[0].0;
        let fmesh = rt
            .get_mesh(fbody, Lod::Coarse, None)
            .await
            .expect("STEP 5: fillet mesh");
        let fview = validate_mesh_blob(&fmesh).expect("STEP 5: fillet MESH1 validates");
        assert!(
            fview.face_count >= 7,
            "STEP 5: filleting one edge adds a rolled face (6→≥7), got {}",
            fview.face_count
        );
        assert_ne!(
            body_sig_set(&fil_snap),
            ext_sig,
            "STEP 5: the filleted body's signature differs from pre-fillet"
        );
        eprintln!(
            "STEP 5 PASS: fillet APPLIED on edge {edge_key} — faces {} → {}",
            view.face_count, fview.face_count
        );
    }

    // The document-under-test now carries sketch → extrude → fillet.
    let head1 = wm.get_worker_head().await.expect("worker 1 head");

    // ── Step 6: save v2 → reopen in a FRESH worker → replay → determinism ───────
    let dir = tempfile::tempdir().expect("tempdir");
    let path1 = dir.path().join("m2_a.onecad");
    rt.save(&path1, save_meta())
        .expect("STEP 6: save container");

    let wm2 = spawn_worker(bin.clone()).await;
    let mut rt2 = open_over(&wm2, &path1);
    let rep2 = regen_all(&mut rt2).await;
    let snap2 = published(&rep2, "STEP 6 replay").clone();
    let head2 = wm2.get_worker_head().await.expect("worker 2 head");

    assert_eq!(
        head1.history_prefix_hash, head2.history_prefix_hash,
        "STEP 6: identical historyPrefixHash chain across two fresh worker processes"
    );
    assert_eq!(
        body_id_set(&fil_snap),
        body_id_set(&snap2),
        "STEP 6: identical body set across processes"
    );
    assert_eq!(
        body_sig_set(&fil_snap),
        body_sig_set(&snap2),
        "STEP 6: identical quantized signatures across processes (determinism)"
    );

    // Byte-stable save: reopen a SECOND time, save again, byte-compare document.json.
    let path2 = dir.path().join("m2_b.onecad");
    rt2.save(&path2, save_meta())
        .expect("STEP 6: re-save container");
    assert_eq!(
        read_document_json(&path1),
        read_document_json(&path2),
        "STEP 6: document.json is byte-identical across an open→save round-trip"
    );
    eprintln!(
        "STEP 6 PASS: hash chain + body set + signatures identical across 2 processes; document.json byte-stable"
    );

    // ── Step 7: STEP export → file exists, non-empty, ISO-10303-21 magic ────────
    let step_path = dir.path().join("m2.step");
    let bytes_written = wm
        .export_step(&step_path.to_string_lossy(), &[body], "AP214IS")
        .await
        .expect("STEP 7: ExportStep");
    assert!(bytes_written > 0, "STEP 7: worker reports bytes written");
    let step_bytes = std::fs::read(&step_path).expect("STEP 7: STEP file exists");
    assert!(!step_bytes.is_empty(), "STEP 7: STEP file is non-empty");
    assert!(
        step_bytes.starts_with(b"ISO-10303-21"),
        "STEP 7: STEP magic header 'ISO-10303-21'"
    );
    eprintln!(
        "STEP 7 PASS: STEP export {} bytes, ISO-10303-21 header",
        step_bytes.len()
    );

    // ── Step 8: undo the fillet → regen → body reverts to the pre-fillet state ──
    assert!(rt.undo(), "STEP 8: undo removes the fillet op");
    let undo_report = regen_all(&mut rt).await;
    let undo_snap = published(&undo_report, "STEP 8 revert").clone();
    assert_eq!(
        body_sig_set(&undo_snap),
        ext_sig,
        "STEP 8: undoing the fillet reverts the body to the pre-fillet signature"
    );
    let ubody = undo_report.changed[0].0;
    let umesh = rt
        .get_mesh(ubody, Lod::Coarse, None)
        .await
        .expect("STEP 8: reverted mesh");
    let uview = validate_mesh_blob(&umesh).expect("STEP 8: MESH1 validates");
    assert_eq!(
        uview.face_count, 6,
        "STEP 8: reverted body is the 6-face box again"
    );
    eprintln!(
        "STEP 8 PASS: fillet undone, body reverted to 6 faces (fillet_applied={fillet_applied})"
    );

    wm.shutdown().await;
    wm2.shutdown().await;
    eprintln!("M2 GATE: all 8 steps asserted");
}

// ─────────────────────────────────────────────────────────────────────────────
// F-WP9 gap 7 — a sketch on a NON-XY plane carries the correct non-standard basis.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m2_non_xy_plane_basis() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: real worker binary not found (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);

    let sid = SketchId(Uuid::from_u128(0x22));
    rt.apply(EditCommand::AddSketch {
        sketch: Sketch::on_world_plane(sid, "OnXZ", WorldPlane::XZ),
    })
    .expect("AddSketch on XZ");
    let session = rt.enter_sketch(sid).await.expect("enter_sketch XZ");

    // The non-standard OneCAD-CPP XZ basis (Sketch.h SketchPlane::XZ):
    // x=(0,1,0), y=(0,0,1), n=(1,0,0).
    assert_eq!(session.plane["kind"], "XZ", "gap 7: plane kind is XZ");
    assert_eq!(session.plane["xAxis"], serde_json::json!([0.0, 1.0, 0.0]));
    assert_eq!(session.plane["yAxis"], serde_json::json!([0.0, 0.0, 1.0]));
    assert_eq!(session.plane["normal"], serde_json::json!([1.0, 0.0, 0.0]));
    eprintln!("gap 7 PASS: XZ sketch carries the non-standard basis");

    wm.shutdown().await;
}
