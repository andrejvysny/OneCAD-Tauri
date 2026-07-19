//! Integration gate for the `onecad-regen` CLI against the **real** OCCT worker.
//!
//! Builds a small `sketch → extrude` document with onecad-core, saves it to a temp
//! `.onecad` container, runs the built `onecad-regen` binary against it
//! (`CARGO_BIN_EXE_onecad-regen`, which cargo sets only for this crate's own
//! integration tests), and asserts the CI-gate contract: **exit 0** + a final
//! geometry-signature line (human mode) and a well-formed `published` object
//! (`--json`).
//!
//! REQUIRE_WORKER-guarded: CI (`ONECAD_REQUIRE_WORKER=1`) hard-fails when no worker
//! resolves; local dev without a worker skips cleanly.

use std::path::PathBuf;
use std::process::Command;

use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord, PlaneKind,
    SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::refs::SketchRegionRef;
use onecad_core::document::variables::Scalar;
use onecad_core::document::Document;
use onecad_core::ids::{ConstraintId, DocumentId, EntityId, RecordId, RegionId, SketchId};
use onecad_core::io::container::{ContainerCaches, ContainerWriter, SaveMeta};
use onecad_core::math::{Vec2, Vec3};
use onecad_core::sketch::{Constraint, Sketch, SketchEntity, WorldPlane};

use onecad_lib::worker::resolve_worker_path;

// ─────────────────────────────────────────────────────────────────────────────
// Worker resolution (mirrors checkpoints.rs / real_worker_smoke.rs)
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

// ─────────────────────────────────────────────────────────────────────────────
// Doc builders (a fully-constrained rectangle + a blind extrude — as checkpoints.rs)
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

fn save_meta() -> SaveMeta {
    SaveMeta {
        app_version: "0.1.0-test".into(),
        occt_fingerprint: None,
        created: "2026-07-19T00:00:00Z".into(),
        modified: "2026-07-19T00:00:00Z".into(),
    }
}

/// Builds a `sketch → extrude` container at `path`.
fn write_box_doc(path: &std::path::Path) {
    let sid = SketchId(Uuid::from_u128(0xA));
    let mut doc = Document::new(DocumentId::new());
    doc.timeline.insert_at_cursor(sketch_record(
        0xA00,
        &rect_sketch(sid, 0x1000, 0.0, 0.0, 40.0, 20.0),
    ));
    doc.timeline
        .insert_at_cursor(extrude_record(0xA01, sid, 25.0));
    ContainerWriter::save(path, &doc, &ContainerCaches::none(), &save_meta())
        .expect("save box container");
}

// ─────────────────────────────────────────────────────────────────────────────
// The gate
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cli_replays_box_and_exits_zero_with_signature() {
    let Some(worker) = real_worker() else {
        eprintln!("skip: no worker binary (set ONECAD_WORKER_PATH)");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("box.onecad");
    write_box_doc(&path);

    // ── Human mode: exit 0 + a body/signature line ──────────────────────────
    let out = Command::new(env!("CARGO_BIN_EXE_onecad-regen"))
        .arg(&path)
        .arg("--worker")
        .arg(&worker)
        .output()
        .expect("run onecad-regen");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "onecad-regen must exit 0 on a clean replay (code {:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        out.status.code(),
    );
    assert!(
        stdout.contains("geometry-signature "),
        "expected a geometry-signature line, got:\n{stdout}"
    );
    assert!(
        stdout.contains("body ") && stdout.contains(" signature "),
        "expected a per-body signature line, got:\n{stdout}"
    );

    // ── JSON mode: a well-formed `published` object with a body + signature ──
    let out = Command::new(env!("CARGO_BIN_EXE_onecad-regen"))
        .arg(&path)
        .arg("--worker")
        .arg(&worker)
        .arg("--json")
        .output()
        .expect("run onecad-regen --json");
    assert!(out.status.success(), "json mode must also exit 0");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json emits a single JSON object");
    assert_eq!(v["outcome"], "published");
    assert_eq!(v["published"], true);
    assert_eq!(v["failedSteps"], 0);
    assert!(
        v["geometrySignature"]
            .as_str()
            .is_some_and(|s| !s.is_empty()),
        "a published replay carries a geometry signature: {v}"
    );
    assert!(
        v["bodies"].as_array().is_some_and(|b| !b.is_empty()),
        "the extrude published at least one body: {v}"
    );
}
