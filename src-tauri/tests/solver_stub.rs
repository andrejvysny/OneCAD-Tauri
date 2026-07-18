//! R-WP12 solver-lane + promotion integration tests over the REAL built
//! `onecad-worker-stub` binary (via [`WorkerManager`] + [`DocumentRuntime`]).
//!
//! Exercises the whole wire path — [`SolverEngine`] `SketchUpsert`/`BeginGesture`/
//! `SolveDrag`/`EndGesture`, latest-wins supersede, and `AcquireElementIds`
//! promotion — against a live child process speaking OCW1, with zero OCCT. Gated on
//! the stub binary existing (built by `cargo test --workspace`); a missing binary
//! skips cleanly.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use onecad_core::document::refs::ElementKind;
use onecad_core::edit::EditCommand;
use onecad_core::ids::{BodyId, EntityId, SketchId, SnapshotId, TopoKey};
use onecad_core::math::Vec2;
use onecad_core::regen::GeometryEngine;
use onecad_core::sketch::{Sketch, SketchEntity, WorldPlane};

use onecad_lib::document_runtime::DocumentRuntime;
use onecad_lib::worker::manager::SupervisorConfig;
use onecad_lib::worker::{MeshProvider, SolverEngine, WorkerManager};

/// The built stub binary, derived from the test executable's target dir.
fn stub_binary() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?; // <target>/debug/deps/<test>
    let debug = exe.parent()?.parent()?; // debug
    let bin = debug.join("onecad-worker-stub");
    bin.exists().then_some(bin)
}

fn fast_config(binary: PathBuf) -> SupervisorConfig {
    SupervisorConfig {
        binary,
        envs: vec![],
        ping_interval: Duration::from_millis(200),
        ping_timeout: Duration::from_millis(500),
        max_missed_pings: 2,
        backoff: vec![Duration::from_millis(10)],
        max_rapid_deaths: 3,
        healthy_threshold: Duration::from_millis(300),
        poison_threshold: 3,
        auto_open_session: false,
    }
}

async fn spawn_ready() -> Option<WorkerManager> {
    let bin = stub_binary()?;
    let wm = WorkerManager::spawn(fast_config(bin));
    assert!(
        wm.wait_ready(Duration::from_secs(5)).await,
        "stub must connect + handshake"
    );
    Some(wm)
}

fn runtime_over(wm: &WorkerManager) -> DocumentRuntime {
    let engine: Arc<dyn GeometryEngine> = Arc::new(wm.clone());
    let meshes: Arc<dyn MeshProvider> = Arc::new(wm.clone());
    let solver: Arc<dyn SolverEngine> = Arc::new(wm.clone());
    DocumentRuntime::new_blank(engine, meshes, solver)
}

/// A single-point sketch (the drag target).
fn sketch_with_point(sid: SketchId, point: EntityId) -> Sketch {
    let mut sk = Sketch::on_world_plane(sid, "Sketch 1", WorldPlane::XY);
    sk.add_entity(SketchEntity::point(
        point,
        Vec2::new_unchecked(0.0, 0.0),
        false,
        false,
    ))
    .unwrap();
    sk
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_sketch_gesture_flow_end_to_end() {
    let Some(wm) = spawn_ready().await else {
        eprintln!("skip: stub binary not built");
        return;
    };
    let mut rt = runtime_over(&wm);
    let sid = SketchId(Uuid::from_u128(0x5c));
    let point = EntityId(Uuid::from_u128(0x100));
    rt.apply(EditCommand::AddSketch {
        sketch: sketch_with_point(sid, point),
    })
    .unwrap();

    // Enter → real dof/status from the stub solver lane.
    let session = rt.enter_sketch(sid).await.expect("enter sketch");
    assert_eq!(session.sketch_id, sid.to_string());
    assert_eq!(session.entities.as_array().map(Vec::len), Some(1));

    // Gesture: begin → drags → pointer-up commits one undo command.
    let g = rt.begin_gesture(sid, point).await.expect("begin gesture");
    assert!(g.ready);
    let d1 = rt.solve_drag([5.0, 0.0]).await.expect("drag 1");
    assert_eq!(d1.status, "success");
    assert_eq!(d1.positions[&point.to_string()], [5.0, 0.0]);
    rt.solve_drag([10.0, 2.0]).await.expect("drag 2");
    let end = rt
        .end_gesture(Some([12.0, 3.0]))
        .await
        .expect("end gesture");
    assert_eq!(
        end.solved_positions[&point.to_string()],
        [12.0, 3.0],
        "final exact position committed"
    );

    // The whole drag is one undo step.
    assert!(rt.undo(), "undo reverts the committed gesture");

    wm.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_solve_drag_latest_wins_supersedes_stale_seq() {
    let Some(wm) = spawn_ready().await else {
        eprintln!("skip: stub binary not built");
        return;
    };
    let sid = SketchId(Uuid::from_u128(0x77));
    let point = EntityId(Uuid::from_u128(0x200));
    let sketch = sketch_with_point(sid, point);
    wm.sketch_upsert(&sketch).await.expect("upsert");
    let rev = 1;
    let gesture = 99;
    wm.begin_gesture(&sid.to_string(), rev, gesture, point, "")
        .await
        .expect("begin");

    // Newest seq 5 solves; a later-arriving stale seq 3 is superseded (latest-wins).
    let newest = wm
        .solve_drag(gesture, 5, point, [5.0, 0.0])
        .await
        .expect("seq 5");
    assert!(!newest.superseded);
    assert_eq!(newest.status, "success");

    let stale = wm
        .solve_drag(gesture, 3, point, [3.0, 0.0])
        .await
        .expect("seq 3");
    assert!(stale.superseded, "a stale seq must supersede (latest-wins)");
    assert!(
        stale.positions.is_empty(),
        "superseded carries no positions"
    );

    wm.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_promote_selection_mints_and_is_stable() {
    let Some(wm) = spawn_ready().await else {
        eprintln!("skip: stub binary not built");
        return;
    };
    let mut rt = runtime_over(&wm);
    let body = BodyId(Uuid::from_u128(0x10));
    let picks = vec![(TopoKey::new("f:22"), None), (TopoKey::new("e:3"), None)];
    let ids = rt
        .promote_selection(SnapshotId(5012), body, picks)
        .await
        .expect("promote");
    assert_eq!(ids.len(), 2);
    assert!(ids[0].element_id.starts_with("el_"), "Rust-minted id");
    assert_eq!(ids[0].kind, "face");
    assert_eq!(ids[1].kind, "edge");

    // Re-pick the same topoKey ⇒ the same id (Invariant 1, Rust-owned identity).
    let again = rt
        .promote_selection(SnapshotId(5012), body, vec![(TopoKey::new("f:22"), None)])
        .await
        .expect("re-promote");
    assert_eq!(again[0].element_id, ids[0].element_id);

    wm.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stub_resolve_refs_passthrough() {
    let Some(wm) = spawn_ready().await else {
        eprintln!("skip: stub binary not built");
        return;
    };
    use onecad_core::document::refs::{AnchorIntent, ElementRef};
    use onecad_core::math::Vec3;
    use onecad_core::regen::{ResolveOutcome, ResolveRef, ResolveRequest};

    let req = ResolveRequest {
        snapshot_id: SnapshotId(5012),
        refs: vec![ResolveRef {
            ref_id: "op_5.input0".into(),
            element: ElementRef {
                primary: None,
                intent: None,
                anchor: Some(AnchorIntent {
                    world_point: Vec3::new_unchecked(1.0, 2.0, 3.0),
                    surface_uv: None,
                    local_frame: None,
                    adjacency_hint: None,
                    extra: Default::default(),
                }),
                extra: Default::default(),
            },
        }],
    };
    let res = wm.resolve_refs(req).await.expect("resolve refs");
    assert_eq!(res.len(), 1);
    // The stub auto-binds an unbound ref (dry run — binds nothing).
    assert!(matches!(res[0].outcome, ResolveOutcome::AutoBind { .. }));
    let _ = ElementKind::Face; // keep the import if the promote test is skipped.

    wm.shutdown().await;
}
