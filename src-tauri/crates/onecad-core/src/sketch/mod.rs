//! Sketch domain: the authoritative 2D parametric sketch (plane + entities +
//! constraints + a derived region cache).
//!
//! This is the document / file-format model (`sketches/*.json` in the v2
//! container), distinct from the op-record wire shape (`SketchOpParams` in
//! [`crate::document::record`], which carries the SCHEMA §7.3 worker-lane JSON
//! opaquely) and from the solver-lane `SketchUpsert` payload (SCHEMA §7.4). The
//! `onecad-protocol` adapter bridges this typed model to those wire shapes.
//!
//! **Serde discipline** (SCHEMA §5): camelCase, no `deny_unknown_fields`; the
//! top-level [`Sketch`] and each [`RegionInfo`] carry an `extra` flatten so
//! document-level unknown keys round-trip. Entity/constraint enums are
//! internally tagged and do NOT preserve alien variants — see the forward-compat
//! notes in [`entity`] / [`constraint`]. Sketch-schema evolution is therefore
//! gated by the `sketch_freeze` snapshots (like `schema_freeze`).

pub mod constraint;
pub mod entity;
pub mod plane;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::document::refs::{ElementRef, Extra};
use crate::ids::{ConstraintId, DatumPlaneId, EntityId, RegionId, SketchId};

pub use constraint::{Constraint, CurvePosition};
pub use entity::SketchEntity;
pub use plane::SketchPlane;

/// A named world reference plane (SCHEMA §7.3 `plane.kind` ∈ `XY`|`XZ`|`YZ`).
/// The concrete basis is [`SketchPlane::xy`]/[`xz`](SketchPlane::xz)/
/// [`yz`](SketchPlane::yz). Serialized as the bare `"XY"`/`"XZ"`/`"YZ"` token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::upper_case_acronyms)]
pub enum WorldPlane {
    /// The XY plane.
    XY,
    /// The XZ plane.
    XZ,
    /// The YZ plane.
    YZ,
}

impl WorldPlane {
    /// The concrete (non-standard) coordinate frame for this named plane.
    #[must_use]
    pub const fn plane(self) -> SketchPlane {
        match self {
            Self::XY => SketchPlane::xy(),
            Self::XZ => SketchPlane::xz(),
            Self::YZ => SketchPlane::yz(),
        }
    }
}

/// How a sketch is attached to the model (what its plane is derived from).
///
/// Internally tagged on `"kind"` ∈ `datum` | `world` | `hostFace`.
// Size disparity is inherent (HostFace carries a rich typed `ElementRef`);
// attachments live inside a `Sketch` behind a `Vec` of sketches and are not
// moved in hot loops, so the payload is left unboxed (matches `Operation`).
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum SketchAttachment {
    /// Bound to a document datum plane.
    Datum {
        /// The datum plane feature.
        datum: DatumPlaneId,
    },
    /// Bound to a named world plane (no datum feature).
    World {
        /// The world plane.
        plane: WorldPlane,
    },
    /// Bound to a solid face (C++ `HostFaceAttachment`).
    ///
    /// `face` is a typed [`ElementRef`] (identity + evidence + anchor) — richer
    /// than C++'s flat `{bodyId, faceId}` so the host face survives edits via
    /// the resolution ladder (mirrors the `FaceRef` decision in
    /// [`crate::document::refs`]). `projected_boundary_version` bumps whenever
    /// the host face's projected boundary edges are re-projected (C++
    /// `HostFaceAttachment::projectedBoundaryVersion`).
    HostFace {
        /// Reference to the host face.
        face: ElementRef,
        /// Version of the projected boundary edges (0 = not yet projected).
        projected_boundary_version: u32,
    },
}

/// Winding of a region loop — part of the deterministic [`derive_region_id`]
/// input. Outer loops are conventionally CCW, holes CW.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Winding {
    /// Counter-clockwise (typical outer loop).
    Ccw,
    /// Clockwise (typical hole).
    Cw,
}

impl Winding {
    /// The discriminant byte fed into the region-id hash (STABLE).
    const fn hash_byte(self) -> u8 {
        match self {
            Self::Ccw => 0,
            Self::Cw => 1,
        }
    }
}

/// Derives a **deterministic, stable** [`RegionId`] from a loop's member entity
/// ids + winding.
///
/// Per the plan ("RegionId derivation ... coordinate w/ sidecar"): the id must
/// be reproducible from loop membership alone, so the Rust core and the C++
/// worker sidecar agree on region identity without shared mutable state.
///
/// **Algorithm (STABLE — changing it remaps every region id; fixture-gated):**
/// 1. Take each member's 16-byte UUID; sort the byte arrays ascending so the id
///    is independent of loop-member ordering.
/// 2. **FNV-1a 64-bit** over: every 16-byte UUID in sorted order, then one
///    winding byte (`0`=Ccw, `1`=Cw).
/// 3. Format as `"r_"` + 16 lowercase hex digits.
///
/// FNV-1a (not SHA-256) is chosen deliberately: it matches the OneCAD-CPP
/// ElementMap hashing family, needs no new dependency, and is fully
/// deterministic. 64 bits is ample here — a collision only causes a
/// recomputed-cache miss (regions are a cache, not authoritative identity), not
/// a correctness bug.
#[must_use]
pub fn derive_region_id(members: &[EntityId], winding: Winding) -> RegionId {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut uuids: Vec<[u8; 16]> = members.iter().map(|e| *e.as_uuid().as_bytes()).collect();
    uuids.sort_unstable();

    let mut hash = FNV_OFFSET;
    let mut mix = |byte: u8| {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    };
    for uuid in &uuids {
        for &byte in uuid {
            mix(byte);
        }
    }
    mix(winding.hash_byte());

    RegionId::new(format!("r_{hash:016x}"))
}

/// Cached closed-profile region of a sketch (outer loop + hole loops).
///
/// **CACHE, NOT AUTHORITATIVE.** Regions are derived by the worker's
/// `SketchRegions` (SCHEMA §7.4) from the entities/constraints; this is a
/// rebuildable projection stored for fast lookup and preview, never a source of
/// truth. On any entity/constraint edit it must be recomputed (see
/// [`Sketch::set_regions`]).
///
/// **DISCREPANCY** (report): SCHEMA §7.4 `SketchRegions` names the wire fields
/// `regionId` / `outerLoop` / `holes`; this cache uses `id` / `outer` / `holes`
/// per the WP spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegionInfo {
    /// Deterministic region identity (see [`derive_region_id`]).
    pub id: RegionId,
    /// Outer boundary loop, as an ordered list of member entity ids.
    pub outer: Vec<EntityId>,
    /// Hole loops (each an ordered list of member entity ids).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holes: Vec<Vec<EntityId>>,
    /// Unknown keys, preserved verbatim (SCHEMA §5).
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Entities + constraints that reference some entity (returned by
/// [`Sketch::remove_entity`] / [`Sketch::dependents_of`]).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dependents {
    /// Entity ids whose geometry references the subject (they would dangle).
    pub entities: Vec<EntityId>,
    /// Constraint ids that reference the subject.
    pub constraints: Vec<ConstraintId>,
}

impl Dependents {
    /// True iff nothing references the subject.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entities.is_empty() && self.constraints.is_empty()
    }
}

/// A sketch mutation rejected by validation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SketchError {
    /// An entity with this id already exists.
    #[error("duplicate entity id {0}")]
    DuplicateEntity(EntityId),
    /// A constraint with this id already exists.
    #[error("duplicate constraint id {0}")]
    DuplicateConstraint(ConstraintId),
    /// An entity references a point entity that is not in the sketch.
    #[error("entity {entity} references missing entity {missing}")]
    DanglingEntityRef {
        /// The offending entity.
        entity: EntityId,
        /// The missing referenced entity.
        missing: EntityId,
    },
    /// A constraint references an entity that is not in the sketch.
    #[error("constraint {constraint} references missing entity {missing}")]
    DanglingConstraintRef {
        /// The offending constraint.
        constraint: ConstraintId,
        /// The missing referenced entity.
        missing: EntityId,
    },
}

/// Serde mirror of [`Sketch`] (the persisted fields only). Deserializing into
/// this and converting rebuilds the (non-serialized) lookup indices; see the
/// `#[serde(from/into)]` on [`Sketch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SketchData {
    id: SketchId,
    name: String,
    plane: SketchPlane,
    attachment: SketchAttachment,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    entities: Vec<SketchEntity>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    constraints: Vec<Constraint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    regions: Vec<RegionInfo>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    extra: Extra,
}

/// A 2D parametric sketch: plane + entities + constraints + a region cache.
///
/// `entities` / `constraints` are kept private and mutated only through the
/// validating API ([`Sketch::add_entity`] / [`add_constraint`](Sketch::add_constraint)
/// / [`remove_entity`](Sketch::remove_entity) / …) so a live `Sketch` never has
/// a duplicate id or (via `add_*`) a dangling reference. Lookup goes through
/// **rebuilt id→index maps that are NOT serialized** — they are reconstructed on
/// deserialize and maintained on every mutation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "SketchData", into = "SketchData")]
pub struct Sketch {
    /// Sketch identity.
    pub id: SketchId,
    /// Human-readable name.
    pub name: String,
    /// The coordinate frame (basis carried verbatim; see [`SketchPlane`]).
    pub plane: SketchPlane,
    /// What the sketch is attached to.
    pub attachment: SketchAttachment,
    /// Region cache (not authoritative — see [`RegionInfo`]).
    pub regions: Vec<RegionInfo>,
    /// Document-level unknown keys, preserved verbatim.
    pub extra: Extra,

    entities: Vec<SketchEntity>,
    constraints: Vec<Constraint>,
    // Rebuilt, never serialized (see SketchData / from/into).
    entity_index: HashMap<EntityId, usize>,
    constraint_index: HashMap<ConstraintId, usize>,
}

impl From<SketchData> for Sketch {
    fn from(d: SketchData) -> Self {
        let mut s = Self {
            id: d.id,
            name: d.name,
            plane: d.plane,
            attachment: d.attachment,
            regions: d.regions,
            extra: d.extra,
            entities: d.entities,
            constraints: d.constraints,
            entity_index: HashMap::new(),
            constraint_index: HashMap::new(),
        };
        // Deserialize trusts the stored file (a valid file has no dup/dangling
        // ids); validation is enforced on the mutation API, not on load.
        s.rebuild_indexes();
        s
    }
}

impl From<Sketch> for SketchData {
    fn from(s: Sketch) -> Self {
        Self {
            id: s.id,
            name: s.name,
            plane: s.plane,
            attachment: s.attachment,
            entities: s.entities,
            constraints: s.constraints,
            regions: s.regions,
            extra: s.extra,
        }
    }
}

impl Sketch {
    /// An empty sketch on `plane` with the given `attachment`.
    #[must_use]
    pub fn new(id: SketchId, name: impl Into<String>, attachment: SketchAttachment) -> Self {
        let plane = match &attachment {
            SketchAttachment::World { plane } => plane.plane(),
            // Datum / host-face frames are derived later from the model; default
            // to XY until resolved.
            _ => SketchPlane::xy(),
        };
        Self {
            id,
            name: name.into(),
            plane,
            attachment,
            regions: Vec::new(),
            extra: Extra::new(),
            entities: Vec::new(),
            constraints: Vec::new(),
            entity_index: HashMap::new(),
            constraint_index: HashMap::new(),
        }
    }

    /// An empty sketch on a named world plane (convenience for tests / defaults).
    #[must_use]
    pub fn on_world_plane(id: SketchId, name: impl Into<String>, plane: WorldPlane) -> Self {
        Self::new(id, name, SketchAttachment::World { plane })
    }

    /// The entities, in authoritative order.
    #[must_use]
    pub fn entities(&self) -> &[SketchEntity] {
        &self.entities
    }

    /// The constraints, in authoritative order.
    #[must_use]
    pub fn constraints(&self) -> &[Constraint] {
        &self.constraints
    }

    /// Looks up an entity by id (via the rebuilt index).
    #[must_use]
    pub fn get_entity(&self, id: EntityId) -> Option<&SketchEntity> {
        self.entity_index.get(&id).map(|&i| &self.entities[i])
    }

    /// Looks up a constraint by id (via the rebuilt index).
    #[must_use]
    pub fn get_constraint(&self, id: ConstraintId) -> Option<&Constraint> {
        self.constraint_index
            .get(&id)
            .map(|&i| &self.constraints[i])
    }

    /// True iff an entity with `id` exists.
    #[must_use]
    pub fn contains_entity(&self, id: EntityId) -> bool {
        self.entity_index.contains_key(&id)
    }

    /// True iff a constraint with `id` exists.
    #[must_use]
    pub fn contains_constraint(&self, id: ConstraintId) -> bool {
        self.constraint_index.contains_key(&id)
    }

    /// Appends an entity, rejecting a duplicate id or a reference to a
    /// point-entity that is not already in the sketch (dangling ref).
    ///
    /// # Errors
    /// [`SketchError::DuplicateEntity`] or [`SketchError::DanglingEntityRef`].
    pub fn add_entity(&mut self, entity: SketchEntity) -> Result<(), SketchError> {
        let id = entity.id();
        if self.entity_index.contains_key(&id) {
            return Err(SketchError::DuplicateEntity(id));
        }
        for referenced in entity.referenced_entities() {
            if !self.entity_index.contains_key(&referenced) {
                return Err(SketchError::DanglingEntityRef {
                    entity: id,
                    missing: referenced,
                });
            }
        }
        let index = self.entities.len();
        self.entities.push(entity);
        self.entity_index.insert(id, index);
        Ok(())
    }

    /// Appends a constraint, rejecting a duplicate id or a reference to an
    /// entity that is not in the sketch (dangling ref).
    ///
    /// # Errors
    /// [`SketchError::DuplicateConstraint`] or
    /// [`SketchError::DanglingConstraintRef`].
    pub fn add_constraint(&mut self, constraint: Constraint) -> Result<(), SketchError> {
        let id = constraint.id();
        if self.constraint_index.contains_key(&id) {
            return Err(SketchError::DuplicateConstraint(id));
        }
        for referenced in constraint.entities() {
            if !self.entity_index.contains_key(&referenced) {
                return Err(SketchError::DanglingConstraintRef {
                    constraint: id,
                    missing: referenced,
                });
            }
        }
        let index = self.constraints.len();
        self.constraints.push(constraint);
        self.constraint_index.insert(id, index);
        Ok(())
    }

    /// Removes an entity and returns what referenced it, or `None` if absent.
    ///
    /// Removal does **not** auto-cascade — the returned [`Dependents`] lets the
    /// edit layer decide whether to cascade (mirrors C++ `Sketch::removeEntity`
    /// reporting; keeps edit *policy* out of the domain model). Because a
    /// removal can leave dangling references, callers should resolve the
    /// dependents.
    pub fn remove_entity(&mut self, id: EntityId) -> Option<Dependents> {
        let index = *self.entity_index.get(&id)?;
        let dependents = self.dependents_of(id);
        self.entities.remove(index);
        self.rebuild_entity_index();
        Some(dependents)
    }

    /// Removes a constraint. Returns `true` if one was removed.
    pub fn remove_constraint(&mut self, id: ConstraintId) -> bool {
        if let Some(&index) = self.constraint_index.get(&id) {
            self.constraints.remove(index);
            self.rebuild_constraint_index();
            true
        } else {
            false
        }
    }

    /// Entities + constraints that reference `id` (would dangle on its removal).
    #[must_use]
    pub fn dependents_of(&self, id: EntityId) -> Dependents {
        let entities = self
            .entities
            .iter()
            .filter(|e| e.referenced_entities().contains(&id))
            .map(SketchEntity::id)
            .collect();
        let constraints = self
            .constraints
            .iter()
            .filter(|c| c.entities().contains(&id))
            .map(Constraint::id)
            .collect();
        Dependents {
            entities,
            constraints,
        }
    }

    /// Replaces the region cache (recomputed by the worker's `SketchRegions`).
    /// Regions are a cache, never authoritative — see [`RegionInfo`].
    pub fn set_regions(&mut self, regions: Vec<RegionInfo>) {
        self.regions = regions;
    }

    fn rebuild_indexes(&mut self) {
        self.rebuild_entity_index();
        self.rebuild_constraint_index();
    }

    fn rebuild_entity_index(&mut self) {
        self.entity_index = self
            .entities
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id(), i))
            .collect();
    }

    fn rebuild_constraint_index(&mut self) {
        self.constraint_index = self
            .constraints
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id(), i))
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vec2;
    use uuid::Uuid;

    fn sid(n: u128) -> SketchId {
        SketchId(Uuid::from_u128(n))
    }
    fn eid(n: u128) -> EntityId {
        EntityId(Uuid::from_u128(n))
    }
    fn cid(n: u128) -> ConstraintId {
        ConstraintId(Uuid::from_u128(n))
    }

    fn sketch_with_two_points() -> (Sketch, EntityId, EntityId) {
        let mut s = Sketch::on_world_plane(sid(1), "Sketch 1", WorldPlane::XY);
        let (p0, p1) = (eid(0x10), eid(0x11));
        s.add_entity(SketchEntity::point(
            p0,
            Vec2::new_unchecked(0.0, 0.0),
            false,
            false,
        ))
        .unwrap();
        s.add_entity(SketchEntity::point(
            p1,
            Vec2::new_unchecked(40.0, 0.0),
            false,
            false,
        ))
        .unwrap();
        (s, p0, p1)
    }

    #[test]
    fn add_entity_rejects_duplicate_id() {
        let (mut s, p0, _) = sketch_with_two_points();
        let dup = SketchEntity::point(p0, Vec2::new_unchecked(1.0, 1.0), false, false);
        assert_eq!(s.add_entity(dup), Err(SketchError::DuplicateEntity(p0)));
    }

    #[test]
    fn add_entity_rejects_dangling_point_ref() {
        let (mut s, p0, _) = sketch_with_two_points();
        let missing = eid(0xDEAD);
        let line = SketchEntity::line(eid(0x20), p0, missing, false);
        assert_eq!(
            s.add_entity(line),
            Err(SketchError::DanglingEntityRef {
                entity: eid(0x20),
                missing,
            })
        );
    }

    #[test]
    fn add_constraint_rejects_dangling_ref_and_duplicate() {
        let (mut s, p0, p1) = sketch_with_two_points();
        // dangling: references a non-existent entity.
        let missing = eid(0xBEEF);
        let bad = Constraint::Coincident {
            id: cid(1),
            point1: p0,
            point2: missing,
        };
        assert_eq!(
            s.add_constraint(bad),
            Err(SketchError::DanglingConstraintRef {
                constraint: cid(1),
                missing,
            })
        );
        // valid.
        let good = Constraint::Coincident {
            id: cid(2),
            point1: p0,
            point2: p1,
        };
        s.add_constraint(good.clone()).unwrap();
        // duplicate id.
        assert_eq!(
            s.add_constraint(good),
            Err(SketchError::DuplicateConstraint(cid(2)))
        );
    }

    #[test]
    fn remove_entity_reports_dependents() {
        let (mut s, p0, p1) = sketch_with_two_points();
        let line = eid(0x20);
        s.add_entity(SketchEntity::line(line, p0, p1, false))
            .unwrap();
        s.add_constraint(Constraint::Horizontal { id: cid(1), line })
            .unwrap();
        // Removing p0: the line references it, the constraint references the line
        // (not p0 directly) — so p0's direct dependents are just the line.
        let deps = s.remove_entity(p0).unwrap();
        assert_eq!(deps.entities, vec![line]);
        assert!(deps.constraints.is_empty());
        assert!(!s.contains_entity(p0));
        // Index stayed consistent after the shift-remove.
        assert!(s.get_entity(p1).is_some());
        assert!(s.get_entity(line).is_some());
    }

    #[test]
    fn lookup_index_survives_serde_round_trip() {
        let (mut s, p0, p1) = sketch_with_two_points();
        s.add_entity(SketchEntity::line(eid(0x20), p0, p1, false))
            .unwrap();
        let json = serde_json::to_string(&s).unwrap();
        // Index is NOT in the JSON.
        assert!(!json.contains("entity_index"));
        assert!(!json.contains("entityIndex"));
        let back: Sketch = serde_json::from_str(&json).unwrap();
        // Rebuilt index works.
        assert!(back.get_entity(p0).is_some());
        assert!(back.get_entity(eid(0x20)).is_some());
        assert_eq!(s, back);
    }

    #[test]
    fn derive_region_id_is_order_independent_and_winding_sensitive() {
        let a = eid(0xA1);
        let b = eid(0xB2);
        let c = eid(0xC3);
        let id1 = derive_region_id(&[a, b, c], Winding::Ccw);
        let id2 = derive_region_id(&[c, a, b], Winding::Ccw);
        assert_eq!(id1, id2, "region id must be independent of member order");
        let id3 = derive_region_id(&[a, b, c], Winding::Cw);
        assert_ne!(id1, id3, "winding must change the region id");
        assert!(id1.as_str().starts_with("r_"), "wire form is r_<hex>");
        assert_eq!(id1.as_str().len(), 2 + 16, "r_ + 16 hex digits");
        // Stability lock: exact value must not drift silently.
        let stable = derive_region_id(&[eid(1), eid(2)], Winding::Ccw);
        assert_eq!(stable.as_str(), "r_fbf1e34acfb51ba4");
    }

    #[test]
    fn attachment_serde_shapes() {
        let host = SketchAttachment::HostFace {
            face: ElementRef {
                primary: None,
                intent: None,
                anchor: None,
                extra: Default::default(),
            },
            projected_boundary_version: 3,
        };
        let v = serde_json::to_value(&host).unwrap();
        assert_eq!(v["kind"], serde_json::json!("hostFace"));
        assert_eq!(v["projectedBoundaryVersion"], serde_json::json!(3));

        let world: SketchAttachment =
            serde_json::from_value(serde_json::json!({ "kind": "world", "plane": "XZ" })).unwrap();
        assert_eq!(
            world,
            SketchAttachment::World {
                plane: WorldPlane::XZ
            }
        );
    }
}
