//! Forward migration of older containers + the version-aware open flow.
//!
//! A [`MigrationRegistry`] chains version-to-version [`MigrationStep`]s applied to
//! the raw `document.json` [`Value`](serde_json::Value) **before** typed
//! deserialization (plan task 5; V1/V2 §9). Steps transform the document at two
//! levels: the whole-document value, and — with `RecordSchemaVersion` awareness —
//! each timeline record value.
//!
//! Open policy (V1/V2 §9.2/§9.3):
//!
//! * **Same version** → no migration, read/write.
//! * **Older version** → build a [`MigrationPlan`], apply it. If the chain does not
//!   reach the current version, or any step's confidence is below `High`, the
//!   document opens **read-only** with a [`MigrationReport`] (guided report).
//! * **Newer version** → best-effort **read-only**. Unknown ops ride through as
//!   [`Opaque`](crate::document::record::Operation::Opaque) frozen nodes and
//!   unknown fields via `extra` (the record codec already does this), so the
//!   document loads without a transform but is never written back.
//!
//! The v2.0 registry is an **empty chain** — the structure exists (and is tested
//! against synthetic future/legacy fixtures) so the later `.onecad` v1.1 importer
//! drops in as one registered step.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::document::Document;

use super::{document_io, Diagnostic, IoError, IoResult};

/// Confidence that a migration produced a faithful document (V1/V2 §9.4). Ordered
/// `Low < Medium < High`; anything below `High` forces a read-only open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MigrationConfidence {
    Low,
    Medium,
    High,
}

/// One version-to-version transform over the raw document JSON.
///
/// A step upgrades a document from [`from_version`](MigrationStep::from_version) to
/// [`to_version`](MigrationStep::to_version). It may rewrite the whole-document
/// value ([`migrate_document`](MigrationStep::migrate_document)) and, if it
/// [`touches_records`](MigrationStep::touches_records), each timeline record value
/// ([`migrate_record`](MigrationStep::migrate_record) — `RecordSchemaVersion`-aware).
// `from_version`/`to_version` describe the version range this step spans; they are
// plain accessors, not constructors — the `wrong_self_convention` lint's
// `from_*`-takes-no-self heuristic does not apply.
#[allow(clippy::wrong_self_convention)]
pub trait MigrationStep: Send + Sync {
    /// The `globalSchemaVersion` this step upgrades **from**.
    fn from_version(&self) -> u32;
    /// The `globalSchemaVersion` this step upgrades **to** (must exceed
    /// `from_version`).
    fn to_version(&self) -> u32;
    /// Confidence this step preserves document semantics.
    fn confidence(&self) -> MigrationConfidence;
    /// Rewrites the whole-document value in place.
    ///
    /// # Errors
    /// A human-facing message if the transform cannot be applied.
    fn migrate_document(&self, document: &mut Value) -> Result<(), String>;
    /// Whether this step also transforms individual timeline records.
    fn touches_records(&self) -> bool {
        false
    }
    /// Rewrites a single timeline record value in place (only called when
    /// [`touches_records`](MigrationStep::touches_records) is `true`). The record's
    /// `recordSchemaVersion` is available in `record["recordSchemaVersion"]`.
    ///
    /// # Errors
    /// A human-facing message if the record cannot be transformed.
    fn migrate_record(&self, record: &mut Value) -> Result<(), String> {
        let _ = record;
        Ok(())
    }
    /// Free-form notes surfaced in the [`MigrationReport`] (e.g. lossy fields).
    fn notes(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Read-only metadata about one planned step (no trait object — `Clone`/`Debug`
/// for the report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepMeta {
    pub from_version: u32,
    pub to_version: u32,
    pub confidence: MigrationConfidence,
}

/// A planned migration chain from a stored version up toward the current version
/// (V1/V2 §9.4 `MigrationPlan`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationPlan {
    /// The stored document's `globalSchemaVersion`.
    pub from_version: u32,
    /// The version this build targets.
    pub target_version: u32,
    /// The ordered chain of steps (empty when already current).
    pub steps: Vec<StepMeta>,
    /// Minimum confidence across the chain (`Low` if the chain is incomplete).
    pub confidence: MigrationConfidence,
    /// Whether any step transforms individual records.
    pub per_record: bool,
    /// Whether the chain reaches [`target_version`](MigrationPlan::target_version).
    pub complete: bool,
    /// Guided-report notes (missing-step gaps, per-step notes).
    pub notes: Vec<String>,
}

impl MigrationPlan {
    /// Whether applying this plan yields a fully-current, high-confidence document
    /// (⇒ the document may open read/write).
    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.complete && self.confidence == MigrationConfidence::High
    }
}

/// The migration outcome attached to a [`LoadOutcome`] (V1/V2 §9.4 guided report).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// The plan that was built (older-version open) — `None` slot unused for the
    /// newer-version case, which carries no plan.
    pub plan: Option<MigrationPlan>,
    /// Whether the transform chain was applied to the document value.
    pub applied: bool,
    /// Why the document was forced read-only, if it was.
    pub read_only_reason: Option<String>,
}

/// The result of opening a document from a container (plan task 5).
#[derive(Debug)]
pub struct LoadOutcome {
    /// The loaded (and, if older, migrated) document.
    pub document: Document,
    /// Whether the document is read-only (newer schema, or a low-confidence /
    /// incomplete migration).
    pub read_only: bool,
    /// Migration detail, when a migration was considered.
    pub migration_report: Option<MigrationReport>,
    /// Whether the container's caches are stale w.r.t. the loaded records
    /// (`opsHash` mismatch, or a migration changed the records). Caches must not be
    /// used when set (Invariant 7).
    pub stale_caches: bool,
    /// Non-fatal load observations (ops.jsonl divergence, migration notes, …).
    pub diagnostics: Vec<Diagnostic>,
}

/// An ordered registry of [`MigrationStep`]s keyed by `from_version`.
#[derive(Default)]
pub struct MigrationRegistry {
    steps: BTreeMap<u32, Box<dyn MigrationStep>>,
}

impl std::fmt::Debug for MigrationRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationRegistry")
            .field("from_versions", &self.steps.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl MigrationRegistry {
    /// An empty registry (the v2.0 default — no legacy chain yet).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a step, keyed by its `from_version`. A later registration at the
    /// same `from_version` replaces the earlier one.
    pub fn register(&mut self, step: Box<dyn MigrationStep>) {
        self.steps.insert(step.from_version(), step);
    }

    /// Builds a [`MigrationPlan`] from `from_version` up to `target_version`
    /// (pure metadata; does not mutate anything).
    #[must_use]
    pub fn plan(&self, from_version: u32, target_version: u32) -> MigrationPlan {
        let mut steps = Vec::new();
        let mut notes = Vec::new();
        let mut per_record = false;
        let mut cur = from_version;
        let mut complete = true;
        while cur < target_version {
            match self.steps.get(&cur) {
                Some(step) if step.to_version() > cur => {
                    steps.push(StepMeta {
                        from_version: step.from_version(),
                        to_version: step.to_version(),
                        confidence: step.confidence(),
                    });
                    notes.extend(step.notes());
                    per_record |= step.touches_records();
                    cur = step.to_version();
                }
                Some(_) => {
                    notes.push(format!(
                        "migration step from schema version {cur} does not advance the version"
                    ));
                    complete = false;
                    break;
                }
                None => {
                    notes.push(format!(
                        "no migration registered from schema version {cur} (target {target_version})"
                    ));
                    complete = false;
                    break;
                }
            }
        }
        let confidence = if complete {
            steps
                .iter()
                .map(|s| s.confidence)
                .min()
                .unwrap_or(MigrationConfidence::High)
        } else {
            MigrationConfidence::Low
        };
        MigrationPlan {
            from_version,
            target_version,
            steps,
            confidence,
            per_record,
            complete,
            notes,
        }
    }

    /// Applies the registered chain to `value`, transforming it in place from
    /// `from_version` toward `target_version` (stops where the chain runs out).
    ///
    /// # Errors
    /// [`IoError::Corrupt`] if a step's transform fails.
    pub fn apply(&self, value: &mut Value, from_version: u32, target_version: u32) -> IoResult<()> {
        let mut cur = from_version;
        while cur < target_version {
            let step = match self.steps.get(&cur) {
                Some(s) if s.to_version() > cur => s,
                _ => break,
            };
            step.migrate_document(value).map_err(|e| {
                IoError::Corrupt(format!(
                    "migration {}→{}: {e}",
                    step.from_version(),
                    step.to_version()
                ))
            })?;
            if step.touches_records() {
                if let Some(records) = value
                    .pointer_mut("/timeline/records")
                    .and_then(Value::as_array_mut)
                {
                    for record in records.iter_mut() {
                        step.migrate_record(record).map_err(|e| {
                            IoError::Corrupt(format!(
                                "migration {}→{} (record): {e}",
                                step.from_version(),
                                step.to_version()
                            ))
                        })?;
                    }
                }
            }
            cur = step.to_version();
        }
        Ok(())
    }
}

/// The migrated document value plus the derived open policy (before typed
/// deserialization). Consumed by the container reader.
#[derive(Debug)]
pub struct MigratedValue {
    /// The (possibly transformed) document JSON, ready for typed decode.
    pub value: Value,
    /// Whether the document must open read-only.
    pub read_only: bool,
    /// Whether a migration changed the records (⇒ caches are stale regardless of
    /// `opsHash`).
    pub records_changed: bool,
    /// Migration detail for the report.
    pub report: Option<MigrationReport>,
    /// Diagnostics to fold into the load outcome.
    pub diagnostics: Vec<Diagnostic>,
}

/// Runs the version-aware open policy over a raw document value (plan task 5).
///
/// * `stored_version` is the manifest's `globalSchemaVersion`.
/// * `target_version` is [`GLOBAL_SCHEMA_VERSION`](super::manifest::GLOBAL_SCHEMA_VERSION).
///
/// Never fails on a *policy* decision (read-only is an outcome, not an error);
/// only a hard transform failure yields [`IoError::Corrupt`].
pub fn open_policy(
    registry: &MigrationRegistry,
    mut value: Value,
    stored_version: u32,
    target_version: u32,
) -> IoResult<MigratedValue> {
    use std::cmp::Ordering;
    match stored_version.cmp(&target_version) {
        Ordering::Equal => Ok(MigratedValue {
            value,
            read_only: false,
            records_changed: false,
            report: None,
            diagnostics: Vec::new(),
        }),
        Ordering::Greater => {
            // Newer file: best-effort read-only, no transform (Opaque ride-through).
            let diag = Diagnostic::warning(
                "newer-schema-read-only",
                format!(
                    "document schema version {stored_version} is newer than this build's \
                     {target_version}; opened read-only (best effort)"
                ),
            );
            Ok(MigratedValue {
                value,
                read_only: true,
                records_changed: false,
                report: Some(MigrationReport {
                    plan: None,
                    applied: false,
                    read_only_reason: Some(diag.message.clone()),
                }),
                diagnostics: vec![diag],
            })
        }
        Ordering::Less => {
            let plan = registry.plan(stored_version, target_version);
            registry.apply(&mut value, stored_version, target_version)?;
            let applied = !plan.steps.is_empty();
            let read_only = !plan.is_lossless();
            let read_only_reason = read_only.then(|| {
                if !plan.complete {
                    format!(
                        "migration from schema version {stored_version} could not reach \
                         {target_version}; opened read-only"
                    )
                } else {
                    format!(
                        "migration from schema version {stored_version} is {:?}-confidence; \
                         opened read-only",
                        plan.confidence
                    )
                }
            });
            let mut diagnostics = Vec::new();
            if applied {
                diagnostics.push(Diagnostic::info(
                    "migration-applied",
                    format!(
                        "migrated document from schema version {stored_version} to {} \
                         ({} step(s), {:?} confidence)",
                        plan.steps.last().map_or(stored_version, |s| s.to_version),
                        plan.steps.len(),
                        plan.confidence
                    ),
                ));
            }
            if let Some(reason) = &read_only_reason {
                diagnostics.push(Diagnostic::warning("migration-read-only", reason.clone()));
            }
            Ok(MigratedValue {
                value,
                read_only,
                records_changed: applied,
                report: Some(MigrationReport {
                    plan: Some(plan),
                    applied,
                    read_only_reason,
                }),
                diagnostics,
            })
        }
    }
}

/// Convenience: run [`open_policy`] and then typed-decode the value into a
/// [`Document`], returning both the document and the policy metadata.
pub(crate) fn migrate_and_decode(
    registry: &MigrationRegistry,
    value: Value,
    stored_version: u32,
    target_version: u32,
) -> IoResult<(Document, MigratedValue)> {
    let migrated = open_policy(registry, value, stored_version, target_version)?;
    let document = document_io::document_from_value(migrated.value.clone())?;
    Ok((document, migrated))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic v0→v1 step: renames a top-level document field. High confidence.
    struct RenameStep;
    impl MigrationStep for RenameStep {
        fn from_version(&self) -> u32 {
            0
        }
        fn to_version(&self) -> u32 {
            1
        }
        fn confidence(&self) -> MigrationConfidence {
            MigrationConfidence::High
        }
        fn migrate_document(&self, document: &mut Value) -> Result<(), String> {
            if let Some(obj) = document.as_object_mut() {
                if let Some(v) = obj.remove("oldName") {
                    obj.insert("newName".into(), v);
                }
                obj.insert("schemaVersion".into(), Value::from(1u32));
            }
            Ok(())
        }
        fn notes(&self) -> Vec<String> {
            vec!["renamed oldName → newName".into()]
        }
    }

    /// A synthetic low-confidence step (forces read-only) that also touches records.
    struct LossyStep;
    impl MigrationStep for LossyStep {
        fn from_version(&self) -> u32 {
            0
        }
        fn to_version(&self) -> u32 {
            1
        }
        fn confidence(&self) -> MigrationConfidence {
            MigrationConfidence::Low
        }
        fn migrate_document(&self, _document: &mut Value) -> Result<(), String> {
            Ok(())
        }
        fn touches_records(&self) -> bool {
            true
        }
        fn migrate_record(&self, record: &mut Value) -> Result<(), String> {
            if let Some(obj) = record.as_object_mut() {
                obj.insert("migrated".into(), Value::Bool(true));
            }
            Ok(())
        }
    }

    #[test]
    fn empty_registry_same_version_is_read_write() {
        let reg = MigrationRegistry::new();
        let out = open_policy(&reg, serde_json::json!({"schemaVersion":1}), 1, 1).unwrap();
        assert!(!out.read_only);
        assert!(out.report.is_none());
    }

    #[test]
    fn newer_version_opens_read_only_without_transform() {
        let reg = MigrationRegistry::new();
        let v = serde_json::json!({"schemaVersion":5});
        let out = open_policy(&reg, v.clone(), 5, 1).unwrap();
        assert!(out.read_only);
        assert_eq!(out.value, v); // untouched
        assert_eq!(out.diagnostics[0].code, "newer-schema-read-only");
    }

    #[test]
    fn high_confidence_chain_migrates_read_write() {
        let mut reg = MigrationRegistry::new();
        reg.register(Box::new(RenameStep));
        let plan = reg.plan(0, 1);
        assert!(plan.is_lossless());
        let out = open_policy(&reg, serde_json::json!({"oldName":7}), 0, 1).unwrap();
        assert!(!out.read_only);
        assert!(out.records_changed);
        assert_eq!(out.value["newName"], serde_json::json!(7));
    }

    #[test]
    fn low_confidence_chain_forces_read_only_and_touches_records() {
        let mut reg = MigrationRegistry::new();
        reg.register(Box::new(LossyStep));
        let plan = reg.plan(0, 1);
        assert!(!plan.is_lossless());
        assert!(plan.per_record);
        let v = serde_json::json!({"timeline":{"records":[{"recordSchemaVersion":1}]}});
        let out = open_policy(&reg, v, 0, 1).unwrap();
        assert!(out.read_only);
        assert_eq!(
            out.value["timeline"]["records"][0]["migrated"],
            Value::Bool(true)
        );
    }

    #[test]
    fn incomplete_chain_is_low_confidence_read_only() {
        // Registry can only go 0→1 but the file is v0 and target is v2 (gap 1→2).
        let mut reg = MigrationRegistry::new();
        reg.register(Box::new(RenameStep));
        let plan = reg.plan(0, 2);
        assert!(!plan.complete);
        assert_eq!(plan.confidence, MigrationConfidence::Low);
        let out = open_policy(&reg, serde_json::json!({"oldName":1}), 0, 2).unwrap();
        assert!(out.read_only);
    }
}
