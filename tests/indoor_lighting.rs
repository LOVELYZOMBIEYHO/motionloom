//! CPU coverage for portable room lighting and reflected-camera authoring.
//! GPU image fixtures live with the world renderer's glass/planar tests.

use motionloom::{
    AnimationInterpolation, AnimationValueType, WorldLighting, animation_properties_for_node_kind,
    inspect_animation_targets, parse_graph_script,
};
use serde_json::Value;

fn stage(lighting: &str, animation: &str) -> String {
    format!(
        r##"<Graph fps="24" duration="2s" size={{[64,64]}}>
<Assets>
<MaterialAsset id="mirror_material" baseColor="#FFFFFF" metallic="1" roughness="0.05" />
<GeometryAsset id="mirror_geometry">
<Primitive shape="box" size={{[2,2,0.02]}} />
</GeometryAsset>
<MeshAsset id="mirror_asset" material="mirror_material" geometry="mirror_geometry" />
</Assets>
<Scene id="room">
<Timeline>
<Track space="3d">
<Sequence duration="2s">
<CompositeGroup id="room_world" space="3d" depth="true" format="rgba16f">
<Camera3D position={{[0,0,4]}} target={{[0,0,0]}} />
{lighting}
<Model id="mirror" asset="mirror_asset" />
</CompositeGroup>
</Sequence>
</Track>
</Timeline>
</Scene>
{animation}
<Present from="room" />
</Graph>"##
    )
}

fn node<'a>(value: &'a Value, kind: &str) -> Option<&'a Value> {
    match value {
        Value::Object(object) => {
            if object.get("kind").and_then(Value::as_str) == Some(kind) {
                return Some(value);
            }
            object.values().find_map(|child| node(child, kind))
        }
        Value::Array(array) => array.iter().find_map(|child| node(child, kind)),
        _ => None,
    }
}

#[test]
fn indoor_lighting_defaults_and_json_roundtrip_are_camera_independent() {
    // Missing bake files must not cause I/O while parsing or inspecting a scene.
    let source = stage(
        "<BakedLighting id=\"room_gi\" src=\"lighting/room.json\" />\n<PlanarReflection id=\"mirror_capture\" target=\"mirror\" />",
        "",
    );
    let graph = parse_graph_script(&source).expect("indoor lighting should parse without a GPU");
    let json = serde_json::to_value(&graph).unwrap();
    let baked = node(&json, "bakedLighting").expect("typed baked-lighting node");
    assert_eq!(baked["src"], "lighting/room.json");
    assert_eq!(baked["blend"], "0");
    assert_eq!(baked["intensity"], "1");
    assert_eq!(baked["specularIntensity"], "1");
    let planar = node(&json, "planarReflection").expect("typed planar-reflection node");
    assert_eq!(planar["target"], "mirror");
    assert_eq!(planar["resolutionScale"], "0.5");
    assert_eq!(planar["clipBias"], "0.01");
    let restored: motionloom::GraphScript = serde_json::from_value(json.clone()).unwrap();
    // raw_script is intentionally excluded from GraphScript's serialized API.
    assert_eq!(json, serde_json::to_value(restored).unwrap());
}

#[test]
fn legacy_lighting_json_keeps_optional_features_disabled() {
    let mut json = serde_json::to_value(WorldLighting::default()).unwrap();
    json.as_object_mut().unwrap().remove("baked_lighting");
    json.as_object_mut().unwrap().remove("planar_reflections");
    let restored: WorldLighting = serde_json::from_value(json).unwrap();
    assert_eq!(restored, WorldLighting::default());
    let legacy = serde_json::to_value(parse_graph_script(&stage("", "")).unwrap()).unwrap();
    assert!(node(&legacy, "bakedLighting").is_none());
    assert!(node(&legacy, "planarReflection").is_none());
}

#[test]
fn indoor_lighting_has_strict_authoring_and_numeric_animation_coverage() {
    let animation = [
        ("room_gi", "blend", "0", "1"),
        ("room_gi", "intensity", "1", "0.7"),
        ("room_gi", "specularIntensity", "1", "0.5"),
        ("mirror_capture", "resolutionScale", "0.5", "1"),
        ("mirror_capture", "clipBias", "0.01", "0.02"),
    ]
    .into_iter()
    .map(|(id, property, from, to)| {
        format!(
            "<AnimationTarget node=\"{id}\" property=\"{property}\">\n<Key time=\"0s\" value=\"{from}\" />\n<Key time=\"1s\" value=\"{to}\" ease=\"linear\" />\n</AnimationTarget>"
        )
    })
    .collect::<Vec<_>>()
    .join("\n");
    let source = stage(
        "<BakedLighting id=\"room_gi\" src=\"lighting/room.json\" blend=\"0\" intensity=\"1\" specularIntensity=\"1\" />\n<PlanarReflection id=\"mirror_capture\" target=\"mirror\" resolutionScale=\"0.5\" clipBias=\"0.01\" />",
        &animation,
    );
    let graph = parse_graph_script(&source).unwrap();
    let targets = inspect_animation_targets(&graph);
    assert!(!targets.has_errors(), "{targets:?}");
    for (kind, properties) in [
        (
            "BakedLighting",
            &["blend", "intensity", "specularIntensity"][..],
        ),
        ("PlanarReflection", &["resolutionScale", "clipBias"][..]),
    ] {
        let descriptors = animation_properties_for_node_kind(kind);
        for property in properties {
            let descriptor = descriptors.iter().find(|p| p.path == *property).unwrap();
            assert_eq!(descriptor.value_type, AnimationValueType::Number);
            assert_eq!(descriptor.interpolation, AnimationInterpolation::Linear);
        }
    }
    let report: Value = serde_json::from_str(
        &motionloom::api::motionloom_analyze_script_for_target_json(&source, "native-webgpu"),
    )
    .unwrap();
    assert_eq!(report["summary"]["errors"], 0, "{report}");
    assert_eq!(report["summary"]["unknownTags"], 0, "{report}");
    assert_eq!(report["summary"]["ignoredAttributes"], 0, "{report}");
}

#[test]
fn duplicate_baked_bindings_and_missing_asset_references_are_rejected() {
    for lighting in [
        "<BakedLighting src=\"a.json\" />\n<BakedLighting src=\"b.json\" />",
        "<BakedLighting />",
        "<BakedLighting src=\"\" />",
        "<PlanarReflection />",
    ] {
        assert!(
            parse_graph_script(&stage(lighting, "")).is_err(),
            "accepted {lighting}"
        );
    }
}

#[test]
fn planar_captures_require_one_model_target_in_the_same_composite() {
    for lighting in [
        "<PlanarReflection target=\"missing\" />",
        "<PlanarReflection target=\"room_world\" />",
        "<PlanarReflection target=\"mirror\" />\n<PlanarReflection target=\"mirror\" />",
    ] {
        assert!(
            parse_graph_script(&stage(lighting, "")).is_err(),
            "accepted {lighting}"
        );
    }
}

#[test]
fn planar_capture_literal_scale_and_bias_are_finite_and_bounded() {
    for scale in ["0", "-0.01", "1.01", "NaN", "inf"] {
        let lighting =
            format!("<PlanarReflection target=\"mirror\" resolutionScale=\"{scale}\" />");
        assert!(
            parse_graph_script(&stage(&lighting, "")).is_err(),
            "accepted scale {scale}"
        );
    }
    for bias in ["-0.01", "NaN", "inf"] {
        let lighting = format!("<PlanarReflection target=\"mirror\" clipBias=\"{bias}\" />");
        assert!(
            parse_graph_script(&stage(&lighting, "")).is_err(),
            "accepted bias {bias}"
        );
    }
    for scale in ["0.001", "0.5", "1"] {
        parse_graph_script(&stage(
            &format!(
                "<PlanarReflection target=\"mirror\" resolutionScale=\"{scale}\" clipBias=\"0\" />"
            ),
            "",
        ))
        .expect("valid capture resolution boundary");
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn animated_planar_scale_is_checked_after_frame_evaluation_without_gpu() {
    use motionloom::lighting_bake::{
        BakeVolumeOptions, LightingBakeOptions, scene_lighting_bake_fingerprint,
    };
    use std::sync::Arc;

    let graph = parse_graph_script(&stage(
        "<PlanarReflection id=\"mirror_capture\" target=\"mirror\" />",
        "<AnimationTarget node=\"mirror_capture\" property=\"resolutionScale\">\n<Key time=\"0s\" value=\"0.5\" />\n<Key time=\"1s\" value=\"1.5\" ease=\"linear\" />\n</AnimationTarget>",
    ))
    .unwrap();
    let options = LightingBakeOptions {
        scene_id: "room".into(),
        day_frame: 0,
        dusk_frame: 0,
        volumes: vec![BakeVolumeOptions {
            id: "room".into(),
            bounds_min: [-2.0; 3],
            bounds_max: [2.0; 3],
            counts: [2; 3],
            reflection_positions: Vec::new(),
        }],
        ..Default::default()
    };
    // Fingerprinting lowers the same frame-local lighting as rendering, but
    // performs neither path tracing nor GPU work nor implicit asset writes.
    pollster::block_on(scene_lighting_bake_fingerprint(
        &graph,
        &options,
        Arc::new(motionloom::PathAssetResolver),
    ))
    .expect("valid first animation frame");
    let invalid_frame = LightingBakeOptions {
        dusk_frame: 24,
        ..options
    };
    let error = pollster::block_on(scene_lighting_bake_fingerprint(
        &graph,
        &invalid_frame,
        Arc::new(motionloom::PathAssetResolver),
    ))
    .expect_err("animated capture resolution must remain in (0,1]");
    assert!(error.to_string().contains("resolutionScale"), "{error}");
}
