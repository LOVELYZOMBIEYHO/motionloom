//! Glass screen reflection uses its own interface and a bounded underlay ray.
use super::super::*;

fn fixture(glass: bool, panel_z: f32) -> String {
    let pane = if glass {
        r#"<Model id="glass_surface" asset="pane_mesh" rotation={[0,45,0]} castShadow="false" />"#
    } else {
        ""
    };
    format!(
        r##"<Graph fps="24" duration="1s" size={{[128,128]}}>
<RenderStyle id="glass_screen_style"><SurfaceStyle shading="physical" />
<LightingStyle ambientIntensity="0" /><AntiAliasingStyle method="off" />
<PostStyle toneMapping="none" exposure="1" /></RenderStyle>
<Assets>
<MaterialAsset id="glass_material" baseColor="#FFFFFF" metallic="0" roughness="0.06"
 transmission="1" ior="1.52" thickness="0.03" specular="1" clearcoat="0.4"
 clearcoatRoughness="0.06" refractionMode="slab" doubleSided="true" />
<!-- Zero diffuse response prevents profile GI differences in this fixture. -->
<MaterialAsset id="dark_material" baseColor="#000000" metallic="1" roughness="1" specular="0" />
<MaterialAsset id="red_material" baseColor="#000000" metallic="1" roughness="1"
 emissive="#FF0000" emissiveStrength="8" />
<GeometryAsset id="pane_geometry"><Primitive shape="box" size={{[1.8,1.8,0.03]}} /></GeometryAsset>
<GeometryAsset id="panel_geometry"><Primitive shape="box" size={{[1.2,1.8,0.04]}} /></GeometryAsset>
<GeometryAsset id="back_geometry"><Primitive shape="box" size={{[5,5,0.03]}} /></GeometryAsset>
<MeshAsset id="pane_mesh" material="glass_material" geometry="pane_geometry" />
<MeshAsset id="panel_mesh" material="red_material" geometry="panel_geometry" />
<MeshAsset id="back_mesh" material="dark_material" geometry="back_geometry" />
</Assets><Background color="#000000" />
<Scene id="glass_screen_stage" renderStyle="glass_screen_style"><Timeline><Track id="glass_screen_track">
<Sequence duration="1s"><CompositeGroup id="glass_screen_room" space="3d" depth="true" format="rgba16f">
<Camera3D position={{[0,0,4]}} target={{[0,0,0]}} fov="55" />
<DirectionalLight direction={{[0,0,-1]}} intensity="0" castShadow="false" />
<Model id="dark_back" asset="back_mesh" position={{[0,0,-0.9]}} castShadow="false" />
<Model id="visible_red" asset="panel_mesh" position={{[1.2,0,{panel_z}]}} rotation={{[0,-45,0]}} castShadow="false" />
{pane}</CompositeGroup></Sequence></Track></Timeline></Scene><Present from="glass_screen_stage" /></Graph>"##
    )
}

async fn frame(
    source: &str,
    profile: crate::ImmediatePreviewProfile,
) -> (image::RgbaImage, Scene3DFrameProfile) {
    let graph = crate::parse_graph_script(source).expect("glass screen fixture parses");
    let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
        .await
        .unwrap();
    renderer.set_immediate_preview_settings(crate::ImmediatePreviewSettings {
        profile,
        dynamic_resolution: false,
        min_resolution_scale: 1.0,
        ..Default::default()
    });
    let image = renderer
        .render_frame_gpu_readback(&graph, 0)
        .await
        .expect("glass screen GPU frame");
    (image, renderer.last_3d_frame_profile())
}

fn no_geometry(profile: Scene3DFrameProfile) {
    assert!(!profile.geometry_transport_enabled);
    assert_eq!(
        (
            profile.hybrid_scene_bytes,
            profile.hybrid_triangles,
            profile.hybrid_nodes
        ),
        (0, 0, 0)
    );
    assert_eq!(profile.planar_capture_draw_calls, 0);
}

fn evidence(label: &str, image: &image::RgbaImage) {
    let Some(root) = std::env::var_os("MOTIONLOOM_HYBRID_EVIDENCE_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).unwrap();
    image.save(root.join(format!("{label}.png"))).unwrap();
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn glass_realtime_reflects_visible_emitter_only_with_profile_ssr() {
    let _policy = transport_policy::test_reference_transport(false);
    pollster::block_on(async {
        let source = fixture(true, 0.0);
        let (off, off_profile) = frame(&source, crate::ImmediatePreviewProfile::Balanced).await;
        let (on, on_profile) = frame(&source, crate::ImmediatePreviewProfile::Cinematic).await;
        no_geometry(off_profile);
        no_geometry(on_profile);
        assert!(!off_profile.screen_space_reflections);
        assert!(on_profile.screen_space_reflections);
        assert!(on_profile.transmission_layers > 0);
        evidence("realtime-glass-ssr-off", &off);
        evidence("realtime-glass-ssr-on", &on);
        let reflected_pixels = (42..86)
            .flat_map(|y| (46..78).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let a = off.get_pixel(x, y).0;
                let b = on.get_pixel(x, y).0;
                b[0] > a[0].saturating_add(12) && b[0] > b[1].saturating_add(20)
            })
            .count();
        assert!(
            reflected_pixels > 40,
            "visible emitter missing from glass SSR: {reflected_pixels} pixels"
        );

        // Same profile pair without glass isolates this from diffuse GI, AA,
        // shadow resolution and the opaque post reflection implementation.
        let control = fixture(false, 0.0);
        let (off_control, _) = frame(&control, crate::ImmediatePreviewProfile::Balanced).await;
        let (on_control, _) = frame(&control, crate::ImmediatePreviewProfile::Cinematic).await;
        assert_eq!(
            off_control.as_raw(),
            on_control.as_raw(),
            "profile control changed without glass"
        );
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn glass_realtime_offscreen_miss_retains_environment_without_false_hit() {
    let _policy = transport_policy::test_reference_transport(false);
    pollster::block_on(async {
        let source = fixture(true, 6.0);
        let (off, off_profile) = frame(&source, crate::ImmediatePreviewProfile::Balanced).await;
        let (on, on_profile) = frame(&source, crate::ImmediatePreviewProfile::Cinematic).await;
        no_geometry(off_profile);
        no_geometry(on_profile);
        // The pane's central front face reflects toward +X and has no visible
        // target here. Its thin side faces can legitimately reflect the dark
        // background, so compare only the known-miss interface region.
        for y in 44..84 {
            for x in 50..76 {
                assert_eq!(
                    off.get_pixel(x, y),
                    on.get_pixel(x, y),
                    "offscreen miss must retain glass fallback at ({x},{y})"
                );
            }
        }
    });
}
