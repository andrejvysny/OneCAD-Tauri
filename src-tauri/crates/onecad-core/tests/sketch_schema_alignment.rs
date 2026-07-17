//! Alignment + documented divergence between the SCHEMA §7.3/§7.4 sketch WIRE
//! shape and this crate's authoritative sketch DOMAIN model.
//!
//! The domain model (`onecad-core::sketch`) is the `sketches/*.json` file format
//! and the authoritative document; the SCHEMA §7.3 entity/constraint payloads
//! are the worker-lane wire shape (carried opaquely by `SketchOpParams`). They
//! are intentionally different representations — the `onecad-protocol` adapter
//! bridges them. This test PINS both the parts that align and the parts that
//! diverge, citing the exact SCHEMA.md lines, so a future SCHEMA edit that
//! changes the relationship trips here.
//!
//! Line citations are against `protocol/SCHEMA.md` at the time of writing:
//! * §7.3 `plane` shape ....... lines 598-601, 616-621
//! * §7.3 `entities[]` shape ... lines 602-606, 622
//! * §7.3 `constraints[]` shape  lines 608-611, 623-627
//! * §7.4 `SketchUpsert` ....... lines 699-710

use onecad_core::document::variables::Scalar;
use onecad_core::sketch::constraint::Constraint;
use onecad_core::sketch::entity::SketchEntity;
use onecad_core::sketch::plane::SketchPlane;

/// ALIGNS: a SCHEMA §7.3 `plane` object (lines 598-601) parses straight into the
/// domain [`SketchPlane`] — the basis field names (`origin`/`xAxis`/`yAxis`/
/// `normal`) match, and the extra `kind` discriminator is ignored. The
/// non-standard XY basis (lines 616-621) is locked.
#[test]
fn schema_7_3_plane_parses_into_domain_plane_and_locks_xy_basis() {
    let schema_plane = serde_json::json!({
        "kind": "XY",
        "origin": [0, 0, 0], "xAxis": [0, 1, 0], "yAxis": [-1, 0, 0], "normal": [0, 0, 1]
    });
    let plane: SketchPlane = serde_json::from_value(schema_plane).unwrap();
    assert_eq!(plane, SketchPlane::xy());
    // Hard invariant (SCHEMA §7.3 lines 616-621 / Sketch.h).
    assert_eq!(
        [plane.x_axis.x, plane.x_axis.y, plane.x_axis.z],
        [0.0, 1.0, 0.0]
    );
    assert_eq!(
        [plane.y_axis.x, plane.y_axis.y, plane.y_axis.z],
        [-1.0, 0.0, 0.0]
    );
}

/// ALIGNS: a SCHEMA §7.3 dimensional `value` (line 611 sends `"value": 40.0` as
/// a bare number) parses into a [`Scalar`].
#[test]
fn schema_7_3_dimensional_value_parses_into_scalar() {
    let value: Scalar = serde_json::from_value(serde_json::json!(40.0)).unwrap();
    assert_eq!(value.value, 40.0);
    assert!(value.expr.is_none());
}

/// DIVERGES (documented): the SCHEMA §7.3 `entities[]` wire element (lines
/// 602-606) tags on `"type"` (PascalCase) and inlines coordinates
/// (`p0`/`p1`/`center`/`start`/`end`). The domain model tags on `"kind"`
/// (camelCase) and references point entities by id. So the raw SCHEMA entity
/// element does NOT deserialize into a domain `SketchEntity`; the adapter must
/// translate. This test pins that boundary.
#[test]
fn schema_7_3_entity_wire_shape_diverges_from_domain() {
    // SCHEMA §7.3 line 603, verbatim.
    let schema_line =
        serde_json::json!({ "id": "e1", "type": "Line", "p0": [0, 0], "p1": [40, 0] });
    // Wrong tag key (`type` vs `kind`) + inline coords => not a domain entity.
    assert!(
        serde_json::from_value::<SketchEntity>(schema_line).is_err(),
        "SCHEMA wire entity must not silently parse as a domain entity"
    );

    // The domain model's own Line shape (tag `kind`, point-id refs).
    let domain_line = serde_json::json!({
        "kind": "line",
        "id": "00000000-0000-0000-0000-000000000001",
        "start": "00000000-0000-0000-0000-000000000002",
        "end": "00000000-0000-0000-0000-000000000003"
    });
    let e: SketchEntity = serde_json::from_value(domain_line).unwrap();
    assert_eq!(e.referenced_entities().len(), 2);
}

/// DIVERGES (documented): the SCHEMA §7.3 `constraints[]` wire element (lines
/// 608-611) is generic — `{id, type, entities:[ids], value?}` under a `"type"`
/// tag. The domain model names each reference per its C++ role under a `"kind"`
/// tag and recovers the flat id list via `Constraint::entities()`. The raw
/// SCHEMA constraint element does NOT deserialize into a domain `Constraint`.
#[test]
fn schema_7_3_constraint_wire_shape_diverges_from_domain() {
    // SCHEMA §7.3 line 611, verbatim.
    let schema_distance =
        serde_json::json!({ "id": "c3", "type": "Distance", "entities": ["e1"], "value": 40.0 });
    assert!(
        serde_json::from_value::<Constraint>(schema_distance).is_err(),
        "SCHEMA wire constraint must not silently parse as a domain constraint"
    );

    // The domain Distance shape (tag `kind`, named entity refs, Scalar value).
    let domain_distance = serde_json::json!({
        "kind": "distance",
        "id": "00000000-0000-0000-0000-000000000001",
        "entity1": "00000000-0000-0000-0000-000000000002",
        "entity2": "00000000-0000-0000-0000-000000000003",
        "value": 40.0
    });
    let c: Constraint = serde_json::from_value(domain_distance).unwrap();
    assert_eq!(c.value().unwrap().value, 40.0);
    assert_eq!(c.entities().len(), 2);
}
