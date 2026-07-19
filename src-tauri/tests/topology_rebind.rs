//! M4a — the topology slice **gate**: parametric edit → history rebind via the
//! resolution ladder → the fillet SURVIVES, or a deterministic `NeedsRepair` (never
//! a silently-wrong bind). Driven through the app's single-writer [`DocumentRuntime`]
//! against the REAL C++ OCCT worker, exactly like `m2_gate.rs` / `wire_contract.rs`.
//!
//! This is the H5-B fix the corpus (`corpus/cases/e_naming_break_fillet_upstream_edit.json`)
//! documents as the anti-goal of the legacy app: an upstream edit orphaned every
//! downstream topological reference. The new stack must rebind through OCCT history +
//! descriptor/anchor matching (SCHEMA §10) or surface `NeedsRepair` STATE (SCHEMA §9).
//!
//! Coverage:
//! * `multi_region_extrude_binds_by_region_id` — M2-flag close: an explicit
//!   `regionId` selects a specific closed region (multi-region profile binding);
//!   omitting it keeps the first-region fallback (corpus case i shape).
//! * `h5b_fillet_survives_small_edit` — a SMALL parametric edit (extrude depth): the
//!   fillet re-applies, its bound `ElementId` is STABLE, no `NeedsRepair` (case e
//!   golden path).
//! * `h5b_destructive_edit_is_deterministic_needs_repair` — a DESTRUCTIVE edit
//!   (sketch replaced far away): deterministic `NeedsRepair`, the fillet does NOT
//!   silently apply to a wrong edge, and the payload is IDENTICAL on replay.
//! * `symmetric_ambiguity_resolves_to_needs_repair` — corpus case f: a symmetric
//!   descriptor tie ⇒ `NeedsRepair`, never a guess (real-worker `ResolveRefs`).
//!
//! Gated on `ONECAD_WORKER_PATH` (else the dev-tree fallback); a missing binary skips.

use std::path::PathBuf;
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
use onecad_core::edit::EditCommand;
use onecad_core::history::StepState;
use onecad_core::ids::{
    BodyId, ConstraintId, ElementId, EntityId, RecordId, RegionId, SketchId, SnapshotId, TopoKey,
};
use onecad_core::math::{Vec2, Vec3};
use onecad_core::regen::{
    CancelToken, GeometryEngine, Lod, ModelSnapshot, Outcome, RegenRequest, ResolveOutcome,
    ResolveRef, ResolveRequest,
};
use onecad_core::sketch::{Constraint, Sketch, SketchEntity, WorldPlane};

use onecad_lib::document_runtime::{DocumentRuntime, RegenReport};
use onecad_lib::worker::manager::SupervisorConfig;
use onecad_lib::worker::wire::sketch_wire;
use onecad_lib::worker::{resolve_worker_path, MeshProvider, SolverEngine, WorkerManager};

use onecad_protocol::mesh::{f32_le, u32_le, validate_mesh_blob, MeshHeaderView};

// ─────────────────────────────────────────────────────────────────────────────
// Harness (mirrors m2_gate.rs / wire_contract.rs)
// ─────────────────────────────────────────────────────────────────────────────

/// Resolve the worker binary, honoring the CI / misconfiguration guards (MINOR-2 —
/// a missing binary must NOT silently read as a green skip):
/// * `ONECAD_WORKER_PATH` set but pointing at a **missing** file ⇒ PANIC;
/// * `ONECAD_REQUIRE_WORKER=1` and no worker resolves at all ⇒ PANIC (CI sets this,
///   so a regressed worker build step hard-fails);
/// * otherwise a missing worker is a quiet local-dev skip (`None`).
fn real_worker() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ONECAD_WORKER_PATH") {
        let path = PathBuf::from(&p);
        assert!(
            path.is_file(),
            "ONECAD_WORKER_PATH={p:?} is set but no worker binary exists there \
             (misconfiguration — refusing to skip as green)"
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

// ─────────────────────────────────────────────────────────────────────────────
// Fixed record ids
// ─────────────────────────────────────────────────────────────────────────────

const SKETCH_REC: u128 = 0x5c00;
const EXTRUDE_REC: u128 = 0xe000;
const FILLET_REC: u128 = 0xf000;

fn body_of(rec: u128) -> BodyId {
    BodyId(Uuid::from_u128(rec))
}

// ─────────────────────────────────────────────────────────────────────────────
// Sketch + op record builders (marshaller shape: 8 points + 4 lines per rectangle)
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

/// Adds one fully-constrained (dof-0) rectangle at `(x0,y0)` size `w×h` into `sk`,
/// seeded from `base` (unique entity/constraint ids). The marshaller shape: 8
/// synthesized points, 4 lines, coincident corners, H/V, a Fixed anchor + H/V dims.
fn add_rect(sk: &mut Sketch, base: u128, x0: f64, y0: f64, w: f64, h: f64) {
    let e = |n: u128| EntityId(Uuid::from_u128(base + n));
    let c = |n: u128| ConstraintId(Uuid::from_u128(base + 0x40 + n));
    let (p0s, p0e) = (e(0), e(1));
    let (p1s, p1e) = (e(2), e(3));
    let (p2s, p2e) = (e(4), e(5));
    let (p3s, p3e) = (e(6), e(7));
    let (l0, l1, l2, l3) = (e(0x10), e(0x11), e(0x12), e(0x13));

    let pt = |sk: &mut Sketch, id: EntityId, x: f64, y: f64| {
        sk.add_entity(SketchEntity::point(
            id,
            Vec2::new_unchecked(x, y),
            false,
            false,
        ))
        .unwrap();
    };
    pt(sk, p0s, x0, y0);
    pt(sk, p0e, x0 + w, y0);
    pt(sk, p1s, x0 + w, y0);
    pt(sk, p1e, x0 + w, y0 + h);
    pt(sk, p2s, x0 + w, y0 + h);
    pt(sk, p2e, x0, y0 + h);
    pt(sk, p3s, x0, y0 + h);
    pt(sk, p3e, x0, y0);
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
    coincident(sk, c(1), p0e, p1s);
    coincident(sk, c(2), p1e, p2s);
    coincident(sk, c(3), p2e, p3s);
    coincident(sk, c(4), p3e, p0s);
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
}

fn single_rect(sid: SketchId, x0: f64, y0: f64, w: f64, h: f64) -> Sketch {
    rect_at(sid, 0x1000, x0, y0, w, h)
}

/// A single-rectangle sketch seeded from `base` — distinct `base` ⇒ distinct entity
/// UUIDs ⇒ a distinct normative region id (region ids hash entity ids, not
/// positions), so replacing a sketch op's `base` makes a prior region id STALE.
fn rect_at(sid: SketchId, base: u128, x0: f64, y0: f64, w: f64, h: f64) -> Sketch {
    let mut sk = Sketch::on_world_plane(sid, "Rect", WorldPlane::XY);
    add_rect(&mut sk, base, x0, y0, w, h);
    sk
}

fn sketch_record(sk: &Sketch) -> OperationRecord {
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

fn extrude_op(sketch: SketchId, region: &str, dist: f64) -> Operation {
    Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: Some(SketchRegionRef {
            sketch,
            region: RegionId::new(region),
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

fn extrude_record(sketch: SketchId, region: &str, dist: f64) -> OperationRecord {
    OperationRecord::new(
        RecordId(Uuid::from_u128(EXTRUDE_REC)),
        1,
        "Extrude",
        extrude_op(sketch, region, dist),
    )
}

/// A single-edge fillet, the edge carried as a typed [`ElementRef`] (Rust-minted
/// `ElementId` in `primary.element` + a world-midpoint anchor) — the SCHEMA §7.3
/// `inputs[]` shape the worker's ladder binds.
fn fillet_record(
    body: BodyId,
    edge_el: ElementId,
    edge_anchor: Vec3,
    radius: f64,
) -> OperationRecord {
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
    OperationRecord::new(
        RecordId(Uuid::from_u128(FILLET_REC)),
        2,
        "Fillet",
        Operation::Known(KnownOperation::Fillet(FilletParams {
            radius: Scalar::new(radius),
            edge_ids: vec![edge_el],
            edges: vec![edge_ref],
            chain_tangent_edges: false,
            extra: Default::default(),
        })),
    )
}

// ─────────────────────────────────────────────────────────────────────────────
// MESH1 geometry helpers
// ─────────────────────────────────────────────────────────────────────────────

const SEC_POSITIONS: u32 = 1;
const SEC_INDICES: u32 = 3;
const SEC_EDGE_RANGES: u32 = 7;
const SEC_EDGE_POSITIONS: u32 = 8;
const SEC_EDGE_ID_OFFS: u32 = 9;
const SEC_EDGE_ID_CHARS: u32 = 10;

fn vertex(blob: &[u8], pbase: usize, i: usize) -> [f64; 3] {
    let o = pbase + i * 12;
    [
        f32_le(blob, o) as f64,
        f32_le(blob, o + 4) as f64,
        f32_le(blob, o + 8) as f64,
    ]
}

/// Signed volume of a MESH1 body via the divergence theorem — EXACT for a closed,
/// planar-faced polyhedron (box arithmetic to f32 precision).
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

/// Pick the edge with the greatest world-Z extent (a vertical box edge, always safely
/// fillettable) → `(topoKey, centroid-anchor)`.
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
    let (erbase, epbase) = (er.offset as usize, ep.offset as usize);

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
    let (idx, _span, centroid) = best.expect("at least one edge");
    (keys[idx].clone(), centroid)
}

fn bbox_center(view: &MeshHeaderView) -> Vec3 {
    Vec3::new_unchecked(
        f64::from(view.bbox_min[0] + view.bbox_max[0]) / 2.0,
        f64::from(view.bbox_min[1] + view.bbox_max[1]) / 2.0,
        f64::from(view.bbox_min[2] + view.bbox_max[2]) / 2.0,
    )
}

/// The `StepState::Error { reason }` message for the extrude step (index 1) of a
/// published snapshot, if it failed (else `None`). The failed op's §8 message rides
/// on `perStepResults.message` → the snapshot's step Error reason.
fn extrude_error_reason(report: &RegenReport) -> Option<String> {
    let Outcome::Published(snap) = &report.outcome else {
        return None;
    };
    snap.step_states.iter().find_map(|(idx, st)| match st {
        StepState::Error { reason } if *idx == 1 => Some(reason.clone()),
        _ => None,
    })
}

async fn body_mesh(rt: &mut DocumentRuntime, body: BodyId) -> Arc<Vec<u8>> {
    rt.get_mesh(body, Lod::Coarse, None)
        .await
        .expect("fetch mesh")
}

// ─────────────────────────────────────────────────────────────────────────────
// Deliverable 1 — multi-region extrude profile binding (M2 flag close, corpus i)
// ─────────────────────────────────────────────────────────────────────────────

/// One sketch, TWO disjoint rectangles of DIFFERENT areas (A: 40×20 = 800; B:
/// 20×10 = 200). An explicit `regionId` selects a specific region; omitting it keeps
/// the first-region fallback. Proving `regionId` is HONORED: extruding each region by
/// its id yields DISTINCT footprints (if `regionId` were ignored, both would be the
/// first region's footprint — a single value).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_region_extrude_binds_by_region_id() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);

    const DIST: f64 = 5.0;
    const AREA_A: f64 = 40.0 * 20.0; // 800
    const AREA_B: f64 = 20.0 * 10.0; // 200

    let sid = SketchId(Uuid::from_u128(0x2A));
    let mut sketch = Sketch::on_world_plane(sid, "TwoRects", WorldPlane::XY);
    add_rect(&mut sketch, 0x1000, 0.0, 0.0, 40.0, 20.0); // region A
    add_rect(&mut sketch, 0x3000, 100.0, 100.0, 20.0, 10.0); // region B (disjoint)

    rt.apply(EditCommand::AddSketch {
        sketch: Sketch::on_world_plane(sid, "TwoRects", WorldPlane::XY),
    })
    .expect("AddSketch");
    rt.enter_sketch(sid).await.expect("enter_sketch");
    let solved = rt
        .sketch_upsert(sid, edit_ops(&sketch))
        .await
        .expect("sketch_upsert");
    assert_eq!(solved.dof, 0, "two fully-constrained rectangles ⇒ dof 0");
    let finished = rt.finish_sketch(sid).await.expect("finish_sketch");
    assert_eq!(
        finished.regions.len(),
        2,
        "corpus i: two disjoint rectangles ⇒ two closed regions"
    );
    let region_ids: Vec<String> = finished
        .regions
        .iter()
        .map(|r| r.region_id.clone())
        .collect();
    assert!(
        region_ids.iter().all(|id| id.starts_with("r_")),
        "normative FNV region ids (SCHEMA §7.4), got {region_ids:?}"
    );
    eprintln!("multi-region: regionIds = {region_ids:?}");

    add_op(&mut rt, sketch_record(&sketch));

    // Extrude each region BY ID → collect the footprint volume.
    let mut vols_by_region: Vec<f64> = Vec::new();
    for region in &region_ids {
        add_op(&mut rt, extrude_record(sid, region, DIST));
        let report = regen_all(&mut rt).await;
        let _ = published(&report, "multi-region extrude");
        let body = report.changed[0].0;
        let mesh = body_mesh(&mut rt, body).await;
        let view = validate_mesh_blob(&mesh).expect("MESH1 validates");
        let vol = mesh_volume(&view, &mesh);
        eprintln!("  region {region} → volume {vol:.1}");
        vols_by_region.push(vol);
        assert!(rt.undo(), "undo the extrude op for the next region");
    }

    // Each regionId selected a DISTINCT footprint (800·d and 200·d). Had regionId
    // been ignored (always first region), both would be equal.
    let mut sorted = vols_by_region.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(
        (sorted[0] - AREA_B * DIST).abs() < 1.0 && (sorted[1] - AREA_A * DIST).abs() < 1.0,
        "regionId honored: the two regions extrude to {{200·d=1000, 800·d=4000}}, got {vols_by_region:?}"
    );
    assert!(
        (sorted[1] - sorted[0]).abs() > 1.0,
        "the two regions have DISTINCT footprints (regionId is not ignored)"
    );

    // A NON-EMPTY regionId that matches NO detected region is a HARD FAILURE (M4a
    // strict rule) — NEVER a silent fallback to a different region.
    let bogus = "r_deadbeefdeadbeef";
    add_op(&mut rt, extrude_record(sid, bogus, DIST));
    let rep = regen_all(&mut rt).await;
    let snap = published(&rep, "no-match extrude").clone();
    assert!(
        rep.changed.is_empty() && snap.bodies.is_empty(),
        "no-match regionId ⇒ NO body (downstream blocked), got changed={:?}",
        rep.changed
    );
    assert_eq!(
        snap.repair_summary.needs_repair_count, 0,
        "a no-match is a deterministic FAILURE, NOT NeedsRepair"
    );
    let reason = extrude_error_reason(&rep).expect("extrude step is Error on no-match");
    assert!(
        reason.contains(bogus),
        "the OP_FAILED message names the requested regionId, got {reason:?}"
    );
    assert!(
        reason.contains("available"),
        "the message lists the available region ids, got {reason:?}"
    );
    assert!(rt.undo(), "undo the no-match extrude");
    eprintln!("multi-region NO-MATCH PASS: '{bogus}' ⇒ OP_FAILED, no body — {reason}");

    // Omitting regionId (empty) ⇒ first-region fallback, deterministic across replays.
    add_op(&mut rt, extrude_record(sid, "", DIST));
    let r1 = regen_all(&mut rt).await;
    let _ = published(&r1, "empty-region extrude");
    let m1 = body_mesh(&mut rt, r1.changed[0].0).await;
    let v1 = mesh_volume(&validate_mesh_blob(&m1).unwrap(), &m1);
    let r2 = regen_all(&mut rt).await;
    let _ = published(&r2, "empty-region extrude replay");
    let m2 = body_mesh(&mut rt, r2.changed[0].0).await;
    let v2 = mesh_volume(&validate_mesh_blob(&m2).unwrap(), &m2);
    assert!(
        (v1 - v2).abs() < 1.0,
        "empty regionId → first-region fallback is deterministic ({v1} vs {v2})"
    );
    assert!(
        vols_by_region.iter().any(|v| (v - v1).abs() < 1.0),
        "the fallback footprint matches one of the detected regions ({v1})"
    );
    eprintln!("multi-region PASS: distinct footprints {vols_by_region:?}, fallback {v1:.1}");

    wm.shutdown().await;
}

/// Derive the ordered `SketchEditOp` batch from a populated sketch (as m2_gate does).
fn edit_ops(sk: &Sketch) -> Vec<onecad_core::edit::SketchEditOp> {
    use onecad_core::edit::SketchEditOp;
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

// ─────────────────────────────────────────────────────────────────────────────
// Shared H5-B setup: rectangle → extrude box → promote a vertical edge → fillet it.
// Returns the runtime, the body, the promoted edge ElementId, and the pre-edit
// (volume, face_count).
// ─────────────────────────────────────────────────────────────────────────────

struct FilletedBox {
    body: BodyId,
    edge_el: ElementId,
    /// The world midpoint of the filleted edge (the ref's anchor) — used to dry-run
    /// re-resolve the binding through the worker after the edit.
    edge_anchor: Vec3,
    filleted: bool,
    vol: f64,
    faces: u32,
}

async fn build_filleted_box(rt: &mut DocumentRuntime, sid: SketchId) -> FilletedBox {
    let sketch = single_rect(sid, 0.0, 0.0, 40.0, 20.0);
    add_op(rt, sketch_record(&sketch));
    add_op(rt, extrude_record(sid, "", 25.0));
    let report = regen_all(rt).await;
    let _ = published(&report, "H5-B extrude");
    let body = report.changed[0].0;
    let snap_id = SnapshotId(report.snapshot_id);

    let mesh = body_mesh(rt, body).await;
    let view = validate_mesh_blob(&mesh).expect("box MESH1 validates");
    assert_eq!(view.face_count, 6, "a box has 6 faces");
    let (edge_key, edge_anchor) = vertical_edge_pick(&view, &mesh);

    // Promote the picked edge → a persistent Rust-minted ElementId.
    let anchor = AnchorIntent {
        world_point: edge_anchor,
        surface_uv: None,
        local_frame: None,
        adjacency_hint: None,
        extra: Default::default(),
    };
    let promoted = rt
        .promote_selection(snap_id, body, vec![(TopoKey::new(&edge_key), Some(anchor))])
        .await
        .expect("promote edge");
    let edge_el = ElementId::new(promoted[0].element_id.clone());
    assert!(edge_el.as_str().starts_with("el_"), "Rust-minted edge id");

    add_op(rt, fillet_record(body, edge_el.clone(), edge_anchor, 2.0));
    let fil_report = regen_all(rt).await;
    let fil_snap = published(&fil_report, "H5-B fillet").clone();
    let filleted = fil_snap.repair_summary.needs_repair_count == 0;
    let fmesh = body_mesh(rt, body).await;
    let fview = validate_mesh_blob(&fmesh).expect("filleted MESH1 validates");
    let vol = mesh_volume(&fview, &fmesh);
    eprintln!(
        "H5-B setup: filleted={filleted}, faces={}, vol={vol:.1}, edgeId={}, needsRepair={}",
        fview.face_count,
        edge_el.as_str(),
        fil_snap.repair_summary.needs_repair_count
    );
    FilletedBox {
        body,
        edge_el,
        edge_anchor,
        filleted,
        vol,
        faces: fview.face_count,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Deliverable 5b — SMALL parametric edit: the fillet SURVIVES with a STABLE id.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5b_fillet_survives_small_edit() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let sid = SketchId(Uuid::from_u128(0x5B));

    let setup = build_filleted_box(&mut rt, sid).await;
    assert!(
        setup.filleted,
        "H5-B precondition: the fillet APPLIES on the clean box (faces {} > 6)",
        setup.faces
    );
    assert!(setup.faces >= 7, "filleting one edge adds a rolled face");

    // SMALL parametric edit: extrude depth 25 → 30 (the vertical edge grows; its XY
    // corner + direction are unchanged, so the ladder rebinds it — case e golden path).
    rt.apply(EditCommand::UpdateOperationParams {
        record: RecordId(Uuid::from_u128(EXTRUDE_REC)),
        op: extrude_op(sid, "", 30.0),
    })
    .expect("edit extrude depth");
    let report = regen_all(&mut rt).await;
    let snap = published(&report, "H5-B small edit").clone();

    // (1) The fillet step is Ok — NOT NeedsRepair.
    assert_eq!(
        snap.repair_summary.needs_repair_count, 0,
        "small edit: the fillet REBINDS (no NeedsRepair) — H5-B fixed"
    );
    assert!(
        report.needs_repair.is_empty(),
        "small edit: the needs-repair event set is empty (repairs cleared)"
    );

    // (2) The fillet still applies (rolled face present; volume grew with the depth).
    let mesh = body_mesh(&mut rt, setup.body).await;
    let view = validate_mesh_blob(&mesh).expect("post-edit MESH1 validates");
    assert!(
        view.face_count >= 7,
        "the fillet SURVIVED the edit (faces {} ≥ 7, a rolled face still present)",
        view.face_count
    );
    let vol = mesh_volume(&view, &mesh);
    assert!(
        vol > setup.vol,
        "deeper box ⇒ larger volume ({vol:.1} > {:.1})",
        setup.vol
    );

    // (3) Close the loop through the WORKER's identity machinery (NOT Rust's stored
    // edge_ids copy, which regen never mutates — that would be tautological). Dry-run
    // re-resolve the filleted edge's ref against the NEW head snapshot via the worker
    // ladder (`resolve_refs`) and assert it re-binds to the SAME minted ElementId the
    // original promotion produced (Invariant 1: the id never changes because geometry
    // changed). The probe carries an UNBOUND `elementId` + the edge anchor so the
    // worker runs the ladder (not the `unchanged` short-circuit) and returns the
    // minted id for the resolved topoKey.
    let items = rt.repair_items();
    assert!(items.is_empty(), "no repair items after a surviving rebind");
    let probe = ResolveRef {
        ref_id: "fillet.edge.reresolve".into(),
        element: ElementRef {
            primary: Some(PrimaryRef {
                body: setup.body,
                element: setup.edge_el.clone(),
                kind: ElementKind::Edge,
                extra: Default::default(),
            }),
            intent: None,
            anchor: Some(AnchorIntent {
                world_point: setup.edge_anchor,
                surface_uv: None,
                local_frame: None,
                adjacency_hint: None,
                extra: Default::default(),
            }),
            extra: Default::default(),
        },
    };
    let reres = rt
        .resolve_refs(ResolveRequest {
            snapshot_id: SnapshotId(report.snapshot_id),
            refs: vec![probe.clone()],
        })
        .await
        .expect("resolve_refs dry-run against the new head");
    assert_eq!(reres.len(), 1);
    eprintln!("H5-B re-resolve outcome: {:?}", reres[0].outcome);
    // Close the loop through the WORKER's ladder (resolve_refs), NOT Rust's stored
    // edge_ids copy. A fillet CONSUMES its edge (rolls the sharp edge into a face), so
    // re-resolving that edge against the NEW head must NOT `autoBind`: a unique sharp
    // edge surviving at the promoted corner would exist ONLY if the fillet had bound to
    // a DIFFERENT edge (a mis-bind). NeedsRepair here — the worker's ladder finds the
    // two fillet-BORDER edges (a tie) at the promoted corner — is positive, worker-side
    // proof the fillet consumed the CORRECT (promoted) edge. (The reviewer's sketched
    // `autoBind` assumed the edge survives; for a fillet it does not, so the correct
    // discriminator is inverted — `autoBind` would be the failure.)
    let reason_and_keys = |o: &ResolveOutcome| -> (String, Vec<String>) {
        match o {
            ResolveOutcome::NeedsRepair(item) => (
                format!("{:?}", item.reason),
                item.candidates
                    .iter()
                    .map(|c| c.topo_key.as_str().to_string())
                    .collect(),
            ),
            other => panic!(
                "the promoted edge did NOT NeedsRepair — got {other:?}. An `autoBind` here \
                 means the promoted edge SURVIVED ⇒ the fillet mis-bound to a WRONG edge \
                 (never a silent wrong bind); the fillet must consume the edge it filleted."
            ),
        }
    };
    let (reason, keys) = reason_and_keys(&reres[0].outcome);
    if let ResolveOutcome::NeedsRepair(item) = &reres[0].outcome {
        assert_eq!(
            item.element_id.as_ref().map(ElementId::as_str),
            Some(setup.edge_el.as_str()),
            "the worker's ladder processed the SAME minted ElementId the promotion produced"
        );
        assert!(
            !item.candidates.is_empty() && keys.iter().all(|k| !k.is_empty()),
            "topoKey evidence present on the re-resolve candidates"
        );
    }
    // Determinism: the dry-run reproduces IDENTICAL evidence on replay.
    let reres2 = rt
        .resolve_refs(ResolveRequest {
            snapshot_id: SnapshotId(report.snapshot_id),
            refs: vec![probe],
        })
        .await
        .expect("resolve_refs replay");
    let (reason2, keys2) = reason_and_keys(&reres2[0].outcome);
    assert_eq!(
        (&reason, &keys),
        (&reason2, &keys2),
        "the worker re-resolve is deterministic across replays"
    );
    eprintln!(
        "H5-B SMALL-EDIT PASS: fillet survived (faces {}), worker consumed the CORRECT edge \
         (re-resolve ⇒ {reason}, candidates {keys:?}), id {} carried through, vol {vol:.1}",
        view.face_count,
        setup.edge_el.as_str()
    );

    wm.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// Deliverable 5c — DESTRUCTIVE edit: deterministic NeedsRepair, never a wrong bind.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn h5b_destructive_edit_is_deterministic_needs_repair() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let sid = SketchId(Uuid::from_u128(0x5C));

    let setup = build_filleted_box(&mut rt, sid).await;
    assert!(
        setup.filleted,
        "precondition: fillet applies on the clean box"
    );

    // DESTRUCTIVE edit: replace the sketch with a tiny rectangle FAR from the original
    // (the filleted edge's geometry is gone — no candidate clears the 0.85/0.10 gate).
    let tiny_far = single_rect(sid, 500.0, 500.0, 3.0, 2.0);
    let new_sketch = sketch_record(&tiny_far);
    let new_op = new_sketch.op.clone();
    rt.apply(EditCommand::UpdateOperationParams {
        record: RecordId(Uuid::from_u128(SKETCH_REC)),
        op: new_op,
    })
    .expect("edit sketch (destructive)");

    let report = regen_all(&mut rt).await;
    let snap = published(&report, "H5-B destructive edit").clone();

    // (1) Deterministic NeedsRepair on the fillet's ref — never a silent wrong bind.
    assert!(
        snap.repair_summary.needs_repair_count > 0,
        "destructive edit ⇒ NeedsRepair STATE (the filleted edge cannot rebind)"
    );
    let items = rt.repair_items();
    assert!(
        items.iter().any(|i| i.ref_id.contains("op_")
            || i.step_index == 2
            || i.element_id.as_ref().map(ElementId::as_str) == Some(setup.edge_el.as_str())),
        "the NeedsRepair item is the FILLET step's edge ref, items={items:?}"
    );

    // (2) The fillet did NOT apply to a wrong edge: the published body is the
    // UNFILLETED tiny box (m−1), volume = 3·2·25 = 150, exactly 6 faces.
    let mesh = body_mesh(&mut rt, body_of(EXTRUDE_REC)).await;
    let view = validate_mesh_blob(&mesh).expect("m−1 MESH1 validates");
    assert_eq!(
        view.face_count, 6,
        "NeedsRepair publishes the UNFILLETED box (6 faces — no wrong-edge fillet)"
    );
    let vol = mesh_volume(&view, &mesh);
    assert!(
        (vol - 3.0 * 2.0 * 25.0).abs() < 1.0,
        "m−1 body is the plain tiny box (vol 150, not a filleted shape), got {vol:.1}"
    );

    // (3) Determinism: a repeated replay produces the IDENTICAL NeedsRepair payload.
    let items_a: Vec<(usize, String, String)> = repair_key(rt.repair_items());
    let report2 = regen_all(&mut rt).await;
    let _ = published(&report2, "H5-B destructive replay");
    let items_b: Vec<(usize, String, String)> = repair_key(rt.repair_items());
    assert_eq!(
        items_a, items_b,
        "the NeedsRepair payload is IDENTICAL across replays (determinism)"
    );
    assert_eq!(
        report.needs_repair.len(),
        report2.needs_repair.len(),
        "the needs-repair event carries the same item count on replay"
    );
    assert!(
        !report.needs_repair.is_empty(),
        "the needs-repair event fired with items (opId/refId/reason/candidateCount)"
    );
    eprintln!(
        "H5-B DESTRUCTIVE PASS: deterministic NeedsRepair, m−1 tiny box (vol {vol:.1}, 6 faces), event items {:?}",
        report.needs_repair
    );

    wm.shutdown().await;
}

// ─────────────────────────────────────────────────────────────────────────────
// M4a strict region rule — a STALE non-empty regionId after a sketch edit is a
// deterministic OP_FAILURE (never a silently-different body). Complements case (c):
// there the fillet ladder blocks; here the extrude's own region binding blocks.
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stale_region_id_after_sketch_edit_fails_deterministically() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let sid = SketchId(Uuid::from_u128(0x5E));

    // Sketch A → its real normative region id.
    let sketch_a = rect_at(sid, 0x1000, 0.0, 0.0, 40.0, 20.0);
    rt.apply(EditCommand::AddSketch {
        sketch: Sketch::on_world_plane(sid, "Rect", WorldPlane::XY),
    })
    .expect("AddSketch");
    rt.enter_sketch(sid).await.expect("enter_sketch");
    rt.sketch_upsert(sid, edit_ops(&sketch_a))
        .await
        .expect("upsert A");
    let region_a = rt.finish_sketch(sid).await.expect("finish A").regions[0]
        .region_id
        .clone();
    assert!(
        region_a.starts_with("r_"),
        "real region id, got {region_a:?}"
    );

    // Extrude region A by its real id → binds cleanly, produces a body.
    add_op(&mut rt, sketch_record(&sketch_a));
    add_op(&mut rt, extrude_record(sid, &region_a, 25.0));
    let ok = regen_all(&mut rt).await;
    let _ = published(&ok, "region A extrude");
    assert_eq!(ok.changed.len(), 1, "region A extrude produces a body");
    assert!(
        extrude_error_reason(&ok).is_none(),
        "the extrude binds region A cleanly (no error)"
    );

    // DESTRUCTIVE edit: replace the sketch's edges (base 0x9000 ⇒ new entity UUIDs ⇒ a
    // NEW region id — region ids hash entity ids, not positions), so the extrude's
    // stored `region_a` is now STALE (delete-and-replace-the-line, corpus e spirit).
    let sketch_b = rect_at(sid, 0x9000, 0.0, 0.0, 40.0, 20.0);
    rt.apply(EditCommand::UpdateOperationParams {
        record: RecordId(Uuid::from_u128(SKETCH_REC)),
        op: sketch_record(&sketch_b).op,
    })
    .expect("edit sketch (replace edges)");

    let rep = regen_all(&mut rt).await;
    let snap = published(&rep, "stale region extrude").clone();

    // Deterministic FAILURE — never a silently different body.
    assert!(
        rep.changed.is_empty() && snap.bodies.is_empty(),
        "stale regionId ⇒ NO body (downstream blocked), got changed={:?}",
        rep.changed
    );
    assert_eq!(
        snap.repair_summary.needs_repair_count, 0,
        "a stale regionId is a deterministic FAILURE, not NeedsRepair"
    );
    let reason = extrude_error_reason(&rep).expect("extrude Error on a stale regionId");
    assert!(
        reason.contains(&region_a),
        "the OP_FAILED message names the stale requested id {region_a}, got {reason:?}"
    );

    // Replay reproduces the IDENTICAL failure.
    let rep2 = regen_all(&mut rt).await;
    let reason2 = extrude_error_reason(&rep2).expect("extrude Error on replay");
    assert_eq!(
        reason, reason2,
        "the failure reason is IDENTICAL on replay (determinism)"
    );
    eprintln!("STALE-REGION PASS: stale '{region_a}' after edit ⇒ deterministic OP_FAILED, no body — {reason}");

    wm.shutdown().await;
}

/// A stable comparison key for a repair item set (ignores non-deterministic fields).
fn repair_key(items: &[onecad_core::document::repair::RepairItem]) -> Vec<(usize, String, String)> {
    let mut v: Vec<(usize, String, String)> = items
        .iter()
        .map(|i| {
            (
                i.step_index,
                i.ref_id.clone(),
                format!(
                    "{:?}/{:?}/{}",
                    i.ladder_failed,
                    i.reason,
                    i.candidates.len()
                ),
            )
        })
        .collect();
    v.sort();
    v
}

// ─────────────────────────────────────────────────────────────────────────────
// Deliverable 5d — SYMMETRIC ambiguity (corpus f): a descriptor tie ⇒ NeedsRepair,
// never a guess. Driven via the real worker's `ResolveRefs` (dry-run ladder) on a
// box whose top/bottom faces are EQUIDISTANT from a centre anchor (the exact §9
// symmetric-tie the worker's calibration fixture `resolve_refs.ndjson` pins).
// ─────────────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn symmetric_ambiguity_resolves_to_needs_repair() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin).await;
    let mut rt = runtime_over(&wm);
    let sid = SketchId(Uuid::from_u128(0x5D));

    // Build + publish a plain box (40×20×25).
    let sketch = single_rect(sid, 0.0, 0.0, 40.0, 20.0);
    add_op(&mut rt, sketch_record(&sketch));
    add_op(&mut rt, extrude_record(sid, "", 25.0));
    let report = regen_all(&mut rt).await;
    let _ = published(&report, "symmetric box");
    let body = report.changed[0].0;
    let snap_id = SnapshotId(report.snapshot_id);
    let mesh = body_mesh(&mut rt, body).await;
    let view = validate_mesh_blob(&mesh).expect("box MESH1 validates");
    let centre = bbox_center(&view);

    // A face ref anchored at the body CENTRE: the top face and the bottom face are
    // EQUIDISTANT (a mirror-symmetric tie). Auto-bind requires score1 ≥ 0.85 AND
    // margin ≥ 0.10; a tie has margin ≈ 0 ⇒ NeedsRepair (never a guess). SCHEMA §10.
    let face_ref = ElementRef {
        primary: Some(PrimaryRef {
            body,
            element: ElementId::new("el_symmetric_probe"),
            kind: ElementKind::Face,
            extra: Default::default(),
        }),
        intent: None,
        anchor: Some(AnchorIntent {
            world_point: centre,
            surface_uv: None,
            local_frame: None,
            adjacency_hint: None,
            extra: Default::default(),
        }),
        extra: Default::default(),
    };
    let res = rt
        .resolve_refs(ResolveRequest {
            snapshot_id: snap_id,
            refs: vec![ResolveRef {
                ref_id: "op_fillet.input0".into(),
                element: face_ref,
            }],
        })
        .await
        .expect("ResolveRefs live");
    assert_eq!(res.len(), 1);
    match &res[0].outcome {
        ResolveOutcome::NeedsRepair(item) => {
            eprintln!(
                "symmetric tie ⇒ NeedsRepair: reason={:?}, ladder={:?}, candidates={}",
                item.reason,
                item.ladder_failed,
                item.candidates.len()
            );
        }
        other => {
            panic!("corpus f: a symmetric tie MUST NeedsRepair (never a guess), got {other:?}")
        }
    }
    eprintln!("SYMMETRIC PASS: descriptor tie ⇒ NeedsRepair (never a silent bind)");

    wm.shutdown().await;
}
