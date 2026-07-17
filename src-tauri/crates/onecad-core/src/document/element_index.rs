//! Document-level element partition index (V1/V2 §3.2 partitioned ElementMap).
//!
//! A minimal, Rust-owned map `ElementId -> {body, kind}` recording, for each
//! **minted** persistent element, its **current** body partition and kind. It is
//! deliberately small: the ID-on-demand policy (V1/V2 §3.1) means only referenced
//! elements are ever minted, so this index holds only the elements a feature
//! input / constraint / named selection actually points at.
//!
//! ## Why partition membership lives here, not in the id
//!
//! An [`ElementId`] is globally unique and **does NOT embed `BodyId`** (SCHEMA §2;
//! `ids.rs`): which body an element belongs to is a **mapping**, not identity, so
//! an element survives body split/merge (its id is unchanged; only its
//! [`ElementEntry::body`] here moves). This index IS that mapping. Folding a
//! regen [`ElementMapDelta`](crate::regen::engine::ElementMapDelta) updates the
//! partition without ever changing an id (Invariant 1).
//!
//! It is a document-level addition (reported in `document/mod.rs`), serialized
//! only when non-empty so existing `document.json` fixtures are unaffected.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::document::refs::ElementKind;
use crate::ids::{BodyId, ElementId};

/// The current partition membership of a minted element.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ElementEntry {
    /// The body this element currently belongs to (moves on split/merge).
    pub body: BodyId,
    /// The topological kind (face/edge/vertex).
    pub kind: ElementKind,
}

impl ElementEntry {
    /// A partition entry.
    #[must_use]
    pub fn new(body: BodyId, kind: ElementKind) -> Self {
        Self { body, kind }
    }
}

/// The document's `ElementId -> {body, kind}` partition index. Serializes
/// transparently as a JSON object keyed by element id (deterministic `BTreeMap`
/// order, Invariant 5).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ElementIndex {
    map: BTreeMap<ElementId, ElementEntry>,
}

impl ElementIndex {
    /// An empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// True iff no elements are indexed (the `skip_serializing_if` predicate).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Number of indexed elements.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// The partition entry for `id`, if indexed.
    #[must_use]
    pub fn get(&self, id: &ElementId) -> Option<&ElementEntry> {
        self.map.get(id)
    }

    /// True iff `id` is indexed.
    #[must_use]
    pub fn contains(&self, id: &ElementId) -> bool {
        self.map.contains_key(id)
    }

    /// The body `id` currently partitions into, if indexed.
    #[must_use]
    pub fn body_of(&self, id: &ElementId) -> Option<BodyId> {
        self.map.get(id).map(|e| e.body)
    }

    /// Records / updates `id`'s partition (a mint or an element-map relabel).
    /// Updating an existing id's body is the split/merge re-partition path — the
    /// id itself never changes (Invariant 1).
    pub fn insert(&mut self, id: ElementId, entry: ElementEntry) {
        self.map.insert(id, entry);
    }

    /// Removes `id` from the index (it left every partition), returning its old
    /// entry if present.
    pub fn remove(&mut self, id: &ElementId) -> Option<ElementEntry> {
        self.map.remove(id)
    }

    /// Iterates the `(id, entry)` pairs in deterministic id order.
    pub fn iter(&self) -> impl Iterator<Item = (&ElementId, &ElementEntry)> {
        self.map.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn body(n: u128) -> BodyId {
        BodyId(Uuid::from_u128(n))
    }

    #[test]
    fn insert_get_remove_and_repartition() {
        let mut idx = ElementIndex::new();
        assert!(idx.is_empty());
        let e = ElementId::new("el_1");
        idx.insert(e.clone(), ElementEntry::new(body(1), ElementKind::Face));
        assert_eq!(idx.body_of(&e), Some(body(1)));
        // Split/merge re-partition: same id, new body — identity unchanged.
        idx.insert(e.clone(), ElementEntry::new(body(2), ElementKind::Face));
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.body_of(&e), Some(body(2)));
        assert!(idx.remove(&e).is_some());
        assert!(idx.is_empty());
    }

    #[test]
    fn serializes_transparently_as_object_when_non_empty() {
        let mut idx = ElementIndex::new();
        idx.insert(
            ElementId::new("el_1"),
            ElementEntry::new(body(1), ElementKind::Edge),
        );
        let v = serde_json::to_value(&idx).unwrap();
        assert!(v.is_object());
        assert_eq!(v["el_1"]["kind"], "edge");
        let back: ElementIndex = serde_json::from_value(v).unwrap();
        assert_eq!(back, idx);
    }
}
