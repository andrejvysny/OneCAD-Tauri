//! The v2 `.onecad` container (a ZIP archive): atomic writer + guarded reader.
//!
//! # Sections
//!
//! | path | class | cap | codec |
//! |------|-------|-----|-------|
//! | `manifest.json` | index (identity-validated) | 64 MB | Deflated |
//! | `document.json` | **authoritative** (hash → `Corrupt`) | 64 MB | Deflated |
//! | `timeline/ops.jsonl` | derived projection (mismatch → warn) | 256 MB | Deflated |
//! | `geometry/<bodyId>.brep` | cache (mismatch → stale) | 1 GB | Stored |
//! | `meshes/<bodyId>.<lod>.mesh` | cache | 1 GB | Stored |
//! | `checkpoints/<step>.json` / `.bin` | cache | 1 GB | Stored |
//! | `preview.png` | cache | 1 GB | Stored |
//!
//! Whole-container caps: **4 GB** total (declared uncompressed) and **10 000**
//! entries. Text sections are Deflated (compressible JSON); opaque binary caches
//! are Stored (already compact — deflating burns CPU for ~nothing). Entries under
//! 64 bytes are Stored regardless (deflate framing exceeds any saving).
//!
//! # Atomic save
//!
//! [`ContainerWriter::save`] writes to a sibling temp file (`.<name>.tmp-<nonce>`),
//! `fsync`s it, atomically `rename`s over the target, then `fsync`s the parent
//! directory (durability of the rename on macOS). The original target is never
//! touched until the rename, and a leftover temp from a crash-before-rename is
//! cleaned up on the next save. So a crash at any point leaves *either* the intact
//! previous file *or* the fully-written new one — never a torn container.
//!
//! # Attack surface
//!
//! [`ContainerReader::open`] treats the archive as adversarial: it enforces the
//! entry-count + total-size caps, rejects path-traversal entry names (`../`,
//! absolute paths, symlink-ish names) via [`zip`]'s `enclosed_name`, and bounds
//! every decompressed read (`Take`-limited — a zip bomb hits its per-section cap
//! and yields [`IoError::TooLarge`] rather than exhausting memory). No hostile
//! input path panics.

use std::collections::BTreeMap;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use zip::result::ZipError;
use zip::{CompressionMethod, ZipArchive};

use crate::document::Document;
use crate::ids::BodyId;

use super::manifest::{Manifest, ManifestEntry, CONTAINER_VERSION, GLOBAL_SCHEMA_VERSION, MAGIC};
use super::migrate::{LoadOutcome, MigrationRegistry};
use super::{document_io, history_io, sha256_hex, Diagnostic, IoError, IoResult};

// ── Section paths ────────────────────────────────────────────────────────────

/// The manifest index path.
pub const MANIFEST_PATH: &str = "manifest.json";
/// The authoritative document body path.
pub const DOCUMENT_PATH: &str = "document.json";
/// The derived timeline projection path.
pub const OPS_PATH: &str = "timeline/ops.jsonl";
/// Geometry (BREP) cache directory prefix.
pub const GEOMETRY_DIR: &str = "geometry/";
/// Mesh (MESH1) cache directory prefix.
pub const MESHES_DIR: &str = "meshes/";
/// Checkpoint artifact directory prefix.
pub const CHECKPOINTS_DIR: &str = "checkpoints/";
/// Optional preview thumbnail path.
pub const PREVIEW_PATH: &str = "preview.png";

// ── Caps (decompression-bomb / DoS guards) ───────────────────────────────────

const MB: u64 = 1024 * 1024;
/// Per-section cap for `manifest.json`.
pub const MAX_MANIFEST_BYTES: u64 = 64 * MB;
/// Per-section cap for `document.json`.
pub const MAX_DOCUMENT_BYTES: u64 = 64 * MB;
/// Per-section cap for `timeline/ops.jsonl`.
pub const MAX_OPS_BYTES: u64 = 256 * MB;
/// Per-section cap for any cache blob (geometry / mesh / checkpoint / preview).
pub const MAX_BLOB_BYTES: u64 = 1024 * MB;
/// Whole-container cap on total declared uncompressed size.
pub const MAX_TOTAL_BYTES: u64 = 4096 * MB;
/// Whole-container cap on entry count.
pub const MAX_ENTRIES: usize = 10_000;
/// Below this size, entries are Stored (deflate framing is not worth it).
const STORE_THRESHOLD: usize = 64;

// ─────────────────────────────────────────────────────────────────────────────
// Writer inputs
// ─────────────────────────────────────────────────────────────────────────────

/// A mesh cache blob (`meshes/<bodyId>.<lod>.mesh`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeshCache {
    /// The body the mesh belongs to.
    pub body: BodyId,
    /// The level-of-detail tag (e.g. `"coarse"`) — used verbatim in the filename.
    pub lod: String,
    /// The MESH1 bytes (opaque to the core).
    pub bytes: Vec<u8>,
}

/// A checkpoint artifact pair (`checkpoints/<step>.json` + optional `.bin`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointCache {
    /// The timeline step the checkpoint represents.
    pub step: usize,
    /// The envelope/metadata JSON bytes.
    pub json: Vec<u8>,
    /// The payload (BREP etc.) bytes, if any.
    pub bin: Option<Vec<u8>>,
}

/// Optional cache artifacts to embed alongside the document. Caches degrade
/// performance, never correctness — a reader may ignore all of them (Invariant 7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContainerCaches {
    /// Per-body BREP blobs (`geometry/<bodyId>.brep`).
    pub geometry: BTreeMap<BodyId, Vec<u8>>,
    /// Per-body/LOD mesh blobs.
    pub meshes: Vec<MeshCache>,
    /// Per-step checkpoint artifacts.
    pub checkpoints: Vec<CheckpointCache>,
    /// Optional preview thumbnail (`preview.png`).
    pub preview_png: Option<Vec<u8>>,
}

impl ContainerCaches {
    /// An empty cache set (a document-only container).
    #[must_use]
    pub fn none() -> Self {
        Self::default()
    }

    /// True iff there are no cache artifacts.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.geometry.is_empty()
            && self.meshes.is_empty()
            && self.checkpoints.is_empty()
            && self.preview_png.is_none()
    }
}

/// Caller-supplied manifest metadata (provenance). Timestamps are RFC3339 strings
/// passed IN — the pure core never reads the wall clock.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveMeta {
    /// The authoring app version.
    pub app_version: String,
    /// The OCCT fingerprint the caches were produced under, if any.
    pub occt_fingerprint: Option<String>,
    /// RFC3339 creation timestamp.
    pub created: String,
    /// RFC3339 last-modified timestamp.
    pub modified: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Writer
// ─────────────────────────────────────────────────────────────────────────────

/// One in-memory section to write: its path and uncompressed bytes.
struct Section {
    path: String,
    bytes: Vec<u8>,
}

/// Global temp-file nonce counter (uniqueness across concurrent saves in-process).
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Writes v2 `.onecad` containers atomically.
#[derive(Debug, Default)]
pub struct ContainerWriter;

impl ContainerWriter {
    /// Atomically writes `document` (+ optional `caches`) to `path`.
    ///
    /// # Errors
    /// [`IoError`] on a serialization or filesystem failure. On any failure the
    /// target file is left untouched and the temp file is removed.
    pub fn save(
        path: &Path,
        document: &Document,
        caches: &ContainerCaches,
        meta: &SaveMeta,
    ) -> IoResult<()> {
        Self::save_inner(path, document, caches, meta, true).map(|_| ())
    }

    /// Crash-simulation hook: writes + fsyncs the temp file but **skips** the
    /// rename, leaving the target untouched and the temp on disk. Returns the temp
    /// path. Used to prove atomicity (a crash between temp-write and rename leaves
    /// the previous container intact); the leftover is cleaned up by the next
    /// [`save`](ContainerWriter::save). Test-only (compiled only under `cfg(test)`,
    /// so it is never dead code in release/consumer builds).
    #[cfg(test)]
    pub(crate) fn save_leaving_temp(
        path: &Path,
        document: &Document,
        caches: &ContainerCaches,
        meta: &SaveMeta,
    ) -> IoResult<PathBuf> {
        Self::save_inner(path, document, caches, meta, false)
    }

    fn save_inner(
        path: &Path,
        document: &Document,
        caches: &ContainerCaches,
        meta: &SaveMeta,
        commit: bool,
    ) -> IoResult<PathBuf> {
        let sections = build_sections(document, caches)?;
        let manifest = build_manifest(document, meta, &sections);
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| IoError::Corrupt(format!("manifest serialize: {e}")))?;

        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = temp_path(path);

        // Write the archive to the temp file; on any error, clean the temp up.
        if let Err(e) = write_archive(&tmp, &manifest_bytes, &sections) {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }

        if !commit {
            return Ok(tmp);
        }

        // Best-effort cleanup of stale temps from a prior crash (not our own).
        cleanup_stale_temps(path, &tmp);
        std::fs::rename(&tmp, path).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            IoError::Io(format!("rename temp → target: {e}"))
        })?;
        fsync_dir(parent);
        Ok(path.to_path_buf())
    }
}

/// Builds the section list (authoritative + derived + caches) with validated
/// names.
fn build_sections(document: &Document, caches: &ContainerCaches) -> IoResult<Vec<Section>> {
    let mut sections = Vec::new();
    sections.push(Section {
        path: DOCUMENT_PATH.to_string(),
        bytes: document_io::serialize_document(document)?,
    });
    sections.push(Section {
        path: OPS_PATH.to_string(),
        bytes: history_io::serialize_ops_jsonl(document.timeline.records())?,
    });
    for (body, bytes) in &caches.geometry {
        sections.push(Section {
            path: format!("{GEOMETRY_DIR}{body}.brep"),
            bytes: bytes.clone(),
        });
    }
    for mesh in &caches.meshes {
        let path = format!("{MESHES_DIR}{}.{}.mesh", mesh.body, mesh.lod);
        guard_authored_name(&path)?;
        sections.push(Section {
            path,
            bytes: mesh.bytes.clone(),
        });
    }
    for cp in &caches.checkpoints {
        sections.push(Section {
            path: format!("{CHECKPOINTS_DIR}{}.json", cp.step),
            bytes: cp.json.clone(),
        });
        if let Some(bin) = &cp.bin {
            sections.push(Section {
                path: format!("{CHECKPOINTS_DIR}{}.bin", cp.step),
                bytes: bin.clone(),
            });
        }
    }
    if let Some(png) = &caches.preview_png {
        sections.push(Section {
            path: PREVIEW_PATH.to_string(),
            bytes: png.clone(),
        });
    }
    Ok(sections)
}

/// Rejects an authored (writer-side) entry name that would escape the archive
/// root — defense in depth against a hostile `lod`/component leaking `/` or `..`.
fn guard_authored_name(name: &str) -> IoResult<()> {
    if name.contains("..") || name.contains('\\') || name.starts_with('/') {
        return Err(IoError::PathTraversal(name.to_string()));
    }
    // Exactly one path component beyond the known prefix (no injected separators).
    let component_count = name.split('/').count();
    if component_count > 2 {
        return Err(IoError::PathTraversal(name.to_string()));
    }
    Ok(())
}

/// Assembles the manifest from the sections (hashing each entry's bytes).
fn build_manifest(document: &Document, meta: &SaveMeta, sections: &[Section]) -> Manifest {
    let entries = sections
        .iter()
        .map(|s| ManifestEntry {
            path: s.path.clone(),
            sha256: sha256_hex(&s.bytes),
        })
        .collect();
    Manifest {
        magic: MAGIC.to_string(),
        container_version: CONTAINER_VERSION,
        global_schema_version: document.schema_version,
        app_version: meta.app_version.clone(),
        occt_fingerprint: meta.occt_fingerprint.clone(),
        document_id: document.id,
        created: meta.created.clone(),
        modified: meta.modified.clone(),
        ops_hash: history_io::ops_hash(document.timeline.records()),
        entries,
        extra: Default::default(),
    }
}

/// Writes the manifest + sections into a fresh zip at `tmp`, fsyncing the file.
fn write_archive(tmp: &Path, manifest_bytes: &[u8], sections: &[Section]) -> IoResult<()> {
    let file = std::fs::File::create(tmp)?;
    let mut zip = zip::ZipWriter::new(file);

    write_zip_entry(&mut zip, MANIFEST_PATH, manifest_bytes)?;
    for s in sections {
        write_zip_entry(&mut zip, &s.path, &s.bytes)?;
    }
    let file = zip
        .finish()
        .map_err(|e| IoError::Io(format!("zip finish: {e}")))?;
    file.sync_all()?;
    Ok(())
}

/// Writes one zip entry with the size/type-appropriate compression method and a
/// fixed timestamp (deterministic output).
fn write_zip_entry<W: Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    name: &str,
    bytes: &[u8],
) -> IoResult<()> {
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(compression_for(name, bytes.len()))
        .last_modified_time(zip::DateTime::default());
    zip.start_file(name, options)
        .map_err(|e| IoError::Io(format!("zip start_file {name}: {e}")))?;
    zip.write_all(bytes)
        .map_err(|e| IoError::Io(format!("zip write {name}: {e}")))?;
    Ok(())
}

/// Deflate compressible text sections; Store opaque binary caches and tiny files.
fn compression_for(name: &str, len: usize) -> CompressionMethod {
    if len < STORE_THRESHOLD {
        return CompressionMethod::Stored;
    }
    let is_text = name == MANIFEST_PATH
        || name == DOCUMENT_PATH
        || name == OPS_PATH
        || name.ends_with(".json");
    if is_text {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    }
}

/// A unique sibling temp path for `path` (`.<name>.tmp-<pid>-<nanos>-<ctr>`).
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "container".into());
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let ctr = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path.with_file_name(format!(".{name}.tmp-{pid}-{nanos}-{ctr}"))
}

/// The temp-file prefix for a target (used to find/clean stale temps).
fn temp_prefix(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "container".into());
    format!(".{name}.tmp-")
}

/// Removes leftover temp files for `target` (from a prior crash), except `keep`.
fn cleanup_stale_temps(target: &Path, keep: &Path) {
    let Some(parent) = target.parent().or_else(|| Some(Path::new("."))) else {
        return;
    };
    let prefix = temp_prefix(target);
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p == keep {
            continue;
        }
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with(prefix.as_str())
        {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// fsyncs a directory so a rename inside it is durable (macOS/Unix). A no-op / best
/// effort elsewhere.
fn fsync_dir(dir: &Path) {
    #[cfg(unix)]
    {
        if let Ok(f) = std::fs::File::open(dir) {
            let _ = f.sync_all();
        }
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Reader
// ─────────────────────────────────────────────────────────────────────────────

/// The result of reading a cache blob on demand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheRead {
    /// The blob was present and its hash matched the manifest.
    Present(Vec<u8>),
    /// The blob is stale (container-level `opsHash` mismatch, or its own hash
    /// no longer matches the manifest) — must not be used (Invariant 7).
    Stale,
    /// No such cache entry in this container.
    Missing,
}

/// An opened container: the manifest, the load outcome (document + policy), and
/// lazy access to cache blobs.
#[derive(Debug)]
pub struct LoadedContainer {
    /// The parsed, identity-validated manifest.
    pub manifest: Manifest,
    /// The load outcome: the (migrated) document, read-only flag, migration report,
    /// stale-caches flag and diagnostics.
    pub outcome: LoadOutcome,
    /// The container path (reopened for lazy cache reads).
    source: PathBuf,
}

impl LoadedContainer {
    /// The loaded document.
    #[must_use]
    pub fn document(&self) -> &Document {
        &self.outcome.document
    }

    /// The cache entries listed in the manifest (everything but the authoritative
    /// document and the derived ops projection).
    #[must_use]
    pub fn cache_entries(&self) -> Vec<&ManifestEntry> {
        self.manifest
            .entries
            .iter()
            .filter(|e| e.path != DOCUMENT_PATH && e.path != OPS_PATH)
            .collect()
    }

    /// Reads a cache blob by its archive path, on demand.
    ///
    /// Returns [`CacheRead::Stale`] without touching the archive when the
    /// container's caches are known stale (`opsHash` mismatch / post-migration), or
    /// when the blob's own hash no longer matches the manifest. Genuine IO / path
    /// / size failures are [`IoError`].
    ///
    /// # Errors
    /// [`IoError`] on a filesystem, traversal or size-cap failure.
    pub fn read_cache(&self, path: &str) -> IoResult<CacheRead> {
        if self.outcome.stale_caches {
            return Ok(CacheRead::Stale);
        }
        let Some(entry) = self.manifest.entry(path) else {
            return Ok(CacheRead::Missing);
        };
        if path == DOCUMENT_PATH || path == OPS_PATH {
            return Ok(CacheRead::Missing); // not a cache
        }
        let file = std::fs::File::open(&self.source)?;
        let mut archive = ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| IoError::Corrupt(format!("reopen archive: {e}")))?;
        match read_named(&mut archive, path, MAX_BLOB_BYTES)? {
            Some(bytes) => {
                if sha256_hex(&bytes) == entry.sha256 {
                    Ok(CacheRead::Present(bytes))
                } else {
                    Ok(CacheRead::Stale)
                }
            }
            None => Ok(CacheRead::Missing),
        }
    }
}

/// Reads v2 `.onecad` containers with attack-surface guards.
#[derive(Debug, Default)]
pub struct ContainerReader;

impl ContainerReader {
    /// Opens a container with the default (empty v2.0) migration registry.
    ///
    /// # Errors
    /// [`IoError`] on a malformed / hostile / corrupt archive.
    pub fn open(path: &Path) -> IoResult<LoadedContainer> {
        Self::open_with_registry(path, &MigrationRegistry::new())
    }

    /// Opens a container, applying `registry`'s migration chain to older documents.
    ///
    /// # Errors
    /// [`IoError`] on a malformed / hostile / corrupt archive, or a hard migration
    /// failure. A newer-schema or low-confidence document is *not* an error — it
    /// opens read-only (see [`LoadOutcome`]).
    pub fn open_with_registry(
        path: &Path,
        registry: &MigrationRegistry,
    ) -> IoResult<LoadedContainer> {
        let file = std::fs::File::open(path)?;
        let mut archive = ZipArchive::new(std::io::BufReader::new(file))
            .map_err(|e| IoError::Corrupt(format!("open archive: {e}")))?;

        guard_directory(&mut archive)?;

        // Manifest (index) — identity-validated.
        let manifest_bytes = read_named(&mut archive, MANIFEST_PATH, MAX_MANIFEST_BYTES)?
            .ok_or_else(|| IoError::Corrupt("missing manifest.json".into()))?;
        let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
            .map_err(|e| IoError::Corrupt(format!("manifest parse: {e}")))?;
        manifest.validate_identity()?;

        // Authoritative document body.
        let doc_bytes = read_named(&mut archive, DOCUMENT_PATH, MAX_DOCUMENT_BYTES)?
            .ok_or_else(|| IoError::Corrupt("missing document.json".into()))?;
        verify_authoritative(&manifest, DOCUMENT_PATH, &doc_bytes)?;
        let doc_value = document_io::parse_value(&doc_bytes)?;

        // Version-aware migration + typed decode.
        let (document, migrated) = super::migrate::migrate_and_decode(
            registry,
            doc_value,
            manifest.global_schema_version,
            GLOBAL_SCHEMA_VERSION,
        )?;

        let mut diagnostics = migrated.diagnostics;

        // Derived ops.jsonl projection: verify integrity + cross-validate content.
        cross_validate_ops(&mut archive, &manifest, &document, &mut diagnostics)?;

        // Cache staleness: opsHash mismatch, or a migration changed the records.
        let recomputed = history_io::ops_hash(document.timeline.records());
        let stale_caches = migrated.records_changed || recomputed != manifest.ops_hash;
        if stale_caches {
            diagnostics.push(Diagnostic::warning(
                "stale-caches",
                "container caches are stale (opsHash mismatch); ignoring geometry/mesh/checkpoint caches",
            ));
        }

        let outcome = LoadOutcome {
            document,
            read_only: migrated.read_only,
            migration_report: migrated.report,
            stale_caches,
            diagnostics,
        };
        Ok(LoadedContainer {
            manifest,
            outcome,
            source: path.to_path_buf(),
        })
    }
}

/// Validates the archive directory up front: entry count, path traversal, and the
/// total declared-uncompressed-size cap. Runs before any decompression.
fn guard_directory<R: Read + Seek>(archive: &mut ZipArchive<R>) -> IoResult<()> {
    if archive.len() > MAX_ENTRIES {
        return Err(IoError::TooLarge(format!(
            "{} entries exceeds the {MAX_ENTRIES} cap",
            archive.len()
        )));
    }
    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let file = archive
            .by_index(i)
            .map_err(|e| IoError::Corrupt(format!("entry {i}: {e}")))?;
        if file.is_dir() {
            continue;
        }
        // `enclosed_name` returns None for any name that would escape the root
        // (absolute, `..`, drive-relative). A None here is a traversal attempt.
        if file.enclosed_name().is_none() {
            return Err(IoError::PathTraversal(file.name().to_string()));
        }
        if file.name().contains('\\') {
            return Err(IoError::PathTraversal(file.name().to_string()));
        }
        total = total.saturating_add(file.size());
        if total > MAX_TOTAL_BYTES {
            return Err(IoError::TooLarge(format!(
                "declared uncompressed size exceeds the {MAX_TOTAL_BYTES}-byte cap"
            )));
        }
    }
    Ok(())
}

/// Reads a named entry, bounding the decompressed size to `cap`. `Ok(None)` when
/// absent.
fn read_named<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    name: &str,
    cap: u64,
) -> IoResult<Option<Vec<u8>>> {
    match archive.by_name(name) {
        Ok(file) => {
            if file.enclosed_name().is_none() {
                return Err(IoError::PathTraversal(name.to_string()));
            }
            Ok(Some(read_capped(file, cap, name)?))
        }
        Err(ZipError::FileNotFound) => Ok(None),
        Err(e) => Err(IoError::Corrupt(format!("read {name}: {e}"))),
    }
}

/// Reads at most `cap` bytes from `r`; more than `cap` is [`IoError::TooLarge`].
fn read_capped<R: Read>(r: R, cap: u64, what: &str) -> IoResult<Vec<u8>> {
    let mut buf = Vec::new();
    // Read one past the cap: if we get cap+1 bytes, the source is over budget.
    r.take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|e| IoError::Corrupt(format!("{what} read: {e}")))?;
    if buf.len() as u64 > cap {
        return Err(IoError::TooLarge(format!(
            "{what} exceeds its {cap}-byte cap"
        )));
    }
    Ok(buf)
}

/// Verifies an authoritative entry's bytes against the manifest hash. A mismatch
/// (or a missing manifest entry) is [`IoError::Corrupt`].
fn verify_authoritative(manifest: &Manifest, path: &str, bytes: &[u8]) -> IoResult<()> {
    let entry = manifest
        .entry(path)
        .ok_or_else(|| IoError::Corrupt(format!("manifest missing entry for {path}")))?;
    if sha256_hex(bytes) != entry.sha256 {
        return Err(IoError::Corrupt(format!("{path} hash mismatch (tampered)")));
    }
    Ok(())
}

/// Reads + integrity-checks + content-cross-validates the derived `ops.jsonl`
/// against the authoritative document, appending a `Warning` on any problem.
/// `document.json` always wins — this never fails the load.
fn cross_validate_ops<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    manifest: &Manifest,
    document: &Document,
    diagnostics: &mut Vec<Diagnostic>,
) -> IoResult<()> {
    let Some(ops_bytes) = read_named(archive, OPS_PATH, MAX_OPS_BYTES)? else {
        return Ok(()); // optional projection absent
    };
    // Integrity of the derived section (bit-rot / tamper). Not authoritative → warn.
    if let Some(entry) = manifest.entry(OPS_PATH) {
        if sha256_hex(&ops_bytes) != entry.sha256 {
            diagnostics.push(Diagnostic::warning(
                "ops-jsonl-integrity",
                "timeline/ops.jsonl hash mismatch; using document.json (authoritative)",
            ));
            return Ok(());
        }
    }
    match history_io::parse_ops_jsonl(&ops_bytes) {
        Ok(lines) => {
            if let Some(diag) = history_io::cross_validate(document.timeline.records(), &lines) {
                diagnostics.push(diag);
            }
        }
        Err(_) => diagnostics.push(Diagnostic::warning(
            "ops-jsonl-integrity",
            "timeline/ops.jsonl is unparseable; using document.json (authoritative)",
        )),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::record::{
        BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation,
    };
    use crate::document::variables::Scalar;
    use crate::ids::{DocumentId, RecordId};
    use uuid::Uuid;

    fn extrude(seed: u128, distance: f64) -> crate::document::record::OperationRecord {
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
        crate::document::record::OperationRecord::new(
            RecordId(Uuid::from_u128(seed)),
            0,
            "Extrude",
            op,
        )
    }

    fn doc(seed: u128, dist: f64) -> Document {
        let mut d = Document::new(DocumentId(Uuid::from_u128(seed)));
        d.timeline.insert_at_cursor(extrude(0x10, dist));
        d.timeline.insert_at_cursor(extrude(0x11, dist + 1.0));
        d
    }

    fn meta() -> SaveMeta {
        SaveMeta {
            app_version: "0.1.0-test".into(),
            occt_fingerprint: Some("occt-7.9.3".into()),
            created: "2026-07-16T00:00:00Z".into(),
            modified: "2026-07-16T00:00:00Z".into(),
        }
    }

    #[test]
    fn save_then_open_preserves_document() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.onecad");
        let d = doc(1, 10.0);
        ContainerWriter::save(&path, &d, &ContainerCaches::none(), &meta()).unwrap();

        let loaded = ContainerReader::open(&path).unwrap();
        assert!(!loaded.outcome.read_only);
        assert!(!loaded.outcome.stale_caches);
        // Document JSON equality (the strongest structural check).
        assert_eq!(
            serde_json::to_value(loaded.document()).unwrap(),
            serde_json::to_value(&d).unwrap()
        );
    }

    #[test]
    fn atomic_save_crash_leaves_original_intact_and_cleans_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.onecad");

        // v1 committed.
        let v1 = doc(1, 10.0);
        ContainerWriter::save(&path, &v1, &ContainerCaches::none(), &meta()).unwrap();

        // Simulate a crash between temp-write and rename: temp written, not renamed.
        let v2 = doc(1, 999.0);
        let leftover =
            ContainerWriter::save_leaving_temp(&path, &v2, &ContainerCaches::none(), &meta())
                .unwrap();
        assert!(
            leftover.exists(),
            "temp file must exist after simulated crash"
        );

        // Original target is untouched: still opens as v1.
        let still_v1 = ContainerReader::open(&path).unwrap();
        assert_eq!(
            serde_json::to_value(still_v1.document()).unwrap(),
            serde_json::to_value(&v1).unwrap()
        );

        // Next real save cleans the stale temp and commits v3.
        let v3 = doc(1, 42.0);
        ContainerWriter::save(&path, &v3, &ContainerCaches::none(), &meta()).unwrap();
        assert!(
            !leftover.exists(),
            "stale temp must be cleaned on next save"
        );
        let now_v3 = ContainerReader::open(&path).unwrap();
        assert_eq!(
            serde_json::to_value(now_v3.document()).unwrap(),
            serde_json::to_value(&v3).unwrap()
        );
    }

    #[test]
    fn ops_hash_change_reports_stale_caches_but_still_loads() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.onecad");
        let d = doc(1, 10.0);
        let mut caches = ContainerCaches::none();
        caches
            .geometry
            .insert(crate::ids::BodyId(Uuid::from_u128(0xB0)), vec![1, 2, 3, 4]);
        ContainerWriter::save(&path, &d, &caches, &meta()).unwrap();

        // Tamper the manifest's opsHash so it no longer matches the records.
        rewrite_manifest_ops_hash(&path, "deadbeef");

        let loaded = ContainerReader::open(&path).unwrap();
        assert!(loaded.outcome.stale_caches);
        assert!(loaded
            .outcome
            .diagnostics
            .iter()
            .any(|d| d.code == "stale-caches"));
        // Document still loads fine.
        assert_eq!(loaded.document().timeline.len(), 2);
        // Cache reads are reported stale (never used).
        let entry = loaded.cache_entries()[0].path.clone();
        assert_eq!(loaded.read_cache(&entry).unwrap(), CacheRead::Stale);
    }

    #[test]
    fn cache_round_trips_when_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.onecad");
        let d = doc(1, 10.0);
        let body = crate::ids::BodyId(Uuid::from_u128(0xB0));
        let mut caches = ContainerCaches::none();
        caches.geometry.insert(body, vec![9, 8, 7, 6, 5]);
        caches.preview_png = Some(vec![0x89, 0x50, 0x4e, 0x47]);
        ContainerWriter::save(&path, &d, &caches, &meta()).unwrap();

        let loaded = ContainerReader::open(&path).unwrap();
        assert!(!loaded.outcome.stale_caches);
        let geo_path = format!("{GEOMETRY_DIR}{body}.brep");
        assert_eq!(
            loaded.read_cache(&geo_path).unwrap(),
            CacheRead::Present(vec![9, 8, 7, 6, 5])
        );
        assert_eq!(
            loaded.read_cache(PREVIEW_PATH).unwrap(),
            CacheRead::Present(vec![0x89, 0x50, 0x4e, 0x47])
        );
        assert_eq!(
            loaded.read_cache("geometry/nope.brep").unwrap(),
            CacheRead::Missing
        );
    }

    /// Rewrites the manifest.json inside a container, replacing its opsHash. Used
    /// to simulate stale caches / tamper without rebuilding the whole archive.
    fn rewrite_manifest_ops_hash(path: &Path, new_hash: &str) {
        let bytes = std::fs::read(path).unwrap();
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        // Read every entry out.
        let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::new();
        for i in 0..archive.len() {
            let mut f = archive.by_index(i).unwrap();
            let name = f.name().to_string();
            let is_dir = f.is_dir();
            let mut buf = Vec::new();
            f.read_to_end(&mut buf).unwrap();
            entries.push((name, buf, is_dir));
        }
        drop(archive);
        // Rewrite manifest.json bytes.
        let out = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(out);
        for (name, buf, is_dir) in entries {
            if is_dir {
                continue;
            }
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(CompressionMethod::Stored);
            zip.start_file(&name, opts).unwrap();
            if name == MANIFEST_PATH {
                let mut m: Manifest = serde_json::from_slice(&buf).unwrap();
                m.ops_hash = new_hash.to_string();
                zip.write_all(&serde_json::to_vec(&m).unwrap()).unwrap();
            } else {
                zip.write_all(&buf).unwrap();
            }
        }
        zip.finish().unwrap();
    }

    #[test]
    fn mesh_and_checkpoint_caches_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.onecad");
        let d = doc(1, 10.0);
        let body = BodyId(Uuid::from_u128(0xB0));
        let caches = ContainerCaches {
            geometry: BTreeMap::new(),
            meshes: vec![MeshCache {
                body,
                lod: "coarse".into(),
                bytes: b"MESH1....".to_vec(),
            }],
            checkpoints: vec![CheckpointCache {
                step: 3,
                json: b"{\"envelope\":1}".to_vec(),
                bin: Some(vec![0xDE, 0xAD, 0xBE, 0xEF]),
            }],
            preview_png: None,
        };
        ContainerWriter::save(&path, &d, &caches, &meta()).unwrap();

        let loaded = ContainerReader::open(&path).unwrap();
        assert!(!loaded.outcome.stale_caches);
        assert_eq!(
            loaded
                .read_cache(&format!("{MESHES_DIR}{body}.coarse.mesh"))
                .unwrap(),
            CacheRead::Present(b"MESH1....".to_vec())
        );
        assert_eq!(
            loaded
                .read_cache(&format!("{CHECKPOINTS_DIR}3.json"))
                .unwrap(),
            CacheRead::Present(b"{\"envelope\":1}".to_vec())
        );
        assert_eq!(
            loaded
                .read_cache(&format!("{CHECKPOINTS_DIR}3.bin"))
                .unwrap(),
            CacheRead::Present(vec![0xDE, 0xAD, 0xBE, 0xEF])
        );
    }

    #[test]
    fn writer_rejects_traversal_in_mesh_lod() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("model.onecad");
        let d = doc(1, 10.0);
        let caches = ContainerCaches {
            meshes: vec![MeshCache {
                body: BodyId(Uuid::from_u128(0xB0)),
                lod: "../../evil".into(), // hostile LOD tag
                bytes: vec![1, 2, 3],
            }],
            ..ContainerCaches::none()
        };
        assert!(matches!(
            ContainerWriter::save(&path, &d, &caches, &meta()),
            Err(IoError::PathTraversal(_))
        ));
        // The target was never created (failure before any commit).
        assert!(!path.exists());
    }
}
