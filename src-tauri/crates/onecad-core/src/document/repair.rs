//! Topological-naming repair state (V1/V2 §3.7 + SCHEMA §9).
//!
//! `NeedsRepair` is a first-class **state**, never an `Err`: a low-confidence or
//! ambiguous (tie) rebind surfaces here rather than silently binding to the
//! wrong element (SCHEMA §9: a false positive is strictly worse than a false
//! negative). Repair = Rust rewrites the `OperationRecord` reference and
//! re-regens — there is no worker `BindRepair` verb.
//!
//! [`RepairState`] stores, **per step** (V1/V2 §3.7): the unresolved refs, their
//! candidate lists + scores, which ladder level failed, and UI-friendly labels.
//! The payload mirrors the SCHEMA §9 `needsRepair` wire shape so a worker
//! `planStep.needsRepair[]` entry maps 1:1 into a [`RepairItem`].

use serde::{Deserialize, Serialize};

use crate::document::refs::{AnchorIntent, Extra};
use crate::ids::ElementId;
use crate::math::Vec3;

/// Which ladder level failed to decide (SCHEMA §9 `ladderFailed`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LadderLevel {
    /// OCCT history gave no / an ambiguous mapping.
    History,
    /// Descriptor + anchor matching was ambiguous / low-confidence.
    Descriptor,
}

/// Why the ladder could not confidently bind (SCHEMA §9 `reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepairReason {
    /// Two or more candidates tie within the policy margin.
    Ambiguous,
    /// No candidate matched the frozen descriptor.
    NoCandidates,
    /// The best candidate scored below the auto-bind threshold.
    LowConfidence,
}

/// One repair candidate returned by the ladder (SCHEMA §9 `candidates[]`).
///
/// `score` is the normalized `[0,1]` versioned confidence and `margin` is
/// `score1 − score2` (SCHEMA §10). `feature_contributions` (SCHEMA
/// `featureContributions`) and any other worker fields round-trip via `extra`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairCandidate {
    /// Snapshot-scoped topology key of the candidate element.
    pub topo_key: crate::ids::TopoKey,
    /// Normalized `[0,1]` confidence.
    pub score: f64,
    /// `score1 − score2` (best minus second-best).
    pub margin: f64,
    /// Candidate world position (for highlighting).
    pub world_pos: Vec3,
    /// Human-readable summary (e.g. `"planar face, area≈120mm²"`).
    pub summary: String,
    /// Unknown keys (e.g. `featureContributions`), preserved verbatim.
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// A single unresolved reference awaiting repair (SCHEMA §9 payload +
/// V1/V2 §3.7 per-step storage).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepairItem {
    /// Timeline step whose regen produced this NeedsRepair.
    pub step_index: usize,
    /// The op-input ref identity (e.g. `"op_5.input0"`).
    pub ref_id: String,
    /// The last-known `ElementId` of the ref, if any (SCHEMA `elementId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub element_id: Option<ElementId>,
    /// Which ladder level failed.
    pub ladder_failed: LadderLevel,
    /// Why binding failed.
    pub reason: RepairReason,
    /// Ranked candidates (sorted by `score` descending; SCHEMA §9).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidates: Vec<RepairCandidate>,
    /// Selection intent captured when the ref was authored (SCHEMA §9 `anchor`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorIntent>,
    /// UI-friendly label (SCHEMA `uiLabel`; V1/V2 §3.7 "UI-friendly labels").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ui_label: String,
}

/// The document's repair state: unresolved refs organized by step (V1/V2 §3.7).
///
/// Stored as a flat, order-stable `Vec<RepairItem>` (each item self-describes
/// its `step_index`); accessors project per-step views and [`clear_from`] drops
/// everything at or after a step (a re-regen from step `k` clears stale repair
/// state for `[k, ∞)`). Serializes transparently as the item array.
///
/// [`clear_from`]: RepairState::clear_from
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RepairState {
    items: Vec<RepairItem>,
}

impl RepairState {
    /// An empty repair state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True iff there are no unresolved refs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Total unresolved-ref count across all steps.
    #[must_use]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// All repair items (order-stable).
    #[must_use]
    pub fn items(&self) -> &[RepairItem] {
        &self.items
    }

    /// The repair items for a single step.
    #[must_use]
    pub fn items_for_step(&self, step: usize) -> Vec<&RepairItem> {
        self.items.iter().filter(|i| i.step_index == step).collect()
    }

    /// True iff any step has unresolved refs (the document `NeedsRepair` badge).
    #[must_use]
    pub fn needs_repair(&self) -> bool {
        !self.items.is_empty()
    }

    /// Replaces the repair items for `step` with `items` (a step re-regen
    /// publishes a fresh NeedsRepair set for that step). Existing items for the
    /// step are dropped first; the result stays sorted by `(step_index, ref_id)`.
    pub fn set_step(&mut self, step: usize, items: Vec<RepairItem>) {
        self.items.retain(|i| i.step_index != step);
        self.items.extend(items);
        self.sort();
    }

    /// Drops all repair items at or after `step` (a re-regen from `step`
    /// invalidates their bindings).
    pub fn clear_from(&mut self, step: usize) {
        self.items.retain(|i| i.step_index < step);
    }

    /// Drops every repair item.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    fn sort(&mut self) {
        self.items.sort_by(|a, b| {
            a.step_index
                .cmp(&b.step_index)
                .then_with(|| a.ref_id.cmp(&b.ref_id))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::TopoKey;

    fn item(step: usize, ref_id: &str) -> RepairItem {
        RepairItem {
            step_index: step,
            ref_id: ref_id.into(),
            element_id: None,
            ladder_failed: LadderLevel::Descriptor,
            reason: RepairReason::Ambiguous,
            candidates: vec![RepairCandidate {
                topo_key: TopoKey::new("f:31"),
                score: 0.91,
                margin: 0.0,
                world_pos: Vec3::new_unchecked(12.0, 3.5, 0.0),
                summary: "planar face".into(),
                extra: Default::default(),
            }],
            anchor: None,
            ui_label: "Fillet edge".into(),
        }
    }

    #[test]
    fn set_step_replaces_and_clear_from_trims() {
        let mut r = RepairState::new();
        r.set_step(2, vec![item(2, "op_2.input0")]);
        r.set_step(5, vec![item(5, "op_5.input0"), item(5, "op_5.input1")]);
        assert_eq!(r.len(), 3);
        assert_eq!(r.items_for_step(5).len(), 2);
        // Re-regen step 5 with a single unresolved ref.
        r.set_step(5, vec![item(5, "op_5.input0")]);
        assert_eq!(r.items_for_step(5).len(), 1);
        // clear_from(5) drops step 5 but keeps step 2.
        r.clear_from(5);
        assert!(r.items_for_step(5).is_empty());
        assert_eq!(r.items_for_step(2).len(), 1);
        assert!(r.needs_repair());
    }

    #[test]
    fn enums_serialize_to_schema_tokens() {
        assert_eq!(
            serde_json::to_value(LadderLevel::History).unwrap(),
            serde_json::json!("history")
        );
        assert_eq!(
            serde_json::to_value(RepairReason::NoCandidates).unwrap(),
            serde_json::json!("no-candidates")
        );
        assert_eq!(
            serde_json::to_value(RepairReason::LowConfidence).unwrap(),
            serde_json::json!("low-confidence")
        );
    }

    #[test]
    fn state_serializes_transparently_as_array() {
        let mut r = RepairState::new();
        r.set_step(1, vec![item(1, "op_1.input0")]);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.is_array());
        let back: RepairState = serde_json::from_value(v).unwrap();
        assert_eq!(r, back);
    }
}
