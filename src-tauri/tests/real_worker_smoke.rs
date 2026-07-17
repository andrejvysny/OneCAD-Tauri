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
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord,
};
use onecad_core::document::variables::Scalar;
use onecad_core::history::{DependencyGraph, Timeline};
use onecad_core::ids::{BodyId, DocumentRevision, JobId, RecordId, SnapshotId, WorkerEpoch};
use onecad_core::regen::{
    GeometryEngine, Lod, PlanArtifacts, PlanContext, PlanEvent, PlanRequest, PolicyVersions,
    RegenPlanner, RegenRequest, TessellateSpec,
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
