//! Native image contracts for shared automatic slab-glass captures. Dense
//! geometry is off camera and only activates the normal production threshold.

use crate::api::{SceneRenderProfile, SceneRenderer, parse_graph_script};
use crate::preview::{ImmediatePreviewProfile, ImmediatePreviewSettings};

use super::super::{Scene3DFrameProfile, planar};

const ASSETS: &str = r##"
<MaterialAsset id="glass_left" baseColor="#FFFFFF" roughness="0.045" normalScale="0"
 transmission="0.97" ior="1.52" thickness="0.012" attenuationColor="#2080FF"
 attenuationDistance="0.015" doubleSided="true" />
<MaterialAsset id="glass_right" baseColor="#FFFFFF" roughness="0.045" normalScale="0"
 transmission="0.97" ior="1.52" thickness="0.012" attenuationColor="#FF8020"
 attenuationDistance="0.015" doubleSided="true" />
<MaterialAsset id="red" baseColor="#000000" specular="0" roughness="1"
 emissive="#FF0000" emissiveStrength="8" />
<MaterialAsset id="blue" baseColor="#000000" specular="0" roughness="1"
 emissive="#0000FF" emissiveStrength="8" />
<MaterialAsset id="dim_white" baseColor="#000000" specular="0" roughness="1"
 emissive="#FFFFFF" emissiveStrength="0.12" />
<MaterialAsset id="black" baseColor="#000000" specular="0" roughness="1" />
<GeometryAsset id="pane_geometry"><Primitive shape="box" size={[1.45,2.2,0.012]} /></GeometryAsset>
<MeshAsset id="pane_left" geometry="pane_geometry" material="glass_left" />
<MeshAsset id="pane_right" geometry="pane_geometry" material="glass_right" />
<GeometryAsset id="card_geometry"><Primitive shape="box" size={[4,4,0.04]} /></GeometryAsset>
<MeshAsset id="red_card" geometry="card_geometry" material="red" />
<MeshAsset id="blue_card" geometry="card_geometry" material="blue" />
<GeometryAsset id="back_geometry"><Primitive shape="box" size={[6,6,0.04]} /></GeometryAsset>
<MeshAsset id="back" geometry="back_geometry" material="dim_white" />
<GeometryAsset id="stripe_geometry"><Primitive shape="box" size={[0.08,4,0.02]} /></GeometryAsset>
<MeshAsset id="stripe" geometry="stripe_geometry" material="black" />
<GeometryAsset id="dense_geometry"><Primitive shape="sphere" radius="1" segments="192" rings="128" /></GeometryAsset>
<MeshAsset id="dense" geometry="dense_geometry" material="black" />
"##;

const LEFT: &str =
    r#"<Model id="pane-left" asset="pane_left" position={[-0.75,0,0]} castShadow="false" />"#;
const RIGHT: &str =
    r#"<Model id="pane-right" asset="pane_right" position={[0.75,0,0]} castShadow="false" />"#;
const CARDS: &str = r#"<Model id="offscreen-red" asset="red_card" position={[-2,0,5]} castShadow="false" />
<Model id="offscreen-blue" asset="blue_card" position={[2,0,5]} castShadow="false" />"#;

fn source(reverse: bool, cards: bool, underlay: bool, oblique: bool, bounces: u32) -> String {
    let panes = if reverse {
        format!("{RIGHT}{LEFT}")
    } else {
        format!("{LEFT}{RIGHT}")
    };
    let cards = if cards { CARDS } else { "" };
    let underlay = if underlay {
        let stripes = (0..17).map(|index| format!(
            "<Model id=\"stripe-{index}\" asset=\"stripe\" position={{[{},0,-0.8]}} castShadow=\"false\" />",
            (index as f32 - 8.0) * 0.28,
        ))
        .collect::<String>();
        format!(
            r#"<Model id="underlay" asset="back" position={{[0,0,-1]}} castShadow="false" />{stripes}"#
        )
    } else {
        String::new()
    };
    let camera = if oblique { "[1.8,0,4]" } else { "[0,0,4]" };
    format!(
        r##"<Graph fps="24" duration="1s" size={{[256,192]}}>
<RenderStyle id="style"><SurfaceStyle shading="physical" />
<LightingStyle ambientIntensity="0" shadowMode="perLight" reflectionBounces="{bounces}" />
<PostStyle toneMapping="none" exposure="1" /><AntiAliasingStyle method="off" /></RenderStyle>
<Assets>{ASSETS}</Assets><Background color="#000000" />
<Scene id="stage" renderStyle="style"><Timeline><Track id="track" space="3d">
<Sequence duration="1s"><CompositeGroup id="room" space="3d" depth="true" format="rgba16f">
<Camera3D position={{{camera}}} target={{[0,0,0]}} fov="45" />
<DirectionalLight direction={{[0,0,-1]}} intensity="0" castShadow="false" />
<Model id="dense-offscreen" asset="dense" position={{[1000,1000,1000]}} castShadow="false" />
{panes}{cards}{underlay}</CompositeGroup></Sequence></Track></Timeline></Scene>
<Present from="stage" /></Graph>"##
    )
}

async fn image(source: &str, enabled: bool) -> (image::RgbaImage, Scene3DFrameProfile) {
    // Scoped thread-local selection also controls the optional light pipeline.
    // Parallel tests never mutate process-wide diagnostic environment variables.
    let _feature = planar::test_automatic_glass(enabled);
    let graph = parse_graph_script(source).expect("automatic planar glass fixture parse");
    let mut renderer = SceneRenderer::new(SceneRenderProfile::Gpu)
        .await
        .expect("automatic planar glass native renderer");
    renderer.set_immediate_preview_settings(ImmediatePreviewSettings {
        profile: ImmediatePreviewProfile::Portable,
        target_fps: 30.0,
        dynamic_resolution: false,
        min_resolution_scale: 1.0,
    });
    let image = renderer
        .render_frame_gpu_readback(&graph, 0)
        .await
        .expect("automatic planar glass GPU frame");
    let profile = renderer.last_3d_frame_profile();
    assert!(profile.hybrid_triangles >= 32_768);
    assert!(!profile.temporal_antialiasing);
    assert_eq!(profile.transmission_layers, 2);
    if enabled {
        assert_eq!(
            profile.planar_capture_count, 1,
            "coplanar panes must share Portable's one capture slot"
        );
        assert!(profile.planar_capture_draw_calls > 0);
        assert_eq!(profile.planar_capture_bytes, 64 * 48 * 24);
        assert!(
            profile.planar_slab_cached_triangles >= 2,
            "fixture must exercise certified light-shader triangles"
        );
        assert!(
            profile.planar_slab_discarded_triangles > 0,
            "fixture must exercise proven closed-slab back-face rejection"
        );
    } else {
        assert_eq!(
            profile.planar_capture_count, 0,
            "disabled fixture must exercise full geometry transport"
        );
    }
    (image, profile)
}

fn evidence(label: &str, image: &image::RgbaImage) {
    let Some(root) = std::env::var_os("MOTIONLOOM_HYBRID_EVIDENCE_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).expect("automatic planar glass evidence directory");
    image
        .save(root.join(format!("planar-glass-{label}.png")))
        .expect("automatic planar glass evidence PNG");
}

fn patch_mean(image: &image::RgbaImage, [left, top, right, bottom]: [u32; 4]) -> [f64; 3] {
    let mut sum = [0_u64; 3];
    for y in top..bottom {
        for x in left..right {
            for channel in 0..3 {
                sum[channel] += u64::from(image.get_pixel(x, y)[channel]);
            }
        }
    }
    sum.map(|value| value as f64 / f64::from((right - left) * (bottom - top)))
}

fn patch_error(
    a: &image::RgbaImage,
    b: &image::RgbaImage,
    [left, top, right, bottom]: [u32; 4],
) -> f64 {
    let mut error = 0_u64;
    for y in top..bottom {
        for x in left..right {
            for channel in 0..3 {
                error += u64::from(a.get_pixel(x, y)[channel].abs_diff(b.get_pixel(x, y)[channel]));
            }
        }
    }
    error as f64 / f64::from((right - left) * (bottom - top) * 3)
}

#[test]
fn automatic_planar_glass_fixtures_are_self_contained_and_parse() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    for reverse in [false, true] {
        for bounces in [1, 2] {
            for input in [
                source(reverse, false, false, false, bounces),
                source(reverse, true, false, false, bounces),
                thick_refraction_source(reverse),
            ] {
                parse_graph_script(&input).expect("automatic planar glass CPU fixture parse");
            }
        }
    }
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn automatic_glass_without_a_normal_texture_keeps_geometric_normals() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let explicit_flat = source(false, true, false, false, 1);
        let missing_map = explicit_flat.replace(" normalScale=\"0\"", "");
        let (flat, _) = image(&explicit_flat, true).await;
        let (missing, _) = image(&missing_map, true).await;
        assert_eq!(
            missing, flat,
            "an absent map must not tilt the geometric normal"
        );
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn automatic_glass_group_reflects_both_offcamera_cards_in_one_portable_capture() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        for bounces in [1, 2] {
            let (absent, _) = image(&source(false, false, false, false, bounces), true).await;
            let input = source(false, true, false, false, bounces);
            let (cached, _) = image(&input, true).await;
            let (direct, _) = image(&input, false).await;
            let (reverse, _) = image(&source(true, true, false, false, bounces), true).await;
            evidence(&format!("offcamera-{bounces}-cached"), &cached);
            evidence(&format!("offcamera-{bounces}-direct"), &direct);
            evidence(&format!("offcamera-{bounces}-reverse"), &reverse);
            for (label, bounds, channel) in [
                ("left red", [68, 76, 100, 116], 0),
                ("right blue", [156, 76, 188, 116], 2),
            ] {
                let old = patch_mean(&absent, bounds);
                let actual = patch_mean(&cached, bounds);
                assert!(
                    actual[channel] > old[channel] + 20.0,
                    "{label} offcamera reflection missing: absent={old:?}, cached={actual:?}"
                );
                assert!(
                    patch_error(&cached, &direct, bounds) < 8.0,
                    "{label} quarter capture diverged from direct transport"
                );
                assert!(
                    patch_error(&cached, &reverse, bounds) <= 1.0,
                    "{label} shared capture changed under Model authoring order"
                );
            }
        }
    });
}

fn thick_refraction_source(reverse: bool) -> String {
    source(reverse, false, true, true, 2)
        .replace("size={[1.45,2.2,0.012]}", "size={[1.45,2.2,0.3]}")
        .replace("thickness=\"0.012\"", "thickness=\"0.3\"")
        .replace(
            "attenuationDistance=\"0.015\"",
            "attenuationDistance=\"0.3\"",
        )
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn automatic_glass_group_preserves_oblique_refraction_and_perpane_absorption() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let input = thick_refraction_source(false);
        let clear = input
            .replace("#2080FF", "#FFFFFF")
            .replace("#FF8020", "#FFFFFF");
        let zero = clear.replace("thickness=\"0.3\"", "thickness=\"0\"");
        let (cached, _) = image(&input, true).await;
        let (direct, _) = image(&input, false).await;
        let (reverse, _) = image(&thick_refraction_source(true), true).await;
        let (clear_direct, _) = image(&clear, false).await;
        let (zero_direct, _) = image(&zero, false).await;
        evidence("refraction-cached", &cached);
        evidence("refraction-direct", &direct);
        evidence("refraction-zero-thickness", &zero_direct);
        for (label, bounds, high, low) in [
            ("left blue absorption", [62, 76, 94, 116], 2, 0),
            ("right red absorption", [160, 76, 192, 116], 0, 2),
        ] {
            let actual = patch_mean(&cached, bounds);
            assert!(
                actual[high] > actual[low] + 15.0,
                "{label} lost its independent optical response: {actual:?}"
            );
            assert!(
                patch_error(&cached, &direct, bounds) < 8.0,
                "{label} cache changed slab refraction/absorption"
            );
            assert!(
                patch_error(&cached, &reverse, bounds) <= 1.0,
                "{label} changed under Model authoring order"
            );
        }
        let refracted_pixels = (48..144)
            .flat_map(|y| (45..215).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                (0..3).any(|channel| {
                    clear_direct.get_pixel(x, y)[channel]
                        .abs_diff(zero_direct.get_pixel(x, y)[channel])
                        > 12
                })
            })
            .count();
        assert!(
            refracted_pixels > 40,
            "fixture does not expose physical slab displacement: {refracted_pixels} changed pixels"
        );
    });
}
