//! Derived dependency graph for regen ordering + dirty-closure computation.
//!
//! Hand-rolled port of the OneCAD-CPP `DependencyGraph`
//! (`src/app/history/DependencyGraph.{h,cpp}`). petgraph is deliberately NOT
//! used (plan justification): the C++ semantics — a producer-index edge build,
//! a Kahn tie-break by *creation index*, `produces_before` anti-time-travel, and
//! suppression/failure propagation — are small and must be reproduced exactly,
//! so a direct port is clearer and cheaper than adapting a general graph crate.
//!
//! Nodes are keyed by [`RecordId`]; creation order is the `Vec<RecordId>`
//! `creation_order` (a node's creation index is its position there, recomputed
//! after every add/remove — mirrors C++ `creationOrder_`).
//!
//! ## Intentional divergences from `DependencyGraph.cpp` (all load-bearing)
//!
//! 1. **Edges from bodies AND sketches.** C++ sketches are not history nodes, so
//!    it only links body inputs to body producers (`rebuildEdges`,
//!    `DependencyGraph.cpp:334-384`). In the v2 schema a `Sketch` op *is* a
//!    timeline node, so this port also links sketch inputs to the `Sketch` op
//!    that produces that `SketchId` (a `sketch_producers` map mirroring
//!    `bodyProducers_`). This is the plan's "producer index … from record
//!    outputs + Sketch ops".
//! 2. **No element→owner-body edges.** C++ links a fillet/shell's face/edge
//!    inputs to the body that owns them by parsing the `"<bodyId>/…"` id prefix
//!    (`DependencyGraph.cpp:363-378`). The v2 `ElementId` is opaque and
//!    **does not embed `BodyId`** (plan "ElementId scheme change"; `ids.rs`), and
//!    core records expose no element→producer mapping, so that linkage cannot be
//!    reconstructed at the pure-core level — it moves to the worker's ElementMap
//!    partition. Element inputs are tracked ([`Node::input_elements`]) but do not
//!    create edges here.
//! 3. **Deterministic queries.** C++ `getFailedOps` / DFS iterate `unordered_*`
//!    (non-deterministic order); this port orders by creation index / uses
//!    `BTreeSet` so results are reproducible (V1/V2 §0.1 invariant 5).
//! 4. **`is_blocked`** (upstream-failure gate) is new here — C++ has no analogue.

use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap, HashSet};

use crate::document::record::{KnownOperation, Operation, OperationRecord};
use crate::error::DomainError;
use crate::ids::{BodyId, RecordId, SketchId};

/// A node: one operation's derived inputs/outputs plus mutable regen flags
/// (OneCAD-CPP `FeatureNode`, `DependencyGraph.h:23-40`).
#[derive(Debug, Clone)]
struct Node {
    record_id: RecordId,
    /// Body inputs (from `Operation::derive_inputs`).
    input_bodies: Vec<BodyId>,
    /// Sketch inputs (from `Operation::derive_inputs`).
    input_sketches: Vec<SketchId>,
    /// Element (face/edge) inputs — tracked but not edge-forming (divergence 2).
    input_elements: Vec<crate::ids::ElementId>,
    /// Bodies this op produces/modifies (`OperationRecord::outputs`).
    output_bodies: Vec<BodyId>,
    /// The `SketchId` this op produces, iff it is a `Sketch` op (divergence 1).
    output_sketch: Option<SketchId>,
    suppressed: bool,
    failed: bool,
    failure_reason: String,
}

impl Node {
    fn from_record(record: &OperationRecord) -> Self {
        // USE the record's derived uniform input view (plan: reuse
        // `Operation::derive_inputs`, which is itself the C++
        // `extractDependencies` port — `record.rs:258-392`).
        let inputs = record.op.derive_inputs();
        let output_sketch = match &record.op {
            Operation::Known(KnownOperation::Sketch(p)) => Some(p.sketch),
            _ => None,
        };
        Self {
            record_id: record.record_id,
            input_bodies: inputs.bodies,
            input_sketches: inputs.sketches,
            input_elements: inputs.elements,
            output_bodies: record.outputs.clone(),
            output_sketch,
            // Seed suppression from the record's persisted flag (Rust: suppression
            // is an `OperationRecord` field; C++ tracks it in the Document and
            // seeds the node false).
            suppressed: record.suppressed,
            failed: false,
            failure_reason: String::new(),
        }
    }
}

/// Directed acyclic dependency graph over timeline records.
#[derive(Debug, Default, Clone)]
pub struct DependencyGraph {
    nodes: HashMap<RecordId, Node>,
    /// Creation order; a node's creation index is its position here.
    creation_order: Vec<RecordId>,
    /// producer → downstream consumers.
    forward_edges: HashMap<RecordId, BTreeSet<RecordId>>,
    /// consumer → upstream producers.
    backward_edges: HashMap<RecordId, BTreeSet<RecordId>>,
    /// Most recent (creation order) producer of each body (C++ `bodyProducers_`).
    body_producers: HashMap<BodyId, RecordId>,
    /// Most recent producer of each sketch (divergence 1; no C++ analogue).
    sketch_producers: HashMap<SketchId, RecordId>,
}

impl DependencyGraph {
    /// An empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Clears all nodes and edges (C++ `clear`, `DependencyGraph.cpp:17-23`).
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.creation_order.clear();
        self.forward_edges.clear();
        self.backward_edges.clear();
        self.body_producers.clear();
        self.sketch_producers.clear();
    }

    /// Number of nodes.
    #[must_use]
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// True iff there are no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// True iff a record with this id is in the graph.
    #[must_use]
    pub fn contains(&self, id: RecordId) -> bool {
        self.nodes.contains_key(&id)
    }

    /// All record ids in creation order (C++ `getAllOpIds`).
    #[must_use]
    pub fn creation_order(&self) -> &[RecordId] {
        &self.creation_order
    }

    /// Creation index (position in creation order) of a record, if present.
    #[must_use]
    pub fn creation_index(&self, id: RecordId) -> Option<usize> {
        self.creation_order.iter().position(|r| *r == id)
    }

    /// Rebuilds the graph from an ordered record slice, replacing all state
    /// (C++ `rebuildFromOperations`, `DependencyGraph.cpp:25-46`).
    pub fn rebuild_from_records(&mut self, records: &[OperationRecord]) {
        self.clear();
        for record in records {
            let node = Node::from_record(record);
            self.creation_order.push(node.record_id);
            self.nodes.insert(node.record_id, node);
        }
        self.rebuild_edges();
    }

    /// Appends a single record and rebuilds edges (C++ `addOperation`,
    /// `DependencyGraph.cpp:48-71`). A duplicate id replaces the node but keeps
    /// its original creation position.
    pub fn add_record(&mut self, record: &OperationRecord) {
        let node = Node::from_record(record);
        if !self.nodes.contains_key(&node.record_id) {
            self.creation_order.push(node.record_id);
        }
        self.nodes.insert(node.record_id, node);
        self.rebuild_edges();
    }

    /// Removes a record and rebuilds edges (C++ `removeOperation`,
    /// `DependencyGraph.cpp:73-94`). No-op if absent.
    pub fn remove_record(&mut self, id: RecordId) {
        if self.nodes.remove(&id).is_none() {
            return;
        }
        self.creation_order.retain(|r| *r != id);
        self.rebuild_edges();
    }

    // ── Queries ──────────────────────────────────────────────────────────────

    /// Topologically sorted record ids: every producer precedes its consumers.
    ///
    /// Kahn's algorithm with a deterministic tie-break — among zero-in-degree
    /// nodes the one with the **lowest creation index** is emitted first
    /// (C++ `topologicalSort` comparator `indexA > indexB`,
    /// `DependencyGraph.cpp:106-165`, ported here as a min-heap of
    /// `Reverse((creation_index, id))`).
    ///
    /// # Errors
    /// [`DomainError::Cycle`] listing the member ids if the graph is cyclic
    /// (task requirement 3; C++ returns an empty vector instead).
    pub fn topological_sort(&self) -> Result<Vec<RecordId>, DomainError> {
        let creation_index: HashMap<RecordId, usize> = self
            .creation_order
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i))
            .collect();
        let idx_of = |id: &RecordId| creation_index.get(id).copied().unwrap_or(usize::MAX);

        // In-degree = number of distinct upstream producers (cpp:116-123).
        let mut in_degree: HashMap<RecordId, usize> =
            self.nodes.keys().map(|id| (*id, 0usize)).collect();
        for (consumer, producers) in &self.backward_edges {
            in_degree.insert(*consumer, producers.len());
        }

        // Min-heap over zero-in-degree nodes, keyed by creation index (cpp:126-138).
        let mut heap: BinaryHeap<Reverse<(usize, RecordId)>> = BinaryHeap::new();
        for (id, deg) in &in_degree {
            if *deg == 0 {
                heap.push(Reverse((idx_of(id), *id)));
            }
        }

        let mut result = Vec::with_capacity(self.nodes.len());
        while let Some(Reverse((_, current))) = heap.pop() {
            result.push(current);
            if let Some(consumers) = self.forward_edges.get(&current) {
                for consumer in consumers {
                    if let Some(deg) = in_degree.get_mut(consumer) {
                        *deg -= 1;
                        if *deg == 0 {
                            heap.push(Reverse((idx_of(consumer), *consumer)));
                        }
                    }
                }
            }
        }

        if result.len() != self.nodes.len() {
            // Cycle: members are the nodes never emitted (cpp:159-162 returns {}).
            let emitted: HashSet<RecordId> = result.iter().copied().collect();
            let mut members: Vec<RecordId> = self
                .creation_order
                .iter()
                .filter(|id| !emitted.contains(id))
                .copied()
                .collect();
            members.sort_by_key(idx_of);
            let list = members
                .iter()
                .map(RecordId::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            return Err(DomainError::Cycle(format!(
                "{} record(s) in cycle: {list}",
                members.len()
            )));
        }
        Ok(result)
    }

    /// True iff the graph contains a cycle (C++ `hasCycle`,
    /// `DependencyGraph.cpp:181-183`).
    #[must_use]
    pub fn has_cycle(&self) -> bool {
        self.topological_sort().is_err()
    }

    /// The transitive set of records that depend on `id` (downstream closure;
    /// C++ `getDownstream` DFS, `DependencyGraph.cpp:167-172`). Never contains
    /// `id` itself.
    #[must_use]
    pub fn downstream(&self, id: RecordId) -> BTreeSet<RecordId> {
        let mut visited = BTreeSet::new();
        self.collect(&self.forward_edges, id, &mut visited);
        visited
    }

    /// The transitive set of records `id` depends on (upstream closure; C++
    /// `getUpstream`, `DependencyGraph.cpp:174-179`).
    #[must_use]
    pub fn upstream(&self, id: RecordId) -> BTreeSet<RecordId> {
        let mut visited = BTreeSet::new();
        self.collect(&self.backward_edges, id, &mut visited);
        visited
    }

    fn collect(
        &self,
        edges: &HashMap<RecordId, BTreeSet<RecordId>>,
        id: RecordId,
        visited: &mut BTreeSet<RecordId>,
    ) {
        if let Some(neighbours) = edges.get(&id) {
            for n in neighbours {
                if visited.insert(*n) {
                    self.collect(edges, *n, visited);
                }
            }
        }
    }

    /// The most recent record (creation order) that outputs `body`, or `None`
    /// (C++ `bodyProducer`, `DependencyGraph.cpp:386-389`).
    #[must_use]
    pub fn body_producer(&self, body: BodyId) -> Option<RecordId> {
        self.body_producers.get(&body).copied()
    }

    /// The most recent record that produces `sketch` (divergence 1; no C++
    /// analogue).
    #[must_use]
    pub fn sketch_producer(&self, sketch: SketchId) -> Option<RecordId> {
        self.sketch_producers.get(&sketch).copied()
    }

    /// Anti-time-travel check: is `body` a valid (non-time-travel) reference
    /// target for `consumer`? True when `body` is a base body (no producer), has
    /// a producer strictly *before* `consumer`, or is produced by `consumer`
    /// itself with no later producer; false only when `body` is produced *only*
    /// by a later op. Ported verbatim from C++ `producesBefore`
    /// (`DependencyGraph.cpp:391-431`).
    #[must_use]
    pub fn produces_before(&self, body: BodyId, consumer: RecordId) -> bool {
        let op_index = match self.creation_index(consumer) {
            Some(i) => i,
            None => return true, // op not tracked yet — don't block (cpp:400-402).
        };

        let (mut produced_before, mut produced_by_op, mut produced_after) = (false, false, false);
        for (i, id) in self.creation_order.iter().enumerate() {
            let Some(node) = self.nodes.get(id) else {
                continue;
            };
            if !node.output_bodies.contains(&body) {
                continue;
            }
            match i.cmp(&op_index) {
                std::cmp::Ordering::Less => produced_before = true,
                std::cmp::Ordering::Equal => produced_by_op = true,
                std::cmp::Ordering::Greater => produced_after = true,
            }
        }

        if !produced_before && !produced_by_op && !produced_after {
            return true; // base/external body (cpp:421-423).
        }
        if produced_before {
            return true; // a valid upstream producer exists (cpp:424-426).
        }
        if produced_by_op && !produced_after {
            return true; // op modifies a body it also produces (cpp:427-429).
        }
        false // only produced by a later op → time travel (cpp:430).
    }

    // ── Suppression (rollback) ───────────────────────────────────────────────

    /// Sets the suppression flag of `id`. When `cascade` is true the same value
    /// propagates to every downstream record (C++ `setSuppressed` +
    /// `suppressDownstream`, `DependencyGraph.cpp:185-201`). C++ only ever
    /// cascades `true`; this port cascades the given value so
    /// suppress/un-suppress are symmetric.
    pub fn set_suppressed(&mut self, id: RecordId, suppressed: bool, cascade: bool) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.suppressed = suppressed;
        } else {
            return;
        }
        if cascade {
            for d in self.downstream(id) {
                if let Some(node) = self.nodes.get_mut(&d) {
                    node.suppressed = suppressed;
                }
            }
        }
    }

    /// Whether `id` is suppressed (C++ `isSuppressed`).
    #[must_use]
    pub fn is_suppressed(&self, id: RecordId) -> bool {
        self.nodes.get(&id).is_some_and(|n| n.suppressed)
    }

    /// A snapshot of every node's suppression flag (C++ `getSuppressionState`,
    /// `DependencyGraph.cpp:203-209`).
    #[must_use]
    pub fn suppression_snapshot(&self) -> HashMap<RecordId, bool> {
        self.nodes
            .iter()
            .map(|(id, n)| (*id, n.suppressed))
            .collect()
    }

    /// Restores suppression flags from a snapshot (C++ `setSuppressionState`,
    /// `DependencyGraph.cpp:211-215`). Ids absent from the graph are ignored.
    pub fn restore_suppression(&mut self, snapshot: &HashMap<RecordId, bool>) {
        for (id, suppressed) in snapshot {
            if let Some(node) = self.nodes.get_mut(id) {
                node.suppressed = *suppressed;
            }
        }
    }

    // ── Failure tracking ─────────────────────────────────────────────────────

    /// Marks `id` as failed with a reason (C++ `setFailed(.., true, reason)`,
    /// `DependencyGraph.cpp:217-223`).
    pub fn mark_failed(&mut self, id: RecordId, reason: impl Into<String>) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.failed = true;
            node.failure_reason = reason.into();
        }
    }

    /// Clears the failure flag of `id` (C++ `setFailed(.., false, {})`).
    pub fn clear_failed(&mut self, id: RecordId) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.failed = false;
            node.failure_reason.clear();
        }
    }

    /// Clears every failure flag (C++ `clearFailures`,
    /// `DependencyGraph.cpp:245-250`).
    pub fn clear_failures(&mut self) {
        for node in self.nodes.values_mut() {
            node.failed = false;
            node.failure_reason.clear();
        }
    }

    /// Whether `id` is marked failed (C++ `isFailed`).
    #[must_use]
    pub fn is_failed(&self, id: RecordId) -> bool {
        self.nodes.get(&id).is_some_and(|n| n.failed)
    }

    /// The failure reason for `id`, if failed (C++ `getFailureReason`).
    #[must_use]
    pub fn failure_reason(&self, id: RecordId) -> Option<&str> {
        self.nodes
            .get(&id)
            .filter(|n| n.failed)
            .map(|n| n.failure_reason.as_str())
    }

    /// All failed record ids in creation order (C++ `getFailedOps`,
    /// `DependencyGraph.cpp:235-243`; ordered here for determinism — divergence 3).
    #[must_use]
    pub fn failed_ops(&self) -> Vec<RecordId> {
        self.creation_order
            .iter()
            .copied()
            .filter(|id| self.is_failed(*id))
            .collect()
    }

    /// Whether `id` is blocked by an upstream failure — true iff any record in
    /// its upstream closure is marked failed (Rust addition; divergence 4).
    #[must_use]
    pub fn is_blocked(&self, id: RecordId) -> bool {
        self.upstream(id).iter().any(|u| self.is_failed(*u))
    }

    // ── Node input/output accessors ──────────────────────────────────────────

    /// The body inputs of a record (from `Operation::derive_inputs`).
    #[must_use]
    pub fn input_bodies(&self, id: RecordId) -> &[BodyId] {
        self.nodes.get(&id).map_or(&[], |n| &n.input_bodies)
    }

    /// The sketch inputs of a record.
    #[must_use]
    pub fn input_sketches(&self, id: RecordId) -> &[SketchId] {
        self.nodes.get(&id).map_or(&[], |n| &n.input_sketches)
    }

    /// The element (face/edge) inputs of a record. These do not form edges here
    /// (divergence 2) but are surfaced for the worker-side ladder / repair.
    #[must_use]
    pub fn input_elements(&self, id: RecordId) -> &[crate::ids::ElementId] {
        self.nodes.get(&id).map_or(&[], |n| &n.input_elements)
    }

    /// The bodies a record produces/modifies (`OperationRecord::outputs`).
    #[must_use]
    pub fn output_bodies(&self, id: RecordId) -> &[BodyId] {
        self.nodes.get(&id).map_or(&[], |n| &n.output_bodies)
    }

    // ── Internals ────────────────────────────────────────────────────────────

    /// Rebuilds forward/backward edges + producer maps by walking creation order
    /// so each input binds to its **most recent prior** producer (C++
    /// `rebuildEdges`, `DependencyGraph.cpp:334-384`).
    fn rebuild_edges(&mut self) {
        self.forward_edges.clear();
        self.backward_edges.clear();
        self.body_producers.clear();
        self.sketch_producers.clear();

        let order = self.creation_order.clone();
        for id in &order {
            // Snapshot the fields we need to avoid holding a borrow while mutating.
            let Some(node) = self.nodes.get(id) else {
                continue;
            };
            let input_bodies = node.input_bodies.clone();
            let input_sketches = node.input_sketches.clone();
            let output_bodies = node.output_bodies.clone();
            let output_sketch = node.output_sketch;

            for body in &input_bodies {
                if let Some(prod) = self.body_producers.get(body).copied() {
                    self.link(prod, *id);
                }
            }
            for sketch in &input_sketches {
                if let Some(prod) = self.sketch_producers.get(sketch).copied() {
                    self.link(prod, *id);
                }
            }
            // Element inputs deliberately form no edges (divergence 2).

            for body in &output_bodies {
                self.body_producers.insert(*body, *id);
            }
            if let Some(sketch) = output_sketch {
                self.sketch_producers.insert(sketch, *id);
            }
        }
    }

    fn link(&mut self, producer: RecordId, consumer: RecordId) {
        // A node never depends on itself (cpp `it->second != opId`).
        if producer == consumer {
            return;
        }
        self.forward_edges
            .entry(producer)
            .or_default()
            .insert(consumer);
        self.backward_edges
            .entry(consumer)
            .or_default()
            .insert(producer);
    }

    /// Adds an explicit dependency edge `producer → consumer`.
    ///
    /// Not part of the producer-index model (which cannot express cycles — every
    /// natural edge points creation-order-forward). Provided for explicit
    /// non-producer dependencies and to exercise the defensive cycle path in
    /// [`Self::topological_sort`].
    ///
    /// # Errors
    /// [`DomainError::RecordNotFound`] if either endpoint is absent.
    pub fn add_edge(&mut self, producer: RecordId, consumer: RecordId) -> Result<(), DomainError> {
        if !self.nodes.contains_key(&producer) {
            return Err(DomainError::RecordNotFound(producer));
        }
        if !self.nodes.contains_key(&consumer) {
            return Err(DomainError::RecordNotFound(consumer));
        }
        self.link(producer, consumer);
        Ok(())
    }
}
