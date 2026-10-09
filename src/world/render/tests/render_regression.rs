// =========================================
// =========================================
// src/world/render/tests/render_regression.rs

//! Locks authored shading names to the numeric selector consumed by WGSL.

#[test]
fn every_render_style_keeps_its_shader_selector() {
    let expected = [
        ("physical", 0.0),
        ("stylized", 1.0),
        ("toon", 2.0),
        ("clay", 3.0),
        ("cel", 4.0),
    ];
    for (name, selector) in expected {
        let lighting = crate::world::WorldLighting {
            render_style: Some(crate::render_style::ResolvedSceneRenderStyle {
                anti_aliasing: None,
                depth_of_field: None,
                universal: Default::default(),
                cel: Default::default(),
                scene_id: "test".into(),
                style_id: None,
                shading: name.into(),
                shading_steps: 3,
                diffuse_wrap: 0.0,
                rim_light: 0.0,
                rim_power: 3.0,
                specular: 1.0,
                roughness_bias: 0.0,
                surface_saturation: 1.0,
                ambient_intensity: 1.0,
                ambient_color: [1.0; 3],
                hard_shadows: false,
        per_light_shadows: false,
                reflection_bounces: 1,
                lighting_preset: None,
                post: Default::default(),
                overrides: Vec::new(),
            }),
            ..Default::default()
        };
        let params = super::super::GpuWorldLightingParams::from_world(
            &lighting,
            super::super::PerspectiveCameraView {
                orthographic: false,
                eye: [0.0; 3],
                right: [1.0, 0.0, 0.0],
                up: [0.0, 1.0, 0.0],
                forward: [0.0, 0.0, -1.0],
                focal_px: 1.0,
                near: 0.01,
                far: 100.0,
                aspect: 1.0,
                optics: [0.0; 4],
            },
            false,
            1,
        );
        assert_eq!(params.surface0[0], selector, "selector for {name}");
    }
}

// A low-IOR dielectric must keep its BRDF highlight on a black background.
// This requires a native adapter and is run explicitly with --ignored.
#[test]
#[ignore = "requires a native GPU adapter"]
fn transparent_water_retains_bright_reflection_without_diffuse_fill() {
    let script = r##"<Graph fps={24} duration="1s" size={[128,128]}>
  <RenderStyle id="water-test">
    <SurfaceStyle shading="physical" />
    <LightingStyle ambientIntensity="0" />
  </RenderStyle>
  <Assets>
    <MaterialAsset id="water" baseColor="#FFFFFF" metallic="0" roughness="0.15" specular="1" transmission="1" ior="1.333" thickness="0.06" attenuationColor="#FFFFFF" attenuationDistance="100" doubleSided="false" depthWrite="true" />
    <GeometryAsset id="drop_geometry">
    <Primitive shape="sphere" radius="0.6" segments="48" rings="32" />
    </GeometryAsset>
    <MeshAsset id="drop" material="water" geometry="drop_geometry" />
  </Assets>
  <Background color="#000000" />
  <Scene id="WaterReflectionStage" renderStyle="water-test">
    <Timeline>
      <Track id="water-test-track">
        <Sequence from="0s" duration="1s">
    <CompositeGroup id="studio" space="3d" depth="true" format="rgba16f">
      <Camera3D position={[0,0,4]} target={[0,0,0]} fov="30" />
      <DirectionalLight direction={[0,0,-1]} color="#FFFFFF" intensity="20" />
      <Model asset="drop" castShadow="false" />
    </CompositeGroup>
        </Sequence>
      </Track>
    </Timeline>
  </Scene>
  <Present from="WaterReflectionStage" />
</Graph>
"##;
    let graph = crate::parse_graph_script(script).expect("water reflection graph");
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
            .await
            .expect("native GPU renderer");
        let lit = renderer
            .render_frame_gpu_readback(&graph, 0)
            .await
            .expect("lit water frame");
        let center = lit.get_pixel(64, 64).0;
        assert!(
            center[..3].iter().all(|channel| *channel > 200),
            "water highlight was suppressed: {center:?}"
        );
        assert_eq!(lit.get_pixel(0, 0).0, [0, 0, 0, 255]);

        // With no light incident on the center, clear water must not add a
        // white Lambert coating to the black scene sampled behind it.
        let unlit_graph =
            crate::parse_graph_script(&script.replace("intensity=\"20\"", "intensity=\"0\""))
                .expect("unlit water graph");
        let unlit = renderer
            .render_frame_gpu_readback(&unlit_graph, 0)
            .await
            .expect("unlit water frame");
        let dark_center = unlit.get_pixel(64, 64).0;
        assert!(
            dark_center[..3].iter().all(|channel| *channel < 16),
            "clear water added diffuse fill: {dark_center:?}"
        );
    });
}
