//! Plan compilation — the pure, deterministic core of regen.
//!
//! [`RegenPlanner::plan`] is a **pure function** of (timeline, dependency graph,
//! available checkpoints, request): same inputs ⇒ byte-identical [`RegenPlan`] and
//! [`HistoryPrefixHash`] (Invariant 5; golden test `planner_is_deterministic`).
//! The plan is a value object the executor converts to a
//! [`PlanRequest`](super::engine::PlanRequest) for the worker.
//!
//! ## History-prefix hash (geometry-relevant wire-op form)
//!
//! [`history_prefix_hash`] fingerprints a timeline prefix as **SHA-256 over the
//! newline-joined canonical wire-op form of each op** (lowercase hex; the empty
//! prefix is the SHA-256 anchor `e3b0c442…`). The canonical form of one op is the
//! `serde_json` string of a `BTreeMap` (sorted keys ⇒ deterministic across
//! runs/platforms) carrying exactly the **geometry-relevant** fields:
//!
//! * `opId` — the op's [`RecordId`](crate::ids::RecordId) string,
//! * `opType` — the operation tag,
//! * `stepIndex` — its position in the from-0 prefix,
//! * `inputs` — the derived [`OperationInputs`] (bodies/sketches/elements),
//! * `params` — the op's typed params,
//! * `determinism` — the reproducibility policy.
//!
//! It **excludes record-level cosmetics** — `name`, record-level `extra`, and the
//! `suppressed` flag — so a **rename (or any cosmetic edit) never invalidates a
//! checkpoint**, while any geometry-affecting edit does (checkpoint staleness
//! detection). Suppression is modeled by *omitting* an op from the executed
//! sequence (see the cumulative prefixes below), not by a flag in the hashed
//! content.
//!
//! Rust is the sole hash authority (X-WP1): the worker treats `expectedBaseHash`
//! and `prefixHashes` as **opaque tokens** it stores/compares/echoes but never
//! recomputes.
//!
//! ### Base vs cumulative prefixes
//!
//! * `expected_base_hash` = [`history_prefix_hash`]`(&records[0..start_step])` —
//!   the fingerprint of the base the worker restores/replays before the first
//!   executed op, and the value a **checkpoint envelope stores** (SCHEMA §7.7): a
//!   checkpoint at step `k` is usable only while its stored hash equals
//!   `history_prefix_hash(&records[0..=k])`, so any edit at/before `k` invalidates
//!   it.
//! * `prefix_hashes[i]` = the running hash **after executing planned op `i`** —
//!   the base lines extended by the canonical lines of `planned_ops[0..=i]`.
//!   Because `planned_ops` already skips `Suppressed` steps, `prefix_hashes` is
//!   indexed by **execution order** (0-based over executed ops), NOT by timeline
//!   step index — a suppressed step leaves a gap in the hashed `stepIndex` values
//!   but no gap in the `prefix_hashes` vector. The worker echoes
//!   `prefix_hashes[last executed]` (or `expected_base_hash` for a base-only
//!   prepare) as `PlanPrepared.historyPrefixHash`; the executor verifies that
//!   opaque echo (X-WP1 item 2 / review F9).

use std::collections::BTreeMap;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::document::record::{DeterminismSettings, Operation, OperationInputs, OperationRecord};
use crate::history::{DependencyGraph, Timeline};
use crate::ids::{DocumentRevision, JobId, WorkerEpoch};

use super::checkpoint::{CheckpointMeta, CheckpointRef};
use super::engine::{PlanArtifacts, PlanRequest, PlannedOp, PolicyVersions};

// ─────────────────────────────────────────────────────────────────────────────
// History-prefix hash
// ─────────────────────────────────────────────────────────────────────────────

/// A deterministic fingerprint of a timeline prefix — SHA-256 over canonical
/// record lines, lowercase hex (SCHEMA §7.2 `expectedBaseHash` / §7.7
/// `historyPrefixHash`). See the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HistoryPrefixHash(pub String);

impl HistoryPrefixHash {
    /// Wraps a hex hash string.
    #[must_use]
    pub fn new(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// The raw hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The fingerprint of the **empty** prefix (SHA-256 of zero bytes) — the
    /// base hash of a replay-from-0 plan.
    #[must_use]
    pub fn empty() -> Self {
        history_prefix_hash(&[])
    }
}

impl std::fmt::Display for HistoryPrefixHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Computes the [`HistoryPrefixHash`] of `records` (a timeline prefix, indexed
/// from timeline step 0).
///
/// Each record is reduced to its **geometry-relevant canonical wire-op form**
/// ([`wire_op_line`]) — cosmetic fields (`name`, record `extra`, `suppressed`) are
/// excluded — joined with `\n` and SHA-256'd. Callers pass a prefix that starts at
/// timeline index 0 so each record's `stepIndex` equals its enumerate position.
#[must_use]
pub fn history_prefix_hash(records: &[OperationRecord]) -> HistoryPrefixHash {
    hash_wire_lines(
        records
            .iter()
            .enumerate()
            .map(|(i, rec)| wire_op_line(i, rec)),
    )
}

/// SHA-256 over `\n`-joined canonical lines, lowercase hex. An empty iterator
/// yields the SHA-256-of-nothing anchor (`e3b0c442…`).
fn hash_wire_lines(lines: impl Iterator<Item = String>) -> HistoryPrefixHash {
    let mut hasher = Sha256::new();
    for (i, line) in lines.enumerate() {
        if i > 0 {
            hasher.update(b"\n");
        }
        hasher.update(line.as_bytes());
    }
    HistoryPrefixHash(hex_lower(&hasher.finalize()))
}

/// The geometry-relevant canonical wire-op form of a full [`OperationRecord`] at
/// timeline `step_index`.
fn wire_op_line(step_index: usize, rec: &OperationRecord) -> String {
    let (op_type, params) = op_type_and_params(&rec.op);
    wire_line(
        &rec.record_id.to_string(),
        op_type,
        step_index,
        &rec.inputs,
        params,
        &rec.determinism,
    )
}

/// The canonical wire-op form of a [`PlannedOp`] (identical output to
/// [`wire_op_line`] for the same underlying op — a `PlannedOp` carries every
/// geometry-relevant field, so `prefix_hashes` need not re-touch the records).
fn planned_op_line(op: &PlannedOp) -> String {
    let (op_type, params) = op_type_and_params(&op.operation);
    wire_line(
        &op.record_id.to_string(),
        op_type,
        op.step_index,
        &op.inputs,
        params,
        &op.determinism,
    )
}

/// Builds the sorted-key (`BTreeMap`) canonical JSON string for one op. Field set
/// is fixed and geometry-relevant; sorted keys make it deterministic.
fn wire_line(
    op_id: &str,
    op_type: Value,
    step_index: usize,
    inputs: &OperationInputs,
    params: Value,
    determinism: &DeterminismSettings,
) -> String {
    let mut m: BTreeMap<&str, Value> = BTreeMap::new();
    m.insert("opId", Value::String(op_id.to_owned()));
    m.insert("opType", op_type);
    m.insert("stepIndex", Value::from(step_index));
    m.insert(
        "inputs",
        serde_json::to_value(inputs).unwrap_or(Value::Null),
    );
    m.insert("params", params);
    m.insert(
        "determinism",
        serde_json::to_value(determinism).unwrap_or(Value::Null),
    );
    serde_json::to_string(&m).unwrap_or_default()
}

/// Splits an [`Operation`] into `(opType, params)` for the canonical form. A
/// Known op contributes its `{opType, params}`; an Opaque frozen node contributes
/// its `opType` plus its remaining raw payload as `params` (lossless — an edit to
/// a frozen node still changes the hash).
fn op_type_and_params(op: &Operation) -> (Value, Value) {
    let op_val = serde_json::to_value(op).unwrap_or(Value::Null);
    match op {
        Operation::Known(_) => {
            let t = op_val.get("opType").cloned().unwrap_or(Value::Null);
            let p = op_val.get("params").cloned().unwrap_or(Value::Null);
            (t, p)
        }
        Operation::Opaque(_) => {
            let mut obj = op_val.as_object().cloned().unwrap_or_default();
            let t = obj.remove("opType").unwrap_or(Value::Null);
            (t, Value::Object(obj))
        }
    }
}

/// The `(expected_base_hash, prefix_hashes)` pair for a plan whose base is
/// `records[0..start_step]` and whose executed slice is `planned_ops` (already
/// suppressed-filtered). `prefix_hashes[i]` is the running hash after
/// `planned_ops[i]`.
fn compute_hashes(
    records: &[OperationRecord],
    start_step: usize,
    planned_ops: &[PlannedOp],
) -> (HistoryPrefixHash, Vec<HistoryPrefixHash>) {
    let mut lines: Vec<String> = records[0..start_step]
        .iter()
        .enumerate()
        .map(|(i, rec)| wire_op_line(i, rec))
        .collect();
    let expected_base_hash = hash_wire_lines(lines.iter().cloned());
    let mut prefix_hashes = Vec::with_capacity(planned_ops.len());
    for op in planned_ops {
        lines.push(planned_op_line(op));
        prefix_hashes.push(hash_wire_lines(lines.iter().cloned()));
    }
    (expected_base_hash, prefix_hashes)
}

/// Lowercase-hex encodes bytes (SCHEMA §2 hash form).
fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

// ─────────────────────────────────────────────────────────────────────────────
// Request / plan value objects
// ─────────────────────────────────────────────────────────────────────────────

/// What regen to compute (V1/V2 §4.1 entrypoints).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegenRequest {
    /// Regenerate up to and including step `k` — the fast preview for a
    /// rollback edit (`RegenToStep(k)`). The edit is at `k`, so `k` is dirty and
    /// must be executed; a checkpoint may accelerate the base up to `k−1`.
    ToStep(usize),
    /// Regenerate `[from, applied_end]` — the commit path (`RegenToEnd(from)`).
    ToEnd { from: usize },
}

/// The context a plan validates checkpoints against (SCHEMA §7.7 / §13 version
/// axes). A checkpoint envelope is only usable when these match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanContext {
    pub policy_versions: PolicyVersions,
    /// The current OCCT fingerprint (governs BREP/checkpoint compatibility).
    pub occt_fingerprint: String,
}

/// An immutable, deterministic regen plan (V1/V2 §4.3). Converted to a
/// [`PlanRequest`] for the worker via [`RegenPlan::into_request`].
#[derive(Debug, Clone, PartialEq)]
pub struct RegenPlan {
    /// First timeline step the worker will **execute** (the base is everything
    /// before it). `= restore.step + 1` when a checkpoint is chosen, else `0`.
    pub start_step: usize,
    /// The inclusive last step the plan targets.
    pub target_step: usize,
    /// The checkpoint (if any) whose restored state is the base
    /// (`None` ⇒ replay-from-0, the naive vertical-slice default).
    pub restore: Option<CheckpointRef>,
    /// The ordered op slice — `records[start_step..=target_step]` in **timeline
    /// order**, with `Suppressed` steps skipped (they keep their `step_index`).
    pub planned_ops: Vec<PlannedOp>,
    /// History-prefix hash of `records[0..start_step]` — the plan's
    /// `expected_base_hash`.
    pub expected_base_hash: HistoryPrefixHash,
    /// Cumulative per-executed-op prefix hashes: `prefix_hashes[i]` is the hash
    /// **after executing `planned_ops[i]`** (X-WP1). Length == `planned_ops.len()`;
    /// indexed by execution order (suppressed steps are absent — see the module
    /// docs). The worker echoes the entry for its last executed op as an opaque
    /// token; the executor verifies the echo.
    pub prefix_hashes: Vec<HistoryPrefixHash>,
}

impl RegenPlan {
    /// True iff there is nothing to execute (empty applied timeline, or a target
    /// that skips no work).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.planned_ops.is_empty()
    }

    /// Converts to a worker [`PlanRequest`], stamping the fencing tokens, policy
    /// versions and requested artifacts (the app/scheduler supplies these).
    #[must_use]
    pub fn into_request(
        self,
        job_id: JobId,
        document_revision: DocumentRevision,
        worker_epoch: WorkerEpoch,
        policy_versions: PolicyVersions,
        artifacts: PlanArtifacts,
    ) -> PlanRequest {
        PlanRequest {
            job_id,
            document_revision,
            worker_epoch,
            expected_base_hash: self.expected_base_hash,
            prefix_hashes: self.prefix_hashes,
            base_checkpoint: self.restore,
            ops: self.planned_ops,
            policy_versions,
            target_step: self.target_step,
            artifacts,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The planner
// ─────────────────────────────────────────────────────────────────────────────

/// Compiles [`RegenPlan`]s. Stateless — [`RegenPlanner::plan`] is a pure function.
#[derive(Debug, Default, Clone, Copy)]
pub struct RegenPlanner;

impl RegenPlanner {
    /// Compiles a plan (V1/V2 §4.3 algorithm, checkpoint-accelerated).
    ///
    /// `graph` is accepted for parity with the V1/V2 pipeline (future
    /// dirty-closure narrowing); the strict-linear timeline prefix is
    /// authoritative for the deterministic op order here, so it is currently
    /// consulted only to keep the signature stable — the linear order IS the
    /// dependency order for a linear timeline.
    ///
    /// Algorithm:
    /// 1. Resolve `requested_start` and `target` from the request, clamped to the
    ///    **applied** prefix `[0, cursor)`.
    /// 2. Choose the highest-step checkpoint that (a) sits at `≤ requested_start −
    ///    1` (a checkpoint at or after the dirty floor is stale), (b) is
    ///    envelope-compatible with `ctx`, and (c) whose stored history-prefix
    ///    hash equals `history_prefix_hash(&records[0..=cp.step])`. None ⇒
    ///    replay-from-0.
    /// 3. `start_step = restore.map(step + 1).unwrap_or(0)`.
    /// 4. `planned_ops = records[start_step..=target]` minus `Suppressed` steps.
    /// 5. `expected_base_hash = history_prefix_hash(&records[0..start_step])`.
    #[must_use]
    pub fn plan(
        timeline: &Timeline,
        graph: &DependencyGraph,
        checkpoints: &[CheckpointMeta],
        request: RegenRequest,
        ctx: &PlanContext,
    ) -> RegenPlan {
        let _ = graph; // reserved (see doc): linear order is authoritative here.
        let records = timeline.records();
        let states = timeline.states();
        let applied = timeline.cursor(); // records[0, applied) are applied.

        // ── (1) requested_start + target, clamped into the applied prefix ──────
        let (requested_start, target) = match request {
            RegenRequest::ToStep(k) => (k, k),
            RegenRequest::ToEnd { from } => (from, applied.saturating_sub(1)),
        };

        // Nothing applied, or a request that starts past the applied end: empty
        // plan (no work). `target` is pinned to the last applied index for a
        // well-formed (start > target) empty plan.
        if applied == 0 || requested_start >= applied {
            let last = applied.saturating_sub(1);
            return RegenPlan {
                start_step: last,
                target_step: last,
                restore: None,
                planned_ops: Vec::new(),
                expected_base_hash: history_prefix_hash(&records[0..applied]),
                prefix_hashes: Vec::new(),
            };
        }
        let target = target.min(applied - 1);

        // ── (2)/(3) checkpoint selection ───────────────────────────────────────
        let ceiling = requested_start.checked_sub(1); // stale at/after the floor.
        let restore_meta =
            ceiling.and_then(|ceil| choose_checkpoint(checkpoints, records, ceil, ctx));
        let start_step = restore_meta.map_or(0, |m| m.step + 1);
        let restore = restore_meta.map(|m| CheckpointRef {
            step_index: m.step,
            checkpoint_id: m.id.clone(),
        });

        // ── (4) op slice, skipping suppressed steps (they keep their index) ────
        let planned_ops: Vec<PlannedOp> = (start_step..=target)
            .filter(|&i| states.get(i) != Some(&crate::history::StepState::Suppressed))
            .filter_map(|i| records.get(i).map(|rec| planned_op(i, rec)))
            .collect();

        // ── (5) base hash + cumulative executed-op prefix hashes ───────────────
        let (expected_base_hash, prefix_hashes) = compute_hashes(records, start_step, &planned_ops);

        RegenPlan {
            start_step,
            target_step: target,
            restore,
            planned_ops,
            expected_base_hash,
            prefix_hashes,
        }
    }

    /// Rebuilds a plan as an equivalent **replay-from-0** plan for the same target
    /// (review F12 / Invariant 7 fallback): `start_step = 0`, no checkpoint, the
    /// full op slice `records[0..=target_step]` minus suppressed steps, and
    /// recomputed hashes. The executor calls this when a checkpoint-based plan
    /// fails at restore/execution **before any step event** (or restore reports
    /// drift), so an unusable cache degrades performance, never correctness.
    #[must_use]
    pub fn without_checkpoint(timeline: &Timeline, target_step: usize) -> RegenPlan {
        let records = timeline.records();
        let states = timeline.states();
        let applied = timeline.cursor();
        if applied == 0 {
            return RegenPlan {
                start_step: 0,
                target_step: 0,
                restore: None,
                planned_ops: Vec::new(),
                expected_base_hash: HistoryPrefixHash::empty(),
                prefix_hashes: Vec::new(),
            };
        }
        let target = target_step.min(applied - 1);
        let planned_ops: Vec<PlannedOp> = (0..=target)
            .filter(|&i| states.get(i) != Some(&crate::history::StepState::Suppressed))
            .filter_map(|i| records.get(i).map(|rec| planned_op(i, rec)))
            .collect();
        let (expected_base_hash, prefix_hashes) = compute_hashes(records, 0, &planned_ops);
        RegenPlan {
            start_step: 0,
            target_step: target,
            restore: None,
            planned_ops,
            expected_base_hash,
            prefix_hashes,
        }
    }
}

/// Builds a [`PlannedOp`] from a record at `step` (input refs verbatim from the
/// record; determinism policy copied).
fn planned_op(step: usize, rec: &OperationRecord) -> PlannedOp {
    PlannedOp {
        record_id: rec.record_id,
        step_index: step,
        operation: rec.op.clone(),
        inputs: rec.inputs.clone(),
        determinism: rec.determinism.clone(),
    }
}

/// Selects the highest-step checkpoint usable as a base: `step ≤ ceiling`,
/// envelope-compatible with `ctx`, and prefix-hash-matching the current records.
/// Returns `None` (⇒ replay-from-0) when no checkpoint qualifies — the
/// correctness-never-depends-on-cache fallback (Invariant 7).
fn choose_checkpoint<'a>(
    checkpoints: &'a [CheckpointMeta],
    records: &[OperationRecord],
    ceiling: usize,
    ctx: &PlanContext,
) -> Option<&'a CheckpointMeta> {
    checkpoints
        .iter()
        .filter(|m| m.step <= ceiling && m.step < records.len())
        .filter(|m| m.envelope.is_compatible(ctx))
        .filter(|m| m.history_prefix_hash == history_prefix_hash(&records[0..=m.step]))
        .max_by_key(|m| m.step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::record::{
        BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord,
    };
    use crate::document::variables::Scalar;
    use crate::ids::RecordId;
    use uuid::Uuid;

    fn extrude(seed: u128, distance: f64) -> OperationRecord {
        let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
            profile: None,
            distance: Scalar::new(distance),
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
        OperationRecord::new(RecordId(Uuid::from_u128(seed)), 0, "Extrude", op)
    }

    fn timeline(n: usize) -> Timeline {
        let mut tl = Timeline::new();
        for i in 0..n {
            tl.insert_at_cursor(extrude(0x10 + i as u128, 5.0 + i as f64));
        }
        tl
    }

    fn ctx() -> PlanContext {
        PlanContext {
            policy_versions: PolicyVersions::default(),
            occt_fingerprint: "fp".into(),
        }
    }

    #[test]
    fn empty_prefix_hash_is_stable() {
        assert_eq!(HistoryPrefixHash::empty(), history_prefix_hash(&[]));
        // SHA-256 of the empty input.
        assert_eq!(
            HistoryPrefixHash::empty().as_str(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hash_is_deterministic_and_prefix_sensitive() {
        let tl = timeline(3);
        let r = tl.records();
        let h2a = history_prefix_hash(&r[0..2]);
        let h2b = history_prefix_hash(&r[0..2]);
        assert_eq!(h2a, h2b, "same prefix → same hash");
        assert_ne!(h2a, history_prefix_hash(&r[0..1]), "prefix length matters");
        assert_ne!(h2a, history_prefix_hash(&r[0..3]));
    }

    #[test]
    fn plan_to_end_from_zero_replays_all() {
        let tl = timeline(3);
        let g = DependencyGraph::new();
        let plan = RegenPlanner::plan(&tl, &g, &[], RegenRequest::ToEnd { from: 0 }, &ctx());
        assert_eq!(plan.start_step, 0);
        assert_eq!(plan.target_step, 2);
        assert!(plan.restore.is_none());
        assert_eq!(plan.planned_ops.len(), 3);
        assert_eq!(plan.expected_base_hash, HistoryPrefixHash::empty());
    }

    #[test]
    fn plan_to_step_targets_k() {
        let tl = timeline(4);
        let g = DependencyGraph::new();
        let plan = RegenPlanner::plan(&tl, &g, &[], RegenRequest::ToStep(2), &ctx());
        assert_eq!(plan.start_step, 0);
        assert_eq!(plan.target_step, 2);
        assert_eq!(plan.planned_ops.len(), 3); // steps 0,1,2
    }

    #[test]
    fn plan_is_pure_same_inputs_same_plan() {
        let tl = timeline(3);
        let g = DependencyGraph::new();
        let a = RegenPlanner::plan(&tl, &g, &[], RegenRequest::ToEnd { from: 0 }, &ctx());
        let b = RegenPlanner::plan(&tl, &g, &[], RegenRequest::ToEnd { from: 0 }, &ctx());
        assert_eq!(a, b);
    }

    #[test]
    fn empty_applied_timeline_yields_empty_plan() {
        let tl = Timeline::new();
        let g = DependencyGraph::new();
        let plan = RegenPlanner::plan(&tl, &g, &[], RegenRequest::ToEnd { from: 0 }, &ctx());
        assert!(plan.is_empty());
    }
}
