//! Autosave layout + crash-recovery session markers.
//!
//! # Layout
//!
//! Under an app-data root, autosave state lives in an `autosave/` subdirectory:
//!
//! ```text
//! <app_data>/autosave/<documentId>.onecad         the autosave container
//! <app_data>/autosave/<documentId>.session.json   the live-session crash marker
//! ```
//!
//! While a document is open, the app writes a [`SessionMarker`] recording its
//! `pid`, the document's real on-disk path (if any), and the last autosave time.
//! On a clean close the marker is removed ([`remove_marker`]). If the app crashes,
//! the marker survives; the next launch calls [`scan_stale_markers`] with an
//! injected liveness predicate — any marker whose `pid` is **not alive** and whose
//! autosave file still exists becomes a [`RecoveryOffer`].
//!
//! ## The autosave writer is the app's job
//!
//! This module owns only the **layout and marker lifecycle**. Writing the autosave
//! container reuses [`ContainerWriter`](super::container::ContainerWriter) at the
//! path from [`autosave_path`]; scheduling (the ~2 min cadence,
//! [`AUTOSAVE_INTERVAL_SECS`]) is the app layer's responsibility — there are no
//! timers here (the core stays pure and side-effect-explicit).
//!
//! ## Platform note — liveness is injected
//!
//! [`scan_stale_markers`] takes `pid_alive: impl Fn(u32) -> bool` rather than
//! probing processes itself, so `onecad-core` needs **no `libc`/`sysinfo`
//! dependency**. On Unix the app typically implements it as `kill(pid, 0) != ESRCH`;
//! on Windows via `OpenProcess`. A conservative predicate (treat unknown as alive)
//! simply defers recovery to a later launch — never a false "safe to discard".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ids::DocumentId;

use super::{IoError, IoResult};

/// Autosave interval, in seconds (~2 min; V1/V2 plan). Advisory — the app
/// scheduler owns the cadence.
pub const AUTOSAVE_INTERVAL_SECS: u64 = 120;

/// The `.onecad` container extension (autosave files and real documents share it).
pub const CONTAINER_EXT: &str = "onecad";

/// The autosave directory under an app-data root (`<app_data>/autosave`).
#[must_use]
pub fn autosave_dir(app_data: &Path) -> PathBuf {
    app_data.join("autosave")
}

/// The autosave container path for a document
/// (`<app_data>/autosave/<documentId>.onecad`).
#[must_use]
pub fn autosave_path(app_data: &Path, document_id: DocumentId) -> PathBuf {
    autosave_dir(app_data).join(format!("{document_id}.{CONTAINER_EXT}"))
}

/// The session-marker path for a document
/// (`<app_data>/autosave/<documentId>.session.json`).
#[must_use]
pub fn marker_path(app_data: &Path, document_id: DocumentId) -> PathBuf {
    autosave_dir(app_data).join(format!("{document_id}.session.json"))
}

/// A live-session crash marker (V1/V2 plan "pid crash marker").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMarker {
    /// The document this session is editing.
    pub document_id: DocumentId,
    /// The owning process id (checked for liveness on the next launch).
    pub pid: u32,
    /// The document's real on-disk path, if it has been saved (RFC3339-free — a
    /// filesystem path). `None` for an unsaved (autosave-only) document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened_path: Option<PathBuf>,
    /// RFC3339 timestamp of the last autosave (caller-supplied — the core does not
    /// read the wall clock).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub last_autosave: String,
}

/// A recovery candidate produced by [`scan_stale_markers`]: a document whose owning
/// process is gone but whose autosave container is still on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryOffer {
    /// The document to offer for recovery.
    pub document_id: DocumentId,
    /// The autosave container to recover from.
    pub autosave_path: PathBuf,
    /// The stale marker (carries the last-autosave time + real path for the UI).
    pub marker: SessionMarker,
}

/// Writes (or overwrites) the session marker for the marker's document, atomically
/// (tmp + rename). Creates the autosave directory if absent.
///
/// # Errors
/// [`IoError::Io`] on a filesystem failure.
pub fn write_marker(app_data: &Path, marker: &SessionMarker) -> IoResult<()> {
    let dir = autosave_dir(app_data);
    std::fs::create_dir_all(&dir)?;
    let path = marker_path(app_data, marker.document_id);
    let bytes = serde_json::to_vec_pretty(marker)
        .map_err(|e| IoError::Io(format!("marker serialize: {e}")))?;
    let tmp = path.with_extension("session.json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Removes a document's session marker (a clean close). Absent marker is not an
/// error.
///
/// # Errors
/// [`IoError::Io`] on a filesystem failure other than "not found".
pub fn remove_marker(app_data: &Path, document_id: DocumentId) -> IoResult<()> {
    let path = marker_path(app_data, document_id);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// Scans the autosave directory for **stale** session markers: those whose `pid`
/// is not alive (per the injected predicate) and whose autosave container still
/// exists. Returns one [`RecoveryOffer`] per stale marker.
///
/// A missing autosave directory yields an empty list (nothing to recover).
/// Unparseable or foreign files in the directory are skipped, not fatal.
///
/// # Errors
/// [`IoError::Io`] if the directory exists but cannot be read.
pub fn scan_stale_markers(
    app_data: &Path,
    pid_alive: impl Fn(u32) -> bool,
) -> IoResult<Vec<RecoveryOffer>> {
    let dir = autosave_dir(app_data);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };

    let mut offers = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if !path.to_string_lossy().ends_with(".session.json") {
            continue;
        }
        // A foreign / half-written marker is skipped, never fatal.
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(marker) = serde_json::from_slice::<SessionMarker>(&bytes) else {
            continue;
        };
        if pid_alive(marker.pid) {
            continue; // owner still running — not stale.
        }
        let autosave = autosave_path(app_data, marker.document_id);
        if !autosave.exists() {
            continue; // nothing to recover from.
        }
        offers.push(RecoveryOffer {
            document_id: marker.document_id,
            autosave_path: autosave,
            marker,
        });
    }
    // Deterministic order (directory iteration order is unspecified).
    offers.sort_by_key(|o| o.document_id.as_uuid());
    Ok(offers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn marker(app_data: &Path, id: DocumentId, pid: u32) -> SessionMarker {
        SessionMarker {
            document_id: id,
            pid,
            opened_path: Some(app_data.join("real.onecad")),
            last_autosave: "2026-07-16T12:00:00Z".into(),
        }
    }

    #[test]
    fn marker_lifecycle_write_read_remove() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let id = DocumentId(Uuid::from_u128(0xABCD));
        write_marker(root, &marker(root, id, 4321)).unwrap();
        assert!(marker_path(root, id).exists());
        // idempotent overwrite
        write_marker(root, &marker(root, id, 4321)).unwrap();
        remove_marker(root, id).unwrap();
        assert!(!marker_path(root, id).exists());
        // remove of an absent marker is fine
        remove_marker(root, id).unwrap();
    }

    #[test]
    fn scan_offers_only_stale_with_existing_autosave() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let dead = DocumentId(Uuid::from_u128(1));
        let alive = DocumentId(Uuid::from_u128(2));
        let dead_no_file = DocumentId(Uuid::from_u128(3));

        write_marker(root, &marker(root, dead, 1000)).unwrap();
        write_marker(root, &marker(root, alive, 2000)).unwrap();
        write_marker(root, &marker(root, dead_no_file, 3000)).unwrap();

        // Only `dead` and `dead_no_file` have dead pids; give `dead` an autosave file.
        std::fs::write(autosave_path(root, dead), b"x").unwrap();

        let alive_pids = [2000u32];
        let offers = scan_stale_markers(root, |pid| alive_pids.contains(&pid)).unwrap();
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].document_id, dead);
        assert_eq!(offers[0].marker.pid, 1000);
    }

    #[test]
    fn scan_missing_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let offers = scan_stale_markers(&tmp.path().join("nope"), |_| false).unwrap();
        assert!(offers.is_empty());
    }

    #[test]
    fn scan_skips_foreign_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(autosave_dir(root)).unwrap();
        std::fs::write(autosave_dir(root).join("junk.session.json"), b"{not json").unwrap();
        std::fs::write(autosave_dir(root).join("readme.txt"), b"hi").unwrap();
        let offers = scan_stale_markers(root, |_| false).unwrap();
        assert!(offers.is_empty());
    }
}
