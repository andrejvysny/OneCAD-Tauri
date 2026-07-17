//! R-WP9 container IO integration + attack-surface suite.
//!
//! Covers: save→load structural equality; a deterministic container-shape insta
//! snapshot; the hostile-zip surface (path traversal, decompression bomb, garbage,
//! truncation, entry-count cap, random fuzz — none may panic); the derived
//! `ops.jsonl` reconciliation (integrity + content divergence → `document.json`
//! wins); version policy (newer → read-only; synthetic migration chain, low
//! confidence → read-only); and entry-hash tamper (authoritative → `Corrupt`,
//! cache → stale).

use std::io::{Cursor, Read, Write};
use std::path::Path;

use serde_json::Value;
use uuid::Uuid;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord,
};
use onecad_core::document::variables::Scalar;
use onecad_core::document::Document;
use onecad_core::ids::{BodyId, DocumentId, RecordId};
use onecad_core::io::container::{
    CacheRead, ContainerCaches, ContainerReader, ContainerWriter, SaveMeta, DOCUMENT_PATH,
    GEOMETRY_DIR, MANIFEST_PATH, MAX_DOCUMENT_BYTES, OPS_PATH,
};
use onecad_core::io::history_io;
use onecad_core::io::manifest::{Manifest, CONTAINER_VERSION, GLOBAL_SCHEMA_VERSION, MAGIC};
use onecad_core::io::migrate::{MigrationConfidence, MigrationRegistry, MigrationStep};
use onecad_core::io::IoError;

// ─────────────────────────────────────────────────────────────────────────────
// Fixtures + raw-zip helpers
// ─────────────────────────────────────────────────────────────────────────────

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

fn small_doc() -> Document {
    let mut d = Document::new(DocumentId(Uuid::from_u128(0xD0C)));
    d.timeline.insert_at_cursor(extrude(0x10, 10.0));
    d.timeline.insert_at_cursor(extrude(0x11, 5.0));
    d
}

fn meta() -> SaveMeta {
    SaveMeta {
        app_version: "0.1.0-test".into(),
        occt_fingerprint: Some("occt-7.9.3-fixed".into()),
        created: "2026-07-16T00:00:00Z".into(),
        modified: "2026-07-16T00:00:00Z".into(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Reads every file entry out of a container (decompressed).
fn read_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).unwrap();
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).unwrap();
        out.push((name, buf));
    }
    out
}

/// Repacks a list of (name, bytes) into a Stored zip at `path`.
fn write_entries(path: &Path, entries: &[(String, Vec<u8>)]) {
    build_raw_zip(
        path,
        &entries
            .iter()
            .map(|(n, b)| (n.as_str(), b.clone(), CompressionMethod::Stored))
            .collect::<Vec<_>>(),
    );
}

/// Builds a raw zip with explicit per-entry compression (for hostile crafting).
fn build_raw_zip(path: &Path, entries: &[(&str, Vec<u8>, CompressionMethod)]) {
    let file = std::fs::File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    for (name, bytes, method) in entries {
        let opts = zip::write::SimpleFileOptions::default().compression_method(*method);
        zip.start_file(*name, opts).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

fn entry_bytes<'a>(entries: &'a mut [(String, Vec<u8>)], name: &str) -> &'a mut Vec<u8> {
    &mut entries.iter_mut().find(|(n, _)| n == name).unwrap().1
}

fn manifest_of(entries: &[(String, Vec<u8>)]) -> Manifest {
    let bytes = &entries.iter().find(|(n, _)| n == MANIFEST_PATH).unwrap().1;
    serde_json::from_slice(bytes).unwrap()
}

fn set_manifest(entries: &mut [(String, Vec<u8>)], manifest: &Manifest) {
    *entry_bytes(entries, MANIFEST_PATH) = serde_json::to_vec(manifest).unwrap();
}

fn tmp() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

// ─────────────────────────────────────────────────────────────────────────────
// Happy path
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn save_load_structural_equality() {
    let dir = tmp();
    let path = dir.path().join("m.onecad");
    let doc = small_doc();
    ContainerWriter::save(&path, &doc, &ContainerCaches::none(), &meta()).unwrap();

    let loaded = ContainerReader::open(&path).unwrap();
    assert!(!loaded.outcome.read_only);
    assert!(!loaded.outcome.stale_caches);
    assert!(loaded.outcome.migration_report.is_none());
    assert!(loaded.outcome.diagnostics.is_empty());
    assert_eq!(
        serde_json::to_value(loaded.document()).unwrap(),
        serde_json::to_value(&doc).unwrap()
    );
}

#[test]
fn insta_snapshot_container_shape() {
    let dir = tmp();
    let path = dir.path().join("m.onecad");
    let doc = small_doc();
    let mut caches = ContainerCaches::none();
    caches
        .geometry
        .insert(BodyId(Uuid::from_u128(0xB0D1)), vec![1, 2, 3, 4, 5, 6]);
    caches.preview_png = Some(vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a]);
    ContainerWriter::save(&path, &doc, &caches, &meta()).unwrap();

    let loaded = ContainerReader::open(&path).unwrap();
    // Manifest shape + entry paths, with content hashes replaced by placeholders
    // (this is a container-shape freeze, not a byte-hash freeze — document bytes
    // are frozen separately by schema_freeze). Redacting manually keeps insta's
    // default feature set.
    let mut manifest = serde_json::to_value(&loaded.manifest).unwrap();
    manifest["opsHash"] = Value::from("[opsHash]");
    for entry in manifest["entries"].as_array_mut().unwrap() {
        entry["sha256"] = Value::from("[sha256]");
    }
    insta::assert_json_snapshot!("container_manifest_shape", manifest);
}

// ─────────────────────────────────────────────────────────────────────────────
// Attack surface — nothing hostile may panic; everything is a typed error
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn hostile_garbage_bytes_is_error_not_panic() {
    let dir = tmp();
    let path = dir.path().join("garbage.onecad");
    std::fs::write(&path, b"\x00\x01\x02not a zip at all\xff\xfe").unwrap();
    assert!(ContainerReader::open(&path).is_err());
}

#[test]
fn hostile_truncated_archive_is_error_not_panic() {
    let dir = tmp();
    let path = dir.path().join("trunc.onecad");
    ContainerWriter::save(&path, &small_doc(), &ContainerCaches::none(), &meta()).unwrap();
    let full = std::fs::read(&path).unwrap();
    std::fs::write(&path, &full[..full.len() / 2]).unwrap(); // chop the central directory
    assert!(ContainerReader::open(&path).is_err());
}

#[test]
fn hostile_path_traversal_entry_rejected() {
    let dir = tmp();
    let path = dir.path().join("evil.onecad");
    let manifest = valid_manifest_bytes();
    build_raw_zip(
        &path,
        &[
            (MANIFEST_PATH, manifest, CompressionMethod::Stored),
            (
                "../../etc/passwd",
                b"pwned".to_vec(),
                CompressionMethod::Stored,
            ),
            (DOCUMENT_PATH, b"{}".to_vec(), CompressionMethod::Stored),
        ],
    );
    assert!(matches!(
        ContainerReader::open(&path),
        Err(IoError::PathTraversal(_))
    ));
}

#[test]
fn hostile_decompression_bomb_hits_section_cap() {
    let dir = tmp();
    let path = dir.path().join("bomb.onecad");
    // document.json decompresses to just over its 64 MB cap, but Deflates tiny.
    let bomb = vec![b' '; (MAX_DOCUMENT_BYTES as usize) + 1];
    build_raw_zip(
        &path,
        &[
            (
                MANIFEST_PATH,
                valid_manifest_bytes(),
                CompressionMethod::Stored,
            ),
            (DOCUMENT_PATH, bomb, CompressionMethod::Deflated),
        ],
    );
    // On-disk file is tiny despite the 64 MB payload (proves it's a real bomb).
    assert!(std::fs::metadata(&path).unwrap().len() < 1024 * 1024);
    assert!(matches!(
        ContainerReader::open(&path),
        Err(IoError::TooLarge(_))
    ));
}

#[test]
fn hostile_entry_count_cap() {
    let dir = tmp();
    let path = dir.path().join("many.onecad");
    let mut entries: Vec<(&str, Vec<u8>, CompressionMethod)> = Vec::new();
    let names: Vec<String> = (0..10_001).map(|i| format!("d/{i}.bin")).collect();
    for n in &names {
        entries.push((n.as_str(), vec![0u8], CompressionMethod::Stored));
    }
    build_raw_zip(&path, &entries);
    assert!(matches!(
        ContainerReader::open(&path),
        Err(IoError::TooLarge(_))
    ));
}

#[test]
fn hostile_random_fuzz_never_panics() {
    let dir = tmp();
    let path = dir.path().join("fuzz.onecad");
    let mut state: u64 = 0x9E3779B97F4A7C15;
    for _ in 0..300 {
        let len = (xorshift(&mut state) % 600) as usize;
        let blob: Vec<u8> = (0..len)
            .map(|_| (xorshift(&mut state) & 0xff) as u8)
            .collect();
        std::fs::write(&path, &blob).unwrap();
        // Must return a typed error (or, vanishingly rarely, Ok) — never panic.
        let _ = ContainerReader::open(&path);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Derived ops.jsonl reconciliation — document.json always wins
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn ops_jsonl_integrity_mismatch_warns_document_wins() {
    let dir = tmp();
    let path = dir.path().join("m.onecad");
    let doc = small_doc();
    ContainerWriter::save(&path, &doc, &ContainerCaches::none(), &meta()).unwrap();

    // Corrupt the ops projection bytes but leave the manifest hash → integrity fails.
    let mut entries = read_entries(&path);
    entry_bytes(&mut entries, OPS_PATH).extend_from_slice(b"\n{tampered}\n");
    write_entries(&path, &entries);

    let loaded = ContainerReader::open(&path).unwrap();
    assert!(loaded
        .outcome
        .diagnostics
        .iter()
        .any(|d| d.code == "ops-jsonl-integrity"));
    // document.json is authoritative and unaffected.
    assert_eq!(
        serde_json::to_value(loaded.document()).unwrap(),
        serde_json::to_value(&doc).unwrap()
    );
}

#[test]
fn ops_jsonl_content_divergence_warns_document_wins() {
    let dir = tmp();
    let path = dir.path().join("m.onecad");
    let doc = small_doc();
    ContainerWriter::save(&path, &doc, &ContainerCaches::none(), &meta()).unwrap();

    // Replace ops.jsonl with a *valid* but shorter projection (1 of 2 records),
    // and re-hash it in the manifest so integrity PASSES — exercising the content
    // cross-validation path.
    let mut entries = read_entries(&path);
    let short = history_io::serialize_ops_jsonl(&doc.timeline.records()[..1]).unwrap();
    let short_hash = sha256_hex(&short);
    *entry_bytes(&mut entries, OPS_PATH) = short;
    let mut manifest = manifest_of(&entries);
    for e in &mut manifest.entries {
        if e.path == OPS_PATH {
            e.sha256 = short_hash.clone();
        }
    }
    set_manifest(&mut entries, &manifest);
    write_entries(&path, &entries);

    let loaded = ContainerReader::open(&path).unwrap();
    assert!(loaded
        .outcome
        .diagnostics
        .iter()
        .any(|d| d.code == "ops-jsonl-divergence"));
    assert_eq!(loaded.document().timeline.len(), 2); // document.json wins
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry-hash tamper: authoritative → Corrupt, cache → stale
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn tamper_authoritative_document_is_corrupt() {
    let dir = tmp();
    let path = dir.path().join("m.onecad");
    ContainerWriter::save(&path, &small_doc(), &ContainerCaches::none(), &meta()).unwrap();

    let mut entries = read_entries(&path);
    entry_bytes(&mut entries, DOCUMENT_PATH).push(b' '); // manifest hash now stale
    write_entries(&path, &entries);

    assert!(matches!(
        ContainerReader::open(&path),
        Err(IoError::Corrupt(_))
    ));
}

#[test]
fn tamper_cache_blob_is_stale_not_corrupt() {
    let dir = tmp();
    let path = dir.path().join("m.onecad");
    let doc = small_doc();
    let body = BodyId(Uuid::from_u128(0xB0D1));
    let mut caches = ContainerCaches::none();
    caches.geometry.insert(body, vec![1, 2, 3, 4]);
    ContainerWriter::save(&path, &doc, &caches, &meta()).unwrap();

    let geo_path = format!("{GEOMETRY_DIR}{body}.brep");
    let mut entries = read_entries(&path);
    entry_bytes(&mut entries, &geo_path).push(0xFF); // corrupt cache bytes only
    write_entries(&path, &entries);

    // Records untouched → opsHash still fresh → the document loads clean...
    let loaded = ContainerReader::open(&path).unwrap();
    assert!(!loaded.outcome.stale_caches);
    // ...but the tampered cache blob reads back Stale (never Corrupt, never used).
    assert_eq!(loaded.read_cache(&geo_path).unwrap(), CacheRead::Stale);
}

// ─────────────────────────────────────────────────────────────────────────────
// Version policy: newer → read-only; synthetic migration chain
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn newer_schema_version_opens_read_only() {
    let dir = tmp();
    let path = dir.path().join("future.onecad");
    ContainerWriter::save(&path, &small_doc(), &ContainerCaches::none(), &meta()).unwrap();

    let mut entries = read_entries(&path);
    let mut manifest = manifest_of(&entries);
    manifest.global_schema_version = GLOBAL_SCHEMA_VERSION + 4; // from the future
    set_manifest(&mut entries, &manifest);
    write_entries(&path, &entries);

    let loaded = ContainerReader::open(&path).unwrap();
    assert!(loaded.outcome.read_only);
    assert!(loaded
        .outcome
        .diagnostics
        .iter()
        .any(|d| d.code == "newer-schema-read-only"));
}

/// A synthetic v0→v1 migration step (no structural change) at a chosen confidence.
struct FakeV0toV1(MigrationConfidence);
impl MigrationStep for FakeV0toV1 {
    fn from_version(&self) -> u32 {
        0
    }
    fn to_version(&self) -> u32 {
        1
    }
    fn confidence(&self) -> MigrationConfidence {
        self.0
    }
    fn migrate_document(&self, document: &mut Value) -> Result<(), String> {
        // Stamp the target version; the v1 shape is otherwise already compatible.
        if let Some(obj) = document.as_object_mut() {
            obj.insert("schemaVersion".into(), Value::from(1u32));
        }
        Ok(())
    }
    fn notes(&self) -> Vec<String> {
        vec!["synthetic v0→v1 fixture step".into()]
    }
}

fn write_v0_container(path: &Path) {
    ContainerWriter::save(path, &small_doc(), &ContainerCaches::none(), &meta()).unwrap();
    let mut entries = read_entries(path);
    let mut manifest = manifest_of(&entries);
    manifest.global_schema_version = 0; // pretend it's a legacy v0 file
    set_manifest(&mut entries, &manifest);
    write_entries(path, &entries);
}

#[test]
fn synthetic_migration_high_confidence_read_write() {
    let dir = tmp();
    let path = dir.path().join("legacy.onecad");
    write_v0_container(&path);

    let mut registry = MigrationRegistry::new();
    registry.register(Box::new(FakeV0toV1(MigrationConfidence::High)));

    let loaded = ContainerReader::open_with_registry(&path, &registry).unwrap();
    assert!(!loaded.outcome.read_only);
    // Migration invalidates caches (records considered changed).
    assert!(loaded.outcome.stale_caches);
    let report = loaded.outcome.migration_report.unwrap();
    assert!(report.applied);
    assert!(report.plan.unwrap().is_lossless());
}

#[test]
fn synthetic_migration_low_confidence_forces_read_only() {
    let dir = tmp();
    let path = dir.path().join("legacy.onecad");
    write_v0_container(&path);

    let mut registry = MigrationRegistry::new();
    registry.register(Box::new(FakeV0toV1(MigrationConfidence::Low)));

    let loaded = ContainerReader::open_with_registry(&path, &registry).unwrap();
    assert!(loaded.outcome.read_only);
    let report = loaded.outcome.migration_report.unwrap();
    assert_eq!(report.plan.unwrap().confidence, MigrationConfidence::Low);
    assert!(report.read_only_reason.is_some());
}

#[test]
fn legacy_version_with_empty_registry_is_read_only() {
    // No migration path registered at all (the v2.0 default) → best-effort read-only.
    let dir = tmp();
    let path = dir.path().join("legacy.onecad");
    write_v0_container(&path);
    let loaded = ContainerReader::open(&path).unwrap();
    assert!(loaded.outcome.read_only);
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn valid_manifest_bytes() -> Vec<u8> {
    let manifest = Manifest {
        magic: MAGIC.to_string(),
        container_version: CONTAINER_VERSION,
        global_schema_version: GLOBAL_SCHEMA_VERSION,
        app_version: String::new(),
        occt_fingerprint: None,
        document_id: DocumentId(Uuid::from_u128(1)),
        created: String::new(),
        modified: String::new(),
        ops_hash: String::new(),
        entries: Vec::new(),
        extra: Default::default(),
    };
    serde_json::to_vec(&manifest).unwrap()
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}
