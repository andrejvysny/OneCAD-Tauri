//! Identifier newtypes shared across the core.
//!
//! Two families:
//!
//! * **UUID-backed identities** (`RecordId`, `BodyId`, `SketchId`,
//!   `DatumPlaneId`, `DocumentId`, `EntityId`, `ConstraintId`, `VariableId`):
//!   Rust-minted, globally unique, `#[serde(transparent)]` so they serialize as
//!   the bare UUID string in the v2 file format. Constructed with `new()`
//!   (UUID v4). These are Rust-owned document identities.
//! * **Opaque string identities** (`ElementId`, `RegionId`) and the transient
//!   `TopoKey`: see the individual docs.
//!
//! `ElementId` intentionally does NOT embed `BodyId`: partition membership
//! (which body an element belongs to) is a *mapping*, not identity, so element
//! ids survive body split/merge (plan "ElementId scheme change"; SCHEMA §2).

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Declares a UUID-backed, `#[serde(transparent)]` identity newtype with a
/// `new()` (UUID v4) constructor and `Display`/`FromStr`.
macro_rules! uuid_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub Uuid);

        // No `Default`: a default-constructed random UUID is a footgun (silent,
        // non-deterministic identity). Mint explicitly with `new()`.
        #[allow(clippy::new_without_default)]
        impl $name {
            /// Mints a fresh, globally-unique id (UUID v4).
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wraps an existing UUID (e.g. loaded from disk or a fixed test seed).
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The underlying UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

uuid_id!(
    /// Identity of an `OperationRecord` in the timeline.
    RecordId
);
uuid_id!(
    /// Identity of a solid body in the document body registry. Rust-minted,
    /// globally unique. Split/merge changes partition membership, not this id.
    BodyId
);
uuid_id!(
    /// Identity of a sketch.
    SketchId
);
uuid_id!(
    /// Identity of a datum plane / reference geometry element.
    DatumPlaneId
);
uuid_id!(
    /// Identity of an open document.
    DocumentId
);
uuid_id!(
    /// Identity of a sketch entity (line, arc, circle, point, …).
    EntityId
);
uuid_id!(
    /// Identity of a sketch constraint.
    ConstraintId
);
uuid_id!(
    /// Identity of a document variable / parameter.
    VariableId
);

/// Declares an opaque, `#[serde(transparent)]` string-backed identity newtype.
macro_rules! string_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Wraps an owned string id (already minted by Rust).
            #[must_use]
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            /// Borrows the underlying string.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(
    /// Globally unique, opaque topological element identity.
    ///
    /// **Minted by Rust at selection time** — the worker returns resolution
    /// evidence (`topoKey → (kind, descriptor, anchor)`); Rust assigns/echoes
    /// the persistent id it owns (SCHEMA §7.5). Globally unique. Never changes
    /// because geometry changed (Invariant 1). Descriptors are evidence, never
    /// identity (Invariant 2). **DOES NOT embed `BodyId`** — partition
    /// membership is a mapping, never encoded in the id (Codex correction in
    /// plan; SCHEMA §2). Example wire form: `"el_00000000000004a1"`.
    ElementId
);
string_id!(
    /// Opaque identity of a closed sketch profile region (for extrude/revolve).
    ///
    /// Minted by Rust; globally unique; opaque string. Example: `"r0"`.
    RegionId
);

/// The kind of a [`TopoKey`] (and of a resolved element): face / edge / vertex /
/// body. Wire char is `f` / `e` / `v` / `b` (SCHEMA §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TopoKind {
    Face,
    Edge,
    Vertex,
    Body,
}

impl TopoKind {
    /// The single-char wire tag (`f`/`e`/`v`/`b`).
    #[must_use]
    pub const fn as_char(self) -> char {
        match self {
            TopoKind::Face => 'f',
            TopoKind::Edge => 'e',
            TopoKind::Vertex => 'v',
            TopoKind::Body => 'b',
        }
    }

    /// Parses the single-char wire tag.
    #[must_use]
    pub const fn from_char(c: char) -> Option<Self> {
        match c {
            'f' => Some(TopoKind::Face),
            'e' => Some(TopoKind::Edge),
            'v' => Some(TopoKind::Vertex),
            'b' => Some(TopoKind::Body),
            _ => None,
        }
    }
}

/// A **snapshot-scoped**, transient topology address stamped on mesh
/// faces/edges: `"<kind>:<index>"`, kind ∈ `f`/`e`/`v`/`b` (SCHEMA §2).
///
/// Example `"f:22"`. Valid ONLY within the `snapshotId` that produced it — it is
/// never persisted as identity. Promoted on demand to a persistent
/// [`ElementId`] via `AcquireElementIds` (SCHEMA §7.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TopoKey(pub String);

/// Error parsing a malformed [`TopoKey`] string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopoKeyParseError(pub String);

impl fmt::Display for TopoKeyParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "malformed TopoKey: {:?}", self.0)
    }
}

impl std::error::Error for TopoKeyParseError {}

impl TopoKey {
    /// Wraps a raw key string without validation (use [`TopoKey::parse`] to
    /// validate).
    #[must_use]
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// Builds a well-formed key from its parts (e.g. `Face`, `22` → `"f:22"`).
    #[must_use]
    pub fn from_parts(kind: TopoKind, index: u64) -> Self {
        Self(format!("{}:{}", kind.as_char(), index))
    }

    /// The raw key string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Validates and parses the key into `(kind, index)`.
    ///
    /// Format is exactly `"<kind>:<index>"` with kind ∈ `f`/`e`/`v`/`b` and
    /// index a base-10 `u64`.
    pub fn parse(&self) -> Result<(TopoKind, u64), TopoKeyParseError> {
        let err = || TopoKeyParseError(self.0.clone());
        let (kind_part, index_part) = self.0.split_once(':').ok_or_else(err)?;
        let mut chars = kind_part.chars();
        let kind = chars
            .next()
            .filter(|_| chars.next().is_none())
            .and_then(TopoKind::from_char)
            .ok_or_else(err)?;
        let index: u64 = index_part.parse().map_err(|_| err())?;
        Ok((kind, index))
    }

    /// True iff the key is well-formed (see [`TopoKey::parse`]).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.parse().is_ok()
    }

    /// The [`TopoKind`] of a well-formed key.
    #[must_use]
    pub fn kind(&self) -> Option<TopoKind> {
        self.parse().ok().map(|(k, _)| k)
    }
}

impl fmt::Display for TopoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Numeric fencing / snapshot ids (u64-backed; carried in the protocol stamp).
// Kept from the WP0 skeleton; used by `regen`. Serialize transparently as the
// bare integer.
// ─────────────────────────────────────────────────────────────────────────────

/// Identity of a published regen snapshot. Bodies, maps, signatures and meshes
/// published together share one `SnapshotId` (Invariant 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapshotId(pub u64);

/// Identity of a regen job (one compiled plan execution).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobId(pub Uuid);

/// Identity of an interactive solver gesture (drag session).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GestureId(pub Uuid);

/// Monotonic authoritative document revision. Used for revision fencing on the
/// prepare/accept publication path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DocumentRevision(pub u64);

/// Monotonic worker epoch. Bumped on every worker (re)start; used together with
/// `DocumentRevision` to fence stale prepared results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkerEpoch(pub u64);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn topokey_parses_valid_kinds() {
        assert_eq!(TopoKey::new("f:22").parse().unwrap(), (TopoKind::Face, 22));
        assert_eq!(TopoKey::new("e:0").parse().unwrap(), (TopoKind::Edge, 0));
        assert_eq!(TopoKey::new("v:7").parse().unwrap(), (TopoKind::Vertex, 7));
        assert_eq!(TopoKey::new("b:3").parse().unwrap(), (TopoKind::Body, 3));
        assert_eq!(TopoKey::from_parts(TopoKind::Face, 22).as_str(), "f:22");
    }

    #[test]
    fn topokey_rejects_malformed() {
        for bad in ["f22", "x:1", "f:", "f:-1", ":3", "ff:1", "f:1:2", ""] {
            assert!(!TopoKey::new(bad).is_valid(), "should reject {bad:?}");
        }
    }

    #[test]
    fn string_id_is_serde_transparent() {
        let e = ElementId::new("el_abc");
        assert_eq!(serde_json::to_string(&e).unwrap(), "\"el_abc\"");
        let back: ElementId = serde_json::from_str("\"el_abc\"").unwrap();
        assert_eq!(back, e);
        assert!(!ElementId::new("el_1").as_str().contains('/')); // never embeds BodyId
    }

    #[test]
    fn uuid_id_display_and_fromstr_round_trip() {
        let s = "00000000-0000-0000-0000-0000000000ff";
        let id: BodyId = s.parse().unwrap();
        assert_eq!(id.to_string(), s);
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{s}\""));
    }
}
