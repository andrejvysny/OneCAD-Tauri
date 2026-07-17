//! `OperationRecord` v2 — the on-disk file-format node (**high-stakes**: changing
//! its serialized shape is a schema change requiring sign-off).
//!
//! Serde contract (plan "Rust core specifics"):
//! * camelCase everywhere; **no `deny_unknown_fields` anywhere**.
//! * The operation is flattened onto the record as `{opType, params}`
//!   (adjacently tagged — SCHEMA §7.3, matching OneCAD-CPP `operationTypeName`
//!   PascalCase tag values).
//! * `extra` (`flatten`) maps at BOTH record and params levels carry unknown
//!   keys forward losslessly.
//! * An unknown `opType` deserializes to [`Operation::Opaque`] and round-trips
//!   byte-stably as a frozen node.
//! * `Scalar` (see [`crate::document::variables`]) carries dimension values
//!   (distance/radius/angle …) and accepts a bare wire number.
//!
//! Fillet and Chamfer are **split** ops (OneCAD-CPP shares `FilletChamferParams`;
//! here they are distinct variants keyed by `opType`).
//!
//! Record (de)serialization is **hand-written** (not derived) because the
//! required combination — flattening an adjacently-tagged-with-untagged-fallback
//! enum next to a second `flatten` extra map — is exactly the corner of serde's
//! `flatten` support that misbehaves. The manual impl gives byte-exact control
//! and is exercised by the snapshot + round-trip tests.
//!
//! **Reserved keys in `extra`.** The `extra` maps (record / params / nested-ref
//! level) are for *unknown* keys only. A caller MUST NOT stash a *reserved* key
//! (one a typed field owns — `opType`, `params`, `recordId`, `distance`, …) in an
//! `extra` map: on serialize the typed field is written first and the `extra`
//! entry is written second under the same name, producing a duplicate key. The
//! (de)serialize path never *reads back into* `extra` a key a typed field
//! claimed, so a well-formed load never populates `extra` with a reserved key;
//! the constraint is on hand-constructed values. The round-trip/proptest suites
//! only ever inject non-reserved (`alien*`) keys.
//!
//! **Duplicate JSON keys — file vs wire divergence.** On the FILE path the core
//! parses through `serde_json`, whose object model is **last-writer-wins** for a
//! duplicated key; a duplicate is therefore silently collapsed, not rejected.
//! This is accepted for the file format (files are Rust-authored; a duplicate can
//! only arise from external tampering, where last-wins is a safe, deterministic
//! resolution). The WIRE path is stricter: SCHEMA §4 mandates that a worker frame
//! with a duplicated object key is a `PROTOCOL_ERROR`. The divergence is
//! intentional — the wire is an adversarial trust boundary, the on-disk document
//! is not.

use serde::de::{self, DeserializeOwned};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use uuid::Uuid;

use crate::document::refs::{AxisRef, ElementRef, Extra, SketchRegionRef};
use crate::document::variables::Scalar;
use crate::ids::{BodyId, ElementId, RecordId, SketchId};
use crate::math::Vec3;

/// The record-schema version stamped on freshly authored records.
pub const RECORD_SCHEMA_VERSION: u32 = 1;

/// Known operation tag values (PascalCase; OneCAD-CPP `operationTypeName` +
/// the new `Sketch` op). An `opType` outside this set becomes
/// [`Operation::Opaque`].
const KNOWN_OP_TYPES: &[&str] = &[
    "Sketch",
    "Extrude",
    "Revolve",
    "Fillet",
    "Chamfer",
    "Shell",
    "Boolean",
    "LinearPattern",
    "CircularPattern",
    "Loft",
    "Sweep",
    "MirrorBody",
];

// ─────────────────────────────────────────────────────────────────────────────
// OperationRecord
// ─────────────────────────────────────────────────────────────────────────────

/// A single node in the linear timeline; the unit of the persisted file format.
#[derive(Debug, Clone, PartialEq)]
pub struct OperationRecord {
    /// Stable record identity.
    pub record_id: RecordId,
    /// Record-schema version (currently [`RECORD_SCHEMA_VERSION`]).
    pub record_schema_version: u32,
    /// Position in the timeline. Serialized for human readability; the **array
    /// order is authoritative** on load.
    pub step_index: u32,
    /// Human-facing name / alias.
    pub name: String,
    /// The operation (`{opType, params}`, flattened onto the record).
    pub op: Operation,
    /// Derived uniform view of the op's typed inputs (bodies/sketches/elements).
    /// Serialized for tooling; rebuilt from `op` on demand.
    pub inputs: OperationInputs,
    /// Bodies produced/modified by this op (OneCAD-CPP `resultBodyIds`).
    pub outputs: Vec<BodyId>,
    /// Determinism policy captured for reproducible replay.
    pub determinism: DeterminismSettings,
    /// Whether the op is suppressed (skipped during regen).
    pub suppressed: bool,
    /// Unknown record-level keys, preserved verbatim.
    pub extra: Extra,
}

impl OperationRecord {
    /// Builds a v1 record with derived inputs and default determinism.
    #[must_use]
    pub fn new(
        record_id: RecordId,
        step_index: u32,
        name: impl Into<String>,
        op: Operation,
    ) -> Self {
        let inputs = op.derive_inputs();
        Self {
            record_id,
            record_schema_version: RECORD_SCHEMA_VERSION,
            step_index,
            name: name.into(),
            op,
            inputs,
            outputs: Vec::new(),
            determinism: DeterminismSettings::default(),
            suppressed: false,
            extra: Extra::new(),
        }
    }
}

impl Serialize for OperationRecord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The op flattens onto the record as its own top-level keys.
        let op_value = serde_json::to_value(&self.op).map_err(serde::ser::Error::custom)?;
        let op_obj = op_value
            .as_object()
            .ok_or_else(|| serde::ser::Error::custom("operation did not serialize to an object"))?;

        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("recordId", &self.record_id)?;
        map.serialize_entry("recordSchemaVersion", &self.record_schema_version)?;
        map.serialize_entry("stepIndex", &self.step_index)?;
        map.serialize_entry("name", &self.name)?;
        for (k, v) in op_obj {
            map.serialize_entry(k, v)?;
        }
        map.serialize_entry("inputs", &self.inputs)?;
        map.serialize_entry("outputs", &self.outputs)?;
        map.serialize_entry("determinism", &self.determinism)?;
        map.serialize_entry("suppressed", &self.suppressed)?;
        for (k, v) in &self.extra {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for OperationRecord {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut map = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;

        fn take<T: DeserializeOwned, E: de::Error>(
            map: &mut serde_json::Map<String, serde_json::Value>,
            key: &str,
        ) -> Result<Option<T>, E> {
            match map.remove(key) {
                None => Ok(None),
                Some(v) => serde_json::from_value(v)
                    .map(Some)
                    .map_err(|e| E::custom(format!("field `{key}`: {e}"))),
            }
        }

        let record_id: RecordId =
            take(&mut map, "recordId")?.ok_or_else(|| de::Error::missing_field("recordId"))?;
        let record_schema_version =
            take(&mut map, "recordSchemaVersion")?.unwrap_or(RECORD_SCHEMA_VERSION);
        let step_index = take(&mut map, "stepIndex")?.unwrap_or(0);
        let name = take(&mut map, "name")?.unwrap_or_default();
        // The stored `inputs` are DERIVED from `op` and treated as advisory: for a
        // Known op they are re-derived below (self-healing, M3); only an Opaque
        // frozen node keeps whatever was on disk.
        let stored_inputs: OperationInputs = take(&mut map, "inputs")?.unwrap_or_default();
        let outputs = take(&mut map, "outputs")?.unwrap_or_default();
        let determinism = take(&mut map, "determinism")?.unwrap_or_default();
        let suppressed = take(&mut map, "suppressed")?.unwrap_or(false);

        // Everything left is op-related (`opType`/`params`) plus record-level extra.
        let op_type = map
            .get("opType")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        let (op, extra) = match op_type {
            Some(tag) if KNOWN_OP_TYPES.contains(&tag.as_str()) => {
                map.remove("opType");
                let params = map.remove("params");
                let mut op_obj = serde_json::Map::new();
                op_obj.insert("opType".into(), serde_json::Value::String(tag));
                if let Some(p) = params {
                    op_obj.insert("params".into(), p);
                }
                let known: KnownOperation =
                    serde_json::from_value(serde_json::Value::Object(op_obj))
                        .map_err(|e| de::Error::custom(format!("operation: {e}")))?;
                (Operation::Known(known), map)
            }
            _ => {
                // Unknown/missing opType → frozen node; everything left is its raw payload.
                (
                    Operation::Opaque(OpaqueOperation { raw: map }),
                    Extra::new(),
                )
            }
        };

        // M3 (Invariant: derived inputs are never trusted from disk): a Known op
        // RE-DERIVES its inputs from `op`, overwriting any tampered/stale stored
        // value; an Opaque frozen node (no typed deps) keeps the stored inputs.
        let inputs = match &op {
            Operation::Known(_) => op.derive_inputs(),
            Operation::Opaque(_) => stored_inputs,
        };

        Ok(OperationRecord {
            record_id,
            record_schema_version,
            step_index,
            name,
            op,
            inputs,
            outputs,
            determinism,
            suppressed,
            extra,
        })
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Operation
// ─────────────────────────────────────────────────────────────────────────────

/// The operation payload of a record: a known op or an opaque frozen node.
///
/// Serialize is untagged (a known op serializes as its `{opType, params}`; an
/// opaque op serializes as its flattened raw map). **Deserialize is
/// hand-written** and gates on [`KNOWN_OP_TYPES`] rather than falling through
/// untagged: an `opType` in the known set MUST deserialize as that typed op —
/// malformed `params` are a hard ERROR, never a silent demotion to `Opaque`
/// (M1). Only an unknown/absent `opType` becomes [`Operation::Opaque`]. This
/// makes the direct-`Operation` path agree with the hand-written
/// [`OperationRecord`] path (both error on a known op with bad params).
// Size disparity is inherent (Extrude carries rich typed face refs); records
// live behind a `Vec` and are not moved in hot loops, so the payload is left
// unboxed for a straightforward hand-written (de)serialize path.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Operation {
    /// A known, typed operation.
    Known(KnownOperation),
    /// An unknown-`opType` op captured verbatim (frozen node).
    Opaque(OpaqueOperation),
}

impl<'de> Deserialize<'de> for Operation {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let map = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let op_type = map
            .get("opType")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        match op_type {
            // Known opType: deserialize as the typed op; malformed params ERROR
            // (do NOT fall back to Opaque — that is the M1 fix).
            Some(tag) if KNOWN_OP_TYPES.contains(&tag.as_str()) => {
                let known: KnownOperation = serde_json::from_value(serde_json::Value::Object(map))
                    .map_err(|e| de::Error::custom(format!("operation `{tag}`: {e}")))?;
                Ok(Operation::Known(known))
            }
            // Unknown/absent opType: frozen node captured verbatim.
            _ => Ok(Operation::Opaque(OpaqueOperation { raw: map })),
        }
    }
}

/// A known operation, adjacently tagged `{opType, params}` (SCHEMA §7.3).
// See [`Operation`] on the size disparity / unboxed rationale.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "opType", content = "params")]
pub enum KnownOperation {
    Sketch(SketchOpParams),
    Extrude(ExtrudeParams),
    Revolve(RevolveParams),
    Fillet(FilletParams),
    Chamfer(ChamferParams),
    Shell(ShellParams),
    Boolean(BooleanParams),
    LinearPattern(LinearPatternParams),
    CircularPattern(CircularPatternParams),
    Loft(LoftParams),
    Sweep(SweepParams),
    MirrorBody(MirrorBodyParams),
}

/// Unknown-`opType` payload, captured as a raw map (frozen node).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpaqueOperation {
    /// The entire op object (`opType`, `params`, and any other keys) verbatim.
    #[serde(flatten)]
    pub raw: Extra,
}

impl Operation {
    /// Derives the uniform input view, mirroring OneCAD-CPP
    /// `DependencyGraph::extractDependencies` (`DependencyGraph.cpp:252-332`)
    /// including param-embedded dependencies. See per-arm comments for the
    /// C++↔Rust parity mapping (SCHEMA/`OperationRecord.h` line citations).
    #[must_use]
    pub fn derive_inputs(&self) -> OperationInputs {
        let mut inputs = OperationInputs::default();
        let known = match self {
            Operation::Known(k) => k,
            // A frozen node exposes no typed deps (it never regenerates).
            Operation::Opaque(_) => return inputs,
        };
        match known {
            // No C++ analogue (Sketch is a new v2 op). A sketch feature has no
            // upstream feature dependency in V1.
            KnownOperation::Sketch(_) => {}

            // Extrude: profile sketch + (target body iff boolean != NewBody).
            // Parity: DependencyGraph.cpp:254-256 (SketchRegionRef→sketch),
            // :270-278 (targetBody only when booleanMode != NewBody).
            // Note: C++ does NOT track ToFace target faces as deps — mirrored here.
            KnownOperation::Extrude(p) => {
                if let Some(profile) = &p.profile {
                    inputs.push_sketch(profile.sketch);
                }
                if p.boolean_mode != BooleanMode::NewBody {
                    if let Some(b) = p.target_body {
                        inputs.push_body(b);
                    }
                }
            }

            // Revolve: profile sketch + axis + (target body iff boolean != NewBody).
            // Parity: DependencyGraph.cpp:279-295.
            KnownOperation::Revolve(p) => {
                if let Some(profile) = &p.profile {
                    inputs.push_sketch(profile.sketch);
                }
                match &p.axis {
                    Some(AxisRef::SketchLine { sketch, .. }) => inputs.push_sketch(*sketch),
                    Some(AxisRef::Element { body, edge, .. }) => {
                        inputs.push_body(*body);
                        inputs.push_element(edge.clone());
                    }
                    None => {}
                }
                if p.boolean_mode != BooleanMode::NewBody {
                    if let Some(b) = p.target_body {
                        inputs.push_body(b);
                    }
                }
            }

            // Fillet/Chamfer: referenced edges (elements) + the operated body.
            // Parity: DependencyGraph.cpp:296-300 (edgeIds→inputEdgeIds) for the
            // edges, PLUS :264-267 (the op's input BodyRef → the operated body) —
            // recovered here from each typed edge ref's `primary.body` (M5). An
            // edge ref without a `primary` (intent-only) contributes no body; the
            // operated body is then bound at regen time. Bare `edge_ids` (no typed
            // ref) contribute only the element id.
            KnownOperation::Fillet(p) => {
                derive_fillet_chamfer_inputs(&mut inputs, &p.edge_ids, &p.edges)
            }
            KnownOperation::Chamfer(p) => {
                derive_fillet_chamfer_inputs(&mut inputs, &p.edge_ids, &p.edges)
            }

            // Shell: open faces (elements) + shelled body. Parity:
            // DependencyGraph.cpp:301-305 (openFaceIds→inputFaceIds); the body
            // comes from the C++ BodyRef input (:264-267), modeled here as
            // `target_body`.
            KnownOperation::Shell(p) => {
                if let Some(b) = p.target_body {
                    inputs.push_body(b);
                }
                for f in &p.open_faces {
                    inputs.push_element(f.clone());
                }
            }

            // Boolean: target + tool bodies. Parity: DependencyGraph.cpp:306-309.
            KnownOperation::Boolean(p) => {
                inputs.push_body(p.target_body);
                inputs.push_body(p.tool_body);
            }

            // Linear/Circular pattern: source body. Parity:
            // DependencyGraph.cpp:310-319.
            KnownOperation::LinearPattern(p) => {
                if let Some(b) = p.source_body {
                    inputs.push_body(b);
                }
            }
            KnownOperation::CircularPattern(p) => {
                if let Some(b) = p.source_body {
                    inputs.push_body(b);
                }
            }

            // Loft: profile sketches. NOTE: C++ extractDependencies OMITS Loft
            // (a gap — LoftParams is absent from the if-chain, cpp:252-332); the
            // Rust port tracks profile sketches so a loft regenerates when a
            // profile sketch changes.
            KnownOperation::Loft(p) => {
                for profile in &p.profiles {
                    inputs.push_sketch(profile.sketch);
                }
            }

            // Sweep: profile sketch + path sketch + path edge. Parity:
            // DependencyGraph.cpp:320-331.
            KnownOperation::Sweep(p) => {
                if let Some(profile) = &p.profile {
                    inputs.push_sketch(profile.sketch);
                }
                if let Some(s) = p.path_sketch {
                    inputs.push_sketch(s);
                }
                if let Some(e) = &p.path_edge {
                    inputs.push_element(e.clone());
                }
            }

            // MirrorBody: source body. NOTE: C++ extractDependencies OMITS
            // MirrorBody (cpp:252-332); the task mandates tracking mirror sources,
            // so the Rust port adds it.
            KnownOperation::MirrorBody(p) => {
                if let Some(b) = p.source_body {
                    inputs.push_body(b);
                }
            }
        }
        inputs
    }
}

/// Shared Fillet/Chamfer input derivation (M5): bare `edge_ids` supply element
/// deps; typed `edges` additionally supply the operated body (`primary.body`,
/// deduped) and — when present — the primary element id. Intent-only edge refs
/// (no `primary`) contribute nothing here; regen binds the body later.
fn derive_fillet_chamfer_inputs(
    inputs: &mut OperationInputs,
    edge_ids: &[ElementId],
    edges: &[ElementRef],
) {
    for e in edge_ids {
        inputs.push_element(e.clone());
    }
    for r in edges {
        if let Some(primary) = &r.primary {
            inputs.push_body(primary.body);
            inputs.push_element(primary.element.clone());
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// OperationInputs (derived uniform view)
// ─────────────────────────────────────────────────────────────────────────────

/// Derived, order-preserving, de-duplicated view of a record's typed inputs,
/// for dependency-graph construction. Faces and edges are unified into
/// `elements` (plan "derived uniform view").
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationInputs {
    pub bodies: Vec<BodyId>,
    pub sketches: Vec<SketchId>,
    pub elements: Vec<ElementId>,
}

impl OperationInputs {
    fn push_body(&mut self, id: BodyId) {
        if !self.bodies.contains(&id) {
            self.bodies.push(id);
        }
    }
    fn push_sketch(&mut self, id: SketchId) {
        if !self.sketches.contains(&id) {
            self.sketches.push(id);
        }
    }
    fn push_element(&mut self, id: ElementId) {
        if !self.elements.contains(&id) {
            self.elements.push(id);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DeterminismSettings
// ─────────────────────────────────────────────────────────────────────────────

/// Determinism policy captured on a record for reproducible replay
/// (OneCAD-CPP `OperationMetadata.h DeterminismSettings` + SCHEMA §7.3
/// `determinism`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeterminismSettings {
    /// `false` = single-threaded OCCT (reproducible); default.
    #[serde(default)]
    pub parallel: bool,
    /// OCCT algorithm knobs (SCHEMA `occtOptions`, e.g. `fuzzyValue`, `useOBB`).
    #[serde(default, skip_serializing_if = "Extra::is_empty")]
    pub occt_options: Extra,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub occt_options_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tolerance_policy_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub solver_policy_hash: String,
    /// Unknown determinism-level keys, preserved verbatim (M2). Distinct from
    /// `occt_options`, which is the typed OCCT-knob map.
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

// ─────────────────────────────────────────────────────────────────────────────
// Enums shared by params (PascalCase wire values, matching SCHEMA §7.3)
// ─────────────────────────────────────────────────────────────────────────────

/// Extrude end condition (OneCAD-CPP `ExtrudeMode`; SCHEMA
/// `Blind`/`ThroughAll`/`Symmetric`/`ToNext`/`ToFace`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExtrudeMode {
    #[default]
    Blind,
    ThroughAll,
    Symmetric,
    ToNext,
    ToFace,
}

/// Feature-fused boolean mode (OneCAD-CPP `BooleanMode`; SCHEMA
/// `NewBody`/`Add`/`Cut`/`Intersect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BooleanMode {
    #[default]
    NewBody,
    Add,
    Cut,
    Intersect,
}

/// Standalone boolean operation (OneCAD-CPP `BooleanParams::Op`; SCHEMA
/// `Union`/`Cut`/`Intersect`). Distinct from the feature-fused [`BooleanMode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BooleanOp {
    #[default]
    Union,
    Cut,
    Intersect,
}

/// Named sketch plane (SCHEMA §7.3 `plane.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PlaneKind {
    #[serde(rename = "XY")]
    #[default]
    Xy,
    #[serde(rename = "XZ")]
    Xz,
    #[serde(rename = "YZ")]
    Yz,
    #[serde(rename = "custom")]
    Custom,
}

fn default_true() -> bool {
    true
}

/// Deserializes an optional `BodyId` where an empty string means "no body"
/// (SCHEMA §7.3 sends `"targetBodyId": ""` for the `NewBody` case).
fn de_opt_body_id<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<BodyId>, D::Error> {
    let opt = Option::<String>::deserialize(deserializer)?;
    match opt {
        None => Ok(None),
        Some(s) if s.is_empty() => Ok(None),
        Some(s) => Uuid::parse_str(&s)
            .map(|u| Some(BodyId(u)))
            .map_err(de::Error::custom),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Param structs (ported 1:1 from OperationRecord.h; SCHEMA §7.3 field names)
// ─────────────────────────────────────────────────────────────────────────────

/// Sketch feature (new v2 op; no OneCAD-CPP `OperationRecord.h` analogue —
/// sketches are inputs there, not ops). Entities/constraints are carried as
/// opaque JSON pending the typed sketch model (separate sketch WP); they
/// round-trip verbatim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchOpParams {
    #[serde(rename = "sketchId")]
    pub sketch: SketchId,
    pub plane: SketchPlaneRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<serde_json::Value>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// A sketch plane (kind + basis) as carried in `SketchOpParams` (SCHEMA §7.3
/// `plane`). The named-plane bases are the NON-STANDARD OneCAD-CPP mapping
/// (see [`crate::sketch::plane`]).
///
/// Not `Copy`: carries an `extra` map so unknown keys injected at the `plane`
/// level round-trip losslessly (M2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchPlaneRef {
    pub kind: PlaneKind,
    pub origin: Vec3,
    pub x_axis: Vec3,
    pub y_axis: Vec3,
    pub normal: Vec3,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Extrude parameters. Ported from OneCAD-CPP `ExtrudeParams`
/// (`OperationRecord.h:91-104`); scalar/enum field names align with SCHEMA §7.3.
///
/// Discrepancies (SCHEMA wins on names; plan's typed-refs otherwise):
/// * `profile` (typed `SketchRegionRef`) is a Rust-core field only — SCHEMA/C++
///   carry the region in the separate `inputs[]`/`input`, not in params.
///   Optional/defaulted so SCHEMA §7.3 payloads (no `profile`) still parse.
/// * `target_face`/`target_face2` are typed `ElementRef`s (serialized
///   `targetFace`/`targetFace2`) for the `ToFace` end condition. SCHEMA §7.3 now
///   carries the SAME typed semantic-ref shape (amended 2026-07-16 — see the
///   SCHEMA Changelog): the previous bare-string `targetFaceId`/`targetFaceId2`
///   could not carry anchor/intent, leaving a ToFace target un-repairable across
///   parametric edits (Invariant 2/3). Absent for non-`ToFace` extrudes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtrudeParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<SketchRegionRef>,
    pub distance: Scalar,
    pub draft_angle_deg: Scalar,
    #[serde(rename = "extrudeMode")]
    pub mode: ExtrudeMode,
    pub boolean_mode: BooleanMode,
    #[serde(
        rename = "targetBodyId",
        default,
        deserialize_with = "de_opt_body_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_body: Option<BodyId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_face: Option<ElementRef>,
    pub two_directions: bool,
    #[serde(rename = "extrudeMode2")]
    pub mode2: ExtrudeMode,
    pub distance2: Scalar,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_face2: Option<ElementRef>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Revolve parameters. Ported from OneCAD-CPP `RevolveParams`
/// (`OperationRecord.h:106-112`); SCHEMA §7.3 field names.
///
/// `profile` is a Rust-core typed input (as for Extrude). `axis` is
/// `Option<AxisRef>`; SCHEMA's `kind:"none"` maps to `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevolveParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<SketchRegionRef>,
    pub angle_deg: Scalar,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub axis: Option<AxisRef>,
    pub boolean_mode: BooleanMode,
    #[serde(
        rename = "targetBodyId",
        default,
        deserialize_with = "de_opt_body_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_body: Option<BodyId>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Fillet parameters (SPLIT from OneCAD-CPP `FilletChamferParams`
/// `OperationRecord.h:114-120`). SCHEMA §7.3 field names.
///
/// The redundant C++/SCHEMA `mode` field (`"Fillet"`) is dropped in favor of the
/// authoritative `opType` tag; if present on input it round-trips via `extra`.
/// `edge_ids` are TopoKeys/ElementIds (bare strings), matching SCHEMA `edgeIds`.
///
/// `edges` is the typed home for SCHEMA §7.3 fillet's per-edge `inputs[]`
/// semantic refs (one `ElementRef` per `edge_ids` entry): each carries the
/// operated body (`primary.body`) plus descriptor/anchor evidence so the edge is
/// repairable across parametric edits via the ladder. Empty for legacy/bare-id
/// fillets (then the operated body is bound at regen time). SCHEMA's `edgeIds`
/// stays a bare-string list — the semantic refs live in `inputs[]`, so no SCHEMA
/// amendment is required (unlike Extrude ToFace, which had no such home; M4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilletParams {
    pub radius: Scalar,
    pub edge_ids: Vec<ElementId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<ElementRef>,
    #[serde(default = "default_true")]
    pub chain_tangent_edges: bool,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Chamfer parameters (SPLIT from OneCAD-CPP `FilletChamferParams`). `radius`
/// doubles as the chamfer distance (OneCAD-CPP comment `OperationRecord.h:117`).
/// `edges` mirrors [`FilletParams::edges`] (typed per-edge semantic refs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChamferParams {
    pub radius: Scalar,
    pub edge_ids: Vec<ElementId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub edges: Vec<ElementRef>,
    #[serde(default = "default_true")]
    pub chain_tangent_edges: bool,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Shell parameters (OneCAD-CPP `ShellParams` `OperationRecord.h:122-125`).
/// `target_body` is the shelled body (C++ supplies it via the `BodyRef` input).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShellParams {
    pub thickness: Scalar,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_faces: Vec<ElementId>,
    #[serde(
        rename = "targetBodyId",
        default,
        deserialize_with = "de_opt_body_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub target_body: Option<BodyId>,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Standalone boolean parameters (OneCAD-CPP `BooleanParams`
/// `OperationRecord.h:127-132`; SCHEMA §7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BooleanParams {
    pub operation: BooleanOp,
    #[serde(rename = "targetBodyId")]
    pub target_body: BodyId,
    #[serde(rename = "toolBodyId")]
    pub tool_body: BodyId,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Linear pattern parameters (OneCAD-CPP `LinearPatternParams`
/// `OperationRecord.h:134-142`).
///
/// Discrepancy: C++ stores the direction as flat `dirX/dirY/dirZ`; per the task
/// ("Vec3 for triples") the Rust port uses a single `direction: Vec3`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinearPatternParams {
    #[serde(
        rename = "sourceBodyId",
        default,
        deserialize_with = "de_opt_body_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_body: Option<BodyId>,
    pub direction: Vec3,
    pub spacing: Scalar,
    pub count: u32,
    #[serde(default = "default_true")]
    pub fuse_result: bool,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Circular pattern parameters (OneCAD-CPP `CircularPatternParams`
/// `OperationRecord.h:171-182`).
///
/// Discrepancy: C++ stores flat `axisX/Y/Z` (point) and `axisDirX/Y/Z`; the Rust
/// port uses `axis_origin: Vec3` and `axis_direction: Vec3`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CircularPatternParams {
    #[serde(
        rename = "sourceBodyId",
        default,
        deserialize_with = "de_opt_body_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_body: Option<BodyId>,
    pub axis_origin: Vec3,
    pub axis_direction: Vec3,
    pub angle_deg: Scalar,
    pub count: u32,
    #[serde(default = "default_true")]
    pub fuse_result: bool,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Loft parameters (OneCAD-CPP `LoftParams` `OperationRecord.h:144-150`).
///
/// Discrepancy: C++ keeps parallel arrays `profileSketchIds` + `profileRegionIds`;
/// the Rust port pairs them into `profiles: Vec<SketchRegionRef>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoftParams {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub profiles: Vec<SketchRegionRef>,
    #[serde(default = "default_true")]
    pub is_solid: bool,
    #[serde(default)]
    pub is_ruled: bool,
    pub boolean_mode: BooleanMode,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Sweep parameters (OneCAD-CPP `SweepParams` `OperationRecord.h:152-158`).
/// `profile` pairs the C++ `profileSketchId`+`profileRegionId`; the path is a
/// sketch wire (`path_sketch`) or a body edge (`path_edge`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SweepParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<SketchRegionRef>,
    #[serde(
        rename = "pathSketchId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub path_sketch: Option<SketchId>,
    #[serde(
        rename = "pathEdgeId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub path_edge: Option<ElementId>,
    pub boolean_mode: BooleanMode,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}

/// Mirror-body parameters (OneCAD-CPP `MirrorBodyParams`
/// `OperationRecord.h:160-169`).
///
/// Discrepancy: C++ stores flat `planePointX/Y/Z` + `planeNormalX/Y/Z`; the Rust
/// port uses `plane_point: Vec3` + `plane_normal: Vec3`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorBodyParams {
    #[serde(
        rename = "sourceBodyId",
        default,
        deserialize_with = "de_opt_body_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub source_body: Option<BodyId>,
    pub plane_point: Vec3,
    pub plane_normal: Vec3,
    #[serde(default)]
    pub fuse_with_original: bool,
    #[serde(flatten, default, skip_serializing_if = "Extra::is_empty")]
    pub extra: Extra,
}
