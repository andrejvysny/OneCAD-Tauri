//! Schema-freeze contract for the OperationRecord v2 file format.
//!
//! ⚠️  THESE SNAPSHOTS ARE THE FILE-FORMAT CONTRACT. Changing any `.snap` here
//! is a schema change and REQUIRES SCHEMA-REVIEW SIGN-OFF + a `protocol/`
//! fixture bump (SCHEMA §13). Do not `cargo insta accept` a diff without it.
//!
//! Covers: (1) a canonical JSON snapshot of every `Operation` variant, (2)
//! unknown-field preservation at record / params / nested-ref levels, (3)
//! unknown-`opType` → `Opaque` lossless round-trip, (4) `derive_inputs` parity
//! with OneCAD-CPP `DependencyGraph::extractDependencies`, (5) SCHEMA §7.3 op
//! payloads deserialize into `Operation`.

mod common;

use common::*;
use onecad_core::document::record::{
    BooleanMode, ExtrudeMode, ExtrudeParams, KnownOperation, Operation, OperationRecord,
};
use onecad_core::document::refs::AxisRef;
use onecad_core::document::variables::Scalar;
use onecad_core::ids::{BodyId, ElementId, RecordId, SketchId};

// ── 1. Canonical snapshots — one per Operation variant ──────────────────────

#[test]
fn every_operation_variant_is_snapshot_locked() {
    for (key, rec) in canonical_records() {
        insta::assert_json_snapshot!(format!("operation_{key}"), rec);
    }
}

// ── 2. Unknown-field preservation (record / params / nested-ref) ────────────

#[test]
fn alien_fields_survive_at_record_level() {
    let rec = record(1, "Extrude 1", op_extrude_newbody());
    let mut v = serde_json::to_value(&rec).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("alienRecordKey".into(), serde_json::json!({ "keep": true }));

    let back: OperationRecord = serde_json::from_value(v).unwrap();
    assert_eq!(
        back.extra.get("alienRecordKey"),
        Some(&serde_json::json!({ "keep": true })),
        "record-level unknown key must land in `extra`"
    );

    let reser = serde_json::to_value(&back).unwrap();
    assert_eq!(
        reser.get("alienRecordKey"),
        Some(&serde_json::json!({ "keep": true })),
        "record-level unknown key must survive re-serialization"
    );
}

#[test]
fn alien_fields_survive_at_params_level() {
    let rec = record(1, "Extrude 1", op_extrude_newbody());
    let mut v = serde_json::to_value(&rec).unwrap();
    v.as_object_mut()
        .unwrap()
        .get_mut("params")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("alienParamKey".into(), serde_json::json!(42));

    let back: OperationRecord = serde_json::from_value(v).unwrap();
    match &back.op {
        Operation::Known(KnownOperation::Extrude(p)) => {
            assert_eq!(p.extra.get("alienParamKey"), Some(&serde_json::json!(42)));
        }
        _ => panic!("expected Extrude"),
    }
    let reser = serde_json::to_value(&back).unwrap();
    assert_eq!(
        reser["params"]["alienParamKey"],
        serde_json::json!(42),
        "params-level unknown key must survive re-serialization"
    );
}

#[test]
fn alien_fields_survive_at_nested_ref_level() {
    // The Cut extrude carries a fully-populated `targetFace` ElementRef.
    let rec = record(2, "Cut 1", op_extrude_cut());
    let mut v = serde_json::to_value(&rec).unwrap();
    v["params"]["targetFace"]["anchor"]
        .as_object_mut()
        .unwrap()
        .insert("alienAnchorKey".into(), serde_json::json!("x"));

    let back: OperationRecord = serde_json::from_value(v).unwrap();
    let reser = serde_json::to_value(&back).unwrap();
    assert_eq!(
        reser["params"]["targetFace"]["anchor"]["alienAnchorKey"],
        serde_json::json!("x"),
        "nested-ref unknown key must survive re-serialization"
    );
}

// ── 3. Unknown opType → Opaque, byte-stable round-trip ──────────────────────

#[test]
fn unknown_op_type_round_trips_losslessly() {
    let rec = record(13, "Frozen 1", op_opaque());
    assert!(matches!(rec.op, Operation::Opaque(_)));

    let v1 = serde_json::to_value(&rec).unwrap();
    // opType + params + the vendor-specific key all survive at the top level.
    assert_eq!(v1["opType"], serde_json::json!("FlangeBend"));
    assert_eq!(v1["params"]["angleDeg"], serde_json::json!(90.0));
    assert_eq!(v1["vendorHint"], serde_json::json!({ "tool": "future" }));

    let rec2: OperationRecord = serde_json::from_value(v1.clone()).unwrap();
    assert!(matches!(rec2.op, Operation::Opaque(_)));
    let v2 = serde_json::to_value(&rec2).unwrap();
    assert_eq!(v1, v2, "Opaque round-trip must be byte-stable (idempotent)");
}

#[test]
fn opaque_deserializes_directly_from_operation() {
    let json = serde_json::json!({
        "opType": "Draft",
        "params": { "angle": 3.0 }
    });
    let op: Operation = serde_json::from_value(json.clone()).unwrap();
    assert!(matches!(op, Operation::Opaque(_)));
    assert_eq!(serde_json::to_value(&op).unwrap(), json);
}

// ── 4. derive_inputs parity table ───────────────────────────────────────────

/// `(bodies, sketches, elements)` expected from `Operation::derive_inputs`.
fn inputs_of(op: &Operation) -> (Vec<BodyId>, Vec<SketchId>, Vec<ElementId>) {
    let i = op.derive_inputs();
    (i.bodies, i.sketches, i.elements)
}

#[test]
fn derive_inputs_matches_cpp_extract_dependencies() {
    // sketch: no upstream deps.
    assert_eq!(inputs_of(&op_sketch()), (vec![], vec![], vec![]));

    // extrude NewBody: only the profile sketch.
    assert_eq!(
        inputs_of(&op_extrude_newbody()),
        (vec![], vec![sketch_1()], vec![])
    );

    // extrude Cut (booleanMode != NewBody): profile sketch + target body.
    assert_eq!(
        inputs_of(&op_extrude_cut()),
        (vec![body_a()], vec![sketch_1()], vec![])
    );

    // revolve: profile sketch + axis sketch (deduped to one).
    assert_eq!(inputs_of(&op_revolve()), (vec![], vec![sketch_1()], vec![]));

    // fillet / chamfer: referenced edges.
    assert_eq!(
        inputs_of(&op_fillet()),
        (vec![], vec![], vec![elem_e14(), elem_e15()])
    );
    assert_eq!(inputs_of(&op_chamfer()), (vec![], vec![], vec![elem_e14()]));

    // shell: shelled body + open faces.
    assert_eq!(
        inputs_of(&op_shell()),
        (vec![body_a()], vec![], vec![ElementId::new("f:3")])
    );

    // boolean: target + tool bodies.
    assert_eq!(
        inputs_of(&op_boolean()),
        (vec![body_a(), body_b()], vec![], vec![])
    );

    // patterns / mirror: source body.
    assert_eq!(
        inputs_of(&op_linear_pattern()),
        (vec![body_a()], vec![], vec![])
    );
    assert_eq!(
        inputs_of(&op_circular_pattern()),
        (vec![body_a()], vec![], vec![])
    );
    assert_eq!(inputs_of(&op_mirror()), (vec![body_a()], vec![], vec![]));

    // loft: profile sketches (Rust adds these; C++ omits Loft).
    assert_eq!(
        inputs_of(&op_loft()),
        (vec![], vec![sketch_1(), SketchId(uid(0x5C02))], vec![])
    );

    // sweep: profile + path sketch.
    assert_eq!(
        inputs_of(&op_sweep()),
        (vec![], vec![sketch_1(), SketchId(uid(0x5C03))], vec![])
    );

    // opaque: no typed deps.
    assert_eq!(inputs_of(&op_opaque()), (vec![], vec![], vec![]));
}

#[test]
fn boolean_mode_newbody_does_not_add_target_body() {
    // The C++ rule: target body is a dependency ONLY when booleanMode != NewBody
    // (DependencyGraph.cpp:272). Here booleanMode == NewBody yet target_body is
    // set — it must NOT be tracked.
    let op = Operation::Known(KnownOperation::Extrude(ExtrudeParams {
        profile: None,
        distance: Scalar::new(5.0),
        draft_angle_deg: Scalar::new(0.0),
        mode: ExtrudeMode::Blind,
        boolean_mode: BooleanMode::NewBody,
        target_body: Some(body_a()),
        target_face: None,
        two_directions: false,
        mode2: ExtrudeMode::Blind,
        distance2: Scalar::new(0.0),
        target_face2: None,
        extra: Default::default(),
    }));
    assert_eq!(inputs_of(&op), (vec![], vec![], vec![]));
}

// ── 5. SCHEMA §7.3 op payloads deserialize into Operation ───────────────────
//
// Verbatim payloads (Extrude/Fillet/Chamfer) are copied from SCHEMA.md §7.3.
// Payloads carrying UUID-backed document ids (Sketch/Revolve/Boolean) use the
// SCHEMA shape with the illustrative opaque ids (`"sk_1"`, `"body_1"`) replaced
// by canonical UUIDs — the Rust file format uses UUID-backed BodyId/SketchId/
// EntityId (see report). All non-doc-id fields are verbatim.

#[test]
fn schema_7_3_extrude_payload_parses() {
    // SCHEMA.md §7.3 "Extrude" (amended 2026-07-16 — bare `targetFaceId`/
    // `targetFaceId2` replaced by typed `targetFace`/`targetFace2` semantic refs;
    // see SCHEMA Changelog). Blind/NewBody example (verbatim), no target face.
    let blind = r#"{
      "opType": "Extrude",
      "opId": "op_5",
      "params": {
        "distance": 25.0,
        "draftAngleDeg": 0.0,
        "extrudeMode": "Blind",
        "booleanMode": "NewBody",
        "targetBodyId": "",
        "twoDirections": false,
        "extrudeMode2": "Blind",
        "distance2": 0.0
      }
    }"#;
    match serde_json::from_str::<Operation>(blind).unwrap() {
        Operation::Known(KnownOperation::Extrude(p)) => {
            assert_eq!(p.distance.value, 25.0);
            assert_eq!(p.mode, ExtrudeMode::Blind);
            assert_eq!(p.boolean_mode, BooleanMode::NewBody);
            assert_eq!(p.target_body, None); // "" → None
            assert_eq!(p.target_face, None);
            // M4: SCHEMA no longer emits a bare `targetFaceId`, so nothing lands
            // in `extra` under that key.
            assert!(!p.extra.contains_key("targetFaceId"));
            assert!(!p.extra.contains_key("targetFaceId2"));
        }
        other => panic!("expected Extrude, got {other:?}"),
    }

    // M4: a `ToFace` extrude targets a face via the typed `targetFace` semantic
    // ref (same {primary, intent, anchor} shape as fillet edges), populating the
    // typed field directly (SCHEMA-alignment).
    let toface = serde_json::json!({
        "opType": "Extrude",
        "params": {
            "distance": 10.0,
            "draftAngleDeg": 0.0,
            "extrudeMode": "ToFace",
            "booleanMode": "Cut",
            "targetBodyId": body_a().to_string(),
            "targetFace": {
                "primary": { "bodyId": body_a().to_string(), "elementId": "el_face_1", "kind": "face" },
                "intent": { "version": 1, "kind": "face", "descriptor": { "surfaceType": "plane" } },
                "anchor": { "worldPoint": [12.0, 3.5, 0.0], "surfaceUv": [0.25, 0.75] }
            },
            "twoDirections": false,
            "extrudeMode2": "Blind",
            "distance2": 0.0
        }
    });
    match serde_json::from_value::<Operation>(toface).unwrap() {
        Operation::Known(KnownOperation::Extrude(p)) => {
            assert_eq!(p.mode, ExtrudeMode::ToFace);
            let tf = p
                .target_face
                .expect("typed targetFace populated from SCHEMA §7.3");
            let primary = tf.primary.expect("targetFace.primary");
            assert_eq!(primary.body, body_a());
            assert_eq!(primary.element, ElementId::new("el_face_1"));
        }
        other => panic!("expected Extrude, got {other:?}"),
    }
}

#[test]
fn schema_7_3_fillet_and_chamfer_payloads_parse() {
    // SCHEMA.md §7.3 "Fillet"/"Chamfer" (lines ~669-672), verbatim.
    let fillet = r#"{ "opType": "Fillet",
      "params": { "mode": "Fillet", "radius": 2.0, "edgeIds": ["e:14", "e:15"], "chainTangentEdges": true } }"#;
    let chamfer = r#"{ "opType": "Chamfer",
      "params": { "mode": "Chamfer", "radius": 1.0, "edgeIds": ["e:14"], "chainTangentEdges": true } }"#;

    match serde_json::from_str::<Operation>(fillet).unwrap() {
        Operation::Known(KnownOperation::Fillet(p)) => {
            assert_eq!(p.radius.value, 2.0);
            assert_eq!(
                p.edge_ids,
                vec![ElementId::new("e:14"), ElementId::new("e:15")]
            );
            assert!(p.chain_tangent_edges);
        }
        other => panic!("expected Fillet, got {other:?}"),
    }
    assert!(matches!(
        serde_json::from_str::<Operation>(chamfer).unwrap(),
        Operation::Known(KnownOperation::Chamfer(_))
    ));
}

#[test]
fn schema_7_3_revolve_payload_parses() {
    // SCHEMA.md §7.3 "Revolve" (lines ~654-662); axis ids substituted with UUIDs.
    let json = serde_json::json!({
        "opType": "Revolve",
        "params": {
            "angleDeg": 360.0,
            "axis": { "kind": "sketchLine", "sketchId": sketch_1().to_string(), "lineId": line_e1().to_string() },
            "booleanMode": "NewBody",
            "targetBodyId": ""
        }
    });
    match serde_json::from_value::<Operation>(json).unwrap() {
        Operation::Known(KnownOperation::Revolve(p)) => {
            assert_eq!(p.angle_deg.value, 360.0);
            assert!(matches!(p.axis, Some(AxisRef::SketchLine { .. })));
            assert_eq!(p.target_body, None);
        }
        other => panic!("expected Revolve, got {other:?}"),
    }
}

#[test]
fn schema_7_3_boolean_payload_parses() {
    // SCHEMA.md §7.3 "Boolean" (line ~687); body ids substituted with UUIDs.
    let json = serde_json::json!({
        "opType": "Boolean",
        "params": { "operation": "Union", "targetBodyId": body_a().to_string(), "toolBodyId": body_b().to_string() }
    });
    match serde_json::from_value::<Operation>(json).unwrap() {
        Operation::Known(KnownOperation::Boolean(p)) => {
            assert_eq!(p.target_body, body_a());
            assert_eq!(p.tool_body, body_b());
        }
        other => panic!("expected Boolean, got {other:?}"),
    }
}

#[test]
fn schema_7_3_sketch_payload_parses_and_locks_xy_basis() {
    // SCHEMA.md §7.3 "Sketch" (lines ~594-613); sketchId substituted with a UUID.
    let json = serde_json::json!({
        "opType": "Sketch",
        "params": {
            "sketchId": sketch_1().to_string(),
            "plane": { "kind": "XY", "origin": [0,0,0], "xAxis": [0,1,0], "yAxis": [-1,0,0], "normal": [0,0,1] },
            "entities": [ { "id": "e1", "type": "Line", "p0": [0,0], "p1": [40,0] } ],
            "constraints": [ { "id": "c1", "type": "Horizontal", "entities": ["e1"] } ]
        }
    });
    match serde_json::from_value::<Operation>(json).unwrap() {
        Operation::Known(KnownOperation::Sketch(p)) => {
            // Hard invariant: non-standard XY basis (SCHEMA §7.3 / Sketch.h).
            assert_eq!(
                [p.plane.x_axis.x, p.plane.x_axis.y, p.plane.x_axis.z],
                [0.0, 1.0, 0.0]
            );
            assert_eq!(
                [p.plane.y_axis.x, p.plane.y_axis.y, p.plane.y_axis.z],
                [-1.0, 0.0, 0.0]
            );
            assert_eq!(p.entities.len(), 1);
        }
        other => panic!("expected Sketch, got {other:?}"),
    }
}

// ── 6. M1: known opType + malformed params ⇒ ERROR (both entry points) ───────
//
// The bug: `Operation`'s old untagged Deserialize silently demoted a KNOWN op
// with bad params to `Opaque`. The hand-written path now ERRORs, matching
// `OperationRecord`'s path. Unknown opType still ⇒ `Opaque`.

/// Attach a `recordId` so a raw op payload is also a valid `OperationRecord`.
fn as_record_json(mut op: serde_json::Value) -> serde_json::Value {
    op.as_object_mut().unwrap().insert(
        "recordId".into(),
        serde_json::json!(RecordId(uid(0x2EC0)).to_string()),
    );
    op
}

#[test]
fn known_optype_with_malformed_params_errors_both_entry_points() {
    // Extrude is KNOWN, but `distance` is a non-numeric string ⇒ hard error.
    let bad = serde_json::json!({
        "opType": "Extrude",
        "params": {
            "distance": "twenty-five",
            "draftAngleDeg": 0.0,
            "extrudeMode": "Blind",
            "booleanMode": "NewBody",
            "twoDirections": false,
            "extrudeMode2": "Blind",
            "distance2": 0.0
        }
    });
    assert!(
        serde_json::from_value::<Operation>(bad.clone()).is_err(),
        "direct Operation must ERROR on a known op with malformed params (M1)"
    );
    assert!(
        serde_json::from_value::<OperationRecord>(as_record_json(bad)).is_err(),
        "OperationRecord must ERROR on a known op with malformed params (M1)"
    );
}

#[test]
fn unknown_optype_becomes_opaque_both_entry_points() {
    let unknown = serde_json::json!({ "opType": "FlangeBend", "params": { "angleDeg": 90.0 } });
    assert!(matches!(
        serde_json::from_value::<Operation>(unknown.clone()).unwrap(),
        Operation::Opaque(_)
    ));
    let rec: OperationRecord = serde_json::from_value(as_record_json(unknown)).unwrap();
    assert!(matches!(rec.op, Operation::Opaque(_)));
}

// ── 7. M3: derived `inputs` are self-healed for Known ops on load ────────────

#[test]
fn known_op_inputs_are_self_healed_on_load() {
    // Round-trip a fillet-with-typed-edges record, then TAMPER its stored
    // `inputs`; on load a Known op re-derives inputs from `op`, discarding the
    // tampered value (M3).
    let rec = record(21, "Fillet self-heal", op_fillet_with_edge_refs());
    let mut v = serde_json::to_value(&rec).unwrap();
    v["inputs"] = serde_json::json!({
        "bodies": [body_b().to_string()],   // wrong body
        "sketches": [sketch_1().to_string()], // spurious sketch
        "elements": ["tampered"]
    });
    let back: OperationRecord = serde_json::from_value(v).unwrap();
    assert_eq!(
        back.inputs, rec.inputs,
        "Known op inputs re-derived on load"
    );
    assert_eq!(
        back.inputs.bodies,
        vec![body_a()],
        "operated body recovered"
    );
    assert!(back.inputs.sketches.is_empty());
    assert_eq!(
        back.inputs.elements,
        vec![ElementId::new("el_e14"), ElementId::new("el_e15")]
    );
}

#[test]
fn opaque_op_keeps_stored_inputs_on_load() {
    // An Opaque frozen node exposes no typed deps; whatever `inputs` are on disk
    // are preserved verbatim (M3).
    let rec = record(22, "Frozen inputs", op_opaque());
    let mut v = serde_json::to_value(&rec).unwrap();
    v["inputs"] = serde_json::json!({
        "bodies": [body_a().to_string()], "sketches": [], "elements": ["e:9"]
    });
    let back: OperationRecord = serde_json::from_value(v).unwrap();
    assert_eq!(
        back.inputs.bodies,
        vec![body_a()],
        "Opaque keeps stored bodies"
    );
    assert_eq!(back.inputs.elements, vec![ElementId::new("e:9")]);
}

// ── 8. M5: Fillet/Chamfer derive_inputs tracks the operated body from edges ──

#[test]
fn fillet_derive_inputs_tracks_operated_body_from_edge_refs() {
    // Two typed edges on the SAME body ⇒ exactly one body input (deduped); an
    // intent-only edge (no `primary`) contributes no body (regen binds it later).
    let (bodies, sketches, elements) = inputs_of(&op_fillet_with_edge_refs());
    assert_eq!(bodies, vec![body_a()], "operated body deduped to one (M5)");
    assert!(sketches.is_empty());
    assert_eq!(
        elements,
        vec![ElementId::new("el_e14"), ElementId::new("el_e15")],
        "edge elements from each ref's primary"
    );
}

// ── 9. Snapshot: extrude with populated occtOptions + record-extra + targetFace ─

#[test]
fn extrude_toface_full_is_snapshot_locked() {
    // NEW snapshot (does not touch the existing 14): a ToFace Cut extrude carrying
    // a typed `targetFace`, plus populated `determinism.occtOptions` and a
    // record-level `extra` key.
    let mut rec = record(20, "Cut ToFace full", op_extrude_cut());
    rec.determinism
        .occt_options
        .insert("fuzzyValue".into(), serde_json::json!(0.001));
    rec.determinism
        .occt_options
        .insert("useOBB".into(), serde_json::json!(true));
    rec.extra
        .insert("uiColor".into(), serde_json::json!("#ff8800"));
    insta::assert_json_snapshot!("operation_extrude_toface_full", rec);
}
