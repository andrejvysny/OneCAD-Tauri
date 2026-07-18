//! R-WP11 real-worker smoke test (skip-if-missing).
//!
//! Spawns the **actual** C++ OCCT worker, completes the handshake + `OpenSession`,
//! drives a trivial `ExecutePlan`, and — on a prepared plan — accepts, fetches the
//! mesh (MESH1 validates), then shuts down cleanly. It is gated on the worker
//! binary existing (`ONECAD_WORKER_PATH` override, else the dev-tree path); a
//! missing binary skips the test cleanly, so it never blocks the chaos gate.
//!
//! Fidelity note: the trivial plan here is a bare Extrude. A profile-less extrude
//! may return a graceful `OP_FAILED` on the real worker (recoverable) — that still
//! exercises the whole wire path (handshake → OpenSession → ExecutePlan streaming →
//! terminal parse) without a crash. A full sketch→extrude profile lands with the
//! M2 integration gate, when the binary is present to validate the payload.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord, PlaneKind,
    SketchOpParams, SketchPlaneRef,
};
use onecad_core::document::variables::Scalar;
use onecad_core::history::{DependencyGraph, Timeline};
use onecad_core::ids::{
    BodyId, DocumentRevision, JobId, RecordId, SketchId, SnapshotId, WorkerEpoch,
};
use onecad_core::math::Vec3;
use onecad_core::regen::{
    Fencing, GeometryEngine, Lod, PlanArtifacts, PlanContext, PlanEvent, PlanRequest,
    PolicyVersions, RegenPlanner, RegenRequest, TessellateSpec,
};

use onecad_lib::worker::manager::SupervisorConfig;
use onecad_lib::worker::{resolve_worker_path, MeshProvider, WorkerManager};

fn real_worker() -> Option<PathBuf> {
    // `resolve_worker_path` honors ONECAD_WORKER_PATH → dev fallback.
    resolve_worker_path()
}

fn smoke_plan() -> PlanRequest {
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: None,
        distance: Scalar::new(10.0),
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
    }));
    let mut tl = Timeline::new();
    tl.insert_at_cursor(OperationRecord::new(
        RecordId(Uuid::from_u128(0x5e)),
        0,
        "Extrude",
        op,
    ));
    let ctx = PlanContext {
        policy_versions: PolicyVersions::default(),
        occt_fingerprint: String::new(),
    };
    RegenPlanner::plan(
        &tl,
        &DependencyGraph::new(),
        &[],
        RegenRequest::ToEnd { from: 0 },
        &ctx,
    )
    .into_request(
        JobId(Uuid::from_u128(1)),
        DocumentRevision(0),
        WorkerEpoch(1),
        PolicyVersions::default(),
        PlanArtifacts {
            tessellate: Some(TessellateSpec {
                lod: Lod::Coarse,
                include_edges: true,
            }),
        },
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_worker_handshake_execute_tessellate_shutdown() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: real worker binary not found (set ONECAD_WORKER_PATH)");
        return;
    };

    // auto_open_session on: the supervisor OpenSessions after the handshake.
    let config = SupervisorConfig::production(bin);
    let wm = WorkerManager::spawn(config);
    assert!(
        wm.wait_ready(Duration::from_secs(10)).await,
        "real worker must connect + handshake + OpenSession"
    );

    // Handshake surfaced the fingerprint policy data (SCHEMA §6).
    let hello = wm.hello().expect("hello result");
    assert_eq!(hello.protocol_version, 1);
    assert!(
        !hello.occt.fingerprint.is_empty(),
        "occt fingerprint present"
    );

    // GetWorkerHead reconciliation probe (no side effects).
    let _ = wm.get_worker_head().await.expect("worker head");

    // Drive a trivial ExecutePlan end-to-end.
    let plan = smoke_plan();
    let known: HashSet<Uuid> = plan.ops.iter().map(|o| o.record_id.as_uuid()).collect();
    let job = plan.job_id;
    let mut rx = wm.execute_plan(plan).await;
    let mut prepared = None;
    let mut hard_failed = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            PlanEvent::Step(_) => {}
            PlanEvent::Prepared(p) => prepared = Some(p),
            PlanEvent::Failed(e) => hard_failed = Some(e),
        }
    }

    if let Some(p) = prepared {
        // Every created body must be adoptable (`body_<opId>` ∈ the plan).
        for r in &p.per_step {
            for b in &r.body_ids {
                assert!(
                    known.contains(&b.as_uuid()),
                    "created body adopts a known opId"
                );
            }
        }
        wm.accept_prepared(
            job,
            onecad_core::regen::Fencing {
                document_revision: DocumentRevision(0),
                worker_epoch: WorkerEpoch(1),
            },
        )
        .await
        .expect("accept the prepared snapshot");

        if let Some(first) = p.per_step.iter().flat_map(|r| &r.body_ids).next().copied() {
            match wm
                .fetch_mesh(first, Lod::Coarse, SnapshotId(p.prepared_snapshot_id.0))
                .await
            {
                Ok(mesh) => assert!(
                    onecad_protocol::mesh::validate_mesh_blob(&mesh).is_ok(),
                    "MESH1 blob validates"
                ),
                Err(e) => eprintln!("tessellate note (profile-less extrude): {e}"),
            }
        }
    } else {
        // A recoverable OP_FAILED is acceptable for the profile-less smoke plan;
        // a Crashed/Protocol failure is a real regression.
        let err = hard_failed.expect("a terminal event");
        assert!(
            matches!(err, onecad_core::regen::EngineError::OpFailed { .. }),
            "profile-less extrude should fail recoverably, not crash: {err}"
        );
    }

    let _ = BodyId(Uuid::nil()); // keep the import if the prepared branch is skipped.
    wm.shutdown().await;
}

/// R-WP11.2 D5 reproduction against the REAL worker: two sequential *replay-from-0*
/// ExecutePlan/AcceptPrepared cycles — exactly the shape the `RegenPlanner` emits
/// (V1 has no checkpoint plumbing, so EVERY regen is a full replay with an
/// empty-anchor `expectedBaseHash`). After cycle 1's accept the worker head token is
/// nonzero, so a strict head-hash fence would reject cycle 2's empty-anchor plan (the
/// sequential-regen blocker). Under D5 a from-0 plan is ALWAYS base-valid, so cycle 2
/// (MORE ops + a HIGHER `documentRevision` + the empty anchor AGAIN) must prepare +
/// accept, and the worker head adopts the new revision.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_worker_sequential_from_zero_regen_two_cycles() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: real worker binary not found (set ONECAD_WORKER_PATH)");
        return;
    };
    let wm = WorkerManager::spawn(SupervisorConfig::production(bin));
    assert!(
        wm.wait_ready(Duration::from_secs(10)).await,
        "real worker must connect + OpenSession"
    );

    let ctx = PlanContext {
        policy_versions: PolicyVersions::default(),
        occt_fingerprint: String::new(),
    };
    // Build the RAW planner output — a from-0 plan with the empty-anchor base (no
    // manual draining/override, unlike the incremental F1 test above).
    let mk = |tl: &Timeline, rev: u64, job: u128| {
        RegenPlanner::plan(
            tl,
            &DependencyGraph::new(),
            &[],
            RegenRequest::ToEnd { from: 0 },
            &ctx,
        )
        .into_request(
            JobId(Uuid::from_u128(job)),
            DocumentRevision(rev),
            WorkerEpoch(1),
            PolicyVersions::default(),
            PlanArtifacts { tessellate: None },
        )
    };

    // Cycle 1 — from-0, one Sketch op, revision 0. Prepare + accept advances the
    // worker head hash past the empty anchor.
    let mut tl1 = Timeline::new();
    tl1.insert_at_cursor(sketch_record(0xf1));
    let plan1 = mk(&tl1, 0, 1);
    assert_eq!(
        plan1.expected_base_hash,
        onecad_core::regen::HistoryPrefixHash::empty(),
        "cycle 1 is a from-0 plan (empty anchor)"
    );
    let p1 = run_plan(&wm, plan1).await.expect("cycle 1 prepares");
    assert_eq!(
        p1.stopped_reason,
        onecad_core::regen::StoppedReason::Completed
    );
    wm.accept_prepared(
        JobId(Uuid::from_u128(1)),
        Fencing {
            document_revision: DocumentRevision(0),
            worker_epoch: WorkerEpoch(1),
        },
    )
    .await
    .expect("accept cycle 1");

    // Cycle 2 — the "edit": append a second Sketch op (MORE ops), bump the revision
    // to 1 (HIGHER), and replay from 0 AGAIN (empty anchor). This is the raw executor
    // shape that the pre-D5 worker rejected once the head had advanced.
    let mut tl2 = Timeline::new();
    tl2.insert_at_cursor(sketch_record(0xf1));
    tl2.insert_at_cursor(sketch_record(0xf2));
    let plan2 = mk(&tl2, 1, 2);
    assert_eq!(
        plan2.expected_base_hash,
        onecad_core::regen::HistoryPrefixHash::empty(),
        "cycle 2 is ALSO a from-0 plan (empty anchor) despite the advanced head"
    );
    assert_eq!(plan2.ops.len(), 2, "cycle 2 carries more ops than cycle 1");
    let p2 = run_plan(&wm, plan2).await.unwrap_or_else(|e| {
        panic!("sequential from-0 regen (cycle 2) must prepare, not be fenced (D5): {e}")
    });
    assert_eq!(
        p2.stopped_reason,
        onecad_core::regen::StoppedReason::Completed
    );
    let accept = wm
        .accept_prepared(
            JobId(Uuid::from_u128(2)),
            Fencing {
                document_revision: DocumentRevision(1),
                worker_epoch: WorkerEpoch(1),
            },
        )
        .await
        .expect("accept cycle 2 (from-0, worker adopts documentRevision 1)");
    assert_eq!(
        accept.document_revision,
        DocumentRevision(1),
        "cycle 2 accept adopts the plan's documentRevision (D4/D5)"
    );

    // Final head reflects cycle 2: revision 1, head hash = cycle 2's last prefix token.
    let head = wm.get_worker_head().await.expect("worker head");
    assert_eq!(
        head.document_revision,
        DocumentRevision(1),
        "final worker head adopts revision 1"
    );
    assert_eq!(
        head.history_prefix_hash, p2.history_prefix_hash,
        "final head hash = cycle 2's echoed history-prefix token"
    );

    wm.shutdown().await;
}

/// A minimal Sketch op — the real worker materializes it into the plan and returns
/// `ok` (advancing the head hash) without any OCCT geometry, so a Sketch-only plan
/// deterministically prepares. Used to advance the worker head across two cycles.
fn sketch_record(seed: u128) -> OperationRecord {
    let plane = SketchPlaneRef {
        kind: PlaneKind::Xy,
        origin: Vec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        // The non-standard OneCAD-CPP XY basis (SCHEMA §7.3 hard invariant).
        x_axis: Vec3 {
            x: 0.0,
            y: 1.0,
            z: 0.0,
        },
        y_axis: Vec3 {
            x: -1.0,
            y: 0.0,
            z: 0.0,
        },
        normal: Vec3 {
            x: 0.0,
            y: 0.0,
            z: 1.0,
        },
        extra: Default::default(),
    };
    let op = Operation::Known(KnownOperation::Sketch(SketchOpParams {
        sketch: SketchId(Uuid::from_u128(seed)),
        plane,
        entities: vec![],
        constraints: vec![],
        extra: Default::default(),
    }));
    OperationRecord::new(RecordId(Uuid::from_u128(seed)), 0, "Sketch", op)
}

/// Runs one `ExecutePlan`, draining events; returns `Ok(prepared)` on a terminal
/// `PlanPrepared` or `Err(engine error)` otherwise.
async fn run_plan(
    wm: &WorkerManager,
    plan: PlanRequest,
) -> Result<onecad_core::regen::PlanPrepared, onecad_core::regen::EngineError> {
    let mut rx = wm.execute_plan(plan).await;
    let mut prepared = None;
    let mut failed = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            PlanEvent::Prepared(p) => prepared = Some(p),
            PlanEvent::Failed(e) => failed = Some(e),
            PlanEvent::Step(_) => {}
        }
    }
    match (prepared, failed) {
        (Some(p), _) => Ok(p),
        (None, Some(e)) => Err(e),
        (None, None) => Err(onecad_core::regen::EngineError::Protocol {
            message: "plan produced no terminal event".into(),
        }),
    }
}

/// R-WP11.1 F1 reproduction against the REAL worker: two sequential
/// ExecutePlan/AcceptPrepared cycles where the SECOND carries a higher
/// `documentRevision` (the Rust-owned edit counter, now ahead of the worker's
/// last-accepted head) and a **nonzero `expectedBaseHash` chain** (`prefixHashes`
/// from cycle 1). Under the pre-D4 worker this was rejected with PROTOCOL_ERROR
/// (every post-edit regen failed); under D4 it must prepare + accept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_worker_post_edit_revision_fencing_two_cycles() {
    let Some(bin) = real_worker() else {
        eprintln!("skip: real worker binary not found (set ONECAD_WORKER_PATH)");
        return;
    };
    // Production config auto-OpenSessions at documentRevision 0 / workerEpoch 1, so
    // the worker head starts at revision 0.
    let wm = WorkerManager::spawn(SupervisorConfig::production(bin));
    assert!(
        wm.wait_ready(Duration::from_secs(10)).await,
        "real worker must connect + OpenSession"
    );

    let ctx = PlanContext {
        policy_versions: PolicyVersions::default(),
        occt_fingerprint: String::new(),
    };
    let mk = |tl: &Timeline, from: usize, rev: u64, job: u128| {
        RegenPlanner::plan(
            tl,
            &DependencyGraph::new(),
            &[],
            RegenRequest::ToEnd { from },
            &ctx,
        )
        .into_request(
            JobId(Uuid::from_u128(job)),
            DocumentRevision(rev),
            WorkerEpoch(1),
            PolicyVersions::default(),
            PlanArtifacts { tessellate: None },
        )
    };

    // Cycle 1 — op0 at revision 0 (expectedBaseHash = empty anchor). Prepare + accept
    // advances the worker head to revision 0, head hash = hash([op0]).
    let mut tl1 = Timeline::new();
    tl1.insert_at_cursor(sketch_record(0xf1));
    let plan1 = mk(&tl1, 0, 0, 1);
    let p1 = run_plan(&wm, plan1)
        .await
        .expect("cycle 1 prepares (Sketch → ok)");
    assert_eq!(
        p1.stopped_reason,
        onecad_core::regen::StoppedReason::Completed
    );
    wm.accept_prepared(
        JobId(Uuid::from_u128(1)),
        Fencing {
            document_revision: DocumentRevision(0),
            worker_epoch: WorkerEpoch(1),
        },
    )
    .await
    .expect("accept cycle 1");

    // Cycle 2 — "edit": append op1, bump documentRevision to 1 (AHEAD of head 0), and
    // chain a **nonzero** expectedBaseHash to cycle 1's echo. V1 has no checkpoint
    // plumbing (the planner would replay from 0 with an empty base), so hand-build the
    // incremental plan the way a checkpoint-accelerated regen would: op1 only,
    // expectedBaseHash = hash([op0]) (the worker's live head). This must NOT be fenced
    // (F1) — it prepares + accepts and the worker adopts revision 1.
    let mut tl2 = Timeline::new();
    tl2.insert_at_cursor(sketch_record(0xf1));
    tl2.insert_at_cursor(sketch_record(0xf2));
    let mut plan2 = mk(&tl2, 0, 1, 2); // from-0 two-op plan
    let h0 = plan2.prefix_hashes[0].clone();
    let h01 = plan2.prefix_hashes[1].clone();
    plan2.ops.drain(0..1);
    plan2.expected_base_hash = h0;
    plan2.prefix_hashes = vec![h01];
    plan2.target_step = 1;
    assert_ne!(
        plan2.expected_base_hash,
        onecad_core::regen::HistoryPrefixHash::empty(),
        "cycle 2 expectedBaseHash chains from cycle 1 (nonzero)"
    );
    let p2 = run_plan(&wm, plan2).await.unwrap_or_else(|e| {
        panic!(
            "post-edit regen (documentRevision 1 > head 0) must prepare, not be fenced (F1): {e}"
        )
    });
    assert_eq!(
        p2.stopped_reason,
        onecad_core::regen::StoppedReason::Completed
    );
    let accept = wm
        .accept_prepared(
            JobId(Uuid::from_u128(2)),
            Fencing {
                document_revision: DocumentRevision(1),
                worker_epoch: WorkerEpoch(1),
            },
        )
        .await
        .expect("accept cycle 2 (worker adopts documentRevision 1)");
    assert_eq!(
        accept.document_revision,
        DocumentRevision(1),
        "the worker head adopts the plan's documentRevision (D4)"
    );

    wm.shutdown().await;
}
