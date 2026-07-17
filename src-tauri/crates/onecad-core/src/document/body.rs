//! Body registry and lifecycle (Rust-owned `BodyId` identity).
//!
//! Split/merge change *partition membership*, never element identity. The
//! registry mints and retires `BodyId`s and applies the V1/V2 §2.2 identity
//! rules through [`BodyRegistry::fold`]. A leak tripwire (in a later WP) asserts
//! the registry is empty on document close.
//!
//! **Lifecycle events** ([`BodyLifecycleEvent`]) match the SCHEMA §7.2 `planStep`
//! `bodyEvents` shape (`{kind, bodyId}` / split `{kind, parent, children}` /
//! merge `{kind, inputs, winner}`) and the §12 `bodyLifecycle` signature inputs
//! (create/modify/delete/split/merge). Note: SCHEMA examples spell body ids as
//! `"body_3"` placeholders; here a [`BodyId`] is a Rust-minted UUID that
//! serializes transparently as its UUID string — the JSON *shape* matches, the
//! id *values* are Rust-owned UUIDs (reported divergence).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ids::{BodyId, RecordId};

/// Per-body document metadata (identity + name + visibility + provenance).
///
/// `created_by` is the [`RecordId`] of the op that first produced the body
/// (V1/V2 §2.3 outputs / lifecycle provenance).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyMeta {
    /// Stable body identity.
    pub id: BodyId,
    /// Human-facing name (tree label).
    pub name: String,
    /// Whether the body is shown in the viewport.
    pub visible: bool,
    /// The op that first produced this body.
    pub created_by: RecordId,
}

impl BodyMeta {
    /// A visible body with the given name and producer.
    #[must_use]
    pub fn new(id: BodyId, name: impl Into<String>, created_by: RecordId) -> Self {
        Self {
            id,
            name: name.into(),
            visible: true,
            created_by,
        }
    }
}

/// An ordered body lifecycle event (V1/V2 §2.2 `BodyLifecycleEvent`).
///
/// Serde: internally tagged on `"kind"` (camelCase values
/// `created`/`modified`/`split`/`merged`/`deleted`), matching the SCHEMA §7.2
/// `bodyEvents` shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum BodyLifecycleEvent {
    /// A new body was produced. `{kind:"created", bodyId}`.
    Created {
        /// The produced body.
        #[serde(rename = "bodyId")]
        body: BodyId,
    },
    /// An existing body was modified in place (identity unchanged).
    /// `{kind:"modified", bodyId}`.
    Modified {
        /// The modified body.
        #[serde(rename = "bodyId")]
        body: BodyId,
    },
    /// A body split into several. One child keeps the original `BodyId` — the
    /// designated **first child** per the worker contract (V1/V2 §2.2). The rest
    /// get fresh ids. `{kind:"split", parent, children}`.
    Split {
        /// The body that split.
        parent: BodyId,
        /// Resulting bodies; `children[0]` retains the parent's identity.
        children: Vec<BodyId>,
    },
    /// Several bodies merged into one. The `winner` keeps its `BodyId`; the
    /// others are retired/aliased (V1/V2 §2.2, winner rule = appendix C — see
    /// [`BodyRegistry::merge_winner`]). `{kind:"merged", inputs, winner}`.
    Merged {
        /// The input bodies consumed by the merge.
        inputs: Vec<BodyId>,
        /// The surviving body (keeps its id).
        winner: BodyId,
    },
    /// A body was deleted. `{kind:"deleted", bodyId}`.
    Deleted {
        /// The deleted body.
        #[serde(rename = "bodyId")]
        body: BodyId,
    },
}

impl BodyLifecycleEvent {
    /// The bodies this event references (for signatures / dirty tracking).
    #[must_use]
    pub fn bodies(&self) -> Vec<BodyId> {
        match self {
            Self::Created { body } | Self::Modified { body } | Self::Deleted { body } => {
                vec![*body]
            }
            Self::Split { parent, children } => {
                let mut v = vec![*parent];
                v.extend(children.iter().copied());
                v
            }
            Self::Merged { inputs, winner } => {
                let mut v = inputs.clone();
                v.push(*winner);
                v
            }
        }
    }
}

/// One entry in the lifecycle log: a [`BodyLifecycleEvent`] stamped with the
/// timeline step that produced it (V1/V2 §2.2 "explicit body list" per feature).
///
/// Serialized flat: `{stepIndex, kind, …}` (the event's tag/fields flatten onto
/// the entry).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleEntry {
    /// Timeline step index that emitted the event.
    pub step_index: usize,
    /// The lifecycle event.
    #[serde(flatten)]
    pub event: BodyLifecycleEvent,
}

/// Owns `BodyId` allocation, per-body metadata and the ordered lifecycle log.
///
/// `bodies` is kept in **creation order** (the "creation index" the appendix-C
/// merge winner rule ranks on); serialization preserves that order. Retired
/// bodies leave `bodies` and gain an `aliases` entry (retired → survivor) so a
/// stale reference can be redirected to the surviving body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyRegistry {
    /// Active bodies, in creation order.
    bodies: Vec<BodyMeta>,
    /// Ordered lifecycle log (`Vec` of events + step index).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    log: Vec<LifecycleEntry>,
    /// Retired-body → surviving-body redirections (split/merge losers).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    aliases: BTreeMap<BodyId, BodyId>,
}

impl BodyRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Active body count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// True iff no active bodies (the leak-tripwire condition on close).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    /// Active bodies in creation order.
    #[must_use]
    pub fn bodies(&self) -> &[BodyMeta] {
        &self.bodies
    }

    /// The ordered lifecycle log.
    #[must_use]
    pub fn log(&self) -> &[LifecycleEntry] {
        &self.log
    }

    /// The retired → survivor alias map.
    #[must_use]
    pub fn aliases(&self) -> &BTreeMap<BodyId, BodyId> {
        &self.aliases
    }

    /// Metadata of an active body, if present.
    #[must_use]
    pub fn get(&self, id: BodyId) -> Option<&BodyMeta> {
        self.bodies.iter().find(|b| b.id == id)
    }

    /// True iff an active body has this id.
    #[must_use]
    pub fn contains(&self, id: BodyId) -> bool {
        self.bodies.iter().any(|b| b.id == id)
    }

    /// Creation index (position in creation order) of an active body.
    #[must_use]
    pub fn creation_index(&self, id: BodyId) -> Option<usize> {
        self.bodies.iter().position(|b| b.id == id)
    }

    /// Follows the alias chain to the current surviving body id (identity if the
    /// body is live or unknown). Bounded against cycles.
    #[must_use]
    pub fn resolve(&self, id: BodyId) -> BodyId {
        let mut cur = id;
        let limit = self.aliases.len() + 1;
        for _ in 0..limit {
            match self.aliases.get(&cur) {
                Some(&next) if next != cur => cur = next,
                _ => break,
            }
        }
        cur
    }

    /// Explicitly registers a body (the `AddBody` edit command) with full control
    /// over its metadata. Returns `false` if the id is already active.
    pub fn register(&mut self, meta: BodyMeta) -> bool {
        if self.contains(meta.id) {
            return false;
        }
        self.bodies.push(meta);
        true
    }

    /// Removes a body from the active set (no lifecycle log entry). Returns the
    /// removed metadata, if any.
    pub fn remove(&mut self, id: BodyId) -> Option<BodyMeta> {
        let i = self.bodies.iter().position(|b| b.id == id)?;
        Some(self.bodies.remove(i))
    }

    /// Sets a body's name. Returns `false` if the body is not active.
    pub fn set_name(&mut self, id: BodyId, name: impl Into<String>) -> bool {
        match self.bodies.iter_mut().find(|b| b.id == id) {
            Some(b) => {
                b.name = name.into();
                true
            }
            None => false,
        }
    }

    /// Sets a body's visibility. Returns `false` if the body is not active.
    pub fn set_visible(&mut self, id: BodyId, visible: bool) -> bool {
        match self.bodies.iter_mut().find(|b| b.id == id) {
            Some(b) => {
                b.visible = visible;
                true
            }
            None => false,
        }
    }

    /// The deterministic merge winner (V1/V2 appendix C): **prefer `target`** if
    /// it is among `inputs`, **else the lowest creation index**, **else the
    /// lowest `BodyId`** (UUID order). Returns `None` for an empty input set.
    #[must_use]
    pub fn merge_winner(&self, inputs: &[BodyId], target: Option<BodyId>) -> Option<BodyId> {
        if inputs.is_empty() {
            return None;
        }
        if let Some(t) = target {
            if inputs.contains(&t) {
                return Some(t);
            }
        }
        inputs.iter().copied().min_by(|a, b| {
            let ia = self.creation_index(*a).unwrap_or(usize::MAX);
            let ib = self.creation_index(*b).unwrap_or(usize::MAX);
            ia.cmp(&ib).then_with(|| a.0.cmp(&b.0))
        })
    }

    /// Applies a lifecycle event (V1/V2 §2.2 identity rules), appends it to the
    /// log, and returns the de-duplicated set of body ids whose state changed.
    ///
    /// **Clear-before-replay contract.** `fold` is append-only: it never
    /// de-duplicates the new entry against existing [`log`](Self::log) history. A
    /// full regen that replays the timeline from scratch MUST start from a fresh
    /// [`BodyRegistry::new`] (or otherwise clear the log), or lifecycle entries
    /// accumulate duplicates across replays. Note the *changed* id set returned
    /// here is de-duplicated per call (see `dedup_preserving`) — that is a
    /// per-event dedup only, unrelated to the whole-log clear-before-replay rule.
    ///
    /// * **Created** — registers a fresh visible body (default name) if absent.
    /// * **Modified** — identity unchanged (no membership change).
    /// * **Split** — `children[0]` keeps the parent's identity (the designated
    ///   first child); the parent is retired/aliased to it if their ids differ;
    ///   `children[1..]` become fresh bodies.
    /// * **Merged** — the `winner` survives (registered inheriting an input's
    ///   metadata if absent); the other inputs are retired/aliased to it.
    /// * **Deleted** — retires the body.
    pub fn fold(
        &mut self,
        step_index: usize,
        by: RecordId,
        event: BodyLifecycleEvent,
    ) -> Vec<BodyId> {
        let changed = self.apply_event(by, &event);
        self.log.push(LifecycleEntry { step_index, event });
        dedup_preserving(changed)
    }

    fn apply_event(&mut self, by: RecordId, event: &BodyLifecycleEvent) -> Vec<BodyId> {
        match event {
            BodyLifecycleEvent::Created { body } => {
                self.ensure_fresh(*body, by);
                vec![*body]
            }
            BodyLifecycleEvent::Modified { body } => vec![*body],
            BodyLifecycleEvent::Deleted { body } => {
                self.remove(*body);
                vec![*body]
            }
            BodyLifecycleEvent::Split { parent, children } => {
                self.apply_split(by, *parent, children)
            }
            BodyLifecycleEvent::Merged { inputs, winner } => self.apply_merge(by, inputs, *winner),
        }
    }

    fn apply_split(&mut self, by: RecordId, parent: BodyId, children: &[BodyId]) -> Vec<BodyId> {
        let mut changed = vec![parent];
        let survivor = children.first().copied();
        let parent_meta = self.get(parent).cloned();
        for &child in children {
            changed.push(child);
            if Some(child) == survivor {
                // The survivor keeps the ORIGINAL BodyId (V1/V2 §2.2).
                if child == parent {
                    continue; // parent survives unchanged
                }
                match &parent_meta {
                    Some(pm) if !self.contains(child) => self.bodies.push(BodyMeta {
                        id: child,
                        name: pm.name.clone(),
                        visible: pm.visible,
                        created_by: pm.created_by,
                    }),
                    _ => self.ensure_fresh(child, by),
                }
                self.retire(parent, child);
            } else {
                self.ensure_fresh(child, by);
            }
        }
        changed
    }

    fn apply_merge(&mut self, by: RecordId, inputs: &[BodyId], winner: BodyId) -> Vec<BodyId> {
        let mut changed: Vec<BodyId> = inputs.to_vec();
        changed.push(winner);
        if !self.contains(winner) {
            let inherited = inputs.iter().find_map(|i| self.get(*i).cloned());
            let meta = match inherited {
                Some(m) => BodyMeta {
                    id: winner,
                    name: m.name,
                    visible: m.visible,
                    created_by: m.created_by,
                },
                None => BodyMeta::new(winner, self.default_name(), by),
            };
            self.bodies.push(meta);
        }
        for &inp in inputs {
            if inp != winner {
                self.retire(inp, winner);
            }
        }
        changed
    }

    /// Registers a fresh visible body with a generated name if the id is absent.
    fn ensure_fresh(&mut self, id: BodyId, by: RecordId) {
        if !self.contains(id) {
            let name = self.default_name();
            self.bodies.push(BodyMeta::new(id, name, by));
        }
    }

    /// Removes `retired` from the active set and records the alias `retired →
    /// survivor` (no self-alias).
    fn retire(&mut self, retired: BodyId, survivor: BodyId) {
        self.remove(retired);
        if retired != survivor {
            self.aliases.insert(retired, survivor);
        }
    }

    fn default_name(&self) -> String {
        format!("Body {}", self.bodies.len() + 1)
    }
}

/// De-duplicates ids preserving first-seen order.
fn dedup_preserving(ids: Vec<BodyId>) -> Vec<BodyId> {
    let mut seen = std::collections::HashSet::new();
    ids.into_iter().filter(|id| seen.insert(*id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn bid(n: u128) -> BodyId {
        BodyId(Uuid::from_u128(n))
    }
    fn rid(n: u128) -> RecordId {
        RecordId(Uuid::from_u128(n))
    }

    #[test]
    fn event_serde_matches_schema_shape() {
        let created = BodyLifecycleEvent::Created { body: bid(3) };
        let v = serde_json::to_value(&created).unwrap();
        assert_eq!(v["kind"], "created");
        assert!(v.get("bodyId").is_some());

        let split = BodyLifecycleEvent::Split {
            parent: bid(1),
            children: vec![bid(1), bid(2)],
        };
        let v = serde_json::to_value(&split).unwrap();
        assert_eq!(v["kind"], "split");
        assert!(v.get("parent").is_some() && v.get("children").is_some());

        let merged = BodyLifecycleEvent::Merged {
            inputs: vec![bid(1), bid(2)],
            winner: bid(1),
        };
        let v = serde_json::to_value(&merged).unwrap();
        assert_eq!(v["kind"], "merged");
        assert!(v.get("inputs").is_some() && v.get("winner").is_some());
    }

    #[test]
    fn created_then_split_keeps_first_child_identity() {
        let mut reg = BodyRegistry::new();
        reg.fold(0, rid(0xA), BodyLifecycleEvent::Created { body: bid(1) });
        assert!(reg.contains(bid(1)));
        // Split parent 1 -> [1 (survivor), 2 (new)].
        let changed = reg.fold(
            1,
            rid(0xB),
            BodyLifecycleEvent::Split {
                parent: bid(1),
                children: vec![bid(1), bid(2)],
            },
        );
        assert!(reg.contains(bid(1)), "survivor keeps original id");
        assert!(reg.contains(bid(2)), "second child is a fresh body");
        assert_eq!(changed, vec![bid(1), bid(2)]);
        // survivor inherited the parent's created_by.
        assert_eq!(reg.get(bid(1)).unwrap().created_by, rid(0xA));
        assert_eq!(reg.log().len(), 2);
    }

    #[test]
    fn split_with_distinct_survivor_aliases_parent() {
        let mut reg = BodyRegistry::new();
        reg.fold(0, rid(0xA), BodyLifecycleEvent::Created { body: bid(1) });
        // children[0] != parent -> parent retired, aliased to survivor.
        reg.fold(
            1,
            rid(0xB),
            BodyLifecycleEvent::Split {
                parent: bid(1),
                children: vec![bid(10), bid(11)],
            },
        );
        assert!(!reg.contains(bid(1)), "parent retired");
        assert!(reg.contains(bid(10)) && reg.contains(bid(11)));
        assert_eq!(reg.resolve(bid(1)), bid(10), "parent aliased to survivor");
    }

    #[test]
    fn merge_winner_rule_appendix_c() {
        let mut reg = BodyRegistry::new();
        reg.register(BodyMeta::new(bid(5), "b5", rid(1))); // creation idx 0
        reg.register(BodyMeta::new(bid(3), "b3", rid(2))); // creation idx 1
                                                           // target preferred when present.
        assert_eq!(
            reg.merge_winner(&[bid(5), bid(3)], Some(bid(3))),
            Some(bid(3))
        );
        // else lowest creation index (bid(5) registered first).
        assert_eq!(reg.merge_winner(&[bid(5), bid(3)], None), Some(bid(5)));
        // else lowest BodyId when neither is registered (equal MAX creation idx).
        assert_eq!(reg.merge_winner(&[bid(9), bid(2)], None), Some(bid(2)));
    }

    #[test]
    fn merge_retires_losers_and_aliases_to_winner() {
        let mut reg = BodyRegistry::new();
        reg.register(BodyMeta::new(bid(1), "a", rid(1)));
        reg.register(BodyMeta::new(bid(2), "b", rid(2)));
        let changed = reg.fold(
            2,
            rid(3),
            BodyLifecycleEvent::Merged {
                inputs: vec![bid(1), bid(2)],
                winner: bid(1),
            },
        );
        assert!(reg.contains(bid(1)) && !reg.contains(bid(2)));
        assert_eq!(reg.resolve(bid(2)), bid(1));
        assert_eq!(changed, vec![bid(1), bid(2)]);
    }

    #[test]
    fn registry_round_trips_through_serde() {
        let mut reg = BodyRegistry::new();
        reg.fold(0, rid(1), BodyLifecycleEvent::Created { body: bid(1) });
        reg.fold(
            1,
            rid(2),
            BodyLifecycleEvent::Split {
                parent: bid(1),
                children: vec![bid(1), bid(2)],
            },
        );
        let json = serde_json::to_string(&reg).unwrap();
        let back: BodyRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(reg, back);
    }
}
