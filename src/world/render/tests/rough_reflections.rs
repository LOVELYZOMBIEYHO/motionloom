//! Native image contracts for bounded-resolution opaque rough reflection radiance.
//! Dense geometry is off camera; visible geometry and offscreen emitters stay simple.

use crate::api::{SceneRenderProfile, SceneRenderer, parse_graph_script};
use crate::preview::{ImmediatePreviewProfile, ImmediatePreviewSettings};

const COMMON_ASSETS: &str = r##"
<MaterialAsset id="rough" baseColor="#FFFFFF" metallic="1" roughness="0.35" normalScale="0" />
<MaterialAsset id="rough_disjoint" baseColor="#FFFFFF" metallic="1" roughness="0.15" normalScale="0" />
<MaterialAsset id="coat_only" baseColor="#000000" metallic="0" specular="0" roughness="1"
  clearcoat="1" clearcoatRoughness="0.28" normalScale="0" />
<MaterialAsset id="red" baseColor="#000000" specular="0" roughness="1"
  emissive="#FF0000" emissiveStrength="1" />
<MaterialAsset id="blue" baseColor="#000000" specular="0" roughness="1"
  emissive="#0000FF" emissiveStrength="1" />
<MaterialAsset id="far_black" baseColor="#000000" specular="0" roughness="1" />
<GeometryAsset id="far_geometry"><Primitive shape="sphere" radius="1" segments="192" rings="128" /></GeometryAsset>
<MeshAsset id="far" geometry="far_geometry" material="far_black" />
<GeometryAsset id="emitter_geometry"><Primitive shape="box" size={[12,12,0.04]} /></GeometryAsset>
<MeshAsset id="red_emitter" geometry="emitter_geometry" material="red" />
<GeometryAsset id="split_geometry"><Primitive shape="box" size={[8,12,0.04]} /></GeometryAsset>
<MeshAsset id="left_emitter" geometry="split_geometry" material="red" />
<MeshAsset id="right_emitter" geometry="split_geometry" material="blue" />
"##;

fn stage(geometry: &str, models: &str) -> String {
    format!(
        r##"<Graph fps="24" duration="1s" size={{[256,192]}}>
<RenderStyle id="style"><SurfaceStyle shading="physical" />
<LightingStyle ambientIntensity="0" reflectionBounces="1" />
<PostStyle toneMapping="none" exposure="1" />
<AntiAliasingStyle method="off" /></RenderStyle>
<Assets>{COMMON_ASSETS}{geometry}</Assets>
<Background color="#000000" />
<Scene id="stage" renderStyle="style"><Timeline><Track id="track" space="3d">
<Sequence duration="1s"><CompositeGroup id="room" space="3d" depth="true" format="rgba16f">
<Camera3D position={{[0,0,4]}} target={{[0,0,0]}} fov="35" />
<DirectionalLight direction={{[0,0,-1]}} intensity="0" castShadow="false" />
<Model id="far_dense" asset="far" position={{[1000,1000,1000]}} castShadow="false" />
{models}</CompositeGroup></Sequence></Track></Timeline></Scene>
<Present from="stage" /></Graph>"##
    )
}

fn broad_sources() -> [String; 2] {
    let bare = stage(
        r#"<GeometryAsset id="face_geometry"><Primitive shape="box" size={[3,2,0.04]} /></GeometryAsset>
<MeshAsset id="face" geometry="face_geometry" material="rough" />"#,
        r#"<Model id="rough_face" asset="face" castShadow="false" />"#,
    );
    let reflected = bare.replace(
        "</CompositeGroup>",
        r#"<Model id="offscreen_red" asset="red_emitter" position={[0,0,5]} castShadow="false" /></CompositeGroup>"#,
    );
    [bare, reflected]
}

fn coating_sources() -> [String; 2] {
    let bare = stage(
        r#"<GeometryAsset id="coat_geometry"><Primitive shape="box" size={[3,2,0.04]} /></GeometryAsset>
<MeshAsset id="coated_face" geometry="coat_geometry" material="coat_only" />"#,
        r#"<Model id="coated_face" asset="coated_face" castShadow="false" />"#,
    );
    let reflected = bare.replace(
        "</CompositeGroup>",
        r#"<Model id="offscreen_red" asset="red_emitter" position={[0,0,5]} castShadow="false" /></CompositeGroup>"#,
    );
    // The dielectric coating has a much smaller Fresnel response than the
    // metallic base fixture. Increase only this emitter's radiance so a
    // missing coat lobe produces a decisive, visible image difference.
    [bare, reflected]
        .map(|source| source.replace("emissiveStrength=\"1\"", "emissiveStrength=\"8\""))
}

fn high_incident_source() -> String {
    // Keep the reflected incident radiance distinct from the final surface
    // radiance. The dark metallic BRDF remains within the main HDR target's
    // range even when its incident light exceeds the cache's half range.
    broad_sources()[1]
        .replace(
            "baseColor=\"#FFFFFF\" metallic=\"1\" roughness=\"0.35\"",
            "baseColor=\"#101010\" metallic=\"1\" roughness=\"0.35\"",
        )
        .replace("exposure=\"1\"", "exposure=\"0.000125\"")
}

fn disjoint_source() -> String {
    stage(
        r#"<GeometryAsset id="face_geometry"><Primitive shape="box" size={[1.45,2,0.04]} /></GeometryAsset>
<MeshAsset id="face" geometry="face_geometry" material="rough_disjoint" />"#,
        r#"<Model id="left_face" asset="face" position={[-0.75,0,0]} castShadow="false" />
<Model id="right_face" asset="face" position={[0.75,0,0]} castShadow="false" />
<Model id="offscreen_red" asset="left_emitter" position={[-4,0,5]} castShadow="false" />
<Model id="offscreen_blue" asset="right_emitter" position={[4,0,5]} castShadow="false" />"#,
    )
}

fn thin_boundary_source() -> String {
    stage(
        r#"<GeometryAsset id="face_geometry"><Primitive shape="box" size={[3,0.75,0.04]} /></GeometryAsset>
<MeshAsset id="face" geometry="face_geometry" material="rough" />
<GeometryAsset id="thin_geometry"><Primitive shape="box" size={[0.021,0.7,0.04]} /></GeometryAsset>
<MeshAsset id="thin" geometry="thin_geometry" material="rough" />
<GeometryAsset id="rib_geometry"><Primitive shape="box" size={[0.02,0.75,0.015]} /></GeometryAsset>
<MeshAsset id="rib" geometry="rib_geometry" material="rough" />"#,
        r#"<Model id="broad_face" asset="face" position={[0,0.6,0]} castShadow="false" />
<Model id="thin_face" asset="thin" position={[0,-0.6,0]} castShadow="false" />
<Model id="near_tilted_rib" asset="rib" position={[-0.5,0.6,0.5]} rotation={[0,60,0]} castShadow="false" />
<Model id="offscreen_red" asset="red_emitter" position={[0,0,5]} castShadow="false" />"#,
    )
}

fn coplanar_route_sources() -> Vec<(&'static str, bool, String)> {
    let mut sources = Vec::new();
    for (kind, roughness, normal_scale, opacity) in
        [("fractional", 0.35, 0.0, 0.5), ("normal", 0.04, 4.0, 1.0)]
    {
        let geometry = format!(
            r##"
<MaterialAsset id="fallback_material" baseColor="#000000" metallic="1" roughness="{roughness}"
  normalScale="{normal_scale}" emissive="#0000FF" emissiveStrength="1" />
<GeometryAsset id="face_geometry"><Primitive shape="box" size={{[3,2,0.04]}} /></GeometryAsset>
<MeshAsset id="cached_face" geometry="face_geometry" material="rough" />
<MeshAsset id="fallback_face" geometry="face_geometry" material="fallback_material" />"##
        );
        let cached = r#"<Model id="cached" asset="cached_face" castShadow="false" />"#;
        let fallback = format!(
            r#"<Model id="fallback" asset="fallback_face" opacity="{opacity}" castShadow="false" />"#
        );
        for full_first in [false, true] {
            let models = if full_first {
                format!("{fallback}{cached}")
            } else {
                format!("{cached}{fallback}")
            };
            let models = format!(
                r#"{models}<Model id="offscreen_red" asset="red_emitter" position={{[0,0,5]}} castShadow="false" />"#
            );
            sources.push((kind, full_first, stage(&geometry, &models)));
        }
    }
    sources
}

fn assert_coplanar_fixture_opaque_phase(source: &str) {
    let graph = parse_graph_script(source).expect("coplanar route fixture parse");
    for id in ["cached_face", "fallback_face"] {
        let primitive = graph
            .assets
            .iter()
            .find(|asset| asset.id == id)
            .and_then(|asset| asset.primitive())
            .expect("resolved coplanar primitive");
        let mesh = crate::world::primitive::generate_primitive_mesh(primitive);
        for material in &mesh.materials {
            let phase = super::super::gpu_world_material_phase(Some(material));
            assert_eq!(
                phase,
                super::super::GpuWorldDrawPhase::Opaque,
                "{id} must remain in the authored opaque sequence"
            );
            assert!(
                super::super::gpu_world_material_depth_write(Some(material), phase),
                "{id} must retain Greater depth writes"
            );
        }
    }
    // Model.opacity changes packed actor coverage, independently of the loaded
    // material inputs used by the production phase/depth functions above.
}

async fn image(source: &str, profile: ImmediatePreviewProfile) -> image::RgbaImage {
    image_with_incident_strength(source, profile, None).await
}

async fn image_with_incident_strength(
    source: &str,
    profile: ImmediatePreviewProfile,
    incident_strength: Option<f32>,
) -> image::RgbaImage {
    let mut graph = parse_graph_script(source).expect("rough reflection fixture parse");
    if let Some(strength) = incident_strength {
        // Imported KHR_materials_emissive_strength values can exceed the DSL's
        // authored cap. Supply that renderer input directly without relaxing
        // parser validation or depending on an external asset file.
        assert!(strength.is_finite() && strength > 65504.0);
        let mut changed = 0;
        for asset in &mut graph.assets {
            let crate::dsl::GraphAssetSource::Primitive(primitive) = &mut asset.source else {
                continue;
            };
            if let Some(material) = primitive.material_definition.as_mut() {
                if material.id == "red" {
                    material.emissive_strength = strength;
                    changed += 1;
                }
            }
        }
        assert!(changed > 0, "HDR fixture needs a retained emitter material");
    }
    let mut renderer = SceneRenderer::new(SceneRenderProfile::Gpu)
        .await
        .expect("native rough reflection renderer");
    renderer.set_immediate_preview_settings(ImmediatePreviewSettings {
        profile,
        target_fps: 30.0,
        dynamic_resolution: false,
        min_resolution_scale: 1.0,
    });
    let rendered = renderer
        .render_frame_gpu_readback(&graph, 0)
        .await
        .expect("rough reflection GPU frame");
    let metrics = renderer.last_3d_frame_profile();
    assert!(metrics.hybrid_triangles >= 32_768);
    assert!(!metrics.temporal_antialiasing);
    match profile {
        ImmediatePreviewProfile::Cinematic => {
            assert_eq!(metrics.rough_reflection_size, Some([64, 48]));
            assert!(metrics.rough_reflection_bytes > 0);
        }
        ImmediatePreviewProfile::Ultra => assert_eq!(metrics.rough_reflection_size, None),
        _ => unreachable!("fixture compares Cinematic to full-resolution Ultra"),
    }
    rendered
}

fn evidence(label: &str, image: &image::RgbaImage) {
    let Some(root) = std::env::var_os("MOTIONLOOM_HYBRID_EVIDENCE_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).expect("create rough reflection evidence directory");
    image
        .save(root.join(format!("rough-reflection-{label}.png")))
        .expect("save rough reflection evidence PNG");
}

fn patch_mean(image: &image::RgbaImage, bounds: [u32; 4]) -> [f64; 3] {
    let [left, top, right, bottom] = bounds;
    let mut sum = [0_u64; 3];
    for y in top..bottom {
        for x in left..right {
            let pixel = image.get_pixel(x, y);
            for channel in 0..3 {
                sum[channel] += pixel[channel] as u64;
            }
        }
    }
    let count = ((right - left) * (bottom - top)) as f64;
    sum.map(|value| value as f64 / count)
}

fn patch_error(actual: &image::RgbaImage, reference: &image::RgbaImage, bounds: [u32; 4]) -> f64 {
    let [left, top, right, bottom] = bounds;
    let error: u64 = (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .map(|(x, y)| {
            (0..3)
                .map(|channel| {
                    actual.get_pixel(x, y)[channel].abs_diff(reference.get_pixel(x, y)[channel])
                        as u64
                })
                .sum::<u64>()
        })
        .sum();
    error as f64 / (((right - left) * (bottom - top) * 3) as f64)
}

#[test]
fn rough_reflection_fixtures_are_self_contained_and_parse() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    for source in broad_sources()
        .into_iter()
        .chain(coating_sources())
        .chain([
            disjoint_source(),
            thin_boundary_source(),
            high_incident_source(),
        ])
        .chain(
            coplanar_route_sources()
                .into_iter()
                .map(|(_, _, source)| source),
        )
    {
        parse_graph_script(&source).expect("rough reflection CPU fixture parse");
    }
}

#[test]
fn coplanar_route_fixture_materials_keep_the_opaque_depth_sequence() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    for (_, _, source) in coplanar_route_sources() {
        assert_coplanar_fixture_opaque_phase(&source);
    }
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_rough_route_preserves_fractional_and_normal_coplanar_order() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        for (kind, full_first, source) in coplanar_route_sources() {
            assert_coplanar_fixture_opaque_phase(&source);
            let actual = image(&source, ImmediatePreviewProfile::Cinematic).await;
            let reference = image(&source, ImmediatePreviewProfile::Ultra).await;
            let order = if full_first {
                "full-first"
            } else {
                "cached-first"
            };
            evidence(&format!("route-{kind}-{order}-cinematic"), &actual);
            evidence(&format!("route-{kind}-{order}-full-reference"), &reference);
            let patch = [88, 64, 168, 128];
            let expected = patch_mean(&reference, patch);
            let rendered = patch_mean(&actual, patch);
            let (winner, other) = if full_first { (2, 0) } else { (0, 2) };
            assert!(
                expected[winner] > expected[other] + 60.0,
                "coplanar reference does not resolve authored order: kind={kind}, order={order}, reference={expected:?}"
            );
            assert!(
                rendered[winner] > rendered[other] + 60.0,
                "routed opaque draws changed authored order: kind={kind}, order={order}, actual={rendered:?}, reference={expected:?}"
            );
            assert!(
                patch_error(&actual, &reference, patch) <= 4.0,
                "routed coverage differs from the original per-pixel path: kind={kind}, order={order}, error={}",
                patch_error(&actual, &reference, patch)
            );
        }
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_rough_cache_retraces_incident_radiance_above_half_range() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let source = high_incident_source();
        let actual = image_with_incident_strength(
            &source,
            ImmediatePreviewProfile::Cinematic,
            Some(200_000.0),
        )
        .await;
        let reference =
            image_with_incident_strength(&source, ImmediatePreviewProfile::Ultra, Some(200_000.0))
                .await;
        evidence("hdr-cinematic", &actual);
        evidence("hdr-full-reference", &reference);
        let patch = [72, 56, 184, 136];
        let full = patch_mean(&reference, patch);
        let coarse = patch_mean(&actual, patch);
        assert!(
            full[0] > 30.0 && full[0] < 240.0 && full[1] < 2.0 && full[2] < 2.0,
            "HDR reference must retain finite, unclipped reflected color: {full:?}"
        );
        assert!(
            patch_error(&actual, &reference, patch) <= 3.0,
            "half-range incident light failed full-resolution fallback: actual={coarse:?}, reference={full:?}, error={}",
            patch_error(&actual, &reference, patch)
        );
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_rough_cache_preserves_coating_without_a_geometry_base_lobe() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let [absent, present] = coating_sources();
        let absent = image(&absent, ImmediatePreviewProfile::Cinematic).await;
        let actual = image(&present, ImmediatePreviewProfile::Cinematic).await;
        let reference = image(&present, ImmediatePreviewProfile::Ultra).await;
        evidence("coat-without-emitter", &absent);
        evidence("coat-cinematic", &actual);
        evidence("coat-full-reference", &reference);
        let patch = [72, 56, 184, 136];
        let before = patch_mean(&absent, patch);
        let after = patch_mean(&actual, patch);
        let full = patch_mean(&reference, patch);
        assert!(
            full[0] > before[0] + 40.0 && full[0] > full[1] + 40.0,
            "coat-only reference must reflect the offscreen emitter: absent={before:?}, full={full:?}"
        );
        assert!(
            after[0] > before[0] + 40.0 && after[0] > after[1] + 40.0,
            "rough coating cache lost reflection energy: absent={before:?}, actual={after:?}"
        );
        assert!(
            patch_error(&actual, &reference, patch) <= 8.0,
            "coat-only rough reflection differs from full reference: error={}",
            patch_error(&actual, &reference, patch)
        );
        assert_eq!(actual.get_pixel(2, 2), absent.get_pixel(2, 2));
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_rough_cache_preserves_offscreen_reflection_energy() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let [absent, present] = broad_sources();
        let absent = image(&absent, ImmediatePreviewProfile::Cinematic).await;
        let actual = image(&present, ImmediatePreviewProfile::Cinematic).await;
        let reference = image(&present, ImmediatePreviewProfile::Ultra).await;
        evidence("broad-without-emitter", &absent);
        evidence("broad-cinematic", &actual);
        evidence("broad-full-reference", &reference);
        let patch = [72, 56, 184, 136];
        let before = patch_mean(&absent, patch);
        let after = patch_mean(&actual, patch);
        assert!(
            after[0] > before[0] + 40.0 && after[0] > after[1] + 40.0,
            "offscreen rough reflection energy missing: absent={before:?}, present={after:?}"
        );
        assert!(
            patch_error(&actual, &reference, patch) <= 12.0,
            "broad rough reflection differs from full reference: error={}",
            patch_error(&actual, &reference, patch)
        );
        assert_eq!(
            actual.get_pixel(2, 2),
            absent.get_pixel(2, 2),
            "offscreen emitter entered the main camera"
        );
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_rough_cache_keeps_disjoint_coplanar_object_colors() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let source = disjoint_source();
        let actual = image(&source, ImmediatePreviewProfile::Cinematic).await;
        let reference = image(&source, ImmediatePreviewProfile::Ultra).await;
        evidence("disjoint-cinematic", &actual);
        evidence("disjoint-full-reference", &reference);
        let left = patch_mean(&actual, [76, 64, 104, 128]);
        let right = patch_mean(&actual, [152, 64, 180, 128]);
        assert!(
            left[0] > left[2] + 60.0 && right[2] > right[0] + 60.0,
            "disjoint rough objects lost their reflection contrast: left={left:?}, right={right:?}"
        );
        // These two-pixel ribbons lie beside the approximately four-pixel
        // gap. At roughness 0.15 every full-resolution cone ray stays on its
        // own emitter half. A 3x3 coarse lookup still includes texels from
        // the opposite coplanar object, with matching normals and roughness.
        // Their identities must prevent those unrelated colors from blending.
        for (patch, primary_channel, other_channel) in
            [([124, 64, 126, 128], 0, 2), ([130, 64, 132, 128], 2, 0)]
        {
            let actual_color = patch_mean(&actual, patch);
            let reference_color = patch_mean(&reference, patch);
            assert!(
                reference_color[primary_channel] > reference_color[other_channel] + 60.0,
                "identity fixture cone crossed emitter halves: patch={patch:?}, reference={reference_color:?}"
            );
            assert!(
                actual_color[primary_channel] > actual_color[other_channel] + 60.0,
                "opposite object leaked reflection color: patch={patch:?}, actual={actual_color:?}"
            );
            assert!(
                patch_error(&actual, &reference, patch) <= 6.0,
                "cross-object boundary changed rough radiance: patch={patch:?}, error={}",
                patch_error(&actual, &reference, patch)
            );
        }
        for y in 48..144 {
            assert_eq!(
                actual.get_pixel(127, y),
                reference.get_pixel(127, y),
                "reflection cache filled the gap between disjoint objects at y={y}"
            );
        }
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_rough_cache_retains_thin_and_depth_normal_boundaries() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let source = thin_boundary_source();
        let actual = image(&source, ImmediatePreviewProfile::Cinematic).await;
        let reference = image(&source, ImmediatePreviewProfile::Ultra).await;
        evidence("thin-boundaries-cinematic", &actual);
        evidence("thin-boundaries-full-reference", &reference);
        // The isolated face projects to about 1.6 pixels. It need not survive
        // the coarse raster grid; failed evidence must trace the full ray.
        let thin_patch = [127, 120, 129, 160];
        let thin_color = patch_mean(&actual, thin_patch);
        assert!(
            thin_color[0] > thin_color[1] + 40.0,
            "isolated thin rough reflection disappeared: {thin_color:?}"
        );
        assert!(
            patch_error(&actual, &reference, thin_patch) <= 4.0,
            "thin object failed full-resolution fallback: error={}",
            patch_error(&actual, &reference, thin_patch)
        );
        // The nearer rib has a different world plane and a 60-degree normal
        // change. Test its narrow projected neighborhood and both adjoining
        // regions rather than tolerating an image-wide average.
        let rib_patch = [82, 34, 87, 66];
        assert!(
            patch_error(&actual, &reference, rib_patch) <= 8.0,
            "depth/normal boundary reused unrelated rough reflection: error={}",
            patch_error(&actual, &reference, rib_patch)
        );
    });
}
