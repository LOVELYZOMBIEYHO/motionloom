use super::*;
use crate::experimental::geometry::ResolvedMesh;
use crate::world::gltf_loader::{GlbMaterialData, GlbTextureData};
use crate::world::{WorldLight, WorldLightKind};

fn plane(
    points: [[f32; 3]; 4],
    normal: [f32; 3],
    color: [f32; 3],
    material: GlbMaterialData,
) -> ResolvedMesh {
    let (t, _) = basis(normal);
    ResolvedMesh {
        name: "fixture".into(),
        positions: points.to_vec(),
        normals: vec![normal; 4],
        tangents: vec![[t[0], t[1], t[2], 1.]; 4],
        uvs: vec![[0., 0.], [1., 0.], [1., 1.], [0., 1.]],
        colors: vec![[color[0], color[1], color[2], 1.]; 4],
        indices: vec![0, 1, 2, 0, 2, 3],
        material,
        textures: vec![],
        uv_source: "authored".into(),
    }
}
fn matte() -> GlbMaterialData {
    GlbMaterialData {
        metallic_factor: 0.,
        ..Default::default()
    }
}
fn wall(color: [f32; 3], material: GlbMaterialData) -> ResolvedMesh {
    plane(
        [
            [0., -10., -10.],
            [0., -10., 10.],
            [0., 10., 10.],
            [0., 10., -10.],
        ],
        [-1., 0., 0.],
        color,
        material,
    )
}
fn environment() -> Environment {
    Environment {
        map: crate::lighting_ibl::LinearEnvironment::new(2, 1, vec![[2.; 3]; 2]).unwrap(),
        intensity: 1.,
        diffuse: 1.,
        specular: 1.,
        rotation: 0.,
    }
}

#[test]
fn closed_wall_blocks_environment_leak() {
    let blocked = TraceScene::new(
        vec![wall([0.; 3], matte())],
        vec![],
        Some(environment()),
        50.,
    )
    .unwrap();
    let open = TraceScene::new(vec![], vec![], Some(environment()), 50.).unwrap();
    let origin = [-1., 0., 0.];
    let direction = [1., 0., 0.];
    assert_eq!(blocked.visibility(origin, direction, 50.).0, [0.; 3]);
    assert_eq!(blocked.visibility(origin, direction, 50.).1, 1.);
    assert_eq!(
        blocked.radiance(origin, direction, 3, &mut Rng::new(7), false),
        [0.; 3]
    );
    assert_eq!(
        open.radiance(origin, direction, 3, &mut Rng::new(7), false),
        [2.; 3]
    );
}

#[test]
fn thin_window_transmits_and_attenuates_channels() {
    let glass = GlbMaterialData {
        transmission_factor: 0.9,
        ior: 1.52,
        thickness_factor: 0.02,
        attenuation_color: [0.25, 1., 1.],
        attenuation_distance: 0.02,
        ..matte()
    };
    let scene =
        TraceScene::new(vec![wall([1.; 3], glass)], vec![], Some(environment()), 50.).unwrap();
    let (through, depth) = scene.visibility([-1., 0., 0.], [1., 0., 0.], 50.);
    assert_eq!(depth, 50.);
    assert!(
        (through[0] / through[1] - 0.25).abs() < 0.001,
        "{through:?}"
    );
    assert!(through[1] > 0.8 && through[1] < 0.9, "{through:?}");
    let radiance = scene.radiance([-1., 0., 0.], [1., 0., 0.], 3, &mut Rng::new(99), false);
    for k in 0..3 {
        assert!((radiance[k] - 2. * through[k]).abs() < 1e-4);
    }
}

#[test]
fn diffuse_wall_color_reaches_floor_on_second_bounce() {
    let floor = plane(
        [[-5., 0., -5.], [-5., 0., 5.], [5., 0., 5.], [5., 0., -5.]],
        [0., 1., 0.],
        [0.7; 3],
        matte(),
    );
    let red = plane(
        [[1., 0., -5.], [1., 0., 5.], [1., 4., 5.], [1., 4., -5.]],
        [-1., 0., 0.],
        [0.85, 0.03, 0.01],
        matte(),
    );
    let light = WorldLight {
        id: Some("sun".into()),
        kind: WorldLightKind::Directional,
        position: [0.; 3],
        direction: [1., -1., 0.],
        color: [1.; 3],
        intensity: 1.,
        range: 0.,
        inner_cone_degrees: 0.,
        outer_cone_degrees: 0.,
        width: 0.,
        height: 0.,
        cast_shadow: true,
        shadow_strength: 1.,
        source_radius: 0.,
        angular_diameter: 0.,
    };
    let scene = TraceScene::new(vec![floor, red], vec![light], None, 50.).unwrap();
    let mut one = [0.; 3];
    let mut two = [0.; 3];
    for i in 0..2048 {
        one = add(
            one,
            scene.radiance([0., 0.5, 0.], [0., -1., 0.], 1, &mut Rng::new(i), false),
        );
        two = add(
            two,
            scene.radiance([0., 0.5, 0.], [0., -1., 0.], 2, &mut Rng::new(i), false),
        );
    }
    one = scale(one, 1. / 2048.);
    two = scale(two, 1. / 2048.);
    assert!(two[0] > one[0] + 0.015, "first {one:?}, second {two:?}");
    assert!(
        two[0] - one[0] > 10. * (two[1] - one[1]),
        "first {one:?}, second {two:?}"
    );
}

fn fixture_script(camera: &str, color: &str, size: f32) -> String {
    format!(
        r##"<Graph fps={{24}} duration="3s" size={{[64,64]}}>
    <Assets><MaterialAsset id="paint" shading="pbr" baseColor="{color}" roughness="0.8" />
    <GeometryAsset id="block_geometry"><Primitive shape="box" size={{[{size},1,1]}} /></GeometryAsset>
    <MeshAsset id="block" geometry="block_geometry" material="paint" /></Assets>
    <Scene id="room"><Timeline><Track space="3d"><Sequence from="0s" duration="3s" out="hold">
    <CompositeGroup space="3d"><Camera3D id="camera" position="{camera}" target={{[0,0,0]}} />
    <DirectionalLight direction={{[0,-1,-1]}} intensity="2" />
    <Model id="box" asset="block" /></CompositeGroup></Sequence></Track></Timeline></Scene>
    <Present from="room" /></Graph>"##
    )
}
fn options() -> LightingBakeOptions {
    LightingBakeOptions {
        scene_id: "room".into(),
        day_frame: 24,
        dusk_frame: 48,
        volumes: vec![BakeVolumeOptions {
            id: "room".into(),
            bounds_min: [-2., 0.8, -2.],
            bounds_max: [2., 2., 2.],
            counts: [2, 2, 2],
            reflection_positions: vec![[1.5, 1.5, 1.5]],
        }],
        rays_per_probe: 32,
        specular_resolution: [8, 4],
        specular_samples_per_pixel: 1,
        ..Default::default()
    }
}

#[test]
fn room_lowering_retains_eight_lights_before_preview_budgeting() {
    let fixtures = (0..7)
        .map(|i| format!("<PointLight id=\"fixture{i}\" position={{[{i},2,0]}} intensity=\"1\" />"))
        .collect::<Vec<_>>()
        .join("\n");
    let source = fixture_script("[0,2,4]", "#FFFFFF", 1.).replace(
        "<Model id=\"box\"",
        &format!("{fixtures}\n<Model id=\"box\""),
    );
    let graph = crate::parse_graph_script(&source).unwrap();
    let lighting = collect_lighting(&graph, "room", 24).unwrap();
    assert_eq!(lighting.lights.len(), 8);
    assert_eq!(lighting.lights[7].id.as_deref(), Some("fixture6"));
}

#[test]
fn dependency_fingerprint_is_deterministic_and_ignores_camera() {
    let a = crate::parse_graph_script(&fixture_script("[0,2,4]", "#DDAA88", 1.)).unwrap();
    let b = crate::parse_graph_script(&fixture_script("[5,3,4]", "#DDAA88", 1.)).unwrap();
    let c = crate::parse_graph_script(&fixture_script("[0,2,4]", "#88AADD", 1.)).unwrap();
    let d = crate::parse_graph_script(&fixture_script("[0,2,4]", "#DDAA88", 1.2)).unwrap();
    let run = |g: &GraphScript| {
        pollster::block_on(scene_lighting_bake_fingerprint(
            g,
            &options(),
            Arc::new(crate::PathAssetResolver),
        ))
        .unwrap()
    };
    let first = run(&a);
    assert_eq!(first, run(&a));
    assert_eq!(first, run(&b));
    assert_ne!(first.combined, run(&c).combined);
    assert_ne!(first.combined, run(&d).combined);
}

#[test]
fn surface_style_switch_preserves_transport_dependencies_and_source_guard_lineage() {
    let source = fixture_script("[0,2,4]", "#DDAA88", 1.).replace(
        "<Scene id=\"room\">",
        "<RenderStyle id=\"style\"><SurfaceStyle shading=\"physical\" /></RenderStyle><Scene id=\"room\" renderStyle=\"style\">",
    );
    let physical = crate::parse_graph_script(&source).unwrap();
    let toon =
        crate::parse_graph_script(&source.replace("shading=\"physical\"", "shading=\"toon\""))
            .unwrap();
    let dependencies = |graph: &GraphScript| {
        pollster::block_on(scene_lighting_bake_fingerprint(
            graph,
            &options(),
            Arc::new(crate::PathAssetResolver),
        ))
        .unwrap()
    };
    assert_eq!(dependencies(&physical), dependencies(&toon));
    // Published assets retain their existing source guard. A preview must omit
    // a stale asset rather than reinterpret its hash under a new lineage.
    assert_ne!(
        scene_lighting_authoring_fingerprint(&physical).unwrap(),
        scene_lighting_authoring_fingerprint(&toon).unwrap()
    );
    let changed = crate::parse_graph_script(&source.replace("#DDAA88", "#88AADD")).unwrap();
    assert_ne!(dependencies(&physical), dependencies(&changed));
}

#[test]
fn texture_bytes_participate_in_cache_and_hdr_keeps_highlights() {
    let mut mesh = wall([0.7; 3], matte());
    mesh.textures.push(GlbTextureData {
        width: 1,
        height: 1,
        rgba: Arc::new(vec![255, 0, 0, 255]),
    });
    let make = |mesh: ResolvedMesh| Prepared {
        name: "day".into(),
        frame: 0,
        scene: TraceScene::new(vec![mesh], vec![], None, 50.).unwrap(),
        lighting_signature: "light".into(),
        environment_signature: "env".into(),
    };
    let before = fingerprint(&[make(mesh.clone())], &options()).unwrap();
    mesh.textures[0].rgba = Arc::new(vec![0, 255, 0, 255]);
    let after = fingerprint(&[make(mesh)], &options()).unwrap();
    assert_ne!(before.textures, after.textures);
    assert_ne!(before.combined, after.combined);
    let mut encoder = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut encoder)
        .encode(&[image::Rgb([12., 2., 0.5])], 1, 1)
        .unwrap();
    let decoded = image::load_from_memory(&encoder).unwrap().to_rgb32f();
    assert!(decoded.get_pixel(0, 0)[0] > 11.9);
}

#[test]
fn full_api_returns_two_states_with_real_camera_independent_payloads() {
    let graph = crate::parse_graph_script(&fixture_script("[0,2,4]", "#DDAA88", 1.)).unwrap();
    let first = pollster::block_on(bake_scene_lighting(
        &graph,
        &options(),
        Arc::new(crate::PathAssetResolver),
    ))
    .unwrap();
    let second = pollster::block_on(bake_scene_lighting(
        &graph,
        &options(),
        Arc::new(crate::PathAssetResolver),
    ))
    .unwrap();
    assert_eq!(first.asset, second.asset);
    assert_eq!(first.asset.states.len(), 2);
    assert_eq!(first.asset.states[0].volumes[0].probes.len(), 8);
    for probe in &first.asset.states[0].volumes[0].probes {
        assert_eq!(probe.depth_moments.len(), 64);
        assert_eq!(probe.visibility.len(), 64);
        assert!(probe.irradiance_sh.iter().flatten().all(|v| v.is_finite()));
    }
    assert_eq!(first.reflection_images.len(), 2);
    assert_eq!(
        first.reflection_images[0].pixels,
        second.reflection_images[0].pixels
    );
}

#[test]
fn excessive_work_is_rejected_before_extraction() {
    let mut settings = options();
    settings.volumes[0].counts = [32; 3];
    assert!(matches!(
        validate(&settings),
        Err(LightingBakeError::Limit(_))
    ));
    settings = options();
    settings.volumes[0].id = "../outside".into();
    assert!(validate(&settings).is_err());
    settings = options();
    settings.volumes[0].reflection_positions.push([0., 1., 0.]);
    assert!(matches!(
        validate(&settings),
        Err(LightingBakeError::Invalid(_))
    ));
}

#[test]
fn authoring_guard_ignores_camera_and_rejects_changed_bake_source() {
    let a = crate::parse_graph_script(&fixture_script("[0,2,4]", "#DDAA88", 1.)).unwrap();
    let b = crate::parse_graph_script(&fixture_script("[5,3,4]", "#DDAA88", 1.)).unwrap();
    let changed = crate::parse_graph_script(&fixture_script("[0,2,4]", "#88AADD", 1.2)).unwrap();
    let fp = scene_lighting_authoring_fingerprint(&a).unwrap();
    assert_eq!(fp, scene_lighting_authoring_fingerprint(&b).unwrap());
    assert_ne!(fp, scene_lighting_authoring_fingerprint(&changed).unwrap());
    let mut evaluated = a.clone();
    evaluated.material_assets[0].roughness = 0.7;
    assert_eq!(
        fp,
        scene_lighting_authoring_fingerprint(&evaluated).unwrap(),
        "raw authoring source remains authoritative for evaluated clones"
    );
    let bundle = pollster::block_on(bake_scene_lighting(
        &a,
        &options(),
        Arc::new(crate::PathAssetResolver),
    ))
    .unwrap();
    assert_eq!(bundle.asset.authoring_fingerprint, fp);
    pollster::block_on(validate_lighting_bake_fingerprint(
        &b,
        &options(),
        &bundle.asset,
        Arc::new(crate::PathAssetResolver),
    ))
    .unwrap();
    assert!(pollster::block_on(validate_lighting_bake_fingerprint(
        &changed,
        &options(),
        &bundle.asset,
        Arc::new(crate::PathAssetResolver)
    ))
    .is_err());
}
