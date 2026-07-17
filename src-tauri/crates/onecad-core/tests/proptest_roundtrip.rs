//! Property-based serde round-trip: for every generated `OperationRecord`,
//! `deserialize(serialize(x)) == x`. Sizes are bounded to stay fast.
//!
//! `Arbitrary` is implemented on the local wrapper newtypes `ArbOperation` /
//! `ArbRecord` (the orphan rule forbids implementing the foreign `Arbitrary`
//! trait directly on `onecad-core`'s foreign types from this test crate).

use proptest::prelude::*;
use uuid::Uuid;

use onecad_core::document::record::{
    BooleanMode, BooleanOp, BooleanParams, ChamferParams, CircularPatternParams,
    DeterminismSettings, ExtrudeMode, ExtrudeParams, FilletParams, KnownOperation,
    LinearPatternParams, LoftParams, MirrorBodyParams, OpaqueOperation, Operation, OperationRecord,
    PlaneKind, RevolveParams, ShellParams, SketchOpParams, SketchPlaneRef, SweepParams,
};
use onecad_core::document::refs::{
    AnchorIntent, AxisRef, ElementKind, ElementRef, Extra, IntentQuery, LocalFrame, PrimaryRef,
    SketchRegionRef,
};
use onecad_core::document::variables::Scalar;
use onecad_core::ids::{BodyId, ElementId, EntityId, RecordId, RegionId, SketchId};
use onecad_core::math::{Vec2, Vec3};

fn uuid_strategy() -> impl Strategy<Value = Uuid> {
    any::<u128>().prop_map(Uuid::from_u128)
}
// Limited-decimal finite values (magnitude ≤ 1e6, ≤ 3 decimals). This keeps the
// property test focused on STRUCTURAL round-trip: full-entropy f64s can hit a
// serde_json parser off-by-1-ULP edge that is orthogonal to the schema and would
// only add flakiness. Real CAD dimensions are limited-decimal.
fn finite() -> impl Strategy<Value = f64> {
    (-1_000_000_000i64..=1_000_000_000).prop_map(|n| n as f64 / 1000.0)
}
fn scalar() -> impl Strategy<Value = Scalar> {
    finite().prop_map(Scalar::new)
}
fn vec3() -> impl Strategy<Value = Vec3> {
    (finite(), finite(), finite()).prop_map(|(x, y, z)| Vec3::new(x, y, z).unwrap())
}
fn vec2() -> impl Strategy<Value = Vec2> {
    (finite(), finite()).prop_map(|(x, y)| Vec2::new(x, y).unwrap())
}

// m2: a small map of NON-reserved (`alien_*`) keys with round-trippable JSON
// values (ints/strings/bools only — no floats, to avoid ULP noise; no reserved
// keys, per the record.rs `extra` contract). Empty ~half the time.
fn extra_strategy() -> impl Strategy<Value = Extra> {
    prop::collection::vec(
        (
            any::<u16>(),
            prop_oneof![
                any::<u32>().prop_map(serde_json::Value::from),
                any::<bool>().prop_map(serde_json::Value::from),
                "[a-z]{0,6}".prop_map(serde_json::Value::from),
            ],
        ),
        0..3,
    )
    .prop_map(|pairs| {
        pairs
            .into_iter()
            .map(|(n, v)| (format!("alien_{n}"), v))
            .collect()
    })
}

// m2: a sometimes-fully-populated ElementRef (identity + evidence + anchor),
// each layer carrying its own alien-key `extra`.
fn element_ref_strategy() -> impl Strategy<Value = ElementRef> {
    let primary =
        (body_id(), opaque_string_id(), extra_strategy()).prop_map(|(body, el, extra)| {
            PrimaryRef {
                body,
                element: ElementId::new(el),
                kind: ElementKind::Edge,
                extra,
            }
        });
    let intent = extra_strategy().prop_map(|extra| IntentQuery {
        version: 1,
        kind: ElementKind::Edge,
        descriptor: serde_json::json!({ "surfaceType": "plane" }),
        extra,
    });
    let anchor = (vec3(), proptest::option::of(vec2()), extra_strategy()).prop_map(
        |(world_point, surface_uv, extra)| AnchorIntent {
            world_point,
            surface_uv,
            local_frame: Some(LocalFrame {
                origin: Vec3::new_unchecked(0.0, 0.0, 0.0),
                x: Vec3::new_unchecked(1.0, 0.0, 0.0),
                y: Vec3::new_unchecked(0.0, 1.0, 0.0),
                z: Vec3::new_unchecked(0.0, 0.0, 1.0),
                extra: Default::default(),
            }),
            adjacency_hint: None,
            extra,
        },
    );
    (
        proptest::option::of(primary),
        proptest::option::of(intent),
        proptest::option::of(anchor),
        extra_strategy(),
    )
        .prop_map(|(primary, intent, anchor, extra)| ElementRef {
            primary,
            intent,
            anchor,
            extra,
        })
}
fn opaque_string_id() -> impl Strategy<Value = String> {
    any::<u32>().prop_map(|n| format!("id_{n:x}"))
}
fn body_id() -> impl Strategy<Value = BodyId> {
    uuid_strategy().prop_map(BodyId)
}
fn sketch_id() -> impl Strategy<Value = SketchId> {
    uuid_strategy().prop_map(SketchId)
}
fn region_ref() -> impl Strategy<Value = SketchRegionRef> {
    (sketch_id(), opaque_string_id(), extra_strategy()).prop_map(|(sketch, r, extra)| {
        SketchRegionRef {
            sketch,
            region: RegionId::new(r),
            extra,
        }
    })
}
fn extrude_mode() -> impl Strategy<Value = ExtrudeMode> {
    prop_oneof![
        Just(ExtrudeMode::Blind),
        Just(ExtrudeMode::ThroughAll),
        Just(ExtrudeMode::Symmetric),
        Just(ExtrudeMode::ToNext),
        Just(ExtrudeMode::ToFace),
    ]
}
fn boolean_mode() -> impl Strategy<Value = BooleanMode> {
    prop_oneof![
        Just(BooleanMode::NewBody),
        Just(BooleanMode::Add),
        Just(BooleanMode::Cut),
        Just(BooleanMode::Intersect),
    ]
}

// m2: a revolve axis spanning all three shapes, each populated variant carrying
// a per-variant alien `extra` map.
fn axis_strategy() -> impl Strategy<Value = Option<AxisRef>> {
    prop_oneof![
        Just(None),
        (sketch_id(), uuid_strategy(), extra_strategy()).prop_map(|(sketch, l, extra)| {
            Some(AxisRef::SketchLine {
                sketch,
                line: EntityId(l),
                extra,
            })
        }),
        (body_id(), opaque_string_id(), extra_strategy()).prop_map(|(body, e, extra)| {
            Some(AxisRef::Element {
                body,
                edge: ElementId::new(e),
                extra,
            })
        }),
    ]
}

fn operation_strategy() -> impl Strategy<Value = Operation> {
    let sketch = (sketch_id(), vec3(), vec3(), vec3(), vec3()).prop_map(
        |(sketch, origin, x_axis, y_axis, normal)| {
            Operation::Known(KnownOperation::Sketch(SketchOpParams {
                sketch,
                plane: SketchPlaneRef {
                    kind: PlaneKind::Xy,
                    origin,
                    x_axis,
                    y_axis,
                    normal,
                    extra: Default::default(),
                },
                entities: vec![],
                constraints: vec![],
                extra: Default::default(),
            }))
        },
    );

    // m2: sometimes-populated `targetFace`/`targetFace2` (typed ElementRefs) and a
    // params-level alien `extra`. Grouped into nested tuples to stay within
    // proptest's tuple arity.
    let extrude = (
        (
            proptest::option::of(region_ref()),
            scalar(),
            scalar(),
            extrude_mode(),
            boolean_mode(),
        ),
        (
            proptest::option::of(body_id()),
            any::<bool>(),
            extrude_mode(),
            scalar(),
        ),
        (
            proptest::option::of(element_ref_strategy()),
            proptest::option::of(element_ref_strategy()),
            extra_strategy(),
        ),
    )
        .prop_map(
            |(
                (profile, distance, draft, mode, bmode),
                (target, two, mode2, distance2),
                (target_face, target_face2, extra),
            )| {
                Operation::Known(KnownOperation::Extrude(ExtrudeParams {
                    profile,
                    distance,
                    draft_angle_deg: draft,
                    mode,
                    boolean_mode: bmode,
                    target_body: target,
                    target_face,
                    two_directions: two,
                    mode2,
                    distance2,
                    target_face2,
                    extra,
                }))
            },
        );

    // m2: axis spans None | SketchLine | Element (each variant with its own
    // per-variant alien `extra`) + params-level alien `extra`.
    let revolve = (
        proptest::option::of(region_ref()),
        scalar(),
        axis_strategy(),
        boolean_mode(),
        proptest::option::of(body_id()),
        extra_strategy(),
    )
        .prop_map(|(profile, angle, axis, bmode, target, extra)| {
            Operation::Known(KnownOperation::Revolve(RevolveParams {
                profile,
                angle_deg: angle,
                axis,
                boolean_mode: bmode,
                target_body: target,
                extra,
            }))
        });

    let fillet = (
        scalar(),
        prop::collection::vec(opaque_string_id(), 0..4),
        any::<bool>(),
        extra_strategy(),
    )
        .prop_map(|(radius, edges, chain, extra)| {
            Operation::Known(KnownOperation::Fillet(FilletParams {
                radius,
                edge_ids: edges.into_iter().map(ElementId::new).collect(),
                edges: vec![],
                chain_tangent_edges: chain,
                extra,
            }))
        });

    let chamfer = (
        scalar(),
        prop::collection::vec(opaque_string_id(), 0..4),
        any::<bool>(),
        extra_strategy(),
    )
        .prop_map(|(radius, edges, chain, extra)| {
            Operation::Known(KnownOperation::Chamfer(ChamferParams {
                radius,
                edge_ids: edges.into_iter().map(ElementId::new).collect(),
                edges: vec![],
                chain_tangent_edges: chain,
                extra,
            }))
        });

    let shell = (
        scalar(),
        prop::collection::vec(opaque_string_id(), 0..3),
        proptest::option::of(body_id()),
    )
        .prop_map(|(thickness, faces, target)| {
            Operation::Known(KnownOperation::Shell(ShellParams {
                thickness,
                open_faces: faces.into_iter().map(ElementId::new).collect(),
                target_body: target,
                extra: Default::default(),
            }))
        });

    let boolean = (
        prop_oneof![
            Just(BooleanOp::Union),
            Just(BooleanOp::Cut),
            Just(BooleanOp::Intersect)
        ],
        body_id(),
        body_id(),
    )
        .prop_map(|(operation, target, tool)| {
            Operation::Known(KnownOperation::Boolean(BooleanParams {
                operation,
                target_body: target,
                tool_body: tool,
                extra: Default::default(),
            }))
        });

    let linear = (
        proptest::option::of(body_id()),
        vec3(),
        scalar(),
        1u32..12,
        any::<bool>(),
    )
        .prop_map(|(source, direction, spacing, count, fuse)| {
            Operation::Known(KnownOperation::LinearPattern(LinearPatternParams {
                source_body: source,
                direction,
                spacing,
                count,
                fuse_result: fuse,
                extra: Default::default(),
            }))
        });

    let circular = (
        proptest::option::of(body_id()),
        vec3(),
        vec3(),
        scalar(),
        1u32..12,
        any::<bool>(),
    )
        .prop_map(|(source, origin, dir, angle, count, fuse)| {
            Operation::Known(KnownOperation::CircularPattern(CircularPatternParams {
                source_body: source,
                axis_origin: origin,
                axis_direction: dir,
                angle_deg: angle,
                count,
                fuse_result: fuse,
                extra: Default::default(),
            }))
        });

    let loft = (
        prop::collection::vec(region_ref(), 0..3),
        any::<bool>(),
        any::<bool>(),
        boolean_mode(),
    )
        .prop_map(|(profiles, solid, ruled, bmode)| {
            Operation::Known(KnownOperation::Loft(LoftParams {
                profiles,
                is_solid: solid,
                is_ruled: ruled,
                boolean_mode: bmode,
                extra: Default::default(),
            }))
        });

    let sweep = (
        proptest::option::of(region_ref()),
        proptest::option::of(sketch_id()),
        proptest::option::of(opaque_string_id()),
        boolean_mode(),
    )
        .prop_map(|(profile, path_sketch, path_edge, bmode)| {
            Operation::Known(KnownOperation::Sweep(SweepParams {
                profile,
                path_sketch,
                path_edge: path_edge.map(ElementId::new),
                boolean_mode: bmode,
                extra: Default::default(),
            }))
        });

    let mirror = (
        proptest::option::of(body_id()),
        vec3(),
        vec3(),
        any::<bool>(),
    )
        .prop_map(|(source, point, normal, fuse)| {
            Operation::Known(KnownOperation::MirrorBody(MirrorBodyParams {
                source_body: source,
                plane_point: point,
                plane_normal: normal,
                fuse_with_original: fuse,
                extra: Default::default(),
            }))
        });

    // Opaque: an opType guaranteed NOT in the known set, so it stays Opaque.
    let opaque = any::<u16>().prop_map(|n| {
        let mut raw = serde_json::Map::new();
        raw.insert(
            "opType".into(),
            serde_json::Value::String(format!("Vendor{n}")),
        );
        raw.insert("params".into(), serde_json::json!({ "k": n }));
        Operation::Opaque(OpaqueOperation { raw })
    });

    prop_oneof![
        sketch, extrude, revolve, fillet, chamfer, shell, boolean, linear, circular, loft, sweep,
        mirror, opaque,
    ]
}

fn record_strategy() -> impl Strategy<Value = OperationRecord> {
    (
        uuid_strategy(),
        any::<u32>(),
        "[a-zA-Z0-9 ]{0,12}",
        operation_strategy(),
        prop::collection::vec(body_id(), 0..3),
        any::<bool>(),
        any::<bool>(),
        // m2: (occtOptions, determinism-level extra, record-level extra).
        (extra_strategy(), extra_strategy(), extra_strategy()),
    )
        .prop_map(
            |(
                rid,
                step,
                name,
                op,
                outputs,
                parallel,
                suppressed,
                (occt_options, det_extra, rec_extra),
            )| {
                let mut rec = OperationRecord::new(RecordId(rid), step, name, op);
                rec.outputs = outputs;
                rec.suppressed = suppressed;
                rec.determinism = DeterminismSettings {
                    parallel,
                    occt_options,
                    extra: det_extra,
                    ..Default::default()
                };
                // Record-level extra only survives for Known ops — an Opaque node
                // folds every top-level key into its raw payload, so injecting a
                // separate `extra` there would not round-trip.
                if matches!(rec.op, Operation::Known(_)) {
                    rec.extra = rec_extra;
                }
                rec
            },
        )
}

/// Local wrapper so we can carry an `Arbitrary` impl (orphan rule).
#[derive(Debug, Clone)]
struct ArbRecord(OperationRecord);

impl Arbitrary for ArbRecord {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;
    fn arbitrary_with(_: ()) -> Self::Strategy {
        record_strategy().prop_map(ArbRecord).boxed()
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    #[test]
    fn record_serde_round_trips(rec in any::<ArbRecord>()) {
        let rec = rec.0;
        let json = serde_json::to_string(&rec).unwrap();
        let back: OperationRecord = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(&rec, &back);
        // And idempotent re-serialization.
        let json2 = serde_json::to_string(&back).unwrap();
        prop_assert_eq!(json, json2);
    }
}
