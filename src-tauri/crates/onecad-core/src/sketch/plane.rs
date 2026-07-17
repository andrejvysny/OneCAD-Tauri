//! The sketch coordinate plane.
//!
//! Ports OneCAD-CPP's **NON-STANDARD** basis VERBATIM from
//! `OneCAD-CPP/src/core/sketch/Sketch.h` (`SketchPlane::XY()/XZ()/YZ()` +
//! `toWorld`/`toSketch`). Do NOT "fix" these to a conventional basis —
//! downstream world geometry and every stored file depend on the exact numbers.
//! A lock-test in this module pins them (failure message cites `Sketch.h`).
//!
//! The named-plane bases (from `Sketch.h`):
//!
//! | plane | x_axis     | y_axis     | normal    | note (Sketch.h)                       |
//! |-------|------------|------------|-----------|---------------------------------------|
//! | XY    | (0, 1, 0)  | (-1, 0, 0) | (0, 0, 1) | Sketch X → World Y+, Sketch Y → World X- |
//! | XZ    | (0, 1, 0)  | (0, 0, 1)  | (1, 0, 0) | User X = Geom Y+, User Z = Geom Z+     |
//! | YZ    | (-1, 0, 0) | (0, 0, 1)  | (0, 1, 0) | User Y = Geom X-, User Z = Geom Z+     |
//!
//! Serde: camelCase (`origin`, `xAxis`, `yAxis`, `normal`) — aligns with the
//! SCHEMA §7.3 `plane` basis field names (the SCHEMA `kind` discriminator is
//! carried by [`SketchAttachment`](crate::sketch::SketchAttachment) in the
//! domain, not by this frame). Unknown fields (e.g. an incoming `kind`) are
//! ignored, so a SCHEMA §7.3 `plane` object parses straight into `SketchPlane`.

use serde::{Deserialize, Serialize};

use crate::math::{Vec2, Vec3};

/// World-space direction that a sketch's local +X axis maps to on the XY plane
/// (Sketch.h `SketchPlane::XY()`): `(0, 1, 0)`.
pub const XY_LOCAL_X_IN_WORLD: [f64; 3] = [0.0, 1.0, 0.0];

/// World-space direction that a sketch's local +Y axis maps to on the XY plane
/// (Sketch.h `SketchPlane::XY()`): `(-1, 0, 0)`.
pub const XY_LOCAL_Y_IN_WORLD: [f64; 3] = [-1.0, 0.0, 0.0];

/// A sketch's coordinate frame in world space: origin + orthonormal basis.
///
/// The basis is carried **verbatim** (never re-derived). See the module docs
/// for the non-standard named-plane bases ported from `Sketch.h`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchPlane {
    /// Plane origin in world coordinates.
    pub origin: Vec3,
    /// World direction of the sketch's local +X axis.
    pub x_axis: Vec3,
    /// World direction of the sketch's local +Y axis.
    pub y_axis: Vec3,
    /// Plane normal (`x_axis × y_axis`), carried verbatim.
    pub normal: Vec3,
}

impl SketchPlane {
    /// Default **XY** plane — NON-STANDARD basis ported verbatim from `Sketch.h`
    /// `SketchPlane::XY()`: `x=(0,1,0)`, `y=(-1,0,0)`, `n=(0,0,1)` (Sketch X →
    /// World Y+, Sketch Y → World X-).
    #[must_use]
    pub const fn xy() -> Self {
        Self {
            origin: Vec3::new_unchecked(0.0, 0.0, 0.0),
            x_axis: Vec3::new_unchecked(0.0, 1.0, 0.0),
            y_axis: Vec3::new_unchecked(-1.0, 0.0, 0.0),
            normal: Vec3::new_unchecked(0.0, 0.0, 1.0),
        }
    }

    /// **XZ** plane — `Sketch.h` `SketchPlane::XZ()`: `x=(0,1,0)`, `y=(0,0,1)`,
    /// `n=(1,0,0)`.
    #[must_use]
    pub const fn xz() -> Self {
        Self {
            origin: Vec3::new_unchecked(0.0, 0.0, 0.0),
            x_axis: Vec3::new_unchecked(0.0, 1.0, 0.0),
            y_axis: Vec3::new_unchecked(0.0, 0.0, 1.0),
            normal: Vec3::new_unchecked(1.0, 0.0, 0.0),
        }
    }

    /// **YZ** plane — `Sketch.h` `SketchPlane::YZ()`: `x=(-1,0,0)`, `y=(0,0,1)`,
    /// `n=(0,1,0)`.
    #[must_use]
    pub const fn yz() -> Self {
        Self {
            origin: Vec3::new_unchecked(0.0, 0.0, 0.0),
            x_axis: Vec3::new_unchecked(-1.0, 0.0, 0.0),
            y_axis: Vec3::new_unchecked(0.0, 0.0, 1.0),
            normal: Vec3::new_unchecked(0.0, 1.0, 0.0),
        }
    }

    /// Constructs a plane from an arbitrary frame, rejecting any non-finite
    /// component (SCHEMA §4). Does NOT check orthonormality — the basis is
    /// carried verbatim, matching [`crate::math::Plane::new`].
    #[must_use]
    pub fn new(origin: Vec3, x_axis: Vec3, y_axis: Vec3, normal: Vec3) -> Option<Self> {
        let p = Self {
            origin,
            x_axis,
            y_axis,
            normal,
        };
        (origin.is_finite() && x_axis.is_finite() && y_axis.is_finite() && normal.is_finite())
            .then_some(p)
    }

    /// Converts a 2D sketch point to 3D world coordinates.
    ///
    /// Ports `Sketch.h` `toWorld`: `origin + p.x·x_axis + p.y·y_axis`.
    #[must_use]
    pub fn to_world(&self, p: Vec2) -> Vec3 {
        Vec3::new_unchecked(
            self.origin.x + p.x * self.x_axis.x + p.y * self.y_axis.x,
            self.origin.y + p.x * self.x_axis.y + p.y * self.y_axis.y,
            self.origin.z + p.x * self.x_axis.z + p.y * self.y_axis.z,
        )
    }

    /// Projects a 3D world point to 2D sketch coordinates.
    ///
    /// Ports `Sketch.h` `toSketch`: with `rel = p3d − origin`,
    /// `(rel·x_axis, rel·y_axis)`.
    #[must_use]
    pub fn to_sketch(&self, p: Vec3) -> Vec2 {
        let rx = p.x - self.origin.x;
        let ry = p.y - self.origin.y;
        let rz = p.z - self.origin.z;
        Vec2::new_unchecked(
            rx * self.x_axis.x + ry * self.x_axis.y + rz * self.x_axis.z,
            rx * self.y_axis.x + ry * self.y_axis.y + rz * self.y_axis.z,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// LOCK-TEST — the exact XY basis numbers. If this fails, someone
    /// "normalized" the coordinate system and broke every stored file.
    #[test]
    fn xy_basis_is_the_non_standard_cpp_basis() {
        const MSG: &str = "non-standard basis is load-bearing — see Sketch.h";
        let p = SketchPlane::xy();
        assert_eq!(
            [p.origin.x, p.origin.y, p.origin.z],
            [0.0, 0.0, 0.0],
            "{MSG}"
        );
        assert_eq!(
            [p.x_axis.x, p.x_axis.y, p.x_axis.z],
            [0.0, 1.0, 0.0],
            "{MSG}"
        );
        assert_eq!(
            [p.y_axis.x, p.y_axis.y, p.y_axis.z],
            [-1.0, 0.0, 0.0],
            "{MSG}"
        );
        assert_eq!(
            [p.normal.x, p.normal.y, p.normal.z],
            [0.0, 0.0, 1.0],
            "{MSG}"
        );
        // The exported constants must agree with the constructor.
        assert_eq!(
            [p.x_axis.x, p.x_axis.y, p.x_axis.z],
            XY_LOCAL_X_IN_WORLD,
            "{MSG}"
        );
        assert_eq!(
            [p.y_axis.x, p.y_axis.y, p.y_axis.z],
            XY_LOCAL_Y_IN_WORLD,
            "{MSG}"
        );
    }

    /// LOCK-TEST — XZ / YZ bases (verified against `Sketch.h`, not guessed).
    #[test]
    fn xz_and_yz_bases_match_sketch_h() {
        const MSG: &str = "non-standard basis is load-bearing — see Sketch.h";
        let xz = SketchPlane::xz();
        assert_eq!(
            [xz.x_axis.x, xz.x_axis.y, xz.x_axis.z],
            [0.0, 1.0, 0.0],
            "{MSG}"
        );
        assert_eq!(
            [xz.y_axis.x, xz.y_axis.y, xz.y_axis.z],
            [0.0, 0.0, 1.0],
            "{MSG}"
        );
        assert_eq!(
            [xz.normal.x, xz.normal.y, xz.normal.z],
            [1.0, 0.0, 0.0],
            "{MSG}"
        );

        let yz = SketchPlane::yz();
        assert_eq!(
            [yz.x_axis.x, yz.x_axis.y, yz.x_axis.z],
            [-1.0, 0.0, 0.0],
            "{MSG}"
        );
        assert_eq!(
            [yz.y_axis.x, yz.y_axis.y, yz.y_axis.z],
            [0.0, 0.0, 1.0],
            "{MSG}"
        );
        assert_eq!(
            [yz.normal.x, yz.normal.y, yz.normal.z],
            [0.0, 1.0, 0.0],
            "{MSG}"
        );
    }

    /// The XY mapping is exactly the C++ `toWorld` contract for the basis axes.
    #[test]
    fn xy_to_world_maps_axes_per_sketch_h() {
        let p = SketchPlane::xy();
        // Sketch X (+1,0) → World Y+ (0,1,0); Sketch Y (0,+1) → World X- (-1,0,0).
        assert_eq!(
            p.to_world(Vec2::new_unchecked(1.0, 0.0)),
            Vec3::new_unchecked(0.0, 1.0, 0.0)
        );
        assert_eq!(
            p.to_world(Vec2::new_unchecked(0.0, 1.0)),
            Vec3::new_unchecked(-1.0, 0.0, 0.0)
        );
    }

    proptest! {
        /// Round-trip: `to_sketch ∘ to_world == id` for every named plane and a
        /// custom translated plane (exercises the origin term).
        #[test]
        fn to_sketch_is_left_inverse_of_to_world(x in -1.0e6f64..1.0e6, y in -1.0e6f64..1.0e6) {
            let custom = SketchPlane {
                origin: Vec3::new_unchecked(10.0, -20.0, 30.0),
                ..SketchPlane::xy()
            };
            for plane in [SketchPlane::xy(), SketchPlane::xz(), SketchPlane::yz(), custom] {
                let p = Vec2::new_unchecked(x, y);
                let back = plane.to_sketch(plane.to_world(p));
                prop_assert!(back.approx_eq(&p, 1e-6), "round-trip drifted: {p:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn plane_serde_camel_case_and_ignores_unknown_kind() {
        let p = SketchPlane::xy();
        let v = serde_json::to_value(p).unwrap();
        assert_eq!(v["xAxis"], serde_json::json!([0.0, 1.0, 0.0]));
        assert_eq!(v["yAxis"], serde_json::json!([-1.0, 0.0, 0.0]));
        // A SCHEMA §7.3 `plane` object (with a `kind` discriminator) parses in,
        // ignoring `kind` (no deny_unknown_fields).
        let schema_plane = serde_json::json!({
            "kind": "XY", "origin": [0, 0, 0], "xAxis": [0, 1, 0],
            "yAxis": [-1, 0, 0], "normal": [0, 0, 1]
        });
        let back: SketchPlane = serde_json::from_value(schema_plane).unwrap();
        assert_eq!(back, SketchPlane::xy());
    }
}
