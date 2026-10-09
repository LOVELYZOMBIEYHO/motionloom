//! Native image regressions for emitter ownership and physical shadow softness.
//! Every comparison keeps geometry/camera fixed while changing a caster flag
//! or emitter setting. Fixtures use no external assets, video or temporal AA.
#![cfg(not(target_arch = "wasm32"))]

use base64::Engine;
use motionloom::{
    ImmediatePreviewProfile, ImmediatePreviewSettings, SceneRenderProfile, SceneRenderer,
    parse_graph_script,
};
use serde_json::Value;

type Region = (u32, u32, u32, u32);

fn stage(assets: &str, lights: &str, models: &str, camera: &str) -> String {
    format!(
        r##"<Graph fps="24" duration="1s" size={{[128,128]}}>
<RenderStyle id="physical">
<SurfaceStyle shading="physical" specular="0" />
<LightingStyle ambientIntensity="0" shadowMode="perLight" />
<AntiAliasingStyle method="off" />
</RenderStyle>
<Assets>
<MaterialAsset id="floor_mat" baseColor="#B0B0B0" roughness="1" specular="0" />
<MaterialAsset id="black" baseColor="#000000" specular="0" />
<GeometryAsset id="floor_geo"><Primitive shape="box" size={{[7,0.04,7]}} /></GeometryAsset>
<MeshAsset id="floor" material="floor_mat" geometry="floor_geo" />
{assets}
</Assets>
<Background color="#000000" />
<Scene id="room" renderStyle="physical"><Timeline><Track space="3d"><Sequence duration="1s">
<CompositeGroup space="3d" depth="true" format="rgba16f">
<Camera3D {camera} up={{[0,0,-1]}} />
<AmbientOcclusion intensity="0" />
<ContactShadow intensity="0" />
<ColorManagement toneMapping="none" exposure="1" />
{lights}
<Model id="floor" asset="floor" position={{[0,-0.02,0]}} castShadow="false" receiveShadow="true" />
{models}
</CompositeGroup>
</Sequence></Track></Timeline></Scene><Present from="room" />
</Graph>"##
    )
}

fn cpu_stage(lights: &str) -> String {
    stage("", lights, "", "position={[0,4,4]} target={[0,0,0]}")
}

fn light<'a>(value: &'a Value, id: &str) -> Option<&'a Value> {
    match value {
        Value::Object(object) => {
            if object.get("id").and_then(Value::as_str) == Some(id)
                && matches!(
                    object.get("kind").and_then(Value::as_str),
                    Some("directionalLight" | "pointLight" | "spotLight" | "rectAreaLight")
                )
            {
                return Some(value);
            }
            object.values().find_map(|child| light(child, id))
        }
        Value::Array(array) => array.iter().find_map(|child| light(child, id)),
        _ => None,
    }
}

fn omit_light_field(value: &mut Value, kind: &str, field: &str) {
    match value {
        Value::Object(object) => {
            if object.get("kind").and_then(Value::as_str) == Some(kind) {
                object.remove(field);
            }
            for child in object.values_mut() {
                omit_light_field(child, kind, field);
            }
        }
        Value::Array(array) => {
            for child in array {
                omit_light_field(child, kind, field);
            }
        }
        _ => {}
    }
}

#[test]
fn emitter_defaults_keep_legacy_visibility_and_zero_source_sizes() {
    let source = cpu_stage(
        "<DirectionalLight id=\"sun\" />\n<PointLight id=\"point\" />\n<SpotLight id=\"spot\" />\n<RectAreaLight id=\"area\" />",
    )
    .replace(" shadowMode=\"perLight\"", "");
    let graph = parse_graph_script(&source).unwrap();
    assert!(
        graph.render_styles[0]
            .lighting
            .as_ref()
            .unwrap()
            .shadow_mode
            .is_none()
    );
    assert!(
        !motionloom::api::resolve_scene_render_style(&graph, "room")
            .unwrap()
            .per_light_shadows
    );
    let json = serde_json::to_value(&graph).unwrap();
    let sun = light(&json, "sun").unwrap();
    assert_eq!(sun["angularDiameter"], 0.0);
    assert_eq!(sun["castShadow"], true);
    assert_eq!(sun["shadowStrength"], "0.8");
    for id in ["point", "spot"] {
        let node = light(&json, id).unwrap();
        assert_eq!(node["sourceRadius"], 0.0);
        assert_eq!(node["castShadow"], false);
    }
    assert_eq!(light(&json, "area").unwrap()["castShadow"], false);

    // Old serialized nodes omit the additive physical source fields.
    let mut old = json.clone();
    for (kind, field) in [
        ("directionalLight", "angularDiameter"),
        ("pointLight", "sourceRadius"),
        ("spotLight", "sourceRadius"),
        ("rectAreaLight", "castShadow"),
    ] {
        omit_light_field(&mut old, kind, field);
    }
    let restored: motionloom::GraphScript = serde_json::from_value(old).unwrap();
    assert_eq!(json, serde_json::to_value(&restored).unwrap());
}

#[test]
fn per_light_attributes_are_typed_roundtrip_and_authoring_recognizes_them() {
    let source = cpu_stage(
        "<DirectionalLight id=\"sun\" angularDiameter=\"0.5\" castShadow=\"true\" />\n<PointLight id=\"point\" sourceRadius=\"0.025\" castShadow=\"true\" />\n<SpotLight id=\"spot\" sourceRadius=\"0.1\" castShadow=\"true\" />\n<RectAreaLight id=\"area\" width=\"0.6\" height=\"0.4\" castShadow=\"true\" />",
    );
    let graph = parse_graph_script(&source).unwrap();
    let style = motionloom::api::resolve_scene_render_style(&graph, "room").unwrap();
    assert!(style.per_light_shadows);
    assert_eq!(
        graph.render_styles[0]
            .lighting
            .as_ref()
            .unwrap()
            .shadow_mode
            .as_deref(),
        Some("perLight")
    );
    let json = serde_json::to_value(&graph).unwrap();
    assert_eq!(
        light(&json, "sun").unwrap()["angularDiameter"]
            .as_f64()
            .unwrap(),
        0.5
    );
    for (id, expected) in [("point", 0.025), ("spot", 0.1)] {
        let actual = light(&json, id).unwrap()["sourceRadius"].as_f64().unwrap();
        assert!((actual - expected).abs() < 1e-7);
    }
    for id in ["sun", "point", "spot", "area"] {
        assert_eq!(light(&json, id).unwrap()["castShadow"], true);
    }
    let restored: motionloom::GraphScript = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(json, serde_json::to_value(&restored).unwrap());
    assert_eq!(
        style,
        motionloom::api::resolve_scene_render_style(&restored, "room").unwrap()
    );
    let report: Value = serde_json::from_str(
        &motionloom::api::motionloom_analyze_script_for_target_json(&source, "native-webgpu"),
    )
    .unwrap();
    for counter in ["errors", "unknownTags", "ignoredAttributes"] {
        assert_eq!(report["summary"][counter], 0, "{report}");
    }
}

#[test]
fn source_sizes_are_finite_literals_with_explicit_bounds() {
    for value in ["0", "0.5", "90"] {
        parse_graph_script(&cpu_stage(&format!(
            "<DirectionalLight angularDiameter=\"{value}\" />"
        )))
        .unwrap();
    }
    for value in [
        "-0.001",
        "90.001",
        "NaN",
        "inf",
        "$time.sec",
        "curve(0:0,1:1)",
    ] {
        assert!(
            parse_graph_script(&cpu_stage(&format!(
                "<DirectionalLight angularDiameter=\"{value}\" />"
            )))
            .is_err(),
            "accepted angularDiameter={value}"
        );
    }
    for kind in ["PointLight", "SpotLight"] {
        for value in ["0", "0.025", "1000"] {
            parse_graph_script(&cpu_stage(&format!("<{kind} sourceRadius=\"{value}\" />")))
                .unwrap();
        }
        for value in [
            "-0.001",
            "1000.001",
            "NaN",
            "inf",
            "$time.sec",
            "curve(0:0,1:1)",
        ] {
            assert!(
                parse_graph_script(&cpu_stage(&format!("<{kind} sourceRadius=\"{value}\" />")))
                    .is_err(),
                "accepted {kind} sourceRadius={value}"
            );
        }
        assert!(
            !motionloom::animation_properties_for_node_kind(kind)
                .iter()
                .any(|property| property.path == "sourceRadius")
        );
    }
    assert!(
        !motionloom::animation_properties_for_node_kind("DirectionalLight")
            .iter()
            .any(|property| property.path == "angularDiameter")
    );
}

#[test]
fn shadow_mode_and_area_shadow_toggle_use_strict_literals() {
    for mode in ["legacy", "perLight"] {
        let source = cpu_stage("").replace("perLight", mode);
        let graph = parse_graph_script(&source).unwrap();
        assert_eq!(
            motionloom::api::resolve_scene_render_style(&graph, "room")
                .unwrap()
                .per_light_shadows,
            mode == "perLight"
        );
    }
    for mode in ["perlight", "all", "true", "$time.sec"] {
        assert!(
            parse_graph_script(&cpu_stage("").replace("perLight", mode)).is_err(),
            "accepted shadowMode={mode}"
        );
    }
    for value in ["true", "false"] {
        parse_graph_script(&cpu_stage(&format!(
            "<RectAreaLight castShadow=\"{value}\" />"
        )))
        .unwrap();
    }
    for value in ["1", "0", "yes", "False", "$time.sec"] {
        assert!(
            parse_graph_script(&cpu_stage(&format!(
                "<RectAreaLight castShadow=\"{value}\" />"
            )))
            .is_err(),
            "accepted area castShadow={value}"
        );
    }
}

async fn renderer() -> SceneRenderer {
    let mut renderer = SceneRenderer::new(SceneRenderProfile::Gpu).await.unwrap();
    // Balanced has no SSR/SSGI; authored AA and explicit AO/contact are off.
    renderer.set_immediate_preview_settings(ImmediatePreviewSettings {
        profile: ImmediatePreviewProfile::Balanced,
        dynamic_resolution: false,
        min_resolution_scale: 1.0,
        ..Default::default()
    });
    renderer
}

async fn render(renderer: &mut SceneRenderer, source: &str) -> image::RgbaImage {
    let graph = parse_graph_script(source).expect("per-light fixture DSL must parse");
    renderer
        .render_frame_gpu_readback(&graph, 0)
        .await
        .expect("native per-light fixture")
}

#[test]
#[ignore = "requires a real native GPU"]
fn retained_shadow_views_reuse_depth_until_a_caster_or_light_projection_moves() {
    let assets = r##"<GeometryAsset id="block_geo"><Primitive shape="box" size={[0.28,0.6,0.65]} /></GeometryAsset>
<MeshAsset id="block" material="black" geometry="block_geo" />"##;
    let source = |caster_x: f32, light_x: f32, radius: f32| {
        let lights = format!(
            r##"<DirectionalLight id="sun" direction={{[0,-1,-0.8]}} intensity="0.8" castShadow="true" angularDiameter="0.5" />
<PointLight id="bulb" position={{[{light_x},2,0]}} intensity="10" range="8" sourceRadius="{radius}" castShadow="true" />"##
        );
        let models = format!(
            "<Model id=\"caster\" asset=\"block\" position={{[{caster_x},1,0]}} castShadow=\"true\" />"
        );
        stage(
            assets,
            &lights,
            &models,
            "position={[0,7,0.001]} target={[0,0,0]} fov=\"44\"",
        )
    };
    pollster::block_on(async {
        let mut renderer = renderer().await;
        let static_source = source(0.8, 1.6, 0.025);
        let first = render(&mut renderer, &static_source).await;
        let initial = renderer.last_3d_frame_profile();
        assert_eq!(
            initial.shadow_view_count, 7,
            "one primary plus six point faces: {initial:?}"
        );
        assert_eq!(initial.shadow_rendered_views, initial.shadow_view_count);
        let repeated = render(&mut renderer, &static_source).await;
        let cached = renderer.last_3d_frame_profile();
        assert_eq!(
            cached.shadow_rendered_views, 0,
            "static depth must be retained: {cached:?}"
        );
        assert_eq!(cached.shadow_cache_hits, cached.shadow_view_count);
        assert_eq!(
            first, repeated,
            "retaining depth must preserve static pixels"
        );

        // Source radius changes receiver filtering, not the depth projection.
        render(&mut renderer, &source(0.8, 1.6, 0.1)).await;
        let radius_only = renderer.last_3d_frame_profile();
        assert_eq!(
            radius_only.shadow_rendered_views, 0,
            "source-radius edit needlessly rebuilt depth: {radius_only:?}"
        );
        assert_eq!(radius_only.shadow_cache_hits, radius_only.shadow_view_count);

        render(&mut renderer, &source(0.55, 1.6, 0.1)).await;
        let moved_caster = renderer.last_3d_frame_profile();
        assert!(
            moved_caster.shadow_rendered_views > 0,
            "moving a caster reused stale depth: {moved_caster:?}"
        );
        assert_eq!(
            moved_caster.shadow_rendered_views + moved_caster.shadow_cache_hits,
            moved_caster.shadow_view_count
        );
        render(&mut renderer, &source(0.55, 1.6, 0.1)).await;
        let settled = renderer.last_3d_frame_profile();
        assert_eq!(settled.shadow_rendered_views, 0);
        assert_eq!(settled.shadow_cache_hits, settled.shadow_view_count);

        render(&mut renderer, &source(0.55, 1.9, 0.1)).await;
        let moved_light = renderer.last_3d_frame_profile();
        assert_eq!(
            moved_light.shadow_rendered_views, 6,
            "moving the point source must rebuild its six faces: {moved_light:?}"
        );
        assert_eq!(
            moved_light.shadow_cache_hits, 1,
            "unmoved primary view should stay cached: {moved_light:?}"
        );
    });
}

fn linear(byte: u8) -> f64 {
    let x = f64::from(byte) / 255.0;
    if x <= 0.04045 {
        x / 12.92
    } else {
        ((x + 0.055) / 1.055).powf(2.4)
    }
}

fn difference(a: &image::RgbaImage, b: &image::RgbaImage, region: Region, channel: usize) -> f64 {
    let (x0, y0, x1, y1) = region;
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += (linear(a.get_pixel(x, y)[channel]) - linear(b.get_pixel(x, y)[channel])).abs();
        }
    }
    sum / f64::from((x1 - x0) * (y1 - y0))
}

fn mean(a: &image::RgbaImage, region: Region, channel: usize) -> f64 {
    let (x0, y0, x1, y1) = region;
    let mut sum = 0.0;
    for y in y0..y1 {
        for x in x0..x1 {
            sum += linear(a.get_pixel(x, y)[channel]);
        }
    }
    sum / f64::from((x1 - x0) * (y1 - y0))
}

#[test]
#[ignore = "requires a real native GPU"]
fn a_nonprimary_blue_shadow_does_not_mask_the_primary_red_light() {
    let assets = r##"<GeometryAsset id="block_geo"><Primitive shape="box" size={[0.28,0.6,0.65]} /></GeometryAsset>
<MeshAsset id="block" material="black" geometry="block_geo" />"##;
    let lights = r##"<DirectionalLight id="red_primary" color="#FF0000" direction={[0,-1,-0.8]} intensity="1.4" castShadow="true" shadowStrength="1" angularDiameter="0" />
<PointLight id="blue_room" color="#0000FF" position={[2,2,0]} intensity="14" range="8" sourceRadius="0.025" castShadow="true" />"##;
    let source = |red: bool, blue: bool| {
        stage(
            assets,
            lights,
            &format!(
                "<Model id=\"red_caster\" asset=\"block\" position={{[-1,1,0.8]}} castShadow=\"{red}\" />\n<Model id=\"blue_caster\" asset=\"block\" position={{[1,1,0]}} castShadow=\"{blue}\" />"
            ),
            "position={[0,7,0.001]} target={[0,0,0]} fov=\"44\"",
        )
    };
    pollster::block_on(async {
        let mut renderer = renderer().await;
        let clear = render(&mut renderer, &source(false, false)).await;
        let red = render(&mut renderer, &source(true, false)).await;
        let blue = render(&mut renderer, &source(false, true)).await;
        clear
            .save("/private/tmp/s99-shadow-owned-clear.png")
            .unwrap();
        red.save("/private/tmp/s99-shadow-owned-red.png").unwrap();
        blue.save("/private/tmp/s99-shadow-owned-blue.png").unwrap();
        // Projected receiver patches exclude both visible caster silhouettes.
        let red_region = (33, 56, 51, 72);
        let blue_region = (54, 56, 74, 72);
        let red_change = difference(&clear, &red, red_region, 0);
        let blue_change = difference(&clear, &blue, blue_region, 2);
        assert!(
            red_change > 0.015,
            "primary red shadow absent: {red_change}"
        );
        assert!(
            blue_change > 0.015,
            "nonprimary blue shadow absent: {blue_change}"
        );
        let red_cross = difference(&clear, &blue, blue_region, 0);
        let blue_cross = difference(&clear, &red, red_region, 2);
        assert!(
            red_cross < blue_change * 0.18 + 0.003,
            "blue caster masked primary red radiance: owned={blue_change}, red={red_cross}"
        );
        assert!(
            blue_cross < red_change * 0.18 + 0.003,
            "red caster masked blue radiance: owned={red_change}, blue={blue_cross}"
        );
    });
}

#[test]
#[ignore = "requires a real native GPU"]
fn point_shadow_cube_covers_receivers_on_both_sides_of_the_emitter() {
    let assets = r##"<GeometryAsset id="block_geo"><Primitive shape="box" size={[0.25,0.3,0.7]} /></GeometryAsset>
<MeshAsset id="block" material="black" geometry="block_geo" />"##;
    let lights = r##"<PointLight id="bulb" position={[0,1.6,0]} intensity="14" range="8" sourceRadius="0.025" castShadow="true" />"##;
    let source = |left: bool, right: bool| {
        stage(
            assets,
            lights,
            &format!(
                "<Model id=\"left_caster\" asset=\"block\" position={{[-1.2,0.8,0]}} castShadow=\"{left}\" />\n<Model id=\"right_caster\" asset=\"block\" position={{[1.2,0.8,0]}} castShadow=\"{right}\" />"
            ),
            "position={[0,7.6,0.001]} target={[0,0,0]} fov=\"48\"",
        )
    };
    pollster::block_on(async {
        let mut renderer = renderer().await;
        let clear = render(&mut renderer, &source(false, false)).await;
        let left = render(&mut renderer, &source(true, false)).await;
        let right = render(&mut renderer, &source(false, true)).await;
        // These floor points have |X-light.X| > |Y-light.Y| and select
        // the -X/+X cube faces, not a shared downward spotlight surrogate.
        let left_region = (6, 48, 31, 80);
        let right_region = (97, 48, 122, 80);
        let left_change = difference(&clear, &left, left_region, 0);
        let right_change = difference(&clear, &right, right_region, 0);
        assert!(
            left_change > 0.008,
            "negative-X point-light shadow absent: {left_change}"
        );
        assert!(
            right_change > 0.008,
            "positive-X point-light shadow absent: {right_change}"
        );
        assert!(difference(&clear, &left, right_region, 0) < left_change * 0.15 + 0.002);
        assert!(difference(&clear, &right, left_region, 0) < right_change * 0.15 + 0.002);
    });
}

#[derive(Debug)]
struct Penumbra {
    dark_pixels: usize,
    transition_pixels: usize,
    // Normalize by sqrt(umbra area), a proxy for shadow perimeter. A larger
    // projected hard shadow alone therefore cannot satisfy the softness test.
    width: f64,
}
fn penumbra(shadowed: &image::RgbaImage, clear: &image::RgbaImage) -> Penumbra {
    let mut dark_pixels = 0;
    let mut transition_pixels = 0;
    for y in 4..124 {
        for x in 4..124 {
            let before = linear(clear.get_pixel(x, y)[0]);
            if before < 0.035 {
                continue;
            } // Exclude black caster/background pixels.
            let ratio = linear(shadowed.get_pixel(x, y)[0]) / before;
            if ratio < 0.35 {
                dark_pixels += 1;
            }
            if (0.35..0.90).contains(&ratio) {
                transition_pixels += 1;
            }
        }
    }
    Penumbra {
        dark_pixels,
        transition_pixels,
        width: transition_pixels as f64 / (dark_pixels.max(1) as f64).sqrt(),
    }
}

#[test]
#[ignore = "requires a real native GPU"]
fn source_radius_and_receiver_separation_both_widen_the_penumbra() {
    let assets = r##"<GeometryAsset id="plate_geo"><Primitive shape="box" size={[0.5,0.08,0.85]} /></GeometryAsset>
<MeshAsset id="plate" material="black" geometry="plate_geo" />"##;
    let source = |radius: f32, height: f32, cast: bool| {
        let lights = format!(
            "<SpotLight id=\"soft_source\" position={{[-1.1,3,0]}} direction={{[0.35,-1,0]}} intensity=\"10\" range=\"10\" innerCone=\"30\" outerCone=\"35\" sourceRadius=\"{radius}\" castShadow=\"true\" />"
        );
        let models = format!(
            "<Model id=\"plate\" asset=\"plate\" position={{[0,{height},0]}} castShadow=\"{cast}\" />"
        );
        stage(
            assets,
            &lights,
            &models,
            "position={[0.3,7,0.001]} target={[0.3,0,0]} fov=\"36\"",
        )
    };
    pollster::block_on(async {
        let mut renderer = renderer().await;
        let far_clear = render(&mut renderer, &source(0.025, 1.7, false)).await;
        let far_small = render(&mut renderer, &source(0.025, 1.7, true)).await;
        let far_large = render(&mut renderer, &source(0.1, 1.7, true)).await;
        let near_clear = render(&mut renderer, &source(0.1, 0.3, false)).await;
        let near_large = render(&mut renderer, &source(0.1, 0.3, true)).await;
        let small = penumbra(&far_small, &far_clear);
        let large = penumbra(&far_large, &far_clear);
        let near = penumbra(&near_large, &near_clear);
        assert!(
            small.dark_pixels > 8 && large.dark_pixels > 8 && near.dark_pixels > 6,
            "fixtures must retain a real umbra: small={small:?}, large={large:?}, near={near:?}"
        );
        assert!(
            large.transition_pixels > 8,
            "large source has no measurable penumbra: {large:?}"
        );
        assert!(
            large.width > small.width * 1.20 + 0.10,
            "quadrupling source radius must soften the same shadow: small={small:?}, large={large:?}"
        );
        assert!(
            large.width > near.width * 1.20 + 0.10,
            "moving the blocker away from the receiver must soften contact: far={large:?}, near={near:?}"
        );
    });
}

#[test]
#[ignore = "requires a real native GPU"]
fn area_light_shadow_changes_its_own_radiance_only() {
    let assets = r##"<GeometryAsset id="plate_geo"><Primitive shape="box" size={[0.45,0.08,0.7]} /></GeometryAsset>
<MeshAsset id="plate" material="black" geometry="plate_geo" />"##;
    let source = |cast: bool| {
        let lights = format!(
            r##"<RectAreaLight id="red_area" position={{[-1.3,3,0]}} direction={{[0.4,-1,0]}} color="#FF0000" intensity="12" width="0.6" height="0.4" castShadow="{cast}" />
<PointLight id="blue_unshadowed" position={{[1.5,2,0]}} color="#0000FF" intensity="10" range="8" castShadow="false" />"##
        );
        stage(
            assets,
            &lights,
            "<Model id=\"plate\" asset=\"plate\" position={[-0.65,1.5,0]} castShadow=\"true\" />",
            "position={[0,7,0.001]} target={[0,0,0]} fov=\"44\"",
        )
    };
    pollster::block_on(async {
        let mut renderer = renderer().await;
        let clear = render(&mut renderer, &source(false)).await;
        let occluded = render(&mut renderer, &source(true)).await;
        clear
            .save("/private/tmp/s99-shadow-area-clear.png")
            .unwrap();
        occluded
            .save("/private/tmp/s99-shadow-area-occluded.png")
            .unwrap();
        let region = (53, 54, 75, 74);
        let red = difference(&clear, &occluded, region, 0);
        let blue = difference(&clear, &occluded, region, 2);
        assert!(
            red > 0.012,
            "area emitter never produced its own caster shadow: {red}"
        );
        assert!(
            blue < red * 0.15 + 0.003,
            "area shadow masked another emitter: red={red}, blue={blue}"
        );
    });
}

fn opacity_texture(mode: u8) -> String {
    let mut image = image::RgbaImage::new(16, 16);
    for y in 0..16 {
        for x in 0..16 {
            let alpha = match mode {
                0 => 0,
                1 => 255,
                _ => {
                    if (x / 4 + y / 4) % 2 == 0 {
                        255
                    } else {
                        0
                    }
                }
            };
            image.put_pixel(x, y, image::Rgba([255, 255, 255, alpha]));
        }
    }
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes.into_inner())
    )
}

#[test]
#[ignore = "requires a real native GPU"]
fn masked_caster_opacity_controls_the_shadow_silhouette() {
    let source = |mode| {
        let assets = format!(
            r##"<ImageAsset id="cutout_image" src="{}" colorSpace="srgb" />
<MaterialAsset id="cutout_mat" baseColor="#000000" baseColorTexture="cutout_image" specular="0" doubleSided="true" alphaMode="mask" alphaCutoff="0.5" />
<GeometryAsset id="cutout_geo"><Mesh>
<Vertex position={{[-0.45,0,-0.45]}} uv={{[0,0]}} /><Vertex position={{[-0.45,0,0.45]}} uv={{[0,1]}} />
<Vertex position={{[0.45,0,0.45]}} uv={{[1,1]}} /><Vertex position={{[0.45,0,-0.45]}} uv={{[1,0]}} />
<Face indices={{[0,1,2,3]}} />
</Mesh></GeometryAsset><MeshAsset id="cutout" material="cutout_mat" geometry="cutout_geo" />"##,
            opacity_texture(mode)
        );
        stage(
            &assets,
            "<PointLight id=\"bulb\" position={[-1.3,3,0]} intensity=\"12\" range=\"10\" sourceRadius=\"0\" castShadow=\"true\" />",
            "<Model id=\"cutout\" asset=\"cutout\" position={[-0.65,1.5,0]} castShadow=\"true\" />",
            "position={[0,7,0.001]} target={[0,0,0]} fov=\"44\"",
        )
    };
    pollster::block_on(async {
        let mut renderer = renderer().await;
        let transparent = render(&mut renderer, &source(0)).await;
        let opaque = render(&mut renderer, &source(1)).await;
        let checker = render(&mut renderer, &source(2)).await;
        // Sample only the projected floor shadow, away from the cutout itself.
        let region = (55, 54, 75, 74);
        let clear = mean(&transparent, region, 0);
        let solid = mean(&opaque, region, 0);
        let holes = mean(&checker, region, 0);
        assert!(
            clear - solid > 0.025,
            "opaque caster failed to occlude: clear={clear}, solid={solid}"
        );
        assert!(
            holes > solid + (clear - solid) * 0.15 && holes < clear - (clear - solid) * 0.15,
            "alpha holes must transmit part of the shadow footprint: clear={clear}, solid={solid}, holes={holes}"
        );
    });
}
