//! End-to-end contracts for the default native/WebGPU raster policy.
use super::super::*;

fn fixture(shading: &str, glass: bool) -> String {
    let optics = if glass {
        r#"transmission="0.9" ior="1.5" thickness="0.1" refractionMode="solid""#
    } else {
        r#"clearcoat="0.8" clearcoatRoughness="0.12""#
    };
    format!(
        r##"<Graph fps={{24}} duration="1s" size={{[64,64]}}>
<RenderStyle id="style"><SurfaceStyle shading="{shading}" /><LightingStyle reflectionBounces="2" />
<AntiAliasingStyle method="fxaa" quality="high" fallback="off" /></RenderStyle>
<Assets><MaterialAsset id="paint" baseColor="#CC3030" roughness="0.2" {optics} />
<GeometryAsset id="g"><Primitive shape="sphere" radius="0.8" segments="16" rings="12" /></GeometryAsset>
<MeshAsset id="ball" material="paint" geometry="g" /></Assets>
<Background color="#8899AA" /><Scene id="physical_realtime_stage" renderStyle="style"><Timeline><Track id="t">
<Sequence from="0s" duration="1s"><CompositeGroup id="physical_realtime_room" space="3d" depth="true" format="rgba16f">
<Camera3D position={{[0,0,4]}} target={{[0,0,0]}} fov="40" />
<DirectionalLight direction={{[-0.4,-0.6,-1]}} color="#FFFFFF" intensity="3" />
<Model asset="ball" /></CompositeGroup></Sequence></Track></Timeline></Scene><Present from="physical_realtime_stage" /></Graph>"##
    )
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn physical_preview_keeps_pbr_without_scene_transport() {
    let _policy = transport_policy::test_reference_transport(false);
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
            .await
            .unwrap();
        renderer.set_immediate_preview_settings(crate::ImmediatePreviewSettings {
            profile: crate::ImmediatePreviewProfile::Cinematic,
            dynamic_resolution: false,
            min_resolution_scale: 1.0,
            ..Default::default()
        });
        let graph = crate::parse_graph_script(&fixture("physical", false)).unwrap();
        let physical = renderer.render_frame_gpu_readback(&graph, 0).await.unwrap();
        let profile = renderer.last_3d_frame_profile();
        assert!(!profile.geometry_transport_enabled);
        assert_eq!(
            (
                profile.hybrid_scene_bytes,
                profile.hybrid_triangles,
                profile.hybrid_nodes
            ),
            (0, 0, 0)
        );
        assert_eq!(profile.rough_reflection_size, None);
        assert!(profile.screen_space_reflections);
        assert!(profile.recursive_reflection_approximated);
        assert_eq!(profile.anti_aliasing_effective, "fxaa");
        let toon = crate::parse_graph_script(&fixture("toon", false)).unwrap();
        let toon = renderer.render_frame_gpu_readback(&toon, 0).await.unwrap();
        assert_ne!(
            physical.as_raw(),
            toon.as_raw(),
            "raster policy must not replace PBR with toon"
        );
        let glass = crate::parse_graph_script(&fixture("physical", true)).unwrap();
        let glass = renderer.render_frame_gpu_readback(&glass, 0).await.unwrap();
        assert_ne!(
            physical.as_raw(),
            glass.as_raw(),
            "authored transmission remains visible"
        );
        let profile = renderer.last_3d_frame_profile();
        assert!(profile.solid_refraction_approximated);
        assert_eq!(profile.hybrid_scene_bytes, 0);
        assert!(
            profile.transmission_layers > 0,
            "solid approximation must obtain an opaque snapshot"
        );
    });
}

#[test]
fn reference_override_is_scoped() {
    let _outer = transport_policy::test_reference_transport(false);
    assert!(!transport_policy::reference_geometry_transport_enabled());
    {
        let _reference = transport_policy::test_reference_transport(true);
        assert!(transport_policy::reference_geometry_transport_enabled());
    }
    assert!(!transport_policy::reference_geometry_transport_enabled());
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn complex_raster_visibility_preserves_alpha_glass_and_shadow_refresh() {
    let _policy = transport_policy::test_reference_transport(false);
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu).await.unwrap();
        renderer.set_immediate_preview_settings(crate::ImmediatePreviewSettings {
            profile: crate::ImmediatePreviewProfile::Balanced,
            dynamic_resolution: false, min_resolution_scale: 1.0, ..Default::default()
        });
        let padding = (0..256).map(|i| format!(
            "<Model id=\"off{i}\" asset=\"ball\" position={{[{},0,0]}} castShadow=\"false\" />", 100+i
        )).collect::<String>();
        for glass in [false, true] {
            let source = fixture("physical", glass)
                .replace("<LightingStyle reflectionBounces=\"2\" />", "<LightingStyle reflectionBounces=\"2\" shadowMode=\"perLight\" />")
                .replace("</Assets>", "<MaterialAsset id=\"alpha\" baseColor=\"#40CC8099\" alphaMode=\"blend\" /><MeshAsset id=\"alpha_ball\" material=\"alpha\" geometry=\"g\" /></Assets>")
                .replace("<Model asset=\"ball\" />", &format!(
                    "<Model id=\"front\" asset=\"alpha_ball\" /><Model id=\"behind\" asset=\"ball\" position={{[0,0,-1]}} />{padding}"));
            let graph = crate::parse_graph_script(&source).unwrap();
            let original = {
                let _disabled = visibility::test_prepass(false);
                renderer.render_frame_gpu_readback(&graph, 0).await.unwrap()
            };
            let filtered = renderer.render_frame_gpu_readback(&graph, 0).await.unwrap();
            let warm = renderer.render_frame_gpu_readback(&graph, 0).await.unwrap();
            assert_eq!(original.as_raw(), filtered.as_raw(), "current-frame visibility changed alpha/glass pixels");
            assert_eq!(filtered.as_raw(), warm.as_raw(), "early resource rejection changed the warm frame");
            let profile = renderer.last_3d_frame_profile();
            assert!(profile.raster_visibility_prepass);
            assert!(profile.frustum_rejected_items >= 256);
            assert_eq!(profile.shadow_rendered_views, 0);
            assert_eq!(profile.shadow_cache_hits, profile.shadow_view_count);
            let changed = crate::parse_graph_script(&source.replace("intensity=\"3\"", "intensity=\"2\"")
                .replace("[-0.4,-0.6,-1]", "[-0.6,-0.4,-1]")).unwrap();
            renderer.render_frame_gpu_readback(&changed, 0).await.unwrap();
            let profile = renderer.last_3d_frame_profile();
            assert!(profile.shadow_rendered_views > 0, "changed light reused stale off-camera shadows");
        }
    });
}
