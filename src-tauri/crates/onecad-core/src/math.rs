//! Minimal geometric primitives used by the domain (sketch / plane / datum /
//! operation parameters).
//!
//! The core does no OCCT math; these types only carry parameters to/from the
//! worker. All components are `f64`. Per SCHEMA §4, `NaN`/`±Infinity` are
//! rejected at construction boundaries — use the checked `new(...)`
//! constructors, which return `None` on a non-finite component. Approximate-eq
//! helpers exist for tests (never for identity — see the descriptor rules).

use serde::{Deserialize, Serialize};

/// Default absolute tolerance for the `approx_eq` helpers (test convenience).
pub const DEFAULT_EPS: f64 = 1e-9;

/// 2D point / vector in a sketch's local plane coordinates.
///
/// Wire form is a JSON array `[x, y]` (SCHEMA §4/§7.3 coordinate convention);
/// non-finite components are rejected on read.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f64; 2]", into = "[f64; 2]")]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

impl TryFrom<[f64; 2]> for Vec2 {
    type Error = &'static str;
    fn try_from(a: [f64; 2]) -> Result<Self, Self::Error> {
        let v = Self { x: a[0], y: a[1] };
        if v.is_finite() {
            Ok(v)
        } else {
            Err("non-finite Vec2 component")
        }
    }
}

impl From<Vec2> for [f64; 2] {
    fn from(v: Vec2) -> Self {
        [v.x, v.y]
    }
}

impl Vec2 {
    /// Constructs a finite `Vec2`, rejecting `NaN`/`±Inf` (SCHEMA §4).
    #[must_use]
    pub fn new(x: f64, y: f64) -> Option<Self> {
        let v = Self { x, y };
        v.is_finite().then_some(v)
    }

    /// Constructs without the finite check (caller guarantees finiteness).
    #[must_use]
    pub const fn new_unchecked(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// True iff every component is finite (no `NaN`/`±Inf`).
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }

    /// Component-wise approximate equality within `eps` (test helper).
    #[must_use]
    pub fn approx_eq(&self, other: &Self, eps: f64) -> bool {
        (self.x - other.x).abs() <= eps && (self.y - other.y).abs() <= eps
    }
}

/// 3D point / vector in world coordinates (Z-up, right-handed).
///
/// Wire form is a JSON array `[x, y, z]` (SCHEMA §4/§7.3 coordinate
/// convention); non-finite components are rejected on read.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "[f64; 3]", into = "[f64; 3]")]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl TryFrom<[f64; 3]> for Vec3 {
    type Error = &'static str;
    fn try_from(a: [f64; 3]) -> Result<Self, Self::Error> {
        let v = Self {
            x: a[0],
            y: a[1],
            z: a[2],
        };
        if v.is_finite() {
            Ok(v)
        } else {
            Err("non-finite Vec3 component")
        }
    }
}

impl From<Vec3> for [f64; 3] {
    fn from(v: Vec3) -> Self {
        [v.x, v.y, v.z]
    }
}

impl Vec3 {
    /// Constructs a finite `Vec3`, rejecting `NaN`/`±Inf` (SCHEMA §4).
    #[must_use]
    pub fn new(x: f64, y: f64, z: f64) -> Option<Self> {
        let v = Self { x, y, z };
        v.is_finite().then_some(v)
    }

    /// Constructs without the finite check (caller guarantees finiteness).
    #[must_use]
    pub const fn new_unchecked(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    /// True iff every component is finite (no `NaN`/`±Inf`).
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    /// Component-wise approximate equality within `eps` (test helper).
    #[must_use]
    pub fn approx_eq(&self, other: &Self, eps: f64) -> bool {
        (self.x - other.x).abs() <= eps
            && (self.y - other.y).abs() <= eps
            && (self.z - other.z).abs() <= eps
    }
}

/// A coordinate plane: an origin plus an orthonormal basis.
///
/// `x_axis`, `y_axis`, `normal` form the plane frame. For the named sketch
/// planes the basis is the NON-STANDARD OneCAD-CPP mapping (see
/// [`crate::sketch::plane`] / SCHEMA §7.3): XY = `{x:(0,1,0), y:(-1,0,0),
/// n:(0,0,1)}`. This type stores the frame verbatim; it does not re-derive it.
///
/// Deserialize routes through [`PlaneWire`] so non-finite components are
/// rejected on read (SCHEMA §4) — the same `try_from` pattern as [`Vec2`]/
/// [`Vec3`]. (The `Vec3` members already reject non-finite; the wrapper makes
/// the plane-level invariant explicit and defends against future field changes.)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PlaneWire")]
pub struct Plane {
    pub origin: Vec3,
    pub x_axis: Vec3,
    pub y_axis: Vec3,
    pub normal: Vec3,
}

/// Raw wire form of [`Plane`]; validated into `Plane` via `TryFrom`.
#[derive(Deserialize)]
struct PlaneWire {
    origin: Vec3,
    x_axis: Vec3,
    y_axis: Vec3,
    normal: Vec3,
}

impl TryFrom<PlaneWire> for Plane {
    type Error = &'static str;
    fn try_from(w: PlaneWire) -> Result<Self, Self::Error> {
        Self::new(w.origin, w.x_axis, w.y_axis, w.normal).ok_or("non-finite Plane component")
    }
}

impl Plane {
    /// Constructs a plane, rejecting any non-finite component (SCHEMA §4). Does
    /// NOT check orthonormality — the basis is carried verbatim.
    #[must_use]
    pub fn new(origin: Vec3, x_axis: Vec3, y_axis: Vec3, normal: Vec3) -> Option<Self> {
        let p = Self {
            origin,
            x_axis,
            y_axis,
            normal,
        };
        p.is_finite().then_some(p)
    }

    /// True iff every component of every vector is finite.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.origin.is_finite()
            && self.x_axis.is_finite()
            && self.y_axis.is_finite()
            && self.normal.is_finite()
    }

    /// Component-wise approximate equality within `eps` (test helper).
    #[must_use]
    pub fn approx_eq(&self, other: &Self, eps: f64) -> bool {
        self.origin.approx_eq(&other.origin, eps)
            && self.x_axis.approx_eq(&other.x_axis, eps)
            && self.y_axis.approx_eq(&other.y_axis, eps)
            && self.normal.approx_eq(&other.normal, eps)
    }
}

/// A 3D affine transform stored as a **row-major 4×4 matrix**.
///
/// Convention: `rows[i][j]` is row `i`, column `j`. A point `p = (x,y,z)` is
/// transformed as the column vector `[x, y, z, 1]ᵀ` left-multiplied by the
/// matrix, i.e. `out_i = Σ_j rows[i][j] · p_j` with `p_3 = 1`. The bottom row is
/// `[0, 0, 0, 1]` for an affine transform. Row-major (not TRS) is chosen so the
/// stored form is engine-agnostic and matches the worker's OCCT `gp_Trsf`
/// row layout without re-decomposition.
///
/// Deserialize routes through [`Transform3Wire`] so non-finite entries are
/// rejected on read (SCHEMA §4) — the same `try_from` pattern as [`Vec2`]/
/// [`Vec3`]. (The `rows` are bare `f64`, so this check is load-bearing.)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Transform3Wire")]
pub struct Transform3 {
    pub rows: [[f64; 4]; 4],
}

/// Raw wire form of [`Transform3`]; validated into `Transform3` via `TryFrom`.
#[derive(Deserialize)]
struct Transform3Wire {
    rows: [[f64; 4]; 4],
}

impl TryFrom<Transform3Wire> for Transform3 {
    type Error = &'static str;
    fn try_from(w: Transform3Wire) -> Result<Self, Self::Error> {
        Self::new(w.rows).ok_or("non-finite Transform3 entry")
    }
}

impl Transform3 {
    /// The 4×4 identity transform.
    pub const IDENTITY: Self = Self {
        rows: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
    };

    /// Constructs a transform, rejecting any non-finite entry (SCHEMA §4).
    #[must_use]
    pub fn new(rows: [[f64; 4]; 4]) -> Option<Self> {
        let t = Self { rows };
        t.is_finite().then_some(t)
    }

    /// True iff every matrix entry is finite (no `NaN`/`±Inf`).
    #[must_use]
    pub fn is_finite(&self) -> bool {
        self.rows.iter().flatten().all(|entry| entry.is_finite())
    }

    /// Component-wise approximate equality within `eps` (test helper).
    #[must_use]
    pub fn approx_eq(&self, other: &Self, eps: f64) -> bool {
        self.rows
            .iter()
            .flatten()
            .zip(other.rows.iter().flatten())
            .all(|(a, b)| (a - b).abs() <= eps)
    }
}

impl Default for Transform3 {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_finite_components() {
        assert!(Vec2::new(f64::NAN, 0.0).is_none());
        assert!(Vec3::new(0.0, f64::INFINITY, 0.0).is_none());
        assert!(Vec3::new(0.0, 0.0, f64::NEG_INFINITY).is_none());
        assert!(Transform3::new([[f64::NAN; 4]; 4]).is_none());
        assert!(Vec3::new(1.0, 2.0, 3.0).is_some());
    }

    #[test]
    fn vec3_wire_form_is_an_array_and_rejects_non_finite_on_read() {
        let v = Vec3::new(1.0, 2.0, 3.0).unwrap();
        assert_eq!(serde_json::to_string(&v).unwrap(), "[1.0,2.0,3.0]");
        let back: Vec3 = serde_json::from_str("[1.0,2.0,3.0]").unwrap();
        assert_eq!(back, v);
        // SCHEMA §4: NaN/Inf rejected on read (here via a JSON that parses to inf).
        assert!(serde_json::from_str::<Vec3>("[1e999,0,0]").is_err());
    }

    #[test]
    fn transform3_and_plane_reject_non_finite_on_deserialize() {
        // Transform3: bare-f64 rows — the try_from check is load-bearing.
        assert_eq!(
            serde_json::to_string(&Transform3::IDENTITY).unwrap(),
            r#"{"rows":[[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0],[0.0,0.0,0.0,1.0]]}"#
        );
        let back: Transform3 = serde_json::from_str(
            r#"{"rows":[[1.0,0.0,0.0,0.0],[0.0,1.0,0.0,0.0],[0.0,0.0,1.0,0.0],[0.0,0.0,0.0,1.0]]}"#,
        )
        .unwrap();
        assert_eq!(back, Transform3::IDENTITY);
        // NaN/Inf in the matrix → error (1e999 parses to +Inf).
        assert!(serde_json::from_str::<Transform3>(
            r#"{"rows":[[1e999,0,0,0],[0,1,0,0],[0,0,1,0],[0,0,0,1]]}"#
        )
        .is_err());

        // Plane round-trips; a non-finite component is rejected on read.
        let p = Plane::new(
            Vec3::new_unchecked(0.0, 0.0, 0.0),
            Vec3::new_unchecked(0.0, 1.0, 0.0),
            Vec3::new_unchecked(-1.0, 0.0, 0.0),
            Vec3::new_unchecked(0.0, 0.0, 1.0),
        )
        .unwrap();
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(serde_json::from_str::<Plane>(&json).unwrap(), p);
        assert!(serde_json::from_str::<Plane>(
            r#"{"origin":[0,0,0],"x_axis":[1e999,0,0],"y_axis":[0,1,0],"normal":[0,0,1]}"#
        )
        .is_err());
    }

    #[test]
    fn approx_eq_within_eps() {
        let a = Vec3::new_unchecked(1.0, 2.0, 3.0);
        let b = Vec3::new_unchecked(1.0 + 1e-10, 2.0, 3.0);
        assert!(a.approx_eq(&b, DEFAULT_EPS));
        assert!(!a.approx_eq(&Vec3::new_unchecked(1.1, 2.0, 3.0), DEFAULT_EPS));
        assert!(Transform3::IDENTITY.approx_eq(&Transform3::default(), 0.0));
    }
}
