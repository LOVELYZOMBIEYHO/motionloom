// =========================================
// =========================================
// src/weaver/tests/physics.rs

use crate::experimental::geometry::ResolvedMesh;
use crate::weaver::*;
use crate::world::gltf_loader::{GlbMaterialData, GlbTextureData};
use crate::world::{WorldCamera, WorldLighting};
use std::sync::Arc;

#[test]
fn lens_preserves_focus_and_fov() {
    let mut job = RenderJob::new("scene", QualityPreset::Ultra);
    job.lens_source = LensSource::Job;
    job.scene_id = "scene".into();
    let camera = WorldCamera::default();
    let mut p = [[0.0; 4]; 26];
    super::super::camera::configure(&mut p, &camera, &job).unwrap();
    assert!((p[3][3] - (35f32.to_radians() / 2.0).tan()).abs() < 1e-6);
    let radius = p[5][3];
    job.lens.f_stop *= 2.0;
    super::super::camera::configure(&mut p, &camera, &job).unwrap();
    assert!((p[5][3] * 2.0 - radius).abs() < 1e-8);
    assert_eq!(p[6][0], job.lens.focus_distance);
}

#[test]
fn authored_optics_and_overrides_match_preview_aperture() {
    use crate::world::{WorldDepthOfField, optics::ResolvedCameraOptics};
    let mut job = RenderJob::new("scene", QualityPreset::Ultra);
    let mut camera = WorldCamera::default();
    let mut p = [[0.0; 4]; 26];
    super::super::camera::configure(&mut p, &camera, &job).unwrap();
    assert_eq!(p[5][3], 0.0, "an omitted DSL DOF must stay sharp");
    camera.depth_of_field = Some(WorldDepthOfField {
        focus_distance: "6".into(),
        focal_length_mm: "50".into(),
        f_stop: "8".into(),
        max_blur_px: "10".into(),
        max_blur_percent_height: false,
    });
    let optics = ResolvedCameraOptics::new(6.0, 50.0, 8.0);
    let uniform = optics.preview_uniform(10.0);
    // Sensor dimensions follow projection, so portrait and landscape use one aperture.
    for resolution in [[1920, 1080], [1080, 1920]] {
        job.resolution = resolution;
        super::super::camera::configure(&mut p, &camera, &job).unwrap();
        assert_eq!(p[5][3], uniform[1]);
        assert_eq!(p[6][0], uniform[0]);
    }
    let tangent = p[3][3];
    job.lens_overrides.focus_distance = Some(3.0);
    super::super::camera::configure(&mut p, &camera, &job).unwrap();
    assert_eq!(p[6][0], 3.0);
    assert_eq!(p[5][3], uniform[1], "focus does not override aperture");
    assert_eq!(p[3][3], tangent, "focus does not change framing");
    job.lens_overrides.enabled = Some(false);
    super::super::camera::configure(&mut p, &camera, &job).unwrap();
    assert_eq!(p[5][3], 0.0);
    camera.depth_of_field = None;
    job.lens_overrides.enabled = None;
    super::super::camera::configure(&mut p, &camera, &job).unwrap();
    assert!(p[5][3] > 0.0, "an explicit numeric override enables DOF");
}

#[test]
fn legacy_jobs_keep_explicit_lenses_and_overrides_are_validated() {
    let mut job = RenderJob::new("scene", QualityPreset::Ultra);
    job.scene_id = "auto".into();
    let mut json = serde_json::to_value(&job).unwrap();
    json.as_object_mut().unwrap().remove("lens_source");
    json.as_object_mut().unwrap().remove("lens_overrides");
    let legacy: RenderJob = serde_json::from_value(json).unwrap();
    assert_eq!(legacy.lens_source, LensSource::Job);
    let mut p = [[0.0; 4]; 26];
    super::super::camera::configure(&mut p, &WorldCamera::default(), &legacy).unwrap();
    assert!(p[5][3] > 0.0);
    assert_eq!(p[6][0], legacy.lens.focus_distance);
    for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
        job.lens_overrides.focus_distance = Some(bad);
        assert!(job.validate().is_err());
    }
}

#[test]
fn active_shots_and_animated_optics_reach_weaver_each_frame() {
    // Exercise the real scene bridge, including local sequence time and the cut.
    let graph = crate::parse_graph_script(r#"
<Graph fps={24} duration="3s" size={[64,64]}>
  <Assets>
<MaterialAsset id="geometry_default" shading="pbr" roughness="0.82" specular="1" emissiveStrength="1" />
    <GeometryAsset id="subject_asset_geometry">
    <Primitive shape="sphere" radius="0.2" />
    </GeometryAsset>
    <MeshAsset id="subject_asset" material="geometry_default" geometry="subject_asset_geometry" />
  </Assets>
  <Scene id="focus_scene">
    <Timeline>
      <Track id="optics" space="3d">
        <Sequence from="0s" duration="2s" out="hide">
          <CompositeGroup id="focused" space="3d">
            <Camera3D position={[0,0,curve("0:6:linear, 2:4:linear")]} target={[0,0,0]}
                      depthOfField="true" focalLength="50" fStop={curve("0:8:linear, 2:4:linear")}
                      focusTarget="@subject" focusDistance={curve("0:6:linear, 2:3:linear")} focusOffset="-0.1" />
            <Model id="subject" asset="subject_asset" position={[2,0,1]} />
          </CompositeGroup>
        </Sequence>
        <Sequence from="2s" duration="1s" out="hide">
          <CompositeGroup id="sharp" space="3d">
            <Camera3D position={[0,0,4]} target={[0,0,0]} depthOfField="false" />
            <Model asset="subject_asset" />
          </CompositeGroup>
        </Sequence>
      </Track>
    </Timeline>
  </Scene>
  <Present from="focus_scene" />
</Graph>
"#).unwrap();
    let mut job = RenderJob::new("scene", QualityPreset::Ultra);
    job.scene_id = "focus_scene".into();
    job.resolution = [64, 64];
    for (frame, focus, f_stop) in [(0, 5.9, 8.0), (24, 4.4, 6.0), (48, 4.0, 0.0)] {
        job.frame = frame;
        let snapshot = pollster::block_on(crate::scene::render::weaver_snapshot(
            &graph,
            &job,
            Arc::new(crate::asset::MemoryAssetResolver::default()),
        ))
        .unwrap();
        let mut p = [[0.0; 4]; 26];
        let lens = super::super::camera::configure(&mut p, &snapshot.camera, &job).unwrap();
        assert!((p[6][0] - focus).abs() < 1e-5);
        if f_stop == 0.0 {
            assert_eq!(p[5][3], 0.0);
        } else {
            assert_eq!(lens.optics.f_stop, f_stop);
            assert!((p[5][3] - 0.05 / (2.0 * f_stop)).abs() < 1e-6);
        }
    }
}

#[test]
fn importance_distribution_integrates_to_one() {
    let dir = std::env::temp_dir().join(format!("weaver-env-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("constant.exr");
    image::Rgb32FImage::from_pixel(8, 4, image::Rgb([4.0, 2.0, 1.0]))
        .save(&path)
        .unwrap();
    let mut data = Vec::new();
    let (tex, cdf) = super::super::lighting::environment(&path, &mut data).unwrap();
    // Offsets are raw u32 bit patterns carried through f32.
    assert_eq!(
        data[tex[0].to_bits() as usize][0],
        4.0,
        "HDR values must not clip"
    );
    let mut integral = 0.0;
    for y in 0..4 {
        for x in 0..8 {
            let omega = 2.0 * std::f32::consts::PI / 8.0
                * ((std::f32::consts::PI * y as f32 / 4.0).cos()
                    - (std::f32::consts::PI * (y + 1) as f32 / 4.0).cos());
            integral += data[cdf[0].to_bits() as usize + y * 8 + x][1] * omega;
        }
    }
    assert!((integral - 1.0).abs() < 1e-5);
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
#[ignore = "requires GPU; compares integrated Lambertian energy and batch invariance"]
fn gpu_lambertian_energy_and_batch_invariance() {
    let texture = |rgba| GlbTextureData {
        width: 1,
        height: 1,
        rgba: Arc::new(Vec::from(rgba)),
    };
    let material = GlbMaterialData {
        metallic_factor: 0.0,
        specular_factor: 0.0,
        ..Default::default()
    };
    let mesh = ResolvedMesh {
        name: "lambert".into(),
        positions: vec![
            [-100.0, -100.0, 0.0],
            [100.0, -100.0, 0.0],
            [100.0, 100.0, 0.0],
            [-100.0, 100.0, 0.0],
        ],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        tangents: vec![[1.0, 0.0, 0.0, 1.0]; 4],
        uvs: vec![[0.5, 0.5]; 4],
        colors: vec![[0.5, 0.5, 0.5, 1.0]; 4],
        indices: vec![0, 1, 2, 0, 2, 3],
        material,
        textures: vec![
            texture([255, 255, 255, 255]),
            texture([128, 128, 255, 255]),
            texture([255, 255, 0, 255]),
            texture([255, 255, 255, 255]),
            texture([255, 255, 255, 255]),
        ],
        uv_source: "test".into(),
    };
    let snapshot = super::super::scene::Snapshot {
        meshes: vec![mesh],
        camera: WorldCamera::default(),
        lighting: WorldLighting::default(),
        time_seconds: 0.0,
        diagnostics: vec![],
        composition: crate::scene::compositor::SceneCompositionPlan::new([8, 8], [8, 8]),
        composition_layers: Vec::new(),
        primary_camera_visibility: vec![true],
    };
    let mut packed = super::super::geometry::pack(&snapshot, false, false).unwrap();
    let env = packed.data.len();
    packed.data.push([1.0, 1.0, 1.0, 0.0]);
    let gpu = pollster::block_on(super::super::backend::wgpu::Gpu::new(&packed)).unwrap();
    let mut p = [[0.0f32; 4]; 26];
    p[0] = [8.0, 8.0, 32.0, f32::from_bits(1989)];
    p[1] = [
        f32::from_bits(packed.triangle_offset),
        f32::from_bits(packed.material_offset),
        f32::from_bits(packed.light_offset),
        0.0,
    ];
    p[2] = [0.0, 0.0, 2.0, 0.0];
    p[3] = [0.0, 0.0, -1.0, 0.1];
    p[4] = [1.0, 0.0, 0.0, 1.0];
    p[5] = [0.0, 1.0, 0.0, 0.0];
    p[7] = [512.0, 512.0, 0.0, 0.0];
    p[8] = [4.0, 4.0, 4.0, 4.0];
    p[9][3] = 64.0;
    p[10] = [f32::from_bits(env as u32), 1.0, 1.0, 0.0];
    p[11] = [1.0, 1.0, 1.0, 1.0];
    p[12] = [0.0, 0.0, 8.0, 8.0];
    p[19] = [1.0, 1.0, 1.0, 1.0];
    let tile = gpu.tile(
        64,
        &vec![0; 64 * super::super::backend::wgpu::FILM_BYTES_PER_PIXEL],
    );
    let mut raw = Vec::new();
    for _ in 0..16 {
        raw = gpu.batch(&tile, &p).unwrap();
    }
    let film = super::super::output::floats(&raw);
    let mean = film
        .chunks_exact(super::super::backend::wgpu::FILM_FLOATS_PER_PIXEL)
        .map(|f| f[0] / f[3])
        .sum::<f32>()
        / 64.0;
    assert!(
        (mean - 0.5).abs() < 0.04,
        "Lambertian reflected energy {mean}"
    );
    assert!(
        film.chunks_exact(super::super::backend::wgpu::FILM_FLOATS_PER_PIXEL)
            .all(|f| f[7] == 0.0 && f[3] == 512.0)
    );
    // Splitting a job into smaller dispatches must preserve every sample exactly.
    p[0][2] = 16.0;
    let other = gpu.tile(
        64,
        &vec![0; 64 * super::super::backend::wgpu::FILM_BYTES_PER_PIXEL],
    );
    let mut resumed = Vec::new();
    for _ in 0..32 {
        resumed = gpu.batch(&other, &p).unwrap();
    }
    assert_eq!(raw, resumed);
    // A crop uses the same camera/RNG coordinates as its full-frame pixel.
    p[0][0] = 1.0;
    p[0][1] = 1.0;
    p[12][0] = 3.0;
    p[12][1] = 4.0;
    let cropped = gpu.tile(
        1,
        &vec![0; super::super::backend::wgpu::FILM_BYTES_PER_PIXEL],
    );
    let mut cropped_bytes = Vec::new();
    for _ in 0..32 {
        cropped_bytes = gpu.batch(&cropped, &p).unwrap();
    }
    let index = (4 * 8 + 3) * super::super::backend::wgpu::FILM_BYTES_PER_PIXEL;
    assert_eq!(
        &raw[index..index + super::super::backend::wgpu::FILM_BYTES_PER_PIXEL],
        cropped_bytes.as_slice()
    );
}

#[test]
#[ignore = "requires GPU; validates multi-submit sampling and partial-round checkpoint resume"]
fn bounded_tile_rounds_preserve_samples_and_resume() {
    use std::collections::BTreeMap;
    use std::path::Path;

    fn checkpoints(output: &Path) -> BTreeMap<String, Vec<u8>> {
        std::fs::read_dir(output.join("checkpoints"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "film-v2")
            })
            .map(|path| {
                let name = path.file_name().unwrap().to_string_lossy().into_owned();
                (name, std::fs::read(path).unwrap())
            })
            .collect()
    }

    fn assert_samples(films: &BTreeMap<String, Vec<u8>>, expected: f32) {
        for (name, raw) in films {
            assert_eq!(
                raw.len() % super::super::backend::wgpu::FILM_BYTES_PER_PIXEL,
                0
            );
            let film = super::super::output::floats(raw);
            assert!(!film.is_empty(), "empty checkpoint {name}");
            for pixel in film.chunks_exact(super::super::backend::wgpu::FILM_FLOATS_PER_PIXEL) {
                assert!(
                    pixel.iter().all(|value| value.is_finite()),
                    "nonfinite film in {name}"
                );
                assert_eq!(pixel[7], 0.0, "invalid sample in {name}");
                assert_eq!(pixel[3], expected, "sample accounting in {name}");
            }
        }
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "weaver-tile-rounds-{}-{unique}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("scene.motionloom");
    // Eighteen tiles cross multiple submission groups, including one-pixel edges.
    // This fixture owns its geometry and needs no sibling showcase assets.
    std::fs::write(
        &source,
        r##"<Graph fps={24} duration="1s" size={[1025,129]}>
  <RenderStyle id="tile_physical">
    <SurfaceStyle shading="physical" specular="1" />
    <LightingStyle ambientIntensity="1" ambientColor="#FFFFFF" />
    <PostStyle toneMapping="aces" exposure="1" />
  </RenderStyle>
  <Assets>
    <MaterialAsset id="diffuse" shading="pbr" baseColor="#808080" roughness="1" specular="0" />
    <GeometryAsset id="wall_geometry"><Primitive shape="box" size={[100,100,0.1]} /></GeometryAsset>
    <MeshAsset id="wall" material="diffuse" geometry="wall_geometry" collision="none" />
  </Assets>
  <Scene id="tile_scene" renderStyle="tile_physical">
    <Timeline>
      <Track id="world_track" space="3d">
        <Sequence from="0s" duration="1s">
          <CompositeGroup id="world" space="3d">
            <Camera3D position={[0,0,2]} target={[0,0,0]} fov="20" />
            <DirectionalLight id="sun" direction={[0,0,-1]} color="#FFFFFF" intensity="2" castShadow="true" />
            <Model asset="wall" />
          </CompositeGroup>
        </Sequence>
      </Track>
    </Timeline>
  </Scene>
  <Present from="tile_scene" />
</Graph>"##,
    )
    .unwrap();
    let mut job = RenderJob::new(&source, QualityPreset::Production);
    job.scene_id = "tile_scene".into();
    job.render_style = "tile_physical".into();
    job.resolution = [1025, 129];
    job.seed = 1989;
    job.sampling.min_samples = 8;
    job.sampling.max_samples = 8;
    job.sampling.noise_threshold = 0.0;
    job.sampling.batch_samples = 4;
    job.light_paths.total = 4;
    job.light_paths.diffuse = 2;
    job.light_paths.glossy = 2;
    job.light_paths.transmission = 4;
    job.light_paths.roulette_start = 3;
    job.output = dir.join("batch-four");
    let baseline = pollster::block_on(render(&job, &CancellationToken::default(), |_| {})).unwrap();
    assert_eq!(baseline.status, "sample_limit_reached");
    let baseline_films = checkpoints(&baseline.output);
    let tile_count = job.resolution[0].div_ceil(128) * job.resolution[1].div_ceil(128);
    assert_eq!(baseline_films.len(), tile_count as usize);
    assert_samples(&baseline_films, 8.0);

    // The requested batch changes scheduling, never the per-pixel RNG sequence.
    job.sampling.batch_samples = 8;
    job.output = dir.join("batch-eight");
    let larger_batch =
        pollster::block_on(render(&job, &CancellationToken::default(), |_| {})).unwrap();
    let larger_batch_films = checkpoints(&larger_batch.output);
    assert_samples(&larger_batch_films, 8.0);
    assert_eq!(baseline_films, larger_batch_films);

    job.output = dir.join("partial-round");
    let cancel = CancellationToken::default();
    let mut callbacks = 0;
    let partial = pollster::block_on(render(&job, &cancel, |_| {
        callbacks += 1;
        // Initial progress precedes dispatch; the next callback follows a saved group.
        if callbacks == 2 {
            cancel.cancel();
        }
    }))
    .unwrap();
    assert_eq!(partial.status, "cancelled");
    assert!(!partial.output.join("render.lock").exists());
    let partial_films = checkpoints(&partial.output);
    assert!(
        !partial_films.is_empty(),
        "cancellation lost the completed group"
    );
    assert!(
        partial_films.len() < tile_count as usize,
        "cancellation completed the entire round"
    );
    assert_samples(&partial_films, 4.0);

    let resumed = pollster::block_on(render(&job, &CancellationToken::default(), |_| {})).unwrap();
    assert_eq!(resumed.status, "sample_limit_reached");
    let resumed_films = checkpoints(&resumed.output);
    assert_samples(&resumed_films, 8.0);
    assert_eq!(larger_batch_films, resumed_films);
    // Remove only the successful fixture's uniquely named directory.
    std::fs::remove_dir_all(dir).unwrap();
}
