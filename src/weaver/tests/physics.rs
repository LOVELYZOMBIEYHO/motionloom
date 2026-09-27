// =========================================
// =========================================
// crates/motionloom/src/weaver/tests/physics.rs

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
        packed.triangle_offset as f32,
        packed.material_offset as f32,
        packed.light_offset as f32,
        0.0,
    ];
    p[2] = [0.0, 0.0, 2.0, 0.0];
    p[3] = [0.0, 0.0, -1.0, 0.1];
    p[4] = [1.0, 0.0, 0.0, 1.0];
    p[5] = [0.0, 1.0, 0.0, 0.0];
    p[7] = [512.0, 512.0, 0.0, 0.0];
    p[8] = [4.0, 4.0, 4.0, 4.0];
    p[9][3] = 64.0;
    p[10] = [env as f32, 1.0, 1.0, 0.0];
    p[11] = [1.0, 1.0, 0.0, 0.0];
    p[12] = [0.0, 0.0, 8.0, 8.0];
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
