//! Checkpoint integration gate (SCHEMA §7.7) against the REAL C++ OCCT worker,
//! driven through the app's [`DocumentRuntime`] like `wire_contract.rs`.
//!
//! Proves the checkpoint round-trip end-to-end:
//!   * a save mints a checkpoint of the head (SaveCheckpoint) into the cache;
//!   * a later edit at/after the checkpoint step regens **incrementally** — the
//!     planner selects the checkpoint, the executor drives RestoreCheckpoint + an
//!     incremental plan — and the final bodies/geometry-signature are **IDENTICAL**
//!     to a forced from-0 replay of the same document (the determinism cross-check);
//!   * the checkpoint is **persisted** into the `.onecad` container and reloaded on
//!     open (durability).
//!
//! REQUIRE_WORKER-guarded (CI hard-fails without a worker; local dev skips cleanly).

use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord, PlaneKind,
    SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::refs::SketchRegionRef;
use onecad_core::document::variables::Scalar;
use onecad_core::edit::EditCommand;
use onecad_core::ids::{ConstraintId, EntityId, RecordId, RegionId, SketchId};
use onecad_core::io::container::SaveMeta;
use onecad_core::math::{Vec2, Vec3};
use onecad_core::regen::{CancelToken, GeometryEngine, ModelSnapshot, Outcome, RegenRequest};
use onecad_core::sketch::{Constraint, Sketch, SketchEntity, WorldPlane};

use onecad_lib::document_runtime::{DocumentRuntime, RegenReport};
use onecad_lib::worker::manager::{SupervisorConfig, WorkerState};
use onecad_lib::worker::{resolve_worker_path, MeshProvider, SolverEngine, WorkerManager};

// ─────────────────────────────────────────────────────────────────────────────
// Harness (mirrors wire_contract.rs)
// ─────────────────────────────────────────────────────────────────────────────

fn real_worker() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("ONECAD_WORKER_PATH") {
        let path = PathBuf::from(&p);
        assert!(
            path.is_file(),
            "ONECAD_WORKER_PATH={p:?} set but no binary there"
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
        "worker must connect + OpenSession"
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

async fn regen(rt: &mut DocumentRuntime, from: usize) -> RegenReport {
    rt.run_regen(RegenRequest::ToEnd { from }, CancelToken::new())
        .await
}

fn published<'a>(report: &'a RegenReport, what: &str) -> &'a Arc<ModelSnapshot> {
    match &report.outcome {
        Outcome::Published(s) => s,
        other => panic!("{what}: expected Published, got {other:?}"),
    }
}

/// The (geometry signature, body count) a published regen settled on — the pair the
/// determinism cross-check compares.
fn head_geometry(report: &RegenReport, what: &str) -> (String, usize) {
    let snap = published(report, what);
    let sig = snap
        .signatures
        .as_ref()
        .map(|s| s.geometry.as_str().to_string())
        .unwrap_or_default();
    (sig, snap.bodies.len())
}

// ─────────────────────────────────────────────────────────────────────────────
// Sketch + op record builders (a fully-constrained rectangle, as wire_contract)
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
    let pt = |sk: &mut Sketch, id, x, y| {
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
    let (_plane, entities, constraints) = onecad_lib::worker::wire::sketch_wire(sk);
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

fn extrude_record(rec: u128, sketch: SketchId, dist: f64) -> OperationRecord {
    let params = ExtrudeParams {
        profile: Some(SketchRegionRef {
            sketch,
            region: RegionId::new(""),
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
    };
    OperationRecord::new(
        RecordId(Uuid::from_u128(rec)),
        0,
        "Extrude",
        Operation::Known(KnownOperation::Extrude(params)),
    )
}

// Fixed record ids for the 4-op history: sketch A, extrude A, sketch B, extrude B.
const SK_A: u128 = 0xA00;
const EX_A: u128 = 0xA01;
const SK_B: u128 = 0xB00;
const EX_B: u128 = 0xB01;

/// Builds the box-A prefix (sketch + extrude) into `rt`.
fn build_prefix(rt: &mut DocumentRuntime) {
    let sa = SketchId(Uuid::from_u128(0xA));
    add_op(
        rt,
        sketch_record(SK_A, &rect_sketch(sa, 0x1000, 0.0, 0.0, 40.0, 20.0)),
    );
    add_op(rt, extrude_record(EX_A, sa, 25.0));
}

/// Appends the box-B suffix (a second independent NewBody).
fn build_suffix(rt: &mut DocumentRuntime) {
    let sb = SketchId(Uuid::from_u128(0xB));
    add_op(
        rt,
        sketch_record(SK_B, &rect_sketch(sb, 0x2000, 60.0, 0.0, 30.0, 15.0)),
    );
    add_op(rt, extrude_record(EX_B, sb, 25.0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkpoint_incremental_matches_from_zero_and_persists() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };

    // ── (1) Baseline: the from-0 replay of the FULL 4-op document (own worker, so
    //        the worker session never cross-contaminates the incremental path) ──────
    let (base_sig, base_bodies) = {
        let wm_base = spawn_worker(bin.clone()).await;
        let mut rt = runtime_over(&wm_base);
        build_prefix(&mut rt);
        build_suffix(&mut rt);
        let report = regen(&mut rt, 0).await;
        let g = head_geometry(&report, "from-0 baseline");
        wm_base.shutdown().await;
        g
    };
    assert_eq!(base_bodies, 2, "baseline: box A + box B");

    // ── (2) Incremental: box A → checkpoint (via save) → add box B → regen from 2 ──
    let wm = spawn_worker(bin.clone()).await;
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ckpt.onecad");
    let mut rt = runtime_over(&wm);
    build_prefix(&mut rt);
    let report = regen(&mut rt, 0).await; // publish box A at head (step 1)
    let _ = published(&report, "box A");

    // Save → mints a checkpoint of the head (step 1) into the cache + the container.
    rt.take_checkpoint_at_head().await;
    assert_eq!(
        rt.checkpoint_count(),
        1,
        "a checkpoint was taken at the head"
    );
    rt.save(&path, save_meta()).expect("save with checkpoint");

    // Add box B (steps 2,3) and regen from step 2 — the checkpoint at step 1 is
    // at/below the dirty floor, so the planner accelerates the base.
    build_suffix(&mut rt);
    let inc_report = regen(&mut rt, 2).await;
    assert!(
        rt.last_regen_used_checkpoint(),
        "the incremental regen selected the step-1 checkpoint (RestoreCheckpoint path)"
    );
    let (inc_sig, inc_bodies) = head_geometry(&inc_report, "incremental");

    // ── (3) The determinism cross-check: incremental == forced from-0 ──────────
    assert_eq!(inc_bodies, base_bodies, "incremental body count == from-0");
    assert_eq!(
        inc_sig, base_sig,
        "incremental geometry signature IDENTICAL to the from-0 replay (RestoreCheckpoint + \
         incremental plan produce the same head)"
    );

    // ── (4) Persistence: reopen the container → the checkpoint reloads ─────────
    let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
    let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
    let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
    let reopened = DocumentRuntime::open(&path, engine, meshes, solver).expect("reopen container");
    assert_eq!(
        reopened.checkpoint_count(),
        1,
        "the persisted checkpoint reloaded from the .onecad container"
    );

    wm.shutdown().await;
    eprintln!(
        "checkpoint PASS: incremental sig {inc_sig} == from-0 {base_sig}, {inc_bodies} bodies, \
         checkpoint persisted + reloaded"
    );
}

fn save_meta() -> SaveMeta {
    SaveMeta {
        app_version: "0.1.0-test".into(),
        occt_fingerprint: None,
        created: "2026-07-19T00:00:00Z".into(),
        modified: "2026-07-19T00:00:00Z".into(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// M5b drill 3a: worker restart with a checkpoint present → from-0 replay converges
// ─────────────────────────────────────────────────────────────────────────────

/// A checkpoint exists in the Rust cache when the worker is killed. The
/// [`WorkerManager`] restarts it (fresh process — its in-session checkpoint map is
/// now empty, `Session.h`), and the restart-hook replay (SCHEMA §8: from-0)
/// converges to the correct head. Per **Invariant 7** the from-0 replay does **not**
/// consult the now-worker-less checkpoint — correctness never depends on the cache —
/// yet the head is exactly right.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn checkpoint_present_worker_restart_replays_and_converges() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = spawn_worker(bin.clone()).await;
    let mut rt = runtime_over(&wm);

    // Box A → regen → checkpoint at head (step 1): the checkpoint now lives in the
    // Rust cache AND the worker's in-session map.
    build_prefix(&mut rt);
    let r = regen(&mut rt, 0).await;
    let _ = published(&r, "box A");
    rt.take_checkpoint_at_head().await;
    assert_eq!(
        rt.checkpoint_count(),
        1,
        "checkpoint minted before the crash"
    );

    // Append box B — the head the post-restart replay must converge to.
    build_suffix(&mut rt);

    // Kill the worker: a graceful Shutdown makes the child exit; the supervisor
    // detects the death and restarts it (a brand-new process). The Rust checkpoint
    // cache survives; the worker's in-session checkpoint does not.
    let epoch_before = wm.epoch().0;
    wm.shutdown().await;
    let mut restarted = false;
    for _ in 0..200 {
        if wm.epoch().0 > epoch_before && wm.state() == WorkerState::Ready {
            restarted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        restarted,
        "worker restarted after the kill (epoch bumped + Ready)"
    );

    // Exactly what the WorkerManager restart hook does (SCHEMA §8 crash → restart +
    // replay): adopt the new epoch, then replay from 0.
    rt.on_worker_restart(wm.epoch());
    let replay = regen(&mut rt, 0).await;

    // Invariant 7: the from-0 restart replay rebuilds from the EMPTY base and never
    // touches the stale checkpoint, yet converges to the correct head.
    assert!(
        !rt.last_regen_used_checkpoint(),
        "the from-0 restart replay bypasses the stale checkpoint (Invariant 7)"
    );
    let snap = published(&replay, "post-restart replay");
    assert_eq!(
        snap.bodies.len(),
        2,
        "converged to the correct head (box A + box B) after the restart"
    );
    assert_eq!(
        rt.checkpoint_count(),
        1,
        "the Rust checkpoint cache survived the worker restart"
    );

    wm.shutdown().await;
    eprintln!(
        "restart drill PASS: from-0 replay converged to 2 bodies with a stale checkpoint present"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// M5b deliverable 4: signature-drift degradation — the planner's compatibility
// gate skips a stale checkpoint and replays from 0 (graceful, never an error).
// ─────────────────────────────────────────────────────────────────────────────

/// Doctors a persisted checkpoint's `envelope.descriptor_version` (a plan-context
/// compatibility axis that is NOT re-adopted on load, unlike the OCCT fingerprint)
/// to a mismatched value, keeping the manifest hash consistent so the checkpoint
/// still **loads** (integrity intact) but the planner's
/// [`is_compatible`](onecad_core::regen::CheckpointEnvelope) gate **rejects** it.
/// Reopening then regenerating must ignore the stale checkpoint (from-0 replay),
/// succeed, and reproduce the correct geometry — Invariant 7 at the real seam.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signature_drift_skips_stale_checkpoint_and_replays_from_zero() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };

    // ── Baseline: a from-0 replay of the FULL 4-op doc (own worker) ─────────────
    let (base_sig, base_bodies) = {
        let wm = spawn_worker(bin.clone()).await;
        let mut rt = runtime_over(&wm);
        build_prefix(&mut rt);
        build_suffix(&mut rt);
        let report = regen(&mut rt, 0).await;
        let g = head_geometry(&report, "from-0 baseline");
        wm.shutdown().await;
        g
    };
    assert_eq!(base_bodies, 2, "baseline: box A + box B");

    // ── Build box A → checkpoint at head → save a container WITH the checkpoint ──
    // (the tempdir must outlive the reopen below, so it is held at test scope).
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("drift.onecad");
    {
        let wm = spawn_worker(bin.clone()).await;
        let mut rt = runtime_over(&wm);
        build_prefix(&mut rt);
        let r = regen(&mut rt, 0).await;
        let _ = published(&r, "box A");
        rt.take_checkpoint_at_head().await;
        assert_eq!(rt.checkpoint_count(), 1, "checkpoint minted");
        rt.save(&path, save_meta()).expect("save with checkpoint");
        wm.shutdown().await;
    }

    // ── Doctor the persisted checkpoint's descriptor_version (version drift) ─────
    doctor_checkpoint_descriptor_version(&path, 1, 999_999);

    // ── Reopen the doctored container + regen with box B, from step 2 ───────────
    let wm = spawn_worker(bin.clone()).await;
    let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
    let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
    let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
    let mut rt = DocumentRuntime::open(&path, engine, meshes, solver).expect("reopen doctored");
    assert_eq!(
        rt.checkpoint_count(),
        1,
        "the doctored checkpoint still LOADS (integrity intact) — the gate is version, not hash"
    );
    build_suffix(&mut rt);
    let report = regen(&mut rt, 2).await;

    // The compatibility gate degraded gracefully: the version-drifted checkpoint was
    // SKIPPED (from-0 replay), the regen SUCCEEDED (no error), and the head is right.
    assert!(
        !rt.last_regen_used_checkpoint(),
        "the version-drifted checkpoint was skipped by the compatibility gate, not consumed"
    );
    let (drift_sig, drift_bodies) = head_geometry(&report, "post-drift replay");
    assert_eq!(
        drift_bodies, base_bodies,
        "converged to the correct body count"
    );
    assert_eq!(
        drift_sig, base_sig,
        "the from-0 fallback reproduces the correct geometry signature (graceful, no error)"
    );

    wm.shutdown().await;
    let _ = std::fs::remove_file(&path);
    eprintln!("drift drill PASS: stale-signature checkpoint skipped → from-0 → sig {drift_sig} == baseline");
}

/// Lowercase-hex SHA-256 (the container's content-hash convention).
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut s = String::with_capacity(digest.len() * 2);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Rewrites a `.onecad` container in place, bumping every
/// `checkpoints/<step>.json` artifact's `envelope.descriptor_version` to
/// `new_version` and updating that entry's manifest `sha256` so the container's
/// integrity stays intact — the checkpoint still loads, but the planner's
/// version-compatibility gate rejects it. Every other entry is preserved verbatim.
fn doctor_checkpoint_descriptor_version(path: &Path, step: usize, new_version: u64) {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).unwrap();
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        entries.push((name, buf));
    }
    drop(archive);

    let cp_name = format!("checkpoints/{step}.json");
    let mut new_hash: Option<String> = None;
    for (name, buf) in &mut entries {
        if *name == cp_name {
            let mut v: serde_json::Value = serde_json::from_slice(buf).unwrap();
            for art in v["artifacts"].as_array_mut().expect("artifacts array") {
                art["envelope"]["descriptor_version"] = serde_json::json!(new_version);
            }
            let nb = serde_json::to_vec(&v).unwrap();
            new_hash = Some(sha256_hex(&nb));
            *buf = nb;
        }
    }
    let new_hash = new_hash.unwrap_or_else(|| panic!("no {cp_name} entry in {path:?}"));

    for (name, buf) in &mut entries {
        if name == "manifest.json" {
            let mut m: serde_json::Value = serde_json::from_slice(buf).unwrap();
            for e in m["entries"].as_array_mut().expect("manifest entries") {
                if e["path"] == cp_name {
                    e["sha256"] = serde_json::json!(new_hash);
                }
            }
            *buf = serde_json::to_vec(&m).unwrap();
        }
    }

    let out = std::fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(out);
    for (name, buf) in entries {
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file(&name, opts).unwrap();
        zip.write_all(&buf).unwrap();
    }
    zip.finish().unwrap();
}
