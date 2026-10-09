//! Native image contracts for geometry-backed reflection and solid transmission.
//! Every source is self-contained and uses the public Scene renderer.
use crate::api::{SceneRenderProfile, SceneRenderer, parse_graph_script};
use crate::preview::{ImmediatePreviewProfile, ImmediatePreviewSettings};

fn stage(materials: &str, geometry: &str, models: &str, bounces: u32) -> String {
    format!(
        r##"<Graph fps="24" duration="1s" size={{[128,128]}}>
<RenderStyle id="style"><SurfaceStyle shading="physical" />
<LightingStyle ambientIntensity="0" reflectionBounces="{bounces}" />
<PostStyle toneMapping="none" exposure="1" />
<AntiAliasingStyle method="off" /></RenderStyle>
<Assets>{materials}{geometry}</Assets>
<Background color="#000000" />
<Scene id="stage" renderStyle="style"><Timeline><Track id="track" space="3d">
<Sequence duration="1s"><CompositeGroup id="room" space="3d" depth="true" format="rgba16f">
<Camera3D position={{[0,0,4]}} target={{[0,0,0]}} fov="35" />
<DirectionalLight direction={{[0,0,-1]}} intensity="0" castShadow="false" />
{models}</CompositeGroup></Sequence></Track></Timeline></Scene>
<Present from="stage" /></Graph>"##
    )
}

async fn image(source: &str) -> image::RgbaImage {
    let graph = parse_graph_script(source).expect("hybrid glass fixture parse");
    let mut renderer = SceneRenderer::new(SceneRenderProfile::Gpu)
        .await
        .expect("native GPU renderer");
    renderer.set_immediate_preview_settings(ImmediatePreviewSettings {
        profile: ImmediatePreviewProfile::Cinematic,
        target_fps: 30.0,
        dynamic_resolution: false,
        min_resolution_scale: 1.0,
    });
    renderer
        .render_frame_gpu_readback(&graph, 0)
        .await
        .expect("hybrid glass GPU frame")
}

fn evidence(label: &str, image: &image::RgbaImage) {
    let Some(root) = std::env::var_os("MOTIONLOOM_HYBRID_EVIDENCE_DIR") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    std::fs::create_dir_all(&root).expect("create hybrid evidence directory");
    image
        .save(root.join(format!("{label}.png")))
        .expect("save hybrid evidence PNG");
}

const SPHERE: &str = r#"<GeometryAsset id="sphere_g"><Primitive shape="sphere" radius="0.85" segments="48" rings="32" /></GeometryAsset>
<MeshAsset id="sphere" material="surface" geometry="sphere_g" />
<GeometryAsset id="red_g"><Primitive shape="box" size={[2,2,0.15]} /></GeometryAsset>
<MeshAsset id="red" material="red" geometry="red_g" />"#;
const RED: &str =
    r##"<MaterialAsset id="red" baseColor="#000000" emissive="#FF0000" emissiveStrength="8" />"##;
const OFFSCREEN_RED: &str =
    r#"<Model id="offscreen" asset="red" position={[0,0,5]} castShadow="false" />"#;
const OFFSCREEN_SURFACES: [&str; 2] = [
    r##"baseColor="#050505" metallic="0" roughness="0.7" specular="0" clearcoat="1" clearcoatRoughness="0.04""##,
    r##"baseColor="#FFFFFF" metallic="0" roughness="0.04" specular="1" transmission="1" ior="1.52" refractionMode="solid""##,
];

fn offscreen_fixture_sources(attributes: &str) -> [String; 2] {
    let materials = format!("<MaterialAsset id=\"surface\" {attributes} />{RED}");
    let absent = stage(
        &materials,
        SPHERE,
        r#"<Model asset="sphere" castShadow="false" />"#,
        1,
    );
    let present = absent.replace(
        "</CompositeGroup>",
        &format!("{OFFSCREEN_RED}</CompositeGroup>"),
    );
    [absent, present]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_coat_and_glass_reflect_objects_behind_the_main_camera() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        for attributes in OFFSCREEN_SURFACES {
            let [absent, present] = offscreen_fixture_sources(attributes);
            let a = image(&absent).await;
            let b = image(&present).await;
            let kind = if attributes.contains("clearcoat") {
                "coat"
            } else {
                "glass"
            };
            evidence(&format!("offscreen-{kind}-without"), &a);
            evidence(&format!("offscreen-{kind}-with"), &b);
            let reflected_pixels = (36..92)
                .flat_map(|y| (36..92).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let old = a.get_pixel(x, y).0;
                    let new = b.get_pixel(x, y).0;
                    new[0] > old[0].saturating_add(12) && new[0] > new[1].saturating_add(20)
                })
                .count();
            assert!(
                reflected_pixels > 20,
                "offscreen red missing for {attributes}: {reflected_pixels} pixels"
            );
            assert_eq!(
                a.get_pixel(2, 2),
                b.get_pixel(2, 2),
                "fixture object entered the main camera"
            );
        }
    });
}

fn checker_geometry() -> (String, String) {
    let mut geometry = String::from(
        r#"<GeometryAsset id="tile_g"><Primitive shape="box" size={[0.5,0.5,0.05]} /></GeometryAsset>
<MeshAsset id="black" material="black" geometry="tile_g" />
<MeshAsset id="white" material="white" geometry="tile_g" />"#,
    );
    geometry.push_str(SPHERE);
    let mut models = String::from(r#"<Model id="glass" asset="sphere" castShadow="false" />"#);
    for y in 0..8 {
        for x in 0..8 {
            let material = if (x + y) % 2 == 0 { "black" } else { "white" };
            models.push_str(&format!(
                "<Model asset=\"{material}\" position={{[{},{},-1.8]}} castShadow=\"false\" />",
                x as f32 * 0.5 - 1.75,
                y as f32 * 0.5 - 1.75
            ));
        }
    }
    (geometry, models)
}

const CHECKER: &str = r##"<MaterialAsset id="black" baseColor="#000000" emissive="#050505" />
<MaterialAsset id="white" baseColor="#000000" emissive="#FFFFFF" />"##;

fn dense_geometry_fixture_sources() -> [String; 2] {
    let (geometry, models) = checker_geometry();
    let source = stage(
        &format!(
            r##"{CHECKER}{RED}<MaterialAsset id="surface" baseColor="#FFFFFF" roughness="0.04"
transmission="1" ior="1.52" refractionMode="solid" />"##
        ),
        &geometry,
        &models,
        2,
    );
    // Far geometry enlarges the transport buffer without entering the
    // camera, reflection rays or authored lighting interval.
    let large_source = source
        .replace(
            "</Assets>",
            r##"<MaterialAsset id="far_black" baseColor="#000000" specular="0" />
<GeometryAsset id="far_dense_geometry"><Primitive shape="sphere" radius="1" segments="192" rings="128" /></GeometryAsset>
<MeshAsset id="far_dense_mesh" geometry="far_dense_geometry" material="far_black" /></Assets>"##,
        )
        .replace(
            "</CompositeGroup>",
            r#"<Model id="far_dense" asset="far_dense_mesh" position={[1000,1000,1000]} castShadow="false" /></CompositeGroup>"#,
        );
    [source, large_source]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_dense_geometry_preserves_solid_pixels_across_consecutive_frames() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let [source, large_source] = dense_geometry_fixture_sources()
            .map(|source| source.replace("[128,128]", "[256,256]"));
        let reference = image(&source).await;
        let graph = parse_graph_script(&large_source).expect("dense transport fixture");
        let mut renderer = SceneRenderer::new(SceneRenderProfile::Gpu)
            .await
            .expect("dense native renderer");
        renderer.set_immediate_preview_settings(ImmediatePreviewSettings {
            profile: ImmediatePreviewProfile::Cinematic,
            target_fps: 30.0,
            dynamic_resolution: false,
            min_resolution_scale: 1.0,
        });
        for frame in 0..3 {
            let actual = renderer
                .render_frame_gpu_readback(&graph, frame)
                .await
                .expect("consecutive dense native frame");
            assert!(renderer.last_3d_frame_profile().hybrid_triangles >= 32_768);
            evidence(&format!("dense-solid-frame-{frame}"), &actual);
            let maximum_error = actual
                .as_raw()
                .iter()
                .zip(reference.as_raw())
                .map(|(actual, reference)| actual.abs_diff(*reference))
                .max()
                .unwrap();
            assert!(
                maximum_error <= 2,
                "dense/temporal transport changed full-resolution pixels at frame {frame}: {maximum_error}"
            );
        }
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_taa_rejects_previous_glass_on_newly_uncovered_opaque_pixels() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    pollster::block_on(async {
        let source = stage(
            r##"<MaterialAsset id="glass" baseColor="#FFFFFF" roughness="0.04"
transmission="1" ior="1.52" thickness="0.3"
attenuationColor="#FF1010" attenuationDistance="0.1" />
<MaterialAsset id="blue" baseColor="#000000" emissive="#0010FF" specular="0" />"##,
            r#"<GeometryAsset id="pane_g"><Primitive shape="box" size={[1.4,1.4,0.3]} /></GeometryAsset>
<MeshAsset id="pane" material="glass" geometry="pane_g" />
<GeometryAsset id="wall_g"><Primitive shape="box" size={[4,4,0.05]} /></GeometryAsset>
<MeshAsset id="wall" material="blue" geometry="wall_g" />"#,
            r#"<Model id="pane" asset="pane" position={[48*$time.sec,0,0]} castShadow="false" />
<Model id="wall" asset="wall" position={[0,0,-1.8]} castShadow="false" />"#,
            1,
        ).replace("method=\"off\"", "method=\"taa\" quality=\"high\"");
        let graph = parse_graph_script(&source).expect("moving glass fixture parse");
        let settings = ImmediatePreviewSettings {
            profile: ImmediatePreviewProfile::Cinematic,
            target_fps: 30.0,
            dynamic_resolution: false,
            min_resolution_scale: 1.0,
        };
        let mut sequence = SceneRenderer::new(SceneRenderProfile::Gpu).await.unwrap();
        sequence.set_immediate_preview_settings(settings);
        let covered = sequence.render_frame_gpu_readback(&graph, 0).await.unwrap();
        let uncovered = sequence.render_frame_gpu_readback(&graph, 1).await.unwrap();
        let mut fresh = SceneRenderer::new(SceneRenderProfile::Gpu).await.unwrap();
        fresh.set_immediate_preview_settings(settings);
        let reference = fresh.render_frame_gpu_readback(&graph, 1).await.unwrap();
        evidence("taa-glass-covered", &covered);
        evidence("taa-glass-uncovered", &uncovered);
        evidence("taa-glass-uncovered-fresh", &reference);
        assert!(
            reference.get_pixel(64, 64)[2] > covered.get_pixel(64, 64)[2].saturating_add(40),
            "fixture did not uncover a distinctly blue wall"
        );
        let worst = (54..74)
            .flat_map(|y| (54..74).map(move |x| (x, y)))
            .flat_map(|(x, y)| {
                uncovered
                    .get_pixel(x, y)
                    .0
                    .into_iter()
                    .zip(reference.get_pixel(x, y).0)
                    .map(|(a, b)| a.abs_diff(b))
            })
            .max()
            .unwrap();
        assert!(
            worst <= 2,
            "previous glass contaminated an unchanged opaque underlay by {worst} levels"
        );
    });
}

fn checker_fixture_sources() -> [String; 2] {
    let (geometry, models) = checker_geometry();
    let material = format!(
        r##"<MaterialAsset id="surface" baseColor="#FFFFFF" roughness="0.04" specular="1" transmission="1" ior="1.52" thickness="0.1" refractionMode="slab" />{CHECKER}{RED}"##
    );
    let slab = stage(&material, &geometry, &models, 1);
    let solid = slab.replace("refractionMode=\"slab\"", "refractionMode=\"solid\"");
    [slab, solid]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_solid_uses_geometry_exit_instead_of_slab_approximation() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [slab, solid] = checker_fixture_sources();
    pollster::block_on(async {
        let a = image(&slab).await;
        let b = image(&solid).await;
        evidence("refraction-slab", &a);
        evidence("refraction-solid", &b);
        let changed = (36..92)
            .flat_map(|y| (36..92).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                a.get_pixel(x, y).0[..3]
                    .iter()
                    .zip(&b.get_pixel(x, y).0[..3])
                    .any(|(a, b)| a.abs_diff(*b) > 10)
            })
            .count();
        assert!(
            changed > 80,
            "solid exit did not change checker refraction: {changed} pixels"
        );
    });
}

fn absorption_fixture_sources() -> [String; 3] {
    let material = r##"<MaterialAsset id="surface" baseColor="#FFFFFF" roughness="0.04" specular="1" transmission="1" ior="1.52" thickness="0.01" attenuationColor="#40FFFF" attenuationDistance="1" refractionMode="solid" />
<MaterialAsset id="white" baseColor="#000000" emissive="#FFFFFF" />"##;
    let geometry = r#"<GeometryAsset id="sphere_g"><Primitive shape="sphere" radius="0.85" segments="48" rings="32" /></GeometryAsset><MeshAsset id="sphere" material="surface" geometry="sphere_g" />
<GeometryAsset id="back_g"><Primitive shape="box" size={[8,8,0.05]} /></GeometryAsset><MeshAsset id="back" material="white" geometry="back_g" />"#;
    let thin = stage(
        material,
        geometry,
        r#"<Model id="glass" asset="sphere" scale="0.7" castShadow="false" /><Model asset="back" position={[0,0,-2]} castShadow="false" />"#,
        1,
    );
    let fake_thickness = thin.replace("thickness=\"0.01\"", "thickness=\"10\"");
    let thick = thin.replace("scale=\"0.7\"", "scale=\"1.2\"");
    [thin, fake_thickness, thick]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_solid_absorption_uses_actual_distance_and_ignores_slab_thickness() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [thin, fake_thickness, thick] = absorption_fixture_sources();
    pollster::block_on(async {
        let a = image(&thin).await;
        let same = image(&fake_thickness).await;
        let b = image(&thick).await;
        evidence("absorption-small-solid", &a);
        evidence("absorption-irrelevant-slab-thickness", &same);
        evidence("absorption-large-solid", &b);
        let center = |image: &image::RgbaImage| {
            (60..68)
                .flat_map(|y| (60..68).map(move |x| (x, y)))
                .map(|(x, y)| image.get_pixel(x, y)[0] as u64)
                .sum::<u64>() as f64
                / 64.0
        };
        assert!(
            (center(&a) - center(&same)).abs() <= 1.0,
            "solid used slab thickness"
        );
        assert!(
            center(&b) + 12.0 < center(&a),
            "longer solid optical path did not absorb more: {} vs {}",
            center(&a),
            center(&b)
        );
    });
}

fn overlapping_fixture_sources() -> [String; 2] {
    let material = r##"<MaterialAsset id="surface" baseColor="#FFFFFF" roughness="0.04" specular="1" transmission="1" ior="1.52" attenuationColor="#80DFFF" attenuationDistance="1" refractionMode="solid" />
<MaterialAsset id="white" baseColor="#000000" emissive="#FFFFFF" />"##;
    let geometry = r#"<GeometryAsset id="sphere_g"><Primitive shape="sphere" radius="0.75" segments="40" rings="24" /></GeometryAsset><MeshAsset id="sphere" material="surface" geometry="sphere_g" />
<GeometryAsset id="back_g"><Primitive shape="box" size={[8,8,0.05]} /></GeometryAsset><MeshAsset id="back" material="white" geometry="back_g" />"#;
    let far = r#"<Model id="far" asset="sphere" position={[-0.25,0,-0.3]} castShadow="false" />"#;
    let near = r#"<Model id="near" asset="sphere" position={[0.25,0,0.3]} castShadow="false" />"#;
    let back = r#"<Model asset="back" position={[0,0,-2]} castShadow="false" />"#;
    let a = stage(material, geometry, &format!("{far}{near}{back}"), 1);
    let b = stage(material, geometry, &format!("{near}{far}{back}"), 1);
    [a, b]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_overlapping_solids_follow_intersections_independent_of_authoring_order() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [a, b] = overlapping_fixture_sources();
    pollster::block_on(async {
        let a = image(&a).await;
        let b = image(&b).await;
        evidence("overlapping-solids-authoring-far-first", &a);
        evidence("overlapping-solids-authoring-near-first", &b);
        let error: u64 = (40..88)
            .flat_map(|y| (40..88).map(move |x| (x, y)))
            .map(|(x, y)| {
                a.get_pixel(x, y).0[..3]
                    .iter()
                    .zip(&b.get_pixel(x, y).0[..3])
                    .map(|(a, b)| a.abs_diff(*b) as u64)
                    .sum::<u64>()
            })
            .sum();
        let mean = error as f64 / (48.0 * 48.0 * 3.0);
        assert!(
            mean <= 1.0,
            "authoring order changed solid overlaps: mean byte error {mean}"
        );
    });
}

fn mirrors_fixture_sources() -> [String; 2] {
    let materials = format!(
        r##"<MaterialAsset id="mirror" baseColor="#FFFFFF" metallic="1" roughness="0.04" />{RED}"##
    );
    let geometry = r#"<GeometryAsset id="mirror_g"><Primitive shape="box" size={[1.8,1.8,0.04]} /></GeometryAsset><MeshAsset id="mirror" material="mirror" geometry="mirror_g" />
<GeometryAsset id="red_g"><Primitive shape="box" size={[1.5,1.5,0.1]} /></GeometryAsset><MeshAsset id="red" material="red" geometry="red_g" />"#;
    let models = r#"<Model id="first" asset="mirror" rotation={[0,45,0]} castShadow="false" />
<Model id="second" asset="mirror" position={[2,0,0]} rotation={[0,-45,0]} castShadow="false" />
<Model id="offscreen" asset="red" position={[2,0,5]} castShadow="false" />"#;
    let one = stage(&materials, geometry, models, 1);
    let two = stage(&materials, geometry, models, 2);
    [one, two]
}

fn crossing_solid_fixture_sources() -> [String; 2] {
    let materials = r##"<MaterialAsset id="cyan" baseColor="#FFFFFF" roughness="0.04" transmission="1" ior="1" attenuationColor="#44FFFF" attenuationDistance="0.5" refractionMode="solid" />
<MaterialAsset id="yellow" baseColor="#FFFFFF" roughness="0.04" transmission="1" ior="1" attenuationColor="#FFFF44" attenuationDistance="0.5" refractionMode="solid" />
<MaterialAsset id="white" baseColor="#000000" emissive="#FFFFFF" />"##;
    let geometry = r#"<GeometryAsset id="pane_g"><Primitive shape="box" size={[2.2,2.1,0.5]} /></GeometryAsset>
<MeshAsset id="cyan" material="cyan" geometry="pane_g" /><MeshAsset id="yellow" material="yellow" geometry="pane_g" />
<GeometryAsset id="back_g"><Primitive shape="box" size={[8,8,0.05]} /></GeometryAsset><MeshAsset id="back" material="white" geometry="back_g" />"#;
    // Both volumes have exactly the same centre and sort distance. Their near
    // boundaries cross at the centre, so no whole-object order can be right
    // for both halves of the image. IOR 1 isolates geometry/Beer composition.
    let a = r#"<Model id="cyan" asset="cyan" rotation={[0,35,0]} castShadow="false" />"#;
    let b = r#"<Model id="yellow" asset="yellow" rotation={[0,-35,0]} castShadow="false" />"#;
    let back = r#"<Model id="back" asset="back" position={[0,0,-2]} castShadow="false" />"#;
    let first = stage(materials, geometry, &format!("{a}{b}{back}"), 1);
    let reversed = stage(materials, geometry, &format!("{b}{a}{back}"), 1);
    [first, reversed]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_crossing_solid_entries_ignore_equal_center_sort_order() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [first, reversed] = crossing_solid_fixture_sources();
    pollster::block_on(async {
        let first = image(&first).await;
        let reversed = image(&reversed).await;
        evidence("crossed-solid-entries-cyan-first", &first);
        evidence("crossed-solid-entries-yellow-first", &reversed);
        let mut error = 0u64;
        let mut tinted = 0usize;
        for y in 24..104 {
            for x in 24..104 {
                let a = first.get_pixel(x, y);
                let b = reversed.get_pixel(x, y);
                error += (0..3).map(|c| a[c].abs_diff(b[c]) as u64).sum::<u64>();
                if a[0].saturating_add(25) < a[1] || a[2].saturating_add(25) < a[1] {
                    tinted += 1;
                }
            }
        }
        assert!(
            tinted > 500,
            "crossed-volume fixture lost visible absorption: {tinted} tinted pixels"
        );
        let mean = error as f64 / (80.0 * 80.0 * 3.0);
        assert!(
            mean <= 1.0,
            "equal-centre solid ordering changed per-pixel entry: mean byte error {mean}"
        );
    });
}

fn slab_shadow_fixture_sources() -> [String; 3] {
    let materials = r##"<MaterialAsset id="glass" baseColor="#FFFFFF" roughness="0.04" transmission="1" ior="1" thickness="0.5" attenuationColor="#40FFFF" attenuationDistance="1" refractionMode="slab" doubleSided="true" />
<MaterialAsset id="receiver" baseColor="#FFFFFF" roughness="1" specular="0" />"##;
    let geometry = r#"<GeometryAsset id="closed_g"><Primitive shape="box" size={[1.2,2,0.25]} /></GeometryAsset><MeshAsset id="closed" material="glass" geometry="closed_g" />
<GeometryAsset id="open_g"><Primitive shape="plane" size={[1.2,2]} /></GeometryAsset><MeshAsset id="open" material="glass" geometry="open_g" />
<GeometryAsset id="receiver_g"><Primitive shape="box" size={[3,3,0.08]} /></GeometryAsset><MeshAsset id="receiver" material="receiver" geometry="receiver_g" />"#;
    let bare = stage(materials, geometry, r#"<Model id="receiver" asset="receiver" castShadow="false" receiveShadow="true" />"#, 1)
        .replace("ambientIntensity=\"0\" reflectionBounces", "ambientIntensity=\"0\" shadowMode=\"perLight\" reflectionBounces")
        .replace(r#"<DirectionalLight direction={[0,0,-1]} intensity="0" castShadow="false" />"#, r#"<PointLight position={[2,0,3]} intensity="20" range="12" castShadow="true" sourceRadius="0" />"#);
    let closed = bare.replace("</CompositeGroup>", r#"<Model id="pane" asset="closed" position={[1,0,1.5]} castShadow="true" /></CompositeGroup>"#);
    let open = bare.replace("</CompositeGroup>", r#"<Model id="pane" asset="open" position={[1,0,1.5]} rotation={[90,0,0]} castShadow="true" /></CompositeGroup>"#);
    [bare, closed, open]
}

fn reflected_slab_fixture_sources() -> [String; 3] {
    // These tiles are radiance controls. An ordinary black PBR tile still
    // reflects the environment and would not be a near-black reference.
    let checker = CHECKER
        .replace("emissive=\"#050505\"", "emissive=\"#000000\" specular=\"0\"")
        .replace("emissive=\"#FFFFFF\"", "emissive=\"#FFFFFF\" specular=\"0\"");
    let materials = format!(
        r##"<MaterialAsset id="mirror" baseColor="#FFFFFF" metallic="1" roughness="0.04" />
<MaterialAsset id="pane" baseColor="#FFFFFF" roughness="0.04" transmission="1" ior="1" thickness="0.5" attenuationColor="#FFFFFF" attenuationDistance="1" refractionMode="slab" doubleSided="true" />{checker}"##
    );
    let geometry = r#"<GeometryAsset id="mirror_g"><Primitive shape="box" size={[2,2,0.05]} /></GeometryAsset><MeshAsset id="mirror" material="mirror" geometry="mirror_g" />
<GeometryAsset id="pane_g"><Primitive shape="box" size={[4.5,4.5,0.9]} /></GeometryAsset><MeshAsset id="pane" material="pane" geometry="pane_g" />
<GeometryAsset id="tile_g"><Primitive shape="box" size={[0.5,0.5,0.05]} /></GeometryAsset><MeshAsset id="black" material="black" geometry="tile_g" /><MeshAsset id="white" material="white" geometry="tile_g" />"#;
    let mut models = String::from(
        r#"<Model id="mirror" asset="mirror" castShadow="false" /><Model id="pane" asset="pane" position={[0,0,5]} castShadow="false" />"#,
    );
    let absent = stage(&materials, geometry, &models, 1);
    // All panes/tiles are behind the main camera at z=4. Only a reflection ray
    // can see the checker through glass. The geometric 0.9 thickness differs
    // from its authored 0.5, so tracing the rear boundary must not charge Beer
    // a second time after the virtual slab exit.
    for y in 0..8 {
        for x in 0..8 {
            let material = if (x + y) % 2 == 0 { "black" } else { "white" };
            models.push_str(&format!(
                "<Model asset=\"{material}\" position={{[{},{},6]}} castShadow=\"false\" />",
                x as f32 * 0.5 - 1.75,
                y as f32 * 0.5 - 1.75
            ));
        }
    }
    let clear = stage(&materials, geometry, &models, 1);
    let tinted = clear.replace(
        "attenuationColor=\"#FFFFFF\"",
        "attenuationColor=\"#40FFFF\"",
    );
    [absent, clear, tinted]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_reflected_slab_reveals_offscreen_checker_with_single_authored_absorption() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [absent, clear, tinted] = reflected_slab_fixture_sources();
    pollster::block_on(async {
        let absent = image(&absent).await;
        let clear = image(&clear).await;
        let tinted = image(&tinted).await;
        evidence("reflected-slab-no-checker", &absent);
        evidence("reflected-slab-clear-checker", &clear);
        evidence("reflected-slab-tinted-checker", &tinted);
        let mut white = 0usize;
        let mut dark = 0usize;
        let mut tinted_white = 0usize;
        let mut revealed = 0usize;
        for y in 36..92 {
            for x in 36..92 {
                let bare = absent.get_pixel(x, y);
                let c = clear.get_pixel(x, y);
                let t = tinted.get_pixel(x, y);
                if c[1] > 220 {
                    white += 1;
                }
                if c[1] < 35 {
                    dark += 1;
                }
                if c[1] > bare[1].saturating_add(35) {
                    revealed += 1;
                }
                if c[1] > 220 && t[1] > 220 && t[0].saturating_add(30) < t[1] {
                    // sqrt(0.25) absorption over one 0.5 path leaves red near
                    // 0.5 linear (about 186 display). Double charging leaves
                    // about 136: a broad tolerance separates the two cases.
                    assert!(
                        t[0] > 155,
                        "reflected closed slab charged authored absorption twice at ({x},{y}): {t:?}"
                    );
                    tinted_white += 1;
                }
            }
        }
        assert!(
            white > 300 && dark > 300 && revealed > 300,
            "offscreen checker missing through reflected slab: white={white}, dark={dark}, revealed={revealed}"
        );
        assert!(
            tinted_white > 300,
            "reflected slab failed to retain colored Beer absorption: {tinted_white}"
        );
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_closed_slab_shadow_applies_authored_pair_once() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [bare, closed, open] = slab_shadow_fixture_sources();
    pollster::block_on(async {
        let bare = image(&bare).await;
        let closed = image(&closed).await;
        let open = image(&open).await;
        evidence("slab-shadow-no-pane", &bare);
        evidence("slab-shadow-closed-box", &closed);
        evidence("slab-shadow-open-plane", &open);
        let bare = center_mean(&bare);
        let closed = center_mean(&closed);
        let open = center_mean(&open);
        assert!(
            open[0] + 25.0 < bare[0],
            "slab fixture lost absorption: bare={bare:?}, open={open:?}"
        );
        for c in 0..3 {
            assert!(
                (closed[c] - open[c]).abs() <= 4.0,
                "closed slab charged its authored paired interfaces again at exit: closed={closed:?}, open={open:?}"
            );
        }
    });
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_two_mirrors_need_requested_second_reflection() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [one, two] = mirrors_fixture_sources();
    pollster::block_on(async {
        let a = image(&one).await;
        let b = image(&two).await;
        evidence("mirrors-one-bounce", &a);
        evidence("mirrors-two-bounces", &b);
        let red = |image: &image::RgbaImage| {
            (60..68)
                .flat_map(|y| (60..68).map(move |x| (x, y)))
                .map(|(x, y)| image.get_pixel(x, y)[0] as u64)
                .sum::<u64>() as f64
                / 64.0
        };
        assert!(
            red(&b) > red(&a) + 30.0,
            "second mirror reflection missing: {} vs {}",
            red(&a),
            red(&b)
        );
    });
}

fn center_mean(image: &image::RgbaImage) -> [f64; 3] {
    let mut sum = [0u64; 3];
    for y in 60..68 {
        for x in 60..68 {
            let pixel = image.get_pixel(x, y);
            for c in 0..3 {
                sum[c] += pixel[c] as u64;
            }
        }
    }
    sum.map(|v| v as f64 / 64.0)
}

fn inside_absorption_fixture_sources() -> [String; 2] {
    let material = r##"<MaterialAsset id="surface" baseColor="#FFFFFF" roughness="0.04" specular="1" transmission="1" ior="1" attenuationColor="#40FFFF" attenuationDistance="1" refractionMode="solid" doubleSided="true" />
<MaterialAsset id="white" baseColor="#000000" emissive="#FFFFFF" />"##;
    let geometry = r#"<GeometryAsset id="sphere_g"><Primitive shape="sphere" radius="0.85" segments="48" rings="32" /></GeometryAsset><MeshAsset id="sphere" material="surface" geometry="sphere_g" />
<GeometryAsset id="back_g"><Primitive shape="box" size={[8,8,0.05]} /></GeometryAsset><MeshAsset id="back" material="white" geometry="back_g" />"#;
    let outside = stage(
        material,
        geometry,
        r#"<Model id="glass" asset="sphere" castShadow="false" /><Model asset="back" position={[0,0,-2]} castShadow="false" />"#,
        1,
    );
    let inside = outside.replace(
        r#"<Camera3D position={[0,0,4]} target={[0,0,0]} fov="35" />"#,
        r#"<Camera3D position={[0,0,0]} target={[0,0,-1]} fov="35" />"#,
    );
    [outside, inside]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_inside_camera_keeps_exit_and_does_not_absorb_the_backface_twice() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [outside, inside] = inside_absorption_fixture_sources();
    pollster::block_on(async {
        let outside_image = image(&outside).await;
        let inside_image = image(&inside).await;
        evidence("camera-outside-solid-absorption", &outside_image);
        evidence("camera-inside-solid-absorption", &inside_image);
        let outside = center_mean(&outside_image);
        let inside = center_mean(&inside_image);
        assert!(
            inside[1] > 170.0 && inside[2] > 170.0,
            "inside exit disappeared: {inside:?}"
        );
        assert!(
            inside[0] + 30.0 < inside[1],
            "inside exit failed to retain volume absorption: {inside:?}"
        );
        assert!(
            inside[0] > outside[0] + 20.0,
            "inside path should traverse one radius; outside two radii: inside={inside:?}, outside={outside:?}"
        );
    });
}

fn inside_tir_fixture_sources() -> [String; 2] {
    let material = r##"<MaterialAsset id="surface" baseColor="#FFFFFF" roughness="0.04" specular="1" transmission="1" ior="1.52" refractionMode="solid" doubleSided="true" />
<MaterialAsset id="red" baseColor="#000000" emissive="#FF0000" emissiveStrength="8" />
<MaterialAsset id="blue" baseColor="#000000" emissive="#0000FF" emissiveStrength="8" />"##;
    let geometry = r#"<GeometryAsset id="box_g"><Primitive shape="box" size={[4,4,4]} /></GeometryAsset><MeshAsset id="box" material="surface" geometry="box_g" />
<GeometryAsset id="red_g"><Primitive shape="box" size={[0.12,1,0.01]} /></GeometryAsset><MeshAsset id="red" material="red" geometry="red_g" />
<GeometryAsset id="blue_g"><Primitive shape="box" size={[12,12,0.05]} /></GeometryAsset><MeshAsset id="blue" material="blue" geometry="blue_g" />"#;
    // The centre ray hits z=-2 at x=1.9: 43.5 degrees in glass exceeds
    // the 41.1-degree critical angle. The red card is on the reflected ray,
    // beyond the narrow direct field of view; blue is outside the solid.
    let source = stage(material,geometry,r#"<Model id="glass" asset="box" castShadow="false" /><Model asset="red" position={[1.95,0,-1.95]} castShadow="false" /><Model asset="blue" position={[0,0,-3]} castShadow="false" />"#,1)
        .replace(r#"<Camera3D position={[0,0,4]} target={[0,0,0]} fov="35" />"#,r#"<Camera3D position={[0,0,0]} target={[0.95,0,-1]} fov="3" />"#);
    let matched = source.replace("ior=\"1.52\"", "ior=\"1\"");
    [matched, source]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_inside_camera_total_internal_reflection_preserves_reflected_energy() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [matched, source] = inside_tir_fixture_sources();
    pollster::block_on(async {
        let air_image = image(&matched).await;
        let tir_image = image(&source).await;
        evidence("camera-inside-index-matched", &air_image);
        evidence("camera-inside-total-internal-reflection", &tir_image);
        let air = center_mean(&air_image);
        let tir = center_mean(&tir_image);
        assert!(
            air[2] > air[0] + 80.0,
            "index-matched control should see the blue exterior: {air:?}"
        );
        assert!(
            tir[0] > 100.0 && tir[0] > tir[2] + 80.0,
            "TIR must reflect the interior red card without leaking blue transmission: {tir:?}"
        );
    });
}

fn shadow_fixture_sources() -> [String; 4] {
    let material = r##"<MaterialAsset id="surface" baseColor="#FFFFFF" roughness="0.04" specular="1" transmission="1" ior="1.52" attenuationColor="#08FFFF" attenuationDistance="0.5" refractionMode="solid" />
<MaterialAsset id="receiver" baseColor="#FFFFFF" roughness="1" specular="0" />"##;
    let geometry = r#"<GeometryAsset id="sphere_g"><Primitive shape="sphere" radius="0.5" segments="40" rings="24" /></GeometryAsset><MeshAsset id="sphere" material="surface" geometry="sphere_g" />
<GeometryAsset id="receiver_g"><Primitive shape="box" size={[3,3,0.08]} /></GeometryAsset><MeshAsset id="receiver" material="receiver" geometry="receiver_g" />"#;
    let receiver =
        r#"<Model id="receiver" asset="receiver" castShadow="false" receiveShadow="true" />"#;
    let glass = r#"<Model id="glass" asset="sphere" position={[1,0,1.5]} castShadow="true" />"#;
    let bare = stage(material,geometry,receiver,1)
        .replace("ambientIntensity=\"0\" reflectionBounces", "ambientIntensity=\"0\" shadowMode=\"perLight\" reflectionBounces")
        .replace(r#"<DirectionalLight direction={[0,0,-1]} intensity="0" castShadow="false" />"#,r#"<PointLight position={[2,0,3]} intensity="20" range="12" castShadow="true" sourceRadius="0" />"#);
    let colored = bare.replace("</CompositeGroup>", &format!("{glass}</CompositeGroup>"));
    let no_cast = colored.replace(
        "position={[1,0,1.5]} castShadow=\"true\"",
        "position={[1,0,1.5]} castShadow=\"false\"",
    );
    let no_receive = colored.replace("receiveShadow=\"true\"", "receiveShadow=\"false\"");
    [bare, colored, no_cast, no_receive]
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn hybrid_colored_glass_shadows_respect_model_cast_and_receive_flags() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    let [bare, colored, no_cast, no_receive] = shadow_fixture_sources();
    pollster::block_on(async {
        let bare_image = image(&bare).await;
        let colored_image = image(&colored).await;
        let no_cast_image = image(&no_cast).await;
        let no_receive_image = image(&no_receive).await;
        evidence("shadow-no-glass", &bare_image);
        evidence("shadow-colored-solid-glass", &colored_image);
        evidence("shadow-cast-disabled", &no_cast_image);
        evidence("shadow-receive-disabled", &no_receive_image);
        let bare = center_mean(&bare_image);
        let colored = center_mean(&colored_image);
        let no_cast = center_mean(&no_cast_image);
        let no_receive = center_mean(&no_receive_image);
        assert!(
            colored[1] > 50.0 && colored[1] > colored[0] + 30.0,
            "glass shadow must retain green/blue light and absorb red: {colored:?}"
        );
        assert!(
            colored[0] + 30.0 < bare[0],
            "glass absorption did not affect receiver: bare={bare:?}, glass={colored:?}"
        );
        for c in 0..3 {
            assert!(
                (no_cast[c] - bare[c]).abs() < 4.0,
                "castShadow=false changed direct receiver: bare={bare:?}, no_cast={no_cast:?}"
            );
            assert!(
                (no_receive[c] - bare[c]).abs() < 4.0,
                "receiveShadow=false changed direct receiver: bare={bare:?}, no_receive={no_receive:?}"
            );
        }
    });
}

#[test]
fn hybrid_native_fixture_sources_parse_without_a_gpu() {
    let _reference = super::super::transport_policy::test_reference_transport(true);
    use crate::scene::model::{Scene3DNode, SceneNode};

    // Validate the exact inputs used by ignored GPU tests in ordinary CPU CI.
    // Counting retained declarations also catches silently omitted inline tags.
    let fixtures = [
        (
            "offscreen coat",
            Vec::from(offscreen_fixture_sources(OFFSCREEN_SURFACES[0])),
        ),
        (
            "offscreen glass",
            Vec::from(offscreen_fixture_sources(OFFSCREEN_SURFACES[1])),
        ),
        ("checker exit", Vec::from(checker_fixture_sources())),
        ("dense consecutive glass", Vec::from(dense_geometry_fixture_sources())),
        ("actual absorption", Vec::from(absorption_fixture_sources())),
        (
            "overlapping solids",
            Vec::from(overlapping_fixture_sources()),
        ),
        ("two mirrors", Vec::from(mirrors_fixture_sources())),
        (
            "inside absorption",
            Vec::from(inside_absorption_fixture_sources()),
        ),
        ("inside TIR", Vec::from(inside_tir_fixture_sources())),
        ("colored shadows", Vec::from(shadow_fixture_sources())),
        (
            "crossed solid entries",
            Vec::from(crossing_solid_fixture_sources()),
        ),
        (
            "closed slab shadows",
            Vec::from(slab_shadow_fixture_sources()),
        ),
        (
            "reflected slabs",
            Vec::from(reflected_slab_fixture_sources()),
        ),
    ];
    for (label, sources) in fixtures {
        for (variant, source) in sources.iter().enumerate() {
            let graph = parse_graph_script(source)
                .unwrap_or_else(|error| panic!("{label} variant {variant}: {error:?}"));
            assert_eq!(graph.size, (128, 128), "{label} variant {variant}");
            assert_eq!(
                graph.material_assets.len(),
                source.matches("<MaterialAsset ").count(),
                "{label} variant {variant} dropped materials"
            );
            assert_eq!(
                graph.geometry_assets.len(),
                source.matches("<GeometryAsset ").count(),
                "{label} variant {variant} dropped geometry"
            );
            let aa = graph.render_styles[0]
                .anti_aliasing
                .as_ref()
                .expect("fixture AA");
            assert_eq!(aa.method.as_deref(), Some("off"));
            assert!(aa.fallback.is_none());
            let SceneNode::Timeline(timeline) = &graph.scenes[0].children[0] else {
                panic!("{label}: expected timeline");
            };
            let SceneNode::Track(track) = &timeline.children[0] else {
                panic!("{label}: expected track");
            };
            let SceneNode::Sequence(sequence) = &track.children[0] else {
                panic!("{label}: expected sequence");
            };
            let SceneNode::Group(group) = &sequence.children[0] else {
                panic!("{label}: expected CompositeGroup");
            };
            let composite = group.composite.as_ref().expect("fixture composite");
            assert_eq!(
                composite
                    .nodes_3d
                    .iter()
                    .filter(|node| matches!(node, Scene3DNode::Model(_)))
                    .count(),
                source.matches("<Model ").count(),
                "{label} variant {variant} dropped models"
            );
            assert_eq!(
                composite
                    .nodes_3d
                    .iter()
                    .filter(|node| matches!(node, Scene3DNode::Camera(_)))
                    .count(),
                1,
                "{label} variant {variant} dropped camera"
            );
        }
    }
}
