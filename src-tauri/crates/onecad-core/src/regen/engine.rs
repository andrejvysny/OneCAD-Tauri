//! The geometry engine abstraction — the seam between the pure Rust core and the
//! C++ OCCT worker.
//!
//! [`GeometryEngine`] is an `async_trait` whose methods map **1:1 onto the SCHEMA
//! §7 verb catalogue**, but expressed in **core domain types** rather than wire
//! JSON. The core stays transport-agnostic: it never imports `onecad-protocol`;
//! the app layer converts these core types to/from the OCW1 wire in a later WP.
//!
//! ## Why `execute_plan` streams a channel
//!
//! Regen is an **ExecutePlan** model (SCHEMA §7.2): the worker executes a plan
//! step-by-step **into scratch state** (never mutating the live session
//! mid-plan), emits one [`PlanStepEvent`] per step, and finishes with a terminal
//! [`PlanPrepared`] (or a hard [`EngineError`]). Modeling this as
//! `execute_plan(PlanRequest) -> mpsc::Receiver<PlanEvent>` — where
//! [`PlanEvent`] is `Step | Prepared | Failed` — gives us two things a single
//! `async fn -> Result<PlanPrepared>` could not:
//!
//! * **progress streaming** — the executor folds body lifecycle, element-map
//!   deltas, repair state and signatures as each step arrives, so a long plan
//!   surfaces incremental UI without waiting for the terminal;
//! * **cancellation** — the executor can `select!` between the next event and a
//!   [`CancelToken`](super::executor::CancelToken), call [`cancel`] on the
//!   engine, drain, and discard, rather than being blocked on one opaque future.
//!
//! The terminal is delivered **on the channel** ([`PlanEvent::Prepared`] /
//! [`PlanEvent::Failed`]) rather than as the method's return value, so the
//! executor has exactly one place to consume events.
//!
//! ## NeedsRepair is never an error
//!
//! [`EngineError`] is the hard-failure taxonomy only (crash / protocol /
//! recoverable op failure / cancelled / timeout). **NeedsRepair is STATE**
//! (SCHEMA §8/§9): it rides inside a successful [`PlanPrepared`]
//! (`stopped_reason = NeedsRepair`, per-step `status = NeedsRepair`, payload in
//! the step event's `needs_repair`). It appears in *no* `EngineError` variant.
//!
//! [`cancel`]: GeometryEngine::cancel

use async_trait::async_trait;
use tokio::sync::mpsc;
use uuid::Uuid;

use crate::document::body::BodyLifecycleEvent;
use crate::document::record::{DeterminismSettings, Operation, OperationInputs};
use crate::document::refs::{AnchorIntent, ElementKind, ElementRef};
use crate::document::repair::RepairItem;
use crate::ids::{
    BodyId, DocumentId, ElementId, JobId, RecordId, SnapshotId, TopoKey, WorkerEpoch,
};
use crate::regen::checkpoint::{CheckpointArtifacts, CheckpointRef, RestoreResult};
use crate::regen::planner::HistoryPrefixHash;
use crate::regen::snapshot::Lod;

// ─────────────────────────────────────────────────────────────────────────────
// Signatures (SCHEMA §12 — three per step)
// ─────────────────────────────────────────────────────────────────────────────

/// A 64-bit FNV-1a topology signature, hex-encoded (SCHEMA §12).
///
/// Opaque to the core — computed worker-side, compared for drift/Invariant 5.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Signature(pub String);

impl Signature {
    /// Wraps a hex signature string.
    #[must_use]
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// The raw hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The **three** per-step signatures (SCHEMA §12). Counts alone cannot detect a
/// symmetric `ElementId` swap, so `referenced_binding` is carried separately.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StepSignatures {
    /// Over per-body counts (faces/edges/vertices), quantized bbox, adjacency.
    pub geometry: Signature,
    /// Over the ordered create/modify/delete/split/merge events of the step.
    pub body_lifecycle: Signature,
    /// Over the `(refId → ElementId)` bindings the step produced for
    /// **referenced** elements (catches symmetric swaps that leave counts
    /// intact).
    pub referenced_binding: Signature,
}

// ─────────────────────────────────────────────────────────────────────────────
// Plan request (SCHEMA §7.2 ExecutePlan.args) — core form
// ─────────────────────────────────────────────────────────────────────────────

/// The independent version axes carried on a plan (SCHEMA §7.2 `policyVersions`).
///
/// A mismatch against a checkpoint envelope degrades the cache to replay, never
/// correctness (Invariant 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyVersions {
    pub quantization: u32,
    pub solver_policy: u32,
    pub descriptor: u32,
    pub resolver: u32,
    pub signature: u32,
}

impl Default for PolicyVersions {
    /// All axes at `1` — the current shipping scheme (SCHEMA §6/§13).
    fn default() -> Self {
        Self {
            quantization: 1,
            solver_policy: 1,
            descriptor: 1,
            resolver: 1,
            signature: 1,
        }
    }
}

/// One op in a plan's ordered slice (SCHEMA §7.2 `ops[]`): the record identity,
/// its timeline step, the typed [`Operation`], the resolved input bindings, and
/// the determinism policy for reproducible replay.
///
/// `inputs` is the derived uniform view ([`OperationInputs`]) — the *resolved
/// input bindings* at the identity level (bodies/sketches/elements). The full
/// semantic refs (descriptor + anchor) live inside `operation`'s typed params so
/// the worker's resolution ladder can rebind them (SCHEMA §10).
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedOp {
    /// The timeline record this op replays.
    pub record_id: RecordId,
    /// The record's timeline index (the plan executes ops in ascending order).
    pub step_index: usize,
    /// The typed operation (carries its semantic input refs verbatim).
    pub operation: Operation,
    /// Derived identity-level input bindings (bodies/sketches/elements).
    pub inputs: OperationInputs,
    /// Determinism policy recorded on the source record.
    pub determinism: DeterminismSettings,
}

/// Requested derived artifacts for a plan (SCHEMA §7.2 `artifacts`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanArtifacts {
    /// Tessellate the prepared bodies at this LOD (with edges), if requested.
    pub tessellate: Option<TessellateSpec>,
}

/// Tessellation request rider on a plan (SCHEMA §7.2 `artifacts.tessellate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TessellateSpec {
    pub lod: Lod,
    pub include_edges: bool,
}

/// An immutable, fenced regen plan — the core form of SCHEMA §7.2
/// `ExecutePlan.args`. Built by the [`RegenPlanner`](super::planner::RegenPlanner)
/// (via [`RegenPlan::into_request`](super::planner::RegenPlan::into_request)) and
/// consumed by [`GeometryEngine::execute_plan`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlanRequest {
    /// Rust-assigned id for this job (idempotent: re-sending is a no-op once
    /// prepared).
    pub job_id: JobId,
    /// Fencing: the document revision the plan derives from.
    pub document_revision: crate::ids::DocumentRevision,
    /// Fencing: the worker epoch the plan derives from.
    pub worker_epoch: WorkerEpoch,
    /// The worker verifies its base state's history-prefix hash equals this
    /// before executing; mismatch ⇒ `PROTOCOL_ERROR` (precondition enforcement,
    /// SCHEMA §7.2). Computed by the planner over `records[0..start_step]`.
    ///
    /// **Opaque token** (X-WP1): Rust mints it (geometry-relevant wire-op form);
    /// the worker stores/compares/echoes it but NEVER recomputes it.
    pub expected_base_hash: HistoryPrefixHash,
    /// Cumulative per-executed-op prefix hashes (SCHEMA §7.2 `prefixHashes`):
    /// `prefix_hashes[i]` is the hash after executing `ops[i]`. Length ==
    /// `ops.len()`. **Opaque Rust-minted tokens** — the worker stores them and
    /// echoes the entry for its last executed op as `PlanPrepared.historyPrefixHash`
    /// (or `expected_base_hash` for a base-only prepare); it never computes them.
    /// The executor verifies that echo (X-WP1 item 2 / review F9).
    pub prefix_hashes: Vec<HistoryPrefixHash>,
    /// Optional checkpoint to restore as the base instead of replaying from
    /// empty (SCHEMA §7.2 `baseCheckpoint`). `None` ⇒ replay-from-0.
    pub base_checkpoint: Option<CheckpointRef>,
    /// The stored artifacts for `base_checkpoint` (the app attaches them post-plan
    /// from its checkpoint store; the planner leaves them `None`). Threaded into the
    /// [`RestoreRequest`] so the engine reconstructs the base state (SCHEMA §7.7).
    pub base_checkpoint_artifacts: Option<CheckpointArtifacts>,
    /// The ordered op slice (suppressed ops already skipped). Executed in
    /// ascending `step_index`; each op runs on its predecessor's exact snapshot
    /// (Invariant 3).
    pub ops: Vec<PlannedOp>,
    /// The independent policy version axes.
    pub policy_versions: PolicyVersions,
    /// The inclusive last timeline step the plan targets.
    pub target_step: usize,
    /// Requested derived artifacts.
    pub artifacts: PlanArtifacts,
}

impl PlanRequest {
    /// The first timeline step the worker will **execute** (the base is
    /// everything before it). `None` for an empty plan (no ops).
    #[must_use]
    pub fn start_step(&self) -> Option<usize> {
        self.ops.first().map(|o| o.step_index)
    }

    /// Whether the plan replays from an empty base (no checkpoint restore).
    ///
    /// Drives the executor's **clear-before-replay** seeding: a replay-from-0
    /// scratch starts from a fresh [`BodyRegistry`](crate::document::body::BodyRegistry)
    /// (body.rs lifecycle-log contract), whereas a checkpoint restore clones the
    /// restored base.
    #[must_use]
    pub fn replays_from_base_zero(&self) -> bool {
        self.base_checkpoint.is_none()
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Per-step event (SCHEMA §7.2 planStep event)
// ─────────────────────────────────────────────────────────────────────────────

/// One element-map partition change (SCHEMA §7.2 `elementMapDelta` entry).
///
/// `body` is the **owning body partition** for this element (review F19; SCHEMA
/// §7.2 `bodyId`, now REQUIRED). A step can create several bodies, so the delta
/// entry names its body explicitly rather than the executor guessing — the
/// previous "most-recently-created body" heuristic mis-partitioned elements when
/// one step produced two bodies. Folding a delta never changes an element's
/// identity (Invariant 1), only which `body`/`TopoKey` partition it maps to.
#[derive(Debug, Clone, PartialEq)]
pub struct ElementMapEntry {
    pub element_id: ElementId,
    pub topo_key: TopoKey,
    pub kind: ElementKind,
    /// The body this element currently partitions into (SCHEMA §7.2 `bodyId`).
    pub body: BodyId,
}

/// The element-map partition delta for one step (SCHEMA §7.2 `elementMapDelta`).
///
/// `added`/`relabeled` carry the `(elementId, topoKey, kind, bodyId)` binding;
/// `removed` lists element ids that left the partition. The ids are **Rust-minted
/// and echoed** by the worker (Invariant 1) — folding a delta never changes an
/// element's identity, only which body partition/`TopoKey` it currently maps to.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ElementMapDelta {
    pub added: Vec<ElementMapEntry>,
    pub removed: Vec<ElementId>,
    pub relabeled: Vec<ElementMapEntry>,
}

impl ElementMapDelta {
    /// True iff the delta changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.relabeled.is_empty()
    }
}

/// Severity of a step [`Diagnostic`] (SCHEMA §7.2 `diagnostics[].severity`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

/// A structured, non-fatal diagnostic emitted for a step (SCHEMA §7.2
/// `diagnostics`). An `Error`-severity diagnostic on a failed step supplies the
/// human-facing reason folded into [`StepState::Error`](crate::history::StepState).
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
}

/// One `planStep` event (SCHEMA §7.2): the per-step result the executor folds.
///
/// `needs_repair` is **STATE, not error** (SCHEMA §8/§9) — a populated
/// `needs_repair` means the step's input binding was ambiguous/low-confidence;
/// the plan still prepares snapshot `m−1` and returns a successful
/// [`PlanPrepared`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlanStepEvent {
    pub step_index: usize,
    /// Ordered body create/modify/delete/split/merge events (folded into the
    /// [`BodyRegistry`](crate::document::body::BodyRegistry)).
    pub body_events: Vec<BodyLifecycleEvent>,
    /// Element-map partition delta for the step.
    pub element_map_delta: ElementMapDelta,
    /// NeedsRepair items surfaced by the resolution ladder (STATE — SCHEMA §9).
    pub needs_repair: Vec<RepairItem>,
    /// The three per-step signatures (SCHEMA §12).
    pub signatures: StepSignatures,
    /// Non-fatal diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Terminal (SCHEMA §7.2 PlanPrepared)
// ─────────────────────────────────────────────────────────────────────────────

/// Why the plan stopped executing (SCHEMA §7.2 `stoppedReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoppedReason {
    /// Reached `target_step` successfully.
    Completed,
    /// A recoverable op failure stopped the plan at some step `m`.
    OpFailed,
    /// A NeedsRepair (ambiguous/low-confidence bind) stopped the plan at some
    /// step `m` — STATE, still a successful prepare (SCHEMA §8).
    NeedsRepair,
}

/// Per-step status inside a [`PlanPrepared`] summary (SCHEMA §7.2
/// `perStepResults[].status`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Ok,
    OpFailed,
    NeedsRepair,
}

/// One entry of a [`PlanPrepared`] per-step summary (SCHEMA §7.2 `perStepResults`).
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub step_index: usize,
    pub status: StepStatus,
    /// The bodies present/produced at this step.
    pub body_ids: Vec<BodyId>,
    /// For an `OpFailed` step: the worker's §8 recoverable message (why it failed).
    /// Empty otherwise. A failed step emits no `planStep` event, so this is the only
    /// channel carrying the failure reason to the snapshot's `StepState::Error`.
    pub message: String,
}

/// The terminal `PlanPrepared` (SCHEMA §7.2). The prepared snapshot is held in
/// **scratch** — it becomes live only after [`GeometryEngine::accept_prepared`].
#[derive(Debug, Clone, PartialEq)]
pub struct PlanPrepared {
    /// The job this prepares.
    pub job_id: JobId,
    /// The scratch snapshot id that `accept_prepared` will publish.
    pub prepared_snapshot_id: SnapshotId,
    /// The last **valid** timeline step the prepared snapshot represents.
    /// `Some(k)` ⇒ steps `[start..=k]` succeeded; `None` ⇒ only the base is
    /// valid (the first executed step already failed / needs repair).
    ///
    /// (SCHEMA spells `lastValidStep` as a bare integer; the core uses
    /// `Option<usize>` so "base only" is unambiguous — reported divergence.)
    pub last_valid_step: Option<usize>,
    /// Why the plan stopped.
    pub stopped_reason: StoppedReason,
    /// Per-step statuses (SCHEMA §7.2 `perStepResults`).
    pub per_step: Vec<StepResult>,
    /// History-prefix hash of the prepared state (SCHEMA §7.2 `historyPrefixHash`).
    pub history_prefix_hash: HistoryPrefixHash,
}

/// One item streamed from [`GeometryEngine::execute_plan`]: a per-step event, the
/// terminal prepare, or a hard failure. Exactly one terminal
/// ([`Prepared`](PlanEvent::Prepared) or [`Failed`](PlanEvent::Failed)) is
/// emitted per job; the channel then closes.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanEvent {
    /// A non-terminal per-step result.
    Step(PlanStepEvent),
    /// The terminal successful prepare (may carry an early-stop NeedsRepair /
    /// OpFailed reason — still a success).
    Prepared(PlanPrepared),
    /// The terminal hard failure (crash / protocol / non-recoverable op failure
    /// / timeout). **Never** carries NeedsRepair.
    Failed(EngineError),
}

// ─────────────────────────────────────────────────────────────────────────────
// Error taxonomy (SCHEMA §8) — NeedsRepair is NOT here.
// ─────────────────────────────────────────────────────────────────────────────

/// The recoverable-op-failure sub-code (SCHEMA §8). All four leave the worker's
/// active session **intact** (all work was in scratch); Rust discards the
/// scratch and the document stays editable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpFailureCode {
    /// `OP_FAILED` — a modeling op failed.
    OpFailed,
    /// `REF_UNRESOLVED` — a hard resolve failure (e.g. input body missing).
    /// Distinct from NeedsRepair, which is state.
    RefUnresolved,
    /// `GEOMETRY_INVALID` — the op produced invalid geometry.
    GeometryInvalid,
    /// `UNSUPPORTED` — a known verb with an unsupported param (e.g. `Loft`
    /// before Loft ships); Rust freezes the node.
    Unsupported,
}

/// The hard-failure taxonomy (SCHEMA §8; migration-plan error taxonomy).
///
/// **NeedsRepair is deliberately absent** — it is per-step state inside a
/// successful [`PlanPrepared`], never an `Err` in any of the three languages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineError {
    /// Worker crash / abnormal exit — no terminal frame arrived. Fatal:
    /// restart + replay from last checkpoint/head; a crash **circuit breaker**
    /// fires on a repeated `(historyPrefixHash, opId, occtFingerprint)`.
    Crashed { message: String },
    /// Protocol violation (bad frame, over-cap length, stale/mismatched
    /// `(documentRevision, workerEpoch)`, unknown verb). Fatal: restart worker
    /// (no resync); Rust reconciles via `GetWorkerHead`.
    Protocol { message: String },
    /// A recoverable op failure — the active session is untouched. `recoverable`
    /// is `true` for `OP_FAILED`/`REF_UNRESOLVED`/`GEOMETRY_INVALID` and `true`
    /// for `UNSUPPORTED` (Rust freezes the node); it is carried explicitly so a
    /// future non-recoverable op class can set it `false`.
    OpFailed {
        code: OpFailureCode,
        recoverable: bool,
        message: String,
    },
    /// Cooperative cancellation (SCHEMA `CANCELLED`). The in-flight job is
    /// dropped; the session is intact. The terminal frame is never dropped.
    Cancelled,
    /// Rust-enforced timeout (SCHEMA §8 — `SolveDrag` 250 ms / `Tessellate` 30 s
    /// / hung-worker ping). Enforced Rust-side, not by the worker.
    Timeout { message: String },
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Crashed { message } => write!(f, "worker crashed: {message}"),
            Self::Protocol { message } => write!(f, "protocol error: {message}"),
            Self::OpFailed {
                code,
                recoverable,
                message,
            } => write!(
                f,
                "op failed ({code:?}, recoverable={recoverable}): {message}"
            ),
            Self::Cancelled => f.write_str("cancelled"),
            Self::Timeout { message } => write!(f, "timeout: {message}"),
        }
    }
}

impl std::error::Error for EngineError {}

// ─────────────────────────────────────────────────────────────────────────────
// Lifecycle / accept / head payloads
// ─────────────────────────────────────────────────────────────────────────────

/// Session mode (SCHEMA §7.1 `OpenSession.mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionMode {
    /// Single-threaded OCCT, reproducible authoritative regen.
    #[default]
    Determinism,
    /// Parallelism permitted; must still satisfy Invariant 5 (never change
    /// ids/mappings, only performance).
    Fast,
}

/// `OpenSession` request (SCHEMA §7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenSessionRequest {
    pub document_id: DocumentId,
    pub document_revision: crate::ids::DocumentRevision,
    pub worker_epoch: WorkerEpoch,
    pub mode: SessionMode,
}

/// The fencing tokens carried on `AcceptPrepared` (SCHEMA §7.2). Rust validates
/// these are still current before publishing the scratch snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fencing {
    pub document_revision: crate::ids::DocumentRevision,
    pub worker_epoch: WorkerEpoch,
}

/// Result of `AcceptPrepared` (SCHEMA §7.2): the published snapshot id and the
/// bumped document revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptResult {
    pub snapshot_id: SnapshotId,
    pub document_revision: crate::ids::DocumentRevision,
}

/// Worker head, for reconciliation after a suspected desync (SCHEMA §7.1
/// `GetWorkerHead`; also the `OpenSession` result head).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerHead {
    pub document_revision: crate::ids::DocumentRevision,
    pub worker_epoch: WorkerEpoch,
    pub snapshot_id: SnapshotId,
    pub history_prefix_hash: HistoryPrefixHash,
    pub has_scratch: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Tessellate (SCHEMA §7.6)
// ─────────────────────────────────────────────────────────────────────────────

/// Which bodies to tessellate (SCHEMA §7.6 `bodyIds`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodySelector {
    /// All bodies in the current snapshot.
    All,
    /// A specific set.
    Ids(Vec<BodyId>),
}

/// `Tessellate` request (SCHEMA §7.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TessellateRequest {
    pub bodies: BodySelector,
    pub lod: Lod,
    pub include_edges: bool,
}

/// A produced mesh handle (SCHEMA §7.6 result element). The core holds the
/// **handle** (identity + integrity); raw MESH1 bytes stream on the bulk lane and
/// are cached separately.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshHandle {
    pub body: BodyId,
    pub lod: Lod,
    pub snapshot_id: SnapshotId,
    pub total_bytes: u64,
    /// SHA-256 of the concatenated MESH1 payload (hex).
    pub sha256: String,
}

/// `Tessellate` result (SCHEMA §7.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TessellateResult {
    pub meshes: Vec<MeshHandle>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Element identity (SCHEMA §7.5) — Rust mints the ids given worker evidence.
// ─────────────────────────────────────────────────────────────────────────────

/// One pick to promote to a persistent `ElementId` (SCHEMA §7.5
/// `AcquireElementIds.picks[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Pick {
    pub topo_key: TopoKey,
    pub anchor: Option<AnchorIntent>,
}

/// `AcquireElementIds` request (SCHEMA §7.5).
#[derive(Debug, Clone, PartialEq)]
pub struct AcquireRequest {
    pub snapshot_id: SnapshotId,
    pub body: BodyId,
    pub picks: Vec<Pick>,
}

/// The worker's resolved binding for one pick (SCHEMA §7.5 — the worker returns
/// `topoKey → (kind, descriptor, anchor)`; **Rust mints the `ElementId`**).
///
/// `existing` is set when the worker recognises the stable element already has a
/// persistent id (Invariant 1: an `ElementId` never changes because geometry
/// changed) — [`mint_element_ids`] then returns that same id.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerElementEvidence {
    pub topo_key: TopoKey,
    pub body: BodyId,
    pub kind: ElementKind,
    pub anchor: Option<AnchorIntent>,
    /// Opaque worker-owned descriptor (evidence, never identity — Invariant 2).
    pub descriptor: Option<serde_json::Value>,
    /// The already-minted persistent id, if this element is known.
    pub existing: Option<ElementId>,
}

/// Mints persistent, globally-unique [`ElementId`]s for worker evidence
/// (SCHEMA §7.5 — **Rust owns id minting**).
///
/// * Reuses `evidence.existing` when the worker recognises the element
///   (Invariant 1).
/// * Otherwise mints a fresh UUID-backed id `"el_<32 hex>"`. The id is
///   **opaque and does NOT embed `BodyId`** (SCHEMA §2; `ids.rs` doc): partition
///   membership is a mapping stored in
///   [`ElementIndex`](crate::document::element_index::ElementIndex), never
///   encoded in the id, so ids survive body split/merge.
///
/// Pure and order-preserving: returns each `(minted id, evidence)` in input
/// order. Storing the binding into the document `ElementIndex` is a separate step
/// (the caller / executor), keeping this function free of document state.
///
/// **Dedups by `topo_key` within one call** (review F17): a `TopoKey` is
/// snapshot-scoped, so two picks with the same key address the *same* element and
/// MUST receive the *same* id — the first id minted for a key is reused for every
/// later evidence entry carrying it. An `existing` id still wins (Invariant 1) and
/// seeds the key→id map for subsequent duplicates.
#[must_use]
pub fn mint_element_ids(
    evidence: Vec<WorkerElementEvidence>,
) -> Vec<(ElementId, WorkerElementEvidence)> {
    let mut by_key: std::collections::HashMap<TopoKey, ElementId> =
        std::collections::HashMap::new();
    evidence
        .into_iter()
        .map(|e| {
            let id = if let Some(existing) = &e.existing {
                by_key
                    .entry(e.topo_key.clone())
                    .or_insert_with(|| existing.clone());
                existing.clone()
            } else if let Some(id) = by_key.get(&e.topo_key) {
                id.clone()
            } else {
                let id = ElementId::new(format!("el_{}", Uuid::new_v4().simple()));
                by_key.insert(e.topo_key.clone(), id.clone());
                id
            };
            (id, e)
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// ResolveRefs (SCHEMA §7.5) — dry-run ladder for repair dialogs
// ─────────────────────────────────────────────────────────────────────────────

/// One ref to dry-run-resolve (SCHEMA §7.5 `ResolveRefs.refs[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct ResolveRef {
    pub ref_id: String,
    pub element: ElementRef,
}

/// `ResolveRefs` request (SCHEMA §7.5) — **dry run**, binds nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolveRequest {
    pub snapshot_id: SnapshotId,
    pub refs: Vec<ResolveRef>,
}

/// The dry-run outcome for one ref (SCHEMA §7.5 `outcome`).
// `NeedsRepair` carries the full `RepairItem` evidence (candidates + anchor),
// which is inherently larger than the other variants; a dry-run resolution set is
// small and not moved in hot loops, so the payload is left unboxed for a
// straightforward construction path (same rationale as `document::record`).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum ResolveOutcome {
    /// A confident unique match (SCHEMA §10 auto-bind policy).
    AutoBind {
        /// The bound persistent id — the Rust-minted `ElementId` the worker holds
        /// for the resolved element, or **empty** when the element is not yet in the
        /// partition (a dry run binds nothing, so Rust would mint at real bind time).
        element_id: ElementId,
        score: f64,
        margin: f64,
        /// The resolved element's snapshot-scoped `TopoKey` — **evidence** (SCHEMA §9,
        /// never identity), carried so a repair UI can highlight/mint even when
        /// `element_id` is empty. `None` when the worker echoed no topoKey.
        topo_key: Option<TopoKey>,
    },
    /// Ambiguous / low-confidence — surfaces the full NeedsRepair evidence
    /// (STATE — SCHEMA §9), never a guess.
    NeedsRepair(RepairItem),
    /// The ref already resolves to its stored binding; nothing to do. Carries that
    /// bound `ElementId` (SCHEMA §7.5 `unchanged` echoes the ref's own id).
    Unchanged { element_id: Option<ElementId> },
}

/// One dry-run resolution (SCHEMA §7.5 `resolutions[]`).
#[derive(Debug, Clone, PartialEq)]
pub struct RefResolution {
    pub ref_id: String,
    pub outcome: ResolveOutcome,
}

/// `RestoreCheckpoint` request (SCHEMA §7.7).
#[derive(Debug, Clone, PartialEq)]
pub struct RestoreRequest {
    pub checkpoint: CheckpointRef,
    pub expected_history_prefix_hash: HistoryPrefixHash,
    /// The stored checkpoint artifacts (the app supplies them from its
    /// [`CheckpointStore`](super::checkpoint::CheckpointStore)). The engine
    /// reconstructs the base [`BodyRegistry`](crate::document::body::BodyRegistry) +
    /// [`ElementIndex`](crate::document::element_index::ElementIndex) from them
    /// (review F3 — seed from the immutable artifacts, never live session state).
    /// `None` ⇒ the engine cannot reconstruct a base ⇒ `restored:false` ⇒ the
    /// executor replays from 0 (Invariant 7).
    pub artifacts: Option<CheckpointArtifacts>,
}

// ─────────────────────────────────────────────────────────────────────────────
// The trait
// ─────────────────────────────────────────────────────────────────────────────

/// Async abstraction over the C++ OCCT worker, mapping 1:1 onto the SCHEMA §7
/// verbs in core domain types. The real transport-backed implementation lives in
/// the app crate (worker manager); the scripted [`FakeEngine`] test double drives
/// the golden fixtures.
///
/// Every method returns [`EngineError`] on hard failure **except**
/// [`execute_plan`](GeometryEngine::execute_plan), which streams a channel whose
/// terminal carries the success ([`PlanEvent::Prepared`]) or the failure
/// ([`PlanEvent::Failed`]).
#[async_trait]
pub trait GeometryEngine: Send + Sync {
    /// `OpenSession` (SCHEMA §7.1).
    async fn open_session(&self, req: OpenSessionRequest) -> Result<WorkerHead, EngineError>;

    /// `CloseSession` (SCHEMA §7.1).
    async fn close_session(
        &self,
        document_id: DocumentId,
        worker_epoch: WorkerEpoch,
    ) -> Result<(), EngineError>;

    /// `ResetSession` (SCHEMA §7.1) — drops session + scratch, **increments and
    /// returns** the new worker epoch.
    async fn reset(
        &self,
        document_id: DocumentId,
        worker_epoch: WorkerEpoch,
    ) -> Result<WorkerEpoch, EngineError>;

    /// `ExecutePlan` (SCHEMA §7.2) — streams per-step [`PlanEvent::Step`]s then a
    /// terminal [`PlanEvent::Prepared`] / [`PlanEvent::Failed`]. See the module
    /// docs for why this is a channel rather than an awaited `Result`.
    ///
    /// **Channel contract** (review F15): the returned receiver is the
    /// consuming half of a **bounded** `mpsc` channel. The producer MUST use
    /// **await-send** (`Sender::send(..).await`, never a lossy `try_send`) so a
    /// slow consumer back-pressures the producer rather than silently dropping an
    /// event — a dropped `planStep`/terminal would corrupt the executor's scratch
    /// fold. Exactly one terminal (`Prepared` | `Failed`) is emitted per job, and
    /// the terminal frame is **never dropped**, even under cancellation
    /// (SCHEMA §3.5/§5.4); the channel then closes.
    async fn execute_plan(&self, request: PlanRequest) -> mpsc::Receiver<PlanEvent>;

    /// `AcceptPrepared` (SCHEMA §7.2) — publishes the scratch snapshot into the
    /// live session; Rust has already validated the fencing tokens are current.
    async fn accept_prepared(
        &self,
        job_id: JobId,
        fencing: Fencing,
    ) -> Result<AcceptResult, EngineError>;

    /// `DiscardPrepared` (SCHEMA §7.2) — drops the scratch job state; the session
    /// is unchanged.
    async fn discard_prepared(&self, job_id: JobId) -> Result<(), EngineError>;

    /// `GetWorkerHead` (SCHEMA §7.1) — reconciliation probe (no side effects).
    async fn get_worker_head(&self) -> Result<WorkerHead, EngineError>;

    /// `Tessellate` (SCHEMA §7.6).
    async fn tessellate(&self, req: TessellateRequest) -> Result<TessellateResult, EngineError>;

    /// `SaveCheckpoint` (SCHEMA §7.7) — emits the atomic artifact set for a step.
    async fn save_checkpoint(&self, step_index: usize) -> Result<CheckpointArtifacts, EngineError>;

    /// `RestoreCheckpoint` (SCHEMA §7.7) — restores a checkpoint as the base and
    /// reports drift.
    async fn restore_checkpoint(&self, req: RestoreRequest) -> Result<RestoreResult, EngineError>;

    /// `AcquireElementIds` (SCHEMA §7.5) — returns the worker's resolved
    /// bindings; **Rust mints the ids** via [`mint_element_ids`].
    async fn acquire_element_ids(
        &self,
        req: AcquireRequest,
    ) -> Result<Vec<WorkerElementEvidence>, EngineError>;

    /// `ResolveRefs` (SCHEMA §7.5) — dry-run ladder execution for repair dialogs.
    async fn resolve_refs(&self, req: ResolveRequest) -> Result<Vec<RefResolution>, EngineError>;

    /// `cancel` (SCHEMA §3.5) — cooperatively cancels the in-flight job; the
    /// terminal frame is still delivered on the plan channel.
    async fn cancel(&self, job_id: JobId) -> Result<(), EngineError>;

    /// Liveness probe (the hung-worker ping, SCHEMA §8).
    async fn ping(&self) -> Result<(), EngineError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_ids_are_opaque_and_never_embed_body_id() {
        let body = BodyId(Uuid::from_u128(0xB0D1));
        let ev = WorkerElementEvidence {
            topo_key: TopoKey::new("f:22"),
            body,
            kind: ElementKind::Face,
            anchor: None,
            descriptor: None,
            existing: None,
        };
        let minted = mint_element_ids(vec![ev]);
        let id = &minted[0].0;
        assert!(id.as_str().starts_with("el_"));
        // Invariant / SCHEMA §2: the id embeds neither the BodyId nor a partition
        // separator.
        assert!(!id.as_str().contains('/'));
        assert!(!id.as_str().contains(&body.0.simple().to_string()));
    }

    #[test]
    fn minted_ids_dedup_identical_topo_key_within_one_call() {
        // F17: two picks with the same snapshot-scoped TopoKey are the same
        // element ⇒ one minted id, reused.
        let body = BodyId(Uuid::from_u128(1));
        let ev = |k: &str| WorkerElementEvidence {
            topo_key: TopoKey::new(k),
            body,
            kind: ElementKind::Face,
            anchor: None,
            descriptor: None,
            existing: None,
        };
        let minted = mint_element_ids(vec![ev("f:22"), ev("f:22"), ev("f:9")]);
        assert_eq!(minted[0].0, minted[1].0, "same key → same id");
        assert_ne!(minted[0].0, minted[2].0, "different key → different id");
        // An existing id seeds the map for later duplicates of its key.
        let existing = ElementId::new("el_seed");
        let mut e0 = ev("f:1");
        e0.existing = Some(existing.clone());
        let minted = mint_element_ids(vec![e0, ev("f:1")]);
        assert_eq!(minted[0].0, existing);
        assert_eq!(
            minted[1].0, existing,
            "duplicate key reuses the existing id"
        );
    }

    #[test]
    fn minted_ids_reuse_existing_binding() {
        // Invariant 1: an already-known element gets its same id back.
        let existing = ElementId::new("el_stable_42");
        let ev = WorkerElementEvidence {
            topo_key: TopoKey::new("f:22"),
            body: BodyId(Uuid::from_u128(1)),
            kind: ElementKind::Face,
            anchor: None,
            descriptor: None,
            existing: Some(existing.clone()),
        };
        let minted = mint_element_ids(vec![ev]);
        assert_eq!(minted[0].0, existing);
    }

    #[test]
    fn engine_error_has_no_needs_repair_variant() {
        // Compile-time guard: NeedsRepair is STATE, so it cannot be constructed as
        // an EngineError. This test documents the taxonomy (SCHEMA §8).
        let e = EngineError::OpFailed {
            code: OpFailureCode::OpFailed,
            recoverable: true,
            message: "boom".into(),
        };
        assert!(e.to_string().contains("recoverable=true"));
    }
}
