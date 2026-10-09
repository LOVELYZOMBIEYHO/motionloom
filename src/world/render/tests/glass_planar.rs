//! Native image fixtures exercise camera-space refraction, pane accumulation,
//! and offscreen objects in a real planar capture rather than shader strings.
use super::super::*;

fn stage(materials: &str, geometry: &str, nodes: &str, camera: &str) -> String {
    format!(
        r##"<Graph fps={{24}} duration="1s" size={{[128,128]}}>
  <RenderStyle id="fixture"><SurfaceStyle shading="physical" /><LightingStyle ambientIntensity="0" /></RenderStyle>
  <Assets>{materials}{geometry}</Assets>
  <Background color="#000000" />
  <Scene id="stage" renderStyle="fixture"><Timeline><Track id="t"><Sequence from="0s" duration="1s">
    <CompositeGroup id="room" space="3d" depth="true" format="rgba16f">
      <Camera3D position={{{camera}}} target={{[0,0,0]}} fov="35" />
      <DirectionalLight direction={{[0,0,-1]}} intensity="0" castShadow="false" />
      {nodes}
    </CompositeGroup>
  </Sequence></Track></Timeline></Scene><Present from="stage" />
</Graph>"##
    )
}

async fn image(renderer: &mut crate::SceneRenderer, source: &str) -> image::RgbaImage {
    let graph = crate::parse_graph_script(source).expect("glass/planar fixture parse");
    renderer
        .render_frame_gpu_readback(&graph, 0)
        .await
        .expect("glass/planar GPU fixture")
}

const GLASS: &str = r##"<MaterialAsset id="glass" baseColor="#FFFFFF" metallic="0" roughness="0.045" specular="1" transmission="1" ior="1.52" thickness="0.3" attenuationColor="#80CCFF" attenuationDistance="1" doubleSided="true" />
<MaterialAsset id="red" baseColor="#000000" emissive="#FF0000" />
<MaterialAsset id="blue" baseColor="#000000" emissive="#0000FF" />
<MaterialAsset id="white" baseColor="#000000" emissive="#FFFFFF" />"##;
const PANES: &str = r##"<GeometryAsset id="pane_geo"><Primitive shape="box" size={[1.8,1.8,0.3]} /></GeometryAsset><MeshAsset id="pane" material="glass" geometry="pane_geo" />
<GeometryAsset id="pattern_geo"><Primitive shape="box" size={[1.4,4,0.05]} /></GeometryAsset><MeshAsset id="left" material="red" geometry="pattern_geo" /><MeshAsset id="right" material="blue" geometry="pattern_geo" />
<GeometryAsset id="back_geo"><Primitive shape="box" size={[5,5,0.05]} /></GeometryAsset><MeshAsset id="back" material="white" geometry="back_geo" />"##;

#[test]
#[ignore = "requires a native GPU adapter"]
fn camera_hidden_environment_is_visible_through_glass() {
    let path =
        std::env::temp_dir().join(format!("motionloom-glass-sky-{}.png", std::process::id()));
    image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 255, 255, 255]))
        .save(&path)
        .unwrap();
    let materials = format!(
        "{GLASS}<ImageAsset id=\"sky\" src=\"{}\" />",
        path.display()
    );
    let source = stage(
        &materials,
        PANES,
        r#"<EnvironmentLight asset="sky" visible="false" intensity="1" diffuseIntensity="0" specularIntensity="0" />
<Model asset="pane" castShadow="false" />"#,
        "[0,0,4]",
    );
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
            .await
            .unwrap();
        let result = image(&mut renderer, &source).await;
        let center = result.get_pixel(64, 64).0;
        let background = result.get_pixel(2, 2).0;
        assert!(
            center[0] > 150 && center[2] > 190,
            "hidden sky missing from glass: {center:?}"
        );
        assert!(
            background[..3].iter().all(|&v| v < 5),
            "camera background unexpectedly revealed: {background:?}"
        );
    });
    std::fs::remove_file(path).unwrap();
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn glass_refraction_is_invariant_under_world_camera_rotation() {
    let original = stage(
        GLASS,
        PANES,
        r#"<Model id="glass" asset="pane" rotation={[0,30,0]} castShadow="false" />
<Model asset="left" position={[-0.7,0,-0.8]} /><Model asset="right" position={[0.7,0,-0.8]} />"#,
        "[0,0,4]",
    );
    let rotated = stage(
        GLASS,
        PANES,
        r#"<Model id="glass" asset="pane" rotation={[0,120,0]} castShadow="false" />
<Model asset="left" position={[-0.8,0,0.7]} rotation={[0,90,0]} /><Model asset="right" position={[-0.8,0,-0.7]} rotation={[0,90,0]} />"#,
        "[4,0,0]",
    );
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
            .await
            .unwrap();
        let a = image(&mut renderer, &original).await;
        let b = image(&mut renderer, &rotated).await;
        let mut total = 0u64;
        for y in 40..88 {
            for x in 40..88 {
                for c in 0..3 {
                    total += (a.get_pixel(x, y)[c] as i32 - b.get_pixel(x, y)[c] as i32)
                        .unsigned_abs() as u64;
                }
            }
        }
        let mean = total as f64 / (48.0 * 48.0 * 3.0);
        assert!(
            mean < 2.0,
            "camera/world rotation changed refraction, mean byte error {mean}"
        );
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn separate_closed_glass_panes_accumulate_once_per_pane() {
    let one = stage(
        GLASS,
        PANES,
        r#"<Model id="far" asset="pane" position={[0,0,0]} castShadow="false" /><Model asset="back" position={[0,0,-1]} />"#,
        "[0,0,4]",
    );
    let two=one.replace(r#"<Model asset="back""#,r#"<Model id="near" asset="pane" position={[0,0,0.8]} castShadow="false" /><Model asset="back""#);
    let reverse=two.replace(r#"<Model id="far" asset="pane" position={[0,0,0]} castShadow="false" /><Model id="near" asset="pane" position={[0,0,0.8]} castShadow="false" />"#,r#"<Model id="near" asset="pane" position={[0,0,0.8]} castShadow="false" /><Model id="far" asset="pane" position={[0,0,0]} castShadow="false" />"#);
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
            .await
            .unwrap();
        let a = image(&mut renderer, &one).await.get_pixel(64, 64).0;
        let b = image(&mut renderer, &two).await.get_pixel(64, 64).0;
        let c = image(&mut renderer, &reverse).await.get_pixel(64, 64).0;
        assert!(
            b[0] + 8 < a[0],
            "second pane failed to accumulate absorption: one={a:?}, two={b:?}"
        );
        assert!(a[0] > 190, "one closed pane absorbed twice: {a:?}");
        for channel in 0..3 {
            assert!(
                (b[channel] as i32 - c[channel] as i32).abs() <= 1,
                "authoring order changed pane overlap: {b:?} vs {c:?}"
            );
        }
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn planar_mirror_reflects_an_object_behind_the_main_camera() {
    let materials = r##"<MaterialAsset id="mirror" baseColor="#FFFFFF" metallic="1" roughness="0.045" transmission="0" />
<MaterialAsset id="red" baseColor="#000000" emissive="#FF0000" />"##;
    let geometry = r#"<GeometryAsset id="mirror_geo"><Primitive shape="box" size={[3,3,0.012]} /></GeometryAsset><MeshAsset id="mirror_mesh" material="mirror" geometry="mirror_geo" />
<GeometryAsset id="ball_geo"><Primitive shape="sphere" radius="0.75" segments="24" rings="16" /></GeometryAsset><MeshAsset id="ball" material="red" geometry="ball_geo" />"#;
    let source = stage(
        materials,
        geometry,
        r#"<PlanarReflection id="mirror_capture" target="mirror" resolutionScale="1" clipBias="0.001" />
<Model id="mirror" asset="mirror_mesh" castShadow="false" /><Model id="offscreen_red" asset="ball" position={[0,0,5]} castShadow="false" />"#,
        "[0,0,4]",
    );
    let no_object = source.replace(
        r#"<Model id="offscreen_red" asset="ball" position={[0,0,5]} castShadow="false" />"#,
        "",
    );
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
            .await
            .unwrap();
        let a = image(&mut renderer, &source).await.get_pixel(64, 64).0;
        let b = image(&mut renderer, &no_object).await.get_pixel(64, 64).0;
        assert!(
            a[0] > b[0] + 30 && a[0] > a[1] + 50,
            "offscreen object missing from mirror: reflected={a:?}, empty={b:?}"
        );
    });
}

fn planar_solid_after_slab_budget_sources() -> [String; 2] {
    // Disable geometry reflection on the primary mirror, so only its planar
    // capture can supply the reflected solid and mask no capture regression.
    let materials = r##"<MaterialAsset id="mirror" baseColor="#FFFFFF" metallic="1" roughness="0.8" />
<MaterialAsset id="slab" baseColor="#FFFFFF" roughness="0.045" transmission="1" ior="1" doubleSided="true" />
<MaterialAsset id="solid" baseColor="#FFFFFF" roughness="0.045" transmission="1" ior="1" attenuationColor="#08FFFF" attenuationDistance="1" refractionMode="solid" doubleSided="true" />
<MaterialAsset id="white" baseColor="#000000" emissive="#FFFFFF" />"##;
    let geometry = r#"<GeometryAsset id="mirror_geo"><Primitive shape="box" size={[3,3,0.012]} /></GeometryAsset><MeshAsset id="mirror_mesh" material="mirror" geometry="mirror_geo" />
<GeometryAsset id="pane_geo"><Primitive shape="box" size={[1,1,0.01]} /></GeometryAsset><MeshAsset id="pane" material="slab" geometry="pane_geo" />
<GeometryAsset id="solid_geo"><Primitive shape="sphere" radius="0.75" segments="24" rings="16" /></GeometryAsset><MeshAsset id="solid_mesh" material="solid" geometry="solid_geo" />
<GeometryAsset id="white_geo"><Primitive shape="box" size={[8,8,0.05]} /></GeometryAsset><MeshAsset id="back" material="white" geometry="white_geo" />"#;
    let reference = stage(
        materials,
        geometry,
        r#"<PlanarReflection id="capture" target="mirror" resolutionScale="1" clipBias="0.001" />
<Model id="mirror" asset="mirror_mesh" castShadow="false" />
<Model id="reflected_solid" asset="solid_mesh" position={[0,0,6]} castShadow="false" />
<Model id="reflected_back" asset="back" position={[0,0,9]} castShadow="false" />"#,
        "[0,0,4]",
    );
    // These offscreen slabs still consume the Portable capture's sixteen
    // snapshots before the nearer solid in reflected-camera draw order.
    // They cannot alter any ray through the central reflected sphere.
    let panes = (0..16)
        .map(|index| {
            format!(
                "<Model id=\"offscreen_slab_{index}\" asset=\"pane\" position={{[30,0,{}]}} castShadow=\"false\" />",
                20 + index
            )
        })
        .collect::<String>();
    let exhausted = reference.replace("</CompositeGroup>", &format!("{panes}</CompositeGroup>"));
    [reference, exhausted]
}

#[test]
fn planar_solid_after_slab_budget_fixtures_parse() {
    for source in planar_solid_after_slab_budget_sources() {
        crate::parse_graph_script(&source).expect("planar solid budget fixture parse");
    }
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn planar_solid_transport_survives_exhausted_slab_snapshot_budget() {
    // Exact solid exits beyond the slab budget belong to reference transport;
    // the default preview intentionally uses bounded snapshot approximations.
    let _reference = transport_policy::test_reference_transport(true);
    let [reference, exhausted] = planar_solid_after_slab_budget_sources();
    pollster::block_on(async {
        let mut renderer = crate::SceneRenderer::new(crate::SceneRenderProfile::Gpu)
            .await
            .unwrap();
        renderer.set_immediate_preview_settings(crate::preview::ImmediatePreviewSettings {
            profile: crate::preview::ImmediatePreviewProfile::Portable,
            target_fps: 30.0,
            dynamic_resolution: false,
            min_resolution_scale: 1.0,
        });
        let a = image(&mut renderer, &reference).await;
        let b = image(&mut renderer, &exhausted).await;
        for (label, result) in [("planar-solid-reference", &a), ("planar-solid-after-slab-budget", &b)] {
            if let Some(root) = std::env::var_os("MOTIONLOOM_HYBRID_EVIDENCE_DIR") {
                let root = std::path::PathBuf::from(root);
                std::fs::create_dir_all(&root).expect("create planar evidence directory");
                result.save(root.join(format!("{label}.png"))).expect("save planar evidence PNG");
            }
        }
        let center = a.get_pixel(64, 64).0;
        assert!(center[2] > 100 && center[0].saturating_add(60) < center[2],
            "fixture did not show solid absorption in the mirror: {center:?}");
        let mut maximum_error = 0u8;
        for y in 58..70 {
            for x in 58..70 {
                for channel in 0..3 {
                    maximum_error = maximum_error.max(a.get_pixel(x, y)[channel].abs_diff(b.get_pixel(x, y)[channel]));
                }
            }
        }
        assert!(maximum_error <= 2,
            "slab snapshot exhaustion changed reflected solid optics: maximum byte error {maximum_error}");
    });
}

#[test]
fn glass_topology_recognizes_closed_boxes_with_split_face_vertices() {
    let positions = [
        [-1., -1., -1.],
        [1., -1., -1.],
        [1., 1., -1.],
        [-1., 1., -1.],
        [-1., -1., 1.],
        [1., -1., 1.],
        [1., 1., 1.],
        [-1., 1., 1.],
    ];
    let faces = [
        [0, 3, 2, 1],
        [4, 5, 6, 7],
        [0, 1, 5, 4],
        [3, 7, 6, 2],
        [0, 4, 7, 3],
        [1, 2, 6, 5],
    ];
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for face in faces {
        let start = vertices.len() as u32;
        for i in face {
            let mut v = super::test_gpu_vertex(0.);
            v.position = positions[i];
            vertices.push(v);
        }
        indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
    }
    assert!(planar::closed_mesh(&vertices, &indices));
    assert!(
        !planar::closed_mesh(&vertices, &indices[..indices.len() - 6]),
        "open surfaces must retain double-sided lighting"
    );
}
