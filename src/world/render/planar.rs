//! Camera-reflected, single-bounce planar captures. Captures use distinct
//! material bindings and never sample the texture currently being written.
use super::*;

#[cfg(test)]
thread_local! {
    static TEST_AUTOMATIC_GLASS: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) struct TestAutomaticGlass(Option<bool>);

#[cfg(test)]
pub(super) fn test_automatic_glass(enabled: bool) -> TestAutomaticGlass {
    TestAutomaticGlass(TEST_AUTOMATIC_GLASS.with(|setting| setting.replace(Some(enabled))))
}

#[cfg(test)]
impl Drop for TestAutomaticGlass {
    fn drop(&mut self) {
        TEST_AUTOMATIC_GLASS.with(|setting| setting.set(self.0));
    }
}

pub(super) fn automatic_glass_enabled() -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    let enabled = std::env::var_os("MOTIONLOOM_TRACE_PLANAR_GLASS").is_some();
    #[cfg(target_arch = "wasm32")]
    let enabled = false;
    #[cfg(test)]
    let enabled = TEST_AUTOMATIC_GLASS
        .with(|setting| setting.get())
        .unwrap_or(enabled);
    enabled
}

pub(super) struct GpuPlanarResource {
    pub(super) color: wgpu::Texture,
    depth: wgpu::Texture,
    snapshot: wgpu::Texture,
    depth_snapshot: wgpu::Texture,
}

#[cfg(test)]
mod automatic_tests {
    use super::*;

    fn fixture() -> (GpuWorldDraw, GpuWorldLightingParams) {
        let resource_key = GpuWorldResourceKey {
            model_path: PathBuf::from("automatic-planar-test"),
            binding_actor: None,
            draw_key: GpuWorldDrawKey {
                material: None,
                texture: None,
                mesh: None,
                mesh_node: None,
            },
        };
        let color = Arc::new(GpuWorldTexture::new(1, 1, vec![255; 4]));
        let normal = Arc::new(GpuWorldTexture::new(1, 1, vec![128, 128, 255, 255]));
        let vertices = [
            [-2.0, -2.0, 0.0],
            [2.0, -2.0, 0.0],
            [2.0, 2.0, 0.0],
            [-2.0, 2.0, 0.0],
        ]
        .map(|position| GpuWorldVertex {
            position,
            normal: [0.0, 0.0, 1.0],
            outline_normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
            joints: [0.0; 4],
            weights: [0.0; 4],
            uv: [0.0; 2],
            color: [1.0; 4],
        })
        .to_vec();
        let draw = GpuWorldDraw {
            instance_key: GpuWorldInstanceKey {
                actor_id: "flat-face".into(),
                resource_key: resource_key.clone(),
            },
            resource_key,
            vertices: Arc::new(vertices),
            indices: Arc::new(vec![0, 1, 2, 0, 2, 3]),
            vertex_signature: 0,
            texture: color.clone(),
            normal_texture: normal,
            metallic_roughness_texture: color.clone(),
            emissive_texture: color.clone(),
            occlusion_texture: color.clone(),
            cel_texture: color,
            bone_matrices: Vec::new(),
            params: GpuWorldParams {
                canvas: [1920.0, 1080.0, 960.0, 540.0],
                model: [0.0, 0.0, 0.0, 1.0],
                actor_rotation: [0.0, 0.0, 0.0, 1.0],
                camera0: [0.0, 0.0, 4.0, 900.0],
                camera1: [1.0, 0.0, 0.0, 0.1],
                camera2: [0.0, 1.0, 0.0, 100.0],
                camera3: [0.0, 0.0, -1.0, 0.0],
                style: [1.0; 4],
                material0: [0.0, 0.08, 0.18, 1.0],
                material4: [1.0; 4],
                ..Default::default()
            },
            phase: GpuWorldDrawPhase::Opaque,
            depth_write: true,
            sort_priority: 0,
            camera_depth: 4.0,
        };
        let camera = PerspectiveCameraView {
            orthographic: false,
            eye: [0.0, 0.0, 4.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            focal_px: 900.0,
            near: 0.1,
            far: 100.0,
            aspect: 16.0 / 9.0,
            optics: [0.0; 4],
        };
        let mut lighting =
            GpuWorldLightingParams::from_world(&WorldLighting::default(), camera, false, 1);
        lighting.surface0[0] = 0.0;
        lighting.surface1[2] = 0.0;
        lighting.reflection0 = [2.0, 3.0, 0.0, 1.0];
        (draw, lighting)
    }

    #[test]
    fn automatic_capture_uses_the_visible_transformed_face_and_bounds_resolution() {
        let (mut draw, lighting) = fixture();
        draw.params.model = [0.5, -0.25, 0.07, 2.0];
        draw.params.actor = [1.0, 2.0, 3.0, 0.0];
        draw.params.camera0 = [1.0, 2.0, 7.0, 900.0];
        let (area, view) = automatic_view_for_target(&draw, &lighting, 1920, 1080, 1024).unwrap();
        assert!(area >= 4096.0);
        assert_eq!([view.width, view.height], [480, 270]);
        assert!(view.params.control[3] > 0.0);
        assert!((view.params.plane[3] + 2.86).abs() < 0.00001);
        assert!((view.params.camera0[2] + 1.28).abs() < 0.00001);
        let (_, small) = automatic_view_for_target(&draw, &lighting, 1920, 1080, 256).unwrap();
        assert!(small.width <= 256 && small.height <= 256);
    }

    #[test]
    fn automatic_material_proof_honors_remapped_sharp_regions_and_normal_extrema() {
        let (mut draw, lighting) = fixture();
        draw.params.material0[1] = 1.0;
        draw.params.material8[2] = 1.0 * 16.0;
        draw.metallic_roughness_texture = Arc::new(GpuWorldTexture::new(
            2,
            1,
            vec![255, 20, 255, 255, 255, 180, 255, 255],
        ));
        assert!(automatic_material_eligible(&draw, &lighting));
        draw.params.material8[2] = 9.0 * 16.0;
        assert!(!automatic_material_eligible(&draw, &lighting));
        draw.params.material8[2] = 1.0 * 16.0;
        draw.normal_texture = Arc::new(GpuWorldTexture::new(
            2,
            1,
            vec![128, 128, 255, 255, 130, 130, 255, 255],
        ));
        assert!(automatic_material_eligible(&draw, &lighting));
        draw.normal_texture = Arc::new(GpuWorldTexture::new(
            2,
            1,
            vec![128, 128, 255, 255, 0, 128, 255, 255],
        ));
        assert!(!automatic_material_eligible(&draw, &lighting));
    }

    #[test]
    fn automatic_capture_rejects_small_hidden_deformed_and_incompatible_surfaces() {
        let (draw, lighting) = fixture();
        for condition in 0..7 {
            let mut altered = draw.clone();
            match condition {
                0 => altered.params.material10[0] = 0.1,
                1 => altered.params.material6[0] = 0.9,
                2 => altered.params.material11[0] = 1.0,
                3 => altered.params.vegetation[0] = 1.0,
                4 => altered.params.material4[3] = 0.5,
                5 => altered.params.material2[3] = 1.0,
                _ => altered.params.actor[2] = 10.0,
            }
            assert!(
                automatic_view_for_target(&altered, &lighting, 1920, 1080, 1024).is_none(),
                "condition {condition}"
            );
        }
        let mut small = draw.clone();
        small.params.model[3] = 0.01;
        assert!(automatic_view_for_target(&small, &lighting, 1920, 1080, 1024).is_none());
        let mut curved = draw;
        Arc::make_mut(&mut curved.vertices)[0].normal = [0.4, 0.0, 0.9165];
        Arc::make_mut(&mut curved.vertices)[2].normal = [0.4, 0.0, 0.9165];
        assert!(automatic_view_for_target(&curved, &lighting, 1920, 1080, 1024).is_none());
    }

    fn slab_fixture(actor: &str, x: f32) -> (GpuWorldDraw, GpuWorldLightingParams) {
        let (mut draw, lighting) = fixture();
        let template = draw.vertices[0];
        let corners = [
            [-2.0, -2.0, 0.0],
            [2.0, -2.0, 0.0],
            [2.0, 2.0, 0.0],
            [-2.0, 2.0, 0.0],
            [-2.0, -2.0, -0.012],
            [2.0, -2.0, -0.012],
            [2.0, 2.0, -0.012],
            [-2.0, 2.0, -0.012],
        ];
        let faces = [
            ([0, 1, 2, 3], [0.0, 0.0, 1.0]),
            ([7, 6, 5, 4], [0.0, 0.0, -1.0]),
            ([4, 5, 1, 0], [0.0, -1.0, 0.0]),
            ([3, 2, 6, 7], [0.0, 1.0, 0.0]),
            ([4, 0, 3, 7], [-1.0, 0.0, 0.0]),
            ([1, 5, 6, 2], [1.0, 0.0, 0.0]),
        ];
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for (positions, normal) in faces {
            let base = vertices.len() as u32;
            let tangent = if normal[0] != 0.0 {
                [0.0, 1.0, 0.0]
            } else {
                [1.0, 0.0, 0.0]
            };
            for index in positions {
                vertices.push(GpuWorldVertex {
                    position: corners[index],
                    normal,
                    outline_normal: normal,
                    tangent,
                    bitangent: cross3(normal, tangent),
                    ..template
                });
            }
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        draw.vertices = Arc::new(vertices);
        draw.indices = Arc::new(indices);
        draw.instance_key.actor_id = actor.to_string();
        draw.params.actor[0] = x;
        draw.params.material6 = [0.98, 1.52, 0.012, 8.0];
        draw.phase = GpuWorldDrawPhase::Transmissive;
        draw.depth_write = false;
        (draw, lighting)
    }

    #[test]
    fn automatic_slab_capture_accepts_closed_entry_face_and_rejects_unsafe_materials() {
        let (draw, lighting) = slab_fixture("pane", 0.0);
        assert!(closed_mesh(&draw.vertices, &draw.indices));
        let (_, view) = automatic_view_for_target(&draw, &lighting, 1920, 1080, 1024).unwrap();
        assert!(view.matches_target("pane"));
        assert_eq!(view.members.len(), 1);
        assert!(view.params.control[3] > 0.0);
        assert!(view.params.plane[3].abs() < 0.000001);
        for condition in 0..12 {
            let mut altered = draw.clone();
            match condition {
                0 => altered.params.material11[0] = 1.0,
                1 => altered.params.material10[0] = 0.1,
                2 => {
                    altered.texture = Arc::new(GpuWorldTexture::new(1, 1, vec![255, 255, 255, 128]))
                }
                3 => Arc::make_mut(&mut altered.vertices)[0].color[3] = 0.8,
                4 => {
                    let mut bone = mat4_identity();
                    bone[12] = 0.1;
                    altered.bone_matrices.push(bone);
                }
                5 => altered.params.vegetation[0] = 0.2,
                6 => altered.params.material0[1] = 0.3,
                7 => altered.phase = GpuWorldDrawPhase::AlphaBlend,
                8 => altered.params.material1[3] = 0.4,
                9 => Arc::make_mut(&mut altered.vertices)[0].normal[0] = f32::NAN,
                10 => altered.params.material7[3] = 0.5,
                _ => altered.params.model[3] = f32::NAN,
            }
            assert!(
                automatic_view_for_target(&altered, &lighting, 1920, 1080, 1024).is_none(),
                "condition {condition}"
            );
        }
    }

    #[test]
    fn certified_slab_runs_cover_original_indices_once_with_only_front_face_cached() {
        let (mut draw,lighting) = slab_fixture("certified-pane",0.0);
        draw.params.style[3] = 0.0;
        draw.params.material0[2] = 0.0;
        let (_,view) = automatic_view_for_target(&draw,&lighting,1920,1080,1024).unwrap();
        let runs = certified_slab_face_runs(&draw,&view,&lighting);
        let mut cursor = 0;
        for run in &runs {
            assert_eq!(run.first_index,cursor,"runs must preserve original index ordering");
            assert_eq!(run.index_count%3,0);
            assert!(run.index_count>0);
            cursor += run.index_count;
        }
        assert_eq!(cursor,draw.indices.len() as u32);
        assert_eq!(runs.iter().filter(|run| run.cached).map(|run| run.index_count).sum::<u32>(),6);
        assert_eq!(runs.iter().filter(|run| run.discarded).map(|run| run.index_count).sum::<u32>(),30);
        assert!(runs.iter().all(|run| run.cached || run.discarded));
        assert_eq!(runs,certified_slab_face_runs(&draw,&view,&lighting));
    }

    #[test]
    fn certified_slab_runs_reject_uncertain_material_frames_planes_and_projections() {
        let (mut draw,lighting) = slab_fixture("certified-pane",0.0);
        draw.params.style[3] = 0.0;
        draw.params.material0[2] = 0.0;
        let (_,view) = automatic_view_for_target(&draw,&lighting,1920,1080,1024).unwrap();
        for condition in 0..7 {
            let mut altered = draw.clone();
            let mut capture = PlanarView {
                target: view.target.clone(),members: view.members.clone(),
                width: view.width,height: view.height,params: view.params,
            };
            match condition {
                0 => {
                    altered.params.material0[1] = 1.0;
                    altered.params.material8[2] = 16.0;
                    altered.metallic_roughness_texture = Arc::new(GpuWorldTexture::new(2,1,
                        vec![255,20,255,255,255,180,255,255]));
                    assert!(automatic_material_eligible(&altered,&lighting),"candidate minimum should still qualify");
                }
                1 => {
                    altered.params.material0[2] = 0.18;
                    altered.normal_texture = Arc::new(GpuWorldTexture::new(2,1,
                        vec![128,128,255,255,130,130,255,255]));
                    assert!(automatic_material_eligible(&altered,&lighting),"0.3° candidate cone should still qualify");
                }
                2 => altered.params.actor_rotation = [0.0,0.0,0.001,1.0],
                3 => capture.params.camera0[3] *= 100.0,
                4 => capture.params.plane[3] += capture.params.control[3]*0.75,
                5 => altered.params.hidden0[0] = 1.0,
                _ => altered.params.style[3] = 1.0,
            }
            let runs = certified_slab_face_runs(&altered,&capture,&lighting);
            assert!(runs.iter().all(|run| !run.cached),"unsafe proof condition {condition} cached a face");
            assert!(runs.iter().all(|run| !run.discarded),"unsafe proof condition {condition} omitted a face");
            assert_eq!(runs.iter().map(|run| run.index_count).sum::<u32>(),draw.indices.len() as u32);
        }
    }

    fn cancellation_slab_fixture() -> (GpuWorldDraw,GpuWorldLightingParams) {
        let (mut draw,lighting) = slab_fixture("cancellation-pane",0.0);
        for vertex in Arc::make_mut(&mut draw.vertices) {
            vertex.position[0] = if vertex.position[0] < 0.0 { -90_909_000.0 } else { -90_908_920.0 };
            vertex.position[1] *= 50.0;
        }
        draw.params.style[3] = 0.0;
        draw.params.material0[2] = 0.0;
        draw.params.model[3] = 1.1;
        draw.params.actor[0] = 100_000_000.0;
        draw.params.camera0 = [97.0,0.0,1000.0,900.0];
        draw.params.camera2[3] = 2000.0;
        (draw,lighting)
    }

    #[test]
    fn certified_slab_backface_proof_retains_faces_when_fma_can_reverse_facing() {
        let (draw,lighting) = cancellation_slab_fixture();
        let local = draw.vertices[16].position[0];
        let separate = local*draw.params.model[3]+draw.params.actor[0];
        let fused = local.mul_add(draw.params.model[3],draw.params.actor[0]);
        assert_eq!(separate,96.0);
        assert!(separate < draw.params.camera0[0] && fused > draw.params.camera0[0],
            "permitted FMA changes which side of the left face contains the camera");
        let (_,view) = automatic_view_for_target(&draw,&lighting,1920,1080,1024).unwrap();
        let runs = certified_slab_face_runs(&draw,&view,&lighting);
        assert!(runs.iter().any(|run| run.cached),"ordinary front projection remains certifiable");
        for index in [24,27] {
            let run = runs.iter().find(|run| (run.first_index..run.first_index+run.index_count).contains(&index)).unwrap();
            assert!(!run.cached && !run.discarded,"uncertain left triangles need full shading");
        }
    }

    #[test]
    fn certified_slab_projection_proof_reserves_non_plane_axis_fma_error() {
        let (mut draw,lighting) = cancellation_slab_fixture();
        draw.params.camera0[2] = 500.0;
        let (_,mut view) = automatic_view_for_target(&draw,&lighting,1920,1080,1024).unwrap();
        let local = draw.vertices[1].position[0];
        let separate = local*draw.params.model[3]+draw.params.actor[0];
        let fused = local.mul_add(draw.params.model[3],draw.params.actor[0]);
        let size = f64::from(view.width);
        let focal = f64::from(view.params.camera0[3]);
        let depth = f64::from(draw.params.camera0[2]);
        let margin = 0.5/size;
        let interior_uv = 1.0-margin-0.0001;
        view.params.camera0[0] = (f64::from(separate)-(interior_uv-0.5)*depth*size/focal) as f32;
        let project = |world: f32| 0.5+(f64::from(world)-f64::from(view.params.camera0[0]))*focal/(depth*size);
        assert!(project(separate) > margin && project(separate) < 1.0-margin);
        assert!(project(fused) > 1.0,"the same permitted transform crosses the capture boundary");
        let runs = certified_slab_face_runs(&draw,&view,&lighting);
        assert!(runs.iter().all(|run| !run.cached && !run.discarded),
            "an uncertain capture must retain the original complete draw");
        assert_eq!(runs.iter().map(|run| run.index_count).sum::<u32>(),draw.indices.len() as u32);
    }

    #[test]
    fn certified_slab_orthographic_front_uses_ray_direction_instead_of_eye_position() {
        let (mut draw,lighting) = fixture();
        draw.phase = GpuWorldDrawPhase::Transmissive;
        draw.depth_write = false;
        draw.params.style[3] = 0.0;
        draw.params.material0[2] = 0.0;
        draw.params.material6 = [0.98,1.52,0.012,8.0];
        draw.params.material8[1] = 1.0;
        draw.params.camera0 = [-10.0,0.0,4.0,50.0];
        draw.params.camera3[3] = 1.0;
        assert!(!closed_mesh(&draw.vertices,&draw.indices));
        for (forward_z,expected_cached) in [(0.6_f32,false),(-0.6,true)] {
            draw.params.camera1 = [0.6,0.0,-forward_z.signum()*0.8,0.1];
            draw.params.camera3 = [0.8,0.0,forward_z,1.0];
            let (_,view) = automatic_view_for_target(&draw,&lighting,1920,1080,1024).unwrap();
            assert_eq!(view.params.plane,[0.0,0.0,1.0,-0.0]);
            let runs = certified_slab_face_runs(&draw,&view,&lighting);
            assert_eq!(runs.iter().any(|run| run.cached),expected_cached,
                "open double-sided pane must follow the shader's orthographic view");
            assert!(runs.iter().all(|run| !run.discarded));
        }
    }

    #[test]
    fn coplanar_slab_group_spends_one_slot_and_unions_all_lookup_regions() {
        let (a, lighting) = slab_fixture("pane-a", -1.6);
        let (b, _) = slab_fixture("pane-b", 1.6);
        let (priority_a, view_a) =
            automatic_view_for_target(&a, &lighting, 1920, 1080, 1024).unwrap();
        let (priority_b, view_b) =
            automatic_view_for_target(&b, &lighting, 1920, 1080, 1024).unwrap();
        let draws = [a, b];
        let groups =
            group_automatic_views(vec![(priority_b, view_b), (priority_a, view_a)], &draws);
        assert_eq!(groups.len(), 1);
        let (priority, group) = &groups[0];
        assert_eq!(group.target, "pane-a");
        assert_eq!(group.members, ["pane-a", "pane-b"]);
        assert!((*priority - priority_a - priority_b).abs() < 0.1);
        assert!(
            draws
                .iter()
                .all(|draw| group.matches_target(&draw.instance_key.actor_id))
        );
        assert!(!group.matches_target("pane-a::different-draw"));
        let regions = draws.map(|draw| {
            draw_screen_bounds(
                &draw,
                reflected_draw_params(&draw, group, [1920, 1080], [0.0; 2]),
                [group.width, group.height],
            )
        });
        let union = regions[0].union(regions[1]);
        assert_ne!(union, regions[0]);
        assert_ne!(union, regions[1]);
        assert!(capture_clip(union, [group.width, group.height], 10).is_some());
    }

    #[test]
    fn automatic_group_rejects_parallel_offset_planes_and_different_cameras() {
        let (a, lighting) = slab_fixture("pane-a", -1.6);
        let (b, _) = slab_fixture("pane-b", 1.6);
        for condition in 0..3 {
            let mut altered = b.clone();
            match condition {
                0 => altered.params.actor[2] = -0.02,
                1 => altered.params.camera0[0] += 0.1,
                _ => altered.params.canvas[2] += 1.0,
            }
            let view_a = automatic_view_for_target(&a, &lighting, 1920, 1080, 1024).unwrap();
            let view_b = automatic_view_for_target(&altered, &lighting, 1920, 1080, 1024).unwrap();
            let groups = group_automatic_views(vec![view_a, view_b], &[a.clone(), altered]);
            assert_eq!(groups.len(), 2, "condition {condition}");
        }
        let (_, mut authored) = automatic_view_for_target(&a, &lighting, 1920, 1080, 1024).unwrap();
        authored.params.control[3] = 0.0;
        let (_, candidate) = automatic_view_for_target(&b, &lighting, 1920, 1080, 1024).unwrap();
        assert!(!automatic_views_coplanar(&authored, &candidate, &[a, b]));
    }

    #[test]
    fn capture_crop_preserves_reflected_lookup_pixels_at_reduced_odd_resolution() {
        let (mut draw, _) = fixture();
        draw.params.canvas = [1921.0, 1081.0, 960.5, 540.5];
        draw.params.model = [0.2, -0.1, 0.0, 0.6];
        let view = view_for_target(&draw, 0.25, 0.001, 1921, 1081, 1024).unwrap();
        let params = reflected_draw_params(&draw, &view, [1921, 1081], [0.5, -0.5]);
        let viewport = [view.width, view.height];
        let bounds = draw_screen_bounds(&draw, params, viewport);
        let padding = ((u64::from(view.width.max(view.height)) * 12).div_ceil(1000) + 4) as u32;
        let clip = capture_clip(bounds, viewport, padding).unwrap();
        assert!(
            u64::from(clip[2]) * u64::from(clip[3])
                < u64::from(view.width) * u64::from(view.height) / 2
        );
        for vertex in draw.vertices.iter() {
            let world = std::array::from_fn::<_, 3, _>(|axis| {
                (vertex.position[axis] - params.model[axis]) * params.model[3]
            });
            let relative =
                std::array::from_fn::<_, 3, _>(|axis| world[axis] - params.camera0[axis]);
            let dot =
                |basis: [f32; 4]| (0..3).map(|axis| relative[axis] * basis[axis]).sum::<f32>();
            let depth = dot(params.camera3);
            let projected = [
                params.canvas[2] + dot(params.camera1) * params.camera0[3] / depth,
                params.canvas[3] - dot(params.camera2) * params.camera0[3] / depth,
            ];
            for jitter in [-0.5, 0.5] {
                assert!(
                    projected[0] + jitter >= clip[0] as f32
                        && projected[0] + jitter < (clip[0] + clip[2]) as f32
                );
                assert!(
                    projected[1] + jitter >= clip[1] as f32
                        && projected[1] + jitter < (clip[1] + clip[3]) as f32
                );
            }
        }
    }

    #[test]
    fn capture_crop_clamps_halo_and_tiles_cover_each_pixel_once() {
        let clip = capture_clip(
            transport_tiles::ScreenBounds::Rect([3, 4, 38, 70]),
            [80, 100],
            14,
        )
        .unwrap();
        assert_eq!(clip, [0, 0, 52, 84]);
        let clip = [95, 100, 170, 91];
        let tiles = capture_tiles(clip, true);
        assert_eq!(tiles.len(), 6);
        assert_eq!(
            tiles.iter().map(|tile| tile[2] * tile[3]).sum::<u32>(),
            clip[2] * clip[3]
        );
        for y in clip[1]..clip[1] + clip[3] {
            for x in clip[0]..clip[0] + clip[2] {
                assert_eq!(
                    tiles
                        .iter()
                        .filter(|tile| x >= tile[0]
                            && x < tile[0] + tile[2]
                            && y >= tile[1]
                            && y < tile[1] + tile[3])
                        .count(),
                    1
                );
            }
        }
        assert_eq!(capture_tiles(clip, false), vec![clip]);
    }

    #[test]
    fn capture_crop_uses_reflected_lookup_bounds_for_thick_authored_targets() {
        let (mut draw, _) = fixture();
        let original = draw.vertices.as_ref().clone();
        let mut vertices = original.clone();
        for vertex in &mut vertices {
            vertex.position[2] = 0.7;
        }
        vertices.extend(original);
        draw.vertices = Arc::new(vertices);
        draw.indices = Arc::new(vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7]);
        let view = view_for_target(&draw, 0.25, 0.001, 1920, 1080, 1024).unwrap();
        let params = reflected_draw_params(&draw, &view, [1920, 1080], [0.0, 0.0]);
        let clip = capture_clip(
            draw_screen_bounds(&draw, params, [view.width, view.height]),
            [view.width, view.height],
            10,
        )
        .unwrap();
        // A legacy viewer-facing finite-thickness back surface also uses its
        // actual reflected projection, rather than the fitted plane's image.
        let depth = -params.camera0[2];
        let lookup_x = params.canvas[2] + 2.0 * params.camera0[3] / depth;
        assert!(lookup_x < (clip[0] + clip[2]) as f32);
        let transport_tiles::ScreenBounds::Rect(main) =
            draw_screen_bounds(&draw, draw.params, [1920, 1080])
        else {
            panic!("fixture needs bounded main coverage");
        };
        assert!(lookup_x > main[2] as f32 * view.width as f32 / 1920.0 + 10.0);
    }

    #[test]
    fn target_union_and_uncertain_projection_retain_the_capture_viewport() {
        use transport_tiles::ScreenBounds;
        let (draw, _) = fixture();
        let mut hidden = draw.clone();
        hidden.params.actor[0] = 100.0;
        assert_eq!(
            main_target_bounds(&[hidden.clone()], &draw.instance_key.actor_id, [1920, 1080]),
            ScreenBounds::Empty
        );
        assert!(matches!(
            main_target_bounds(
                &[hidden, draw.clone()],
                &draw.instance_key.actor_id,
                [1920, 1080]
            ),
            ScreenBounds::Rect(_)
        ));
        let mut wind = draw.clone();
        wind.params.vegetation[0] = 1.0;
        assert_eq!(
            main_target_bounds(&[wind], &draw.instance_key.actor_id, [1920, 1080]),
            ScreenBounds::Full
        );
        let mut deformed = draw.clone();
        let mut bone = mat4_identity();
        bone[12] = 0.01;
        deformed.bone_matrices.push(bone);
        assert_eq!(
            draw_screen_bounds(&deformed, deformed.params, [1920, 1080]),
            ScreenBounds::Full
        );
        let mut near = draw;
        near.params.camera0[2] = 0.0;
        Arc::make_mut(&mut near.vertices)[0].position[2] = -0.2;
        Arc::make_mut(&mut near.vertices)[2].position[2] = 0.2;
        assert_eq!(
            draw_screen_bounds(&near, near.params, [1920, 1080]),
            ScreenBounds::Full
        );
        assert_eq!(
            capture_clip(ScreenBounds::Full, [480, 270], 10),
            Some([0, 0, 480, 270])
        );
        assert_eq!(capture_clip(ScreenBounds::Empty, [480, 270], 10), None);
    }

    #[test]
    fn thin_slab_snapshot_padding_bounds_oblique_refraction_without_full_capture() {
        let (mut draw, _) = fixture();
        draw.params.material6 = [1.0, 1.52, 0.012, 1.0];
        draw.phase = GpuWorldDrawPhase::Transmissive;
        draw.normal_texture = Arc::new(GpuWorldTexture::new(
            2,
            1,
            vec![100, 120, 255, 255, 140, 150, 255, 255],
        ));
        let view = view_for_target(&draw, 0.25, 0.001, 1920, 1080, 1024).unwrap();
        let params = reflected_draw_params(&draw, &view, [1920, 1080], [0.0, 0.0]);
        let viewport = [view.width, view.height];
        let padding = slab_snapshot_padding(&draw, params, viewport, [0.0, 0.0]).unwrap();
        assert!(padding < 20);
        let normalize = |value: [f64; 3]| {
            let length = value.iter().map(|v| v * v).sum::<f64>().sqrt();
            value.map(|v| v / length)
        };
        let blur_padding = (f64::from(view.width.max(view.height)) * 0.006).ceil() + 4.0;
        for x in -2..=2 {
            for y in -2..=2 {
                let point = [f64::from(x), f64::from(y), 0.0];
                let incident = normalize([point[0], point[1], 4.0]);
                for normal_xy in [[100.0, 120.0], [140.0, 150.0]] {
                    let normal = normalize([
                        (normal_xy[0] / 127.5 - 1.0) * f64::from(params.material0[2]),
                        (normal_xy[1] / 127.5 - 1.0) * f64::from(params.material0[2]),
                        -1.0,
                    ]);
                    let eta = 1.0 / f64::from(params.material6[1]);
                    let cosine = (0..3)
                        .map(|axis| incident[axis] * normal[axis])
                        .sum::<f64>();
                    let k = 1.0 - eta * eta * (1.0 - cosine * cosine);
                    let inside = std::array::from_fn::<_, 3, _>(|axis| {
                        eta * incident[axis] - (eta * cosine + k.sqrt()) * normal[axis]
                    });
                    let distance = f64::from(params.material6[2]) / inside[2].max(0.05);
                    let exit = std::array::from_fn::<_, 3, _>(|axis| {
                        point[axis] + inside[axis] * distance
                    });
                    for scene_depth in [4.1, 10.0, 100.0] {
                        let behind =
                            (scene_depth - (exit[2] + 4.0)).max(0.0) / incident[2].max(0.05);
                        let endpoint = std::array::from_fn::<_, 3, _>(|axis| {
                            exit[axis] + incident[axis] * behind
                        });
                        for axis in 0..2 {
                            let displacement = ((endpoint[axis] / (endpoint[2] + 4.0)
                                - point[axis] / 4.0)
                                * f64::from(params.camera0[3]))
                            .abs();
                            assert!(displacement + blur_padding <= f64::from(padding));
                        }
                    }
                }
            }
        }
        draw.params.material6[2] = 400.0;
        let params = reflected_draw_params(&draw, &view, [1920, 1080], [0.0, 0.0]);
        assert_eq!(
            slab_snapshot_padding(&draw, params, viewport, [0.0, 0.0]),
            None
        );
        draw.params.material6[2] = 0.012;
        draw.params.vegetation[0] = 1.0;
        let params = reflected_draw_params(&draw, &view, [1920, 1080], [0.0, 0.0]);
        assert_eq!(
            slab_snapshot_padding(&draw, params, viewport, [0.0, 0.0]),
            None
        );
    }
    fn near_crossing_slab() -> GpuWorldDraw {
        let (mut draw, _) = fixture();
        draw.params.camera0 = [0.0, 0.0, 0.0, 900.0];
        draw.params.camera3 = [0.0, 0.0, 1.0, 0.0];
        draw.params.material6 = [0.98, 1.52, 0.012, 8.0];
        let template = draw.vertices[0];
        draw.vertices = Arc::new(
            (0..8)
                .map(|corner| GpuWorldVertex {
                    position: [
                        if corner & 1 == 0 { 2.0 } else { 4.0 },
                        if corner & 2 == 0 { -2.0 } else { 2.0 },
                        if corner & 4 == 0 { 0.05 } else { 5.0 },
                    ],
                    ..template
                })
                .collect(),
        );
        draw
    }

    #[test]
    fn offscreen_near_crossing_slab_has_no_capture_dependency() {
        let draw = near_crossing_slab();
        assert_eq!(
            draw_screen_bounds(&draw, draw.params, [480, 270]),
            transport_tiles::ScreenBounds::Full
        );
        assert_eq!(
            clipped_slab_snapshot_padding(
                &draw,
                draw.params,
                [480, 270],
                [0.0; 2],
                [0.0, 0.0, 1.0, 0.0],
                0.0,
                [100, 100, 50, 40]
            ),
            Some(0)
        );
        let padding = clipped_slab_snapshot_padding(
            &draw,
            draw.params,
            [480, 270],
            [0.0; 2],
            [0.0, 0.0, 1.0, 0.0],
            0.0,
            [340, 100, 40, 40],
        )
        .unwrap();
        assert!(padding > 0 && padding < 20);
    }

    #[test]
    fn clipped_capture_volume_keeps_new_caps_when_camera_is_inside_box() {
        let mut draw = near_crossing_slab();
        for (corner, vertex) in Arc::make_mut(&mut draw.vertices).iter_mut().enumerate() {
            vertex.position = std::array::from_fn(|axis| {
                if corner & (1 << axis) == 0 {
                    -10.0
                } else {
                    10.0
                }
            });
        }
        let points = clipped_capture_box(
            &draw,
            draw.params,
            [480, 270],
            [0.0, 0.0, 1.0, 0.0],
            0.0,
            Some([230, 120, 20, 20]),
        )
        .unwrap();
        assert!(!points.is_empty());
        let minimum = points.iter().map(|p| p[5]).fold(f64::INFINITY, f64::min);
        assert!((minimum - f64::from(draw.params.camera1[3])).abs() < 1e-8);
        assert_eq!(
            clipped_slab_snapshot_padding(
                &draw,
                draw.params,
                [480, 270],
                [0.0; 2],
                [0.0, 0.0, 1.0, 0.0],
                0.0,
                [230, 120, 20, 20]
            ),
            None
        );
    }
}

#[derive(Clone, Copy, Default)]
pub(super) struct PlanarParams {
    pub(super) camera0: [f32; 4],
    pub(super) camera1: [f32; 4],
    pub(super) camera2: [f32; 4],
    pub(super) camera3: [f32; 4],
    pub(super) plane: [f32; 4],
    pub(super) control: [f32; 4],
}
impl PlanarParams {
    pub(super) fn bytes(self) -> Vec<u8> {
        [
            self.camera0,
            self.camera1,
            self.camera2,
            self.camera3,
            self.plane,
            self.control,
        ]
        .into_iter()
        .flatten()
        .flat_map(f32::to_ne_bytes)
        .collect()
    }
}

pub(super) struct PlanarView {
    pub(super) target: String,
    // The canonical target owns one resource. Proven coplanar automatic faces
    // share that resource and are all excluded from their reflected camera.
    pub(super) members: Vec<String>,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) params: PlanarParams,
}

impl PlanarView {
    pub(super) fn matches_target(&self, actor: &str) -> bool {
        self.members.iter().any(|member| member == actor)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CertifiedSlabRun {
    pub(super) first_index: u32,
    pub(super) index_count: u32,
    pub(super) cached: bool,
    pub(super) discarded: bool,
}

/// Partition original primitive order, never regroup front/back faces. Only
/// constant axis-aligned frames are certified: their normalized interpolants
/// cannot tilt or cancel inside a triangle. Uncertain faces retain full shading.
pub(super) fn certified_slab_face_runs(
    draw: &GpuWorldDraw,
    view: &PlanarView,
    lighting: &GpuWorldLightingParams,
) -> Vec<CertifiedSlabRun> {
    let full = || vec![CertifiedSlabRun {
        first_index: 0, index_count: draw.indices.len() as u32, cached: false, discarded: false,
    }];
    let p = draw.params;
    let axis_vector = |v: [f32; 3]| v.iter().filter(|&&c| c != 0.0).count() == 1
        && v.iter().all(|&c| c == 0.0 || c.abs() == 1.0);
    let plane_normal = [view.params.plane[0],view.params.plane[1],view.params.plane[2]];
    if !automatic_material_eligible(draw,lighting)
        || draw.phase != GpuWorldDrawPhase::Transmissive
        || view.params.control[3] <= 0.0
        || view.params.control[0] <= 0.5 || view.params.control[1] > 0.5
        || p.canvas[0] <= 0.0 || p.canvas[1] <= 0.0
        || view.width == 0 || view.height == 0
        || p.actor_rotation != [0.0,0.0,0.0,1.0]
        || !axis_vector(plane_normal)
        || p.style[3] != 0.0
        || p.material4[3] != 1.0 || p.style[0] != 1.0
        || [p.hidden0,p.hidden1,p.hidden2,p.hidden3,p.hidden4,p.hidden5,p.hidden6,p.hidden7]
            .into_iter().flatten().any(|v| v != 0.0)
        || draw.indices.len() > u32::MAX as usize || draw.indices.len() % 3 != 0
        || draw.vertices.iter().any(|v| {
            v.position.into_iter().chain(v.normal).chain(v.tangent).chain(v.bitangent)
                .chain(v.uv).chain(v.color).any(|c| !c.is_finite())
                || v.color[3] != 1.0 || v.joints != [0.0;4]
                || !(v.weights == [0.0;4] || v.weights == [1.0,0.0,0.0,0.0])
        })
    { return full(); }

    // Linear filtering and mip averaging stay inside channel extrema. This
    // stronger proof uses the roughness maximum, unlike candidate selection.
    let channel = ((p.material8[2] + 0.5) as u32 >> 4) & 15;
    let bounds = draw.metallic_roughness_texture.channel_bounds;
    let endpoint = |hi: usize| {
        if channel & 7 == 4 {
            (0..3).map(|i| bounds[i][hi] as f32 / 255.0 * [0.2126,0.7152,0.0722][i]).sum()
        } else { bounds[if channel & 7 < 4 { (channel & 7) as usize } else { 0 }][hi] as f32 / 255.0 }
    };
    let (lo,hi) = (endpoint(0),endpoint(1));
    let (lo,hi) = if channel & 8 != 0 { (1.0-hi,1.0-lo) } else { (lo,hi) };
    let maximum = p.material0[1] * if p.material0[1] >= 0.0 { hi } else { lo } + lighting.surface1[2];
    if !maximum.is_finite() || maximum > 0.11999 { return full(); }
    let normal_bounds = draw.normal_texture.channel_bounds;
    let z = normal_bounds[2][0] as f32 / 127.5 - 1.0;
    let extent = |axis: usize| normal_bounds[axis].iter()
        .map(|&v| (v as f32 / 127.5 - 1.0).abs()).fold(0.0_f32,f32::max) * p.material0[2].abs();
    let (x,y) = (extent(0),extent(1));
    // A stricter cone leaves arithmetic slack below the shader's 0.1° limit.
    if z <= 0.0 || z / (x*x+y*y+z*z).sqrt() < 0.999999 { return full(); }

    let axis = plane_normal.iter().position(|&v| v != 0.0).unwrap();
    // Main instance preparation writes this exact manifold proof to material8.w.
    // Retain all these triangles in the optical BVH; omit only raster fragments
    // that the closed-slab entry would reject before any shading or refraction.
    let closed_slab = closed_mesh(&draw.vertices,&draw.indices);
    let ulp = |v: f32| {
        let positive = v.abs();
        f64::from(f32::from_bits(positive.to_bits()+1))-f64::from(positive)
    };
    let world_position = |v: GpuWorldVertex| {
        let difference: [f32;3] = std::array::from_fn(|i| v.position[i]-p.model[i]);
        let product = difference.map(|v| v*p.model[3]);
        let position = std::array::from_fn::<_,3,_>(|i| product[i]+p.actor[i]);
        // GPU subtraction/multiplication/addition may fuse while the CPU's
        // intermediate products round separately. Cancellation can make a
        // camera-relative reserve tiny despite several world-coordinate ulps.
        // Include every axis, also reserving interpolation roundoff.
        let error = std::array::from_fn::<_,3,_>(|i| {
            2.0*ulp(difference[i])*f64::from(p.model[3]).abs()
                +2.0*ulp(product[i])+4.0*ulp(position[i])
        });
        (position,error)
    };
    let facing = |normal: [f32;3],position: [f32;3],error: [f64;3]| {
        let orthographic = p.camera3[3] > 0.5;
        let direction: [f64;3] = std::array::from_fn(|i| {
            if orthographic { -f64::from(p.camera3[i]) }
            else { f64::from(p.camera0[i])-f64::from(position[i]) }
        });
        let transform_error = if orthographic { 0.0 }
            else { (0..3).map(|i| error[i]*f64::from(normal[i]).abs()).sum() };
        let length = direction.iter().map(|v| v.abs()).sum::<f64>().max(1.0);
        let reserve = transform_error+64.0*f64::from(f32::EPSILON)*length;
        let value = (0..3).map(|i| f64::from(normal[i])*direction[i]).sum::<f64>();
        (value,reserve)
    };
    let discarded = |face: &[u32]| -> bool {
        if !closed_slab { return false; }
        let Some(a) = draw.vertices.get(face[0] as usize).copied() else { return false; };
        let Some(b) = draw.vertices.get(face[1] as usize).copied() else { return false; };
        let Some(c) = draw.vertices.get(face[2] as usize).copied() else { return false; };
        if [b,c].iter().any(|v| v.normal != a.normal || v.tangent != a.tangent || v.bitangent != a.bitangent)
            || !axis_vector(a.normal) || !axis_vector(a.tangent) || !axis_vector(a.bitangent)
            || dot3(a.normal,a.tangent) != 0.0 || dot3(a.normal,a.bitangent) != 0.0
            || dot3(a.tangent,a.bitangent) != 0.0
        { return false; }
        [a,b,c].into_iter().all(|v| {
            let (position,error) = world_position(v);
            let (value,reserve) = facing(a.normal,position,error);
            error.iter().all(|v| v.is_finite()) && value < -reserve
        })
    };
    let certified = |face: &[u32]| -> bool {
        let Some(a) = draw.vertices.get(face[0] as usize).copied() else { return false; };
        let Some(b) = draw.vertices.get(face[1] as usize).copied() else { return false; };
        let Some(c) = draw.vertices.get(face[2] as usize).copied() else { return false; };
        if [b,c].iter().any(|v| v.normal != a.normal || v.tangent != a.tangent || v.bitangent != a.bitangent)
            || a.normal != plane_normal || !axis_vector(a.tangent) || !axis_vector(a.bitangent)
            || dot3(a.normal,a.tangent) != 0.0 || dot3(a.normal,a.bitangent) != 0.0
            || dot3(a.tangent,a.bitangent) != 0.0
        { return false; }
        let positions = [a,b,c].map(world_position);
        // An exactly constant axis coordinate survives interpolation. Reserve
        // subtraction/product/world ulps for backend interpolation and FMA
        // differences; never widen the shader plane tolerance.
        if positions[1].0[axis] != positions[0].0[axis] || positions[2].0[axis] != positions[0].0[axis] { return false; }
        positions.iter().all(|&(position,world_error)| {
            let distance = f64::from(position[axis])*f64::from(plane_normal[axis])+f64::from(view.params.plane[3]);
            let error = world_error[axis];
            let tolerance = f64::from(view.params.control[3]);
            let (facing,facing_error) = facing(plane_normal,position,world_error);
            if world_error.iter().any(|v| !v.is_finite())
                || distance.abs() > tolerance*0.5 || distance.abs()+error >= tolerance
                || facing <= facing_error.max(0.00001)
            { return false; }
            let relative: [f64;3] = std::array::from_fn(|i| f64::from(position[i])-f64::from(view.params.camera0[i]));
            let dot = |basis: [f32;4]| (0..3).map(|i| relative[i]*f64::from(basis[i])).sum::<f64>();
            let dot_error = |basis: [f32;4]| {
                let transform = (0..3).map(|i| world_error[i]*f64::from(basis[i]).abs()).sum::<f64>();
                let scale = (0..3).map(|i| (relative[i].abs()+world_error[i])*f64::from(basis[i]).abs()).sum::<f64>().max(1.0);
                transform+64.0*f64::from(f32::EPSILON)*scale
            };
            let depth = dot(view.params.camera3);
            let depth_error = dot_error(view.params.camera3);
            let orthographic = view.params.camera3[3] > 0.5;
            // Match the shader's denominator clamp as well as its near test.
            let minimum_depth = f64::from(view.params.camera1[3]).max(if orthographic { 0.0 } else { 0.0001 });
            if !depth_error.is_finite() || depth <= minimum_depth+depth_error { return false; }
            let w = if orthographic { 1.0 } else { depth };
            let w_error = if orthographic { 0.0 } else { depth_error };
            // Homogeneous half-texel inequalities are linear in world position.
            // Positive values at all vertices cover every perspective barycentric
            // interior, including current jitter's changed raster coverage.
            [view.params.camera1,view.params.camera2].into_iter().enumerate().all(|(i,basis)| {
                let size = f64::from(if i == 0 { view.width } else { view.height });
                let focal = f64::from(view.params.camera0[3])/size;
                let origin = f64::from(p.canvas[i+2])/f64::from(p.canvas[i]);
                let lateral = dot(basis);
                let lateral_error = dot_error(basis);
                let numerator = origin*w + lateral*focal*if i == 0 { 1.0 } else { -1.0 };
                let margin = 0.5/size;
                let arithmetic_scale = (origin.abs()*(w.abs()+w_error)
                    +focal.abs()*(lateral.abs()+lateral_error)+w.abs()+w_error).max(1.0);
                let arithmetic_error = 64.0*f64::from(f32::EPSILON)*arithmetic_scale;
                let lateral_error = focal.abs()*lateral_error+arithmetic_error;
                let lower_error = (origin-margin).abs()*w_error+lateral_error;
                let upper_error = (1.0-margin-origin).abs()*w_error+lateral_error;
                lower_error.is_finite() && upper_error.is_finite()
                    && numerator-margin*w > lower_error && (1.0-margin)*w-numerator > upper_error
            })
        })
    };
    let mut runs: Vec<CertifiedSlabRun> = Vec::new();
    for (triangle,face) in draw.indices.chunks_exact(3).enumerate() {
        let discarded = discarded(face);
        let cached = !discarded && certified(face);
        if let Some(run) = runs.last_mut().filter(|run| run.cached == cached && run.discarded == discarded) { run.index_count += 3; }
        else { runs.push(CertifiedSlabRun { first_index: triangle as u32*3,index_count: 3,cached,discarded }); }
    }
    if runs.iter().any(|run| run.cached) { runs } else { full() }
}

fn reflected_draw_params(
    draw: &GpuWorldDraw,
    view: &PlanarView,
    output: [u32; 2],
    jitter: [f32; 2],
) -> GpuWorldParams {
    let mut params = draw.params;
    params.camera0 = view.params.camera0;
    params.camera1 = view.params.camera1;
    params.camera2 = view.params.camera2;
    params.camera3 = view.params.camera3;
    let scale = [
        view.width as f32 / output[0].max(1) as f32,
        view.height as f32 / output[1].max(1) as f32,
    ];
    params.canvas = [
        view.width as f32,
        view.height as f32,
        draw.params.canvas[2] * scale[0] - jitter[0],
        draw.params.canvas[3] * scale[1] - jitter[1],
    ];
    params
}

fn draw_screen_bounds(
    draw: &GpuWorldDraw,
    params: GpuWorldParams,
    viewport: [u32; 2],
) -> transport_tiles::ScreenBounds {
    let bounds = draw
        .bone_matrices
        .iter()
        .all(|matrix| *matrix == mat4_identity())
        .then(|| rigid_vertex_bounds(&draw.vertices))
        .flatten();
    transport_tiles::ScreenBounds::bounds(bounds, params, viewport)
}

fn main_target_bounds(
    draws: &[GpuWorldDraw],
    target: &str,
    output: [u32; 2],
) -> transport_tiles::ScreenBounds {
    draws
        .iter()
        .filter(|draw| draw.instance_key.actor_id == target)
        .fold(transport_tiles::ScreenBounds::Empty, |bounds, draw| {
            bounds.union(draw_screen_bounds(draw, draw.params, output))
        })
}

/// Scissors restrict shading, while attachment clears and camera projection
/// retain the entire capture. A reflected target's actual lookup coordinates
/// also cover authored bevels and finite-thickness faces away from its plane.
fn capture_clip(
    bounds: transport_tiles::ScreenBounds,
    viewport: [u32; 2],
    padding: u32,
) -> Option<[u32; 4]> {
    use transport_tiles::ScreenBounds;
    if viewport.contains(&0) {
        return None;
    }
    let edges = match bounds {
        ScreenBounds::Empty => return None,
        ScreenBounds::Full => return Some([0, 0, viewport[0], viewport[1]]),
        ScreenBounds::Rect(edges) => edges,
    };
    let left = edges[0].saturating_sub(padding).min(viewport[0]);
    let top = edges[1].saturating_sub(padding).min(viewport[1]);
    let right = edges[2].saturating_add(padding).min(viewport[0]);
    let bottom = edges[3].saturating_add(padding).min(viewport[1]);
    (left < right && top < bottom).then_some([left, top, right - left, bottom - top])
}

fn inflate_capture_clip(clip: [u32; 4], viewport: [u32; 2], padding: u32) -> [u32; 4] {
    capture_clip(
        transport_tiles::ScreenBounds::Rect([
            clip[0],
            clip[1],
            clip[0].saturating_add(clip[2]),
            clip[1].saturating_add(clip[3]),
        ]),
        viewport,
        padding,
    )
    .expect("a nonempty capture clip remains nonempty when inflated")
}

fn capture_tiles(clip: [u32; 4], bounded: bool) -> Vec<[u32; 4]> {
    if !bounded {
        return vec![clip];
    }
    let right = clip[0] + clip[2];
    let bottom = clip[1] + clip[3];
    let mut tiles = Vec::new();
    for y in ((clip[1] / 128) * 128..bottom).step_by(128) {
        for x in ((clip[0] / 128) * 128..right).step_by(128) {
            let left = x.max(clip[0]);
            let top = y.max(clip[1]);
            let tile_right = x.saturating_add(128).min(right);
            let tile_bottom = y.saturating_add(128).min(bottom);
            tiles.push([left, top, tile_right - left, tile_bottom - top]);
        }
    }
    tiles
}

/// Clip a rigid bounding box against the capture plane and, optionally, the
/// reached screen footprint. This prevents an offscreen near-crossing window
/// from making every snapshot dependency cover the whole reflected viewport.
/// Both world and view coordinates travel through clipping; no inverse camera
/// assumption is needed. The box still encloses all rasterized geometry.
fn clipped_capture_box(
    draw: &GpuWorldDraw,
    params: GpuWorldParams,
    viewport: [u32; 2],
    plane: [f32; 4],
    bias: f32,
    region: Option<[u32; 4]>,
) -> Option<Vec<[f64; 6]>> {
    if params.vegetation[0] != 0.0 || !draw.bone_matrices.iter().all(|m| *m == mat4_identity()) {
        return None;
    }
    let (minimum, maximum) = rigid_vertex_bounds(&draw.vertices)?;
    if minimum
        .into_iter()
        .chain(maximum)
        .chain(params.canvas)
        .chain(params.model)
        .chain(params.actor)
        .chain(params.actor_rotation)
        .chain(params.camera0)
        .chain(params.camera1)
        .chain(params.camera2)
        .chain(params.camera3)
        .chain(plane)
        .chain([bias])
        .any(|v| !v.is_finite())
        || params.canvas[0] <= 0.0
        || params.canvas[1] <= 0.0
        || params.camera0[3] <= 0.0
        || params.camera1[3] < 0.0
        || params.actor_rotation.iter().map(|v| v * v).sum::<f32>() <= f32::EPSILON
    {
        return None;
    }
    let rotation = quat_normalize_xyzw(params.actor_rotation);
    let corners: [[f64; 6]; 8] = std::array::from_fn(|corner| {
        let local = std::array::from_fn(|axis| {
            let value = if corner & (1 << axis) == 0 {
                minimum[axis]
            } else {
                maximum[axis]
            };
            (value - params.model[axis]) * params.model[3]
        });
        let rotated = quat_rotate_vec3(rotation, local);
        let world: [f32; 3] = std::array::from_fn(|axis| rotated[axis] + params.actor[axis]);
        let relative: [f64; 3] =
            std::array::from_fn(|axis| f64::from(world[axis]) - f64::from(params.camera0[axis]));
        let view: [f64; 3] = std::array::from_fn(|axis| {
            let basis = [params.camera1, params.camera2, params.camera3][axis];
            (0..3).map(|i| relative[i] * f64::from(basis[i])).sum()
        });
        [
            f64::from(world[0]),
            f64::from(world[1]),
            f64::from(world[2]),
            view[0],
            view[1],
            view[2],
        ]
    });
    if corners.iter().flatten().any(|v| !v.is_finite()) {
        return None;
    }
    let mut planes = vec![[
        f64::from(plane[0]),
        f64::from(plane[1]),
        f64::from(plane[2]),
        0.0,
        0.0,
        0.0,
        f64::from(plane[3] - bias) + 0.00001,
    ]];
    if let Some(region) = region {
        let scale = [
            f64::from(viewport[0]) / f64::from(params.canvas[0]),
            f64::from(viewport[1]) / f64::from(params.canvas[1]),
        ];
        if scale.iter().any(|v| *v <= 0.0 || !v.is_finite()) {
            return None;
        }
        let edges = [
            region[0].saturating_sub(3),
            region[1].saturating_sub(3),
            region[0].saturating_add(region[2]).saturating_add(3),
            region[1].saturating_add(region[3]).saturating_add(3),
        ];
        let focal = f64::from(params.camera0[3]);
        let origin = [f64::from(params.canvas[2]), f64::from(params.canvas[3])];
        planes.push([0.0, 0.0, 0.0, 0.0, 0.0, 1.0, -f64::from(params.camera1[3])]);
        let coefficients = [
            origin[0] - f64::from(edges[0]) / scale[0],
            f64::from(edges[2]) / scale[0] - origin[0],
            origin[1] - f64::from(edges[1]) / scale[1],
            f64::from(edges[3]) / scale[1] - origin[1],
        ];
        for (axis, sign, coefficient) in [
            (3, focal, coefficients[0]),
            (3, -focal, coefficients[1]),
            (4, -focal, coefficients[2]),
            (4, focal, coefficients[3]),
        ] {
            let mut clip = [0.0; 7];
            clip[axis] = sign;
            clip[if params.camera3[3] > 0.5 { 6 } else { 5 }] = coefficient;
            planes.push(clip);
        }
    }
    let mut faces = [
        [0, 2, 3, 1],
        [4, 5, 7, 6],
        [0, 1, 5, 4],
        [2, 6, 7, 3],
        [0, 4, 6, 2],
        [1, 3, 7, 5],
    ]
    .into_iter()
    .map(|face| face.into_iter().map(|i| corners[i]).collect::<Vec<_>>())
    .collect::<Vec<_>>();
    for plane in &planes {
        let mut clipped_faces = Vec::new();
        let mut cap: Vec<[f64; 6]> = Vec::new();
        for polygon in faces {
            if polygon.is_empty() {
                continue;
            }
            let distance =
                |point: [f64; 6]| (0..6).map(|i| point[i] * plane[i]).sum::<f64>() + plane[6];
            let mut clipped = Vec::new();
            let mut previous = *polygon.last().expect("nonempty bounding face");
            let mut previous_distance = distance(previous);
            for point in polygon {
                let current_distance = distance(point);
                if (previous_distance >= 0.0) != (current_distance >= 0.0) {
                    let t = previous_distance / (previous_distance - current_distance);
                    let intersection =
                        std::array::from_fn(|i| previous[i] + (point[i] - previous[i]) * t);
                    clipped.push(intersection);
                    if !cap.iter().any(|p| {
                        (0..3)
                            .map(|i| (p[i] - intersection[i]).powi(2))
                            .sum::<f64>()
                            < 1e-16
                    }) {
                        cap.push(intersection);
                    }
                }
                if current_distance >= 0.0 {
                    clipped.push(point);
                }
                previous = point;
                previous_distance = current_distance;
            }
            if !clipped.is_empty() {
                clipped_faces.push(clipped);
            }
        }
        // Keep each new cap: later frustum planes can create vertices inside
        // the original box, including a camera or crop entirely inside it.
        if cap.len() >= 3 {
            let normal: [f64; 3] = std::array::from_fn(|i| {
                plane[i]
                    + plane[3] * f64::from(params.camera1[i])
                    + plane[4] * f64::from(params.camera2[i])
                    + plane[5] * f64::from(params.camera3[i])
            });
            let dot = |a: [f64; 3], b: [f64; 3]| (0..3).map(|i| a[i] * b[i]).sum::<f64>();
            let cross = |a: [f64; 3], b: [f64; 3]| {
                [
                    a[1] * b[2] - a[2] * b[1],
                    a[2] * b[0] - a[0] * b[2],
                    a[0] * b[1] - a[1] * b[0],
                ]
            };
            let length = dot(normal, normal).sqrt();
            if !length.is_finite() || length <= 1e-12 {
                return None;
            }
            let normal = normal.map(|v| v / length);
            let u = cross(
                normal,
                if normal[0].abs() < 0.9 {
                    [1.0, 0.0, 0.0]
                } else {
                    [0.0, 1.0, 0.0]
                },
            );
            let v = cross(normal, u);
            let center: [f64; 3] =
                std::array::from_fn(|i| cap.iter().map(|p| p[i]).sum::<f64>() / cap.len() as f64);
            let angle = |p: &[f64; 6]| {
                let relative = std::array::from_fn(|i| p[i] - center[i]);
                dot(relative, v).atan2(dot(relative, u))
            };
            cap.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
            clipped_faces.push(cap);
        }
        faces = clipped_faces;
        if faces.is_empty() {
            break;
        }
    }
    let result: Vec<_> = faces.into_iter().flatten().collect();
    result
        .iter()
        .flatten()
        .all(|v| v.is_finite())
        .then_some(result)
}

fn clipped_slab_snapshot_padding(
    draw: &GpuWorldDraw,
    params: GpuWorldParams,
    viewport: [u32; 2],
    jitter: [f32; 2],
    plane: [f32; 4],
    bias: f32,
    region: [u32; 4],
) -> Option<u32> {
    let vertices = clipped_capture_box(draw, params, viewport, plane, bias, Some(region))?;
    if vertices.is_empty() {
        return Some(0);
    }
    let minimum_depth = vertices.iter().map(|v| v[5]).fold(f64::INFINITY, f64::min);
    slab_snapshot_padding_inner(draw, params, viewport, jitter, Some(minimum_depth))
}

/// Bound every snapshot lookup of a rigid thin slab, including normal mapping.
/// Missing deformation/depth evidence retains the full capture viewport.
#[cfg(test)]
fn slab_snapshot_padding(
    draw: &GpuWorldDraw,
    params: GpuWorldParams,
    viewport: [u32; 2],
    jitter: [f32; 2],
) -> Option<u32> {
    slab_snapshot_padding_inner(draw, params, viewport, jitter, None)
}

fn slab_snapshot_padding_inner(
    draw: &GpuWorldDraw,
    params: GpuWorldParams,
    viewport: [u32; 2],
    jitter: [f32; 2],
    clipped_depth: Option<f64>,
) -> Option<u32> {
    if params.vegetation[0] != 0.0
        || !draw.bone_matrices.iter().all(|m| *m == mat4_identity())
        || params
            .material6
            .iter()
            .chain(params.model.iter())
            .chain(params.actor.iter())
            .chain(params.actor_rotation.iter())
            .chain(params.camera0.iter())
            .chain(params.camera1.iter())
            .chain(params.camera2.iter())
            .chain(params.camera3.iter())
            .chain(params.canvas.iter())
            .any(|value| !value.is_finite())
        || !params.material0[2].is_finite()
    {
        return None;
    }
    let bounds = rigid_vertex_bounds(&draw.vertices)?;
    if clipped_depth.is_none()
        && !matches!(
            transport_tiles::ScreenBounds::bounds(Some(bounds), params, viewport),
            transport_tiles::ScreenBounds::Rect(_)
        )
    {
        return None;
    }
    let ior = f64::from(params.material6[1].clamp(1.0, 3.0));
    let normal_bounds = draw.normal_texture.channel_bounds;
    if normal_bounds.iter().any(|range| range[0] > range[1]) {
        return None;
    }
    let extent = |channel: usize| {
        normal_bounds[channel]
            .iter()
            .map(|value| (f64::from(*value) / 127.5 - 1.0).abs())
            .fold(0.0_f64, f64::max)
            * f64::from(params.material0[2]).abs()
    };
    let z = f64::from(normal_bounds[2][0]) / 127.5 - 1.0;
    let x = extent(0);
    let y = extent(1);
    let cosine = if z > 0.0 {
        z / (x * x + y * y + z * z).sqrt()
    } else {
        0.0
    };
    let sine = (1.0 - cosine * cosine).max(0.0).sqrt();
    // Refracted directions retain at least sqrt(1-eta^2) along -normal.
    // The shader clamps its geometric denominator to .05 in every case.
    let denominator =
        (((1.0 - 1.0 / (ior * ior)).max(0.0).sqrt() * cosine - sine / ior) - 0.00001).max(0.05);
    let distance = f64::from(params.material6[2].max(0.0)) / denominator;
    let focal = f64::from(params.camera0[3]).abs();
    let displacement = if params.camera3[3] > 0.5 {
        distance * focal
    } else {
        let rotation = quat_normalize_xyzw(params.actor_rotation);
        let mut minimum_depth = f64::INFINITY;
        for corner in 0..8 {
            let local = std::array::from_fn(|axis| {
                let value = if corner & (1 << axis) == 0 {
                    bounds.0[axis]
                } else {
                    bounds.1[axis]
                };
                (value - params.model[axis]) * params.model[3]
            });
            let rotated = quat_rotate_vec3(rotation, local);
            let depth = (0..3)
                .map(|axis| {
                    f64::from(rotated[axis] + params.actor[axis] - params.camera0[axis])
                        * f64::from(params.camera3[axis])
                })
                .sum::<f64>();
            minimum_depth = minimum_depth.min(depth);
        }
        let lower_depth = clipped_depth.unwrap_or(minimum_depth) - distance;
        if !lower_depth.is_finite() || lower_depth <= f64::from(params.camera1[3]) + 0.00001 {
            return None;
        }
        let extent = (0..2)
            .map(|axis| {
                let center = f64::from(params.canvas[axis + 2]) + f64::from(jitter[axis]);
                center.abs().max((f64::from(viewport[axis]) - center).abs()) + 2.0
            })
            .fold(0.0_f64, f64::max);
        distance * (focal + extent) / lower_depth
    };
    let blur = (f64::from(viewport[0].max(viewport[1])) * 0.006).ceil();
    let padding = displacement.ceil() + blur + 4.0;
    (padding.is_finite() && padding <= f64::from(u32::MAX)).then_some(padding as u32)
}

fn reflect_vector(v: [f32; 3], n: [f32; 3]) -> [f32; 3] {
    let twice = 2.0 * (v[0] * n[0] + v[1] * n[1] + v[2] * n[2]);
    [
        v[0] - twice * n[0],
        v[1] - twice * n[1],
        v[2] - twice * n[2],
    ]
}

pub(super) fn view_for_target(
    draw: &GpuWorldDraw,
    scale: f32,
    bias: f32,
    width: u32,
    height: u32,
    limit: u32,
) -> Result<PlanarView, WorldRenderError> {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for v in draw.vertices.iter() {
        for axis in 0..3 {
            lo[axis] = lo[axis].min(v.position[axis]);
            hi[axis] = hi[axis].max(v.position[axis]);
        }
    }
    let spans = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    // The source surface may itself be rotated within a MeshAsset. Fit its
    // dominant face rather than assuming an axis-aligned local box.
    let mut local_normal = [0.0; 3];
    let mut largest_triangle = 0.0_f32;
    for face in draw.indices.chunks_exact(3) {
        let a = draw.vertices[face[0] as usize].position;
        let b = draw.vertices[face[1] as usize].position;
        let c = draw.vertices[face[2] as usize].position;
        let ab = std::array::from_fn(|i| b[i] - a[i]);
        let ac = std::array::from_fn(|i| c[i] - a[i]);
        let n = cross3(ab, ac);
        let area = n.into_iter().map(|v| v * v).sum::<f32>();
        if area > largest_triangle {
            largest_triangle = area;
            local_normal = normalize3(n);
        }
    }
    let projection =
        |position: [f32; 3]| (0..3).map(|i| position[i] * local_normal[i]).sum::<f32>();
    let mut plane_lo = f32::INFINITY;
    let mut plane_hi = f32::NEG_INFINITY;
    for v in draw.vertices.iter() {
        let distance = projection(v.position);
        plane_lo = plane_lo.min(distance);
        plane_hi = plane_hi.max(distance);
    }
    let thickness = plane_hi - plane_lo;
    let largest = spans.into_iter().fold(0.0_f32, f32::max);
    if !largest.is_finite()
        || largest <= 0.0
        || largest_triangle <= 0.0
        || thickness > largest * 0.2
        || !draw
            .bone_matrices
            .iter()
            .all(|matrix| *matrix == mat4_identity())
    {
        return Err(WorldRenderError::GpuRender {
            message: format!(
                "PlanarReflection target '{}' must be a rigid planar surface",
                draw.instance_key.actor_id
            ),
        });
    }
    let p = draw.params;
    let mut normal = quat_rotate_vec3(p.actor_rotation, local_normal);
    let source_center: [f32; 3] = std::array::from_fn(|i| (lo[i] + hi[i]) * 0.5);
    let face_mid = (plane_lo + plane_hi) * 0.5 - projection(source_center);
    let center = std::array::from_fn(|i| {
        (source_center[i] + local_normal[i] * face_mid - p.model[i]) * p.model[3]
    });
    let rotated = quat_rotate_vec3(p.actor_rotation, center);
    let mut point: [f32; 3] = std::array::from_fn(|i| p.actor[i] + rotated[i]);
    let facing: f32 = (0..3).map(|i| (p.camera0[i] - point[i]) * normal[i]).sum();
    if facing < 0.0 {
        normal = normal.map(|v| -v);
    }
    // Reflect about the visible face, not the middle of a finite-thickness box.
    for i in 0..3 {
        point[i] += normal[i] * thickness * p.model[3].abs() * 0.5;
    }
    let d = -(0..3).map(|i| point[i] * normal[i]).sum::<f32>();
    let signed = (0..3).map(|i| p.camera0[i] * normal[i]).sum::<f32>() + d;
    let eye: [f32; 3] = std::array::from_fn(|i| p.camera0[i] - 2.0 * signed * normal[i]);
    let reflection_scale = scale
        .clamp(f32::EPSILON, 1.0)
        .min(limit.max(1) as f32 / width.max(height).max(1) as f32);
    let w = ((width as f32 * reflection_scale).round() as u32).max(1);
    let h = ((height as f32 * reflection_scale).round() as u32).max(1);
    let right = reflect_vector([p.camera1[0], p.camera1[1], p.camera1[2]], normal);
    let up = reflect_vector([p.camera2[0], p.camera2[1], p.camera2[2]], normal);
    let forward = reflect_vector([p.camera3[0], p.camera3[1], p.camera3[2]], normal);
    Ok(PlanarView {
        target: draw.instance_key.actor_id.clone(),
        members: vec![draw.instance_key.actor_id.clone()],
        width: w,
        height: h,
        params: PlanarParams {
            camera0: [
                eye[0],
                eye[1],
                eye[2],
                p.camera0[3] * h as f32 / height as f32,
            ],
            camera1: [right[0], right[1], right[2], p.camera1[3]],
            camera2: [up[0], up[1], up[2], p.camera2[3]],
            camera3: [forward[0], forward[1], forward[2], p.camera3[3]],
            plane: [normal[0], normal[1], normal[2], d],
            control: [1.0, 0.0, bias.max(0.0), 0.0],
        },
    })
}

/// Conservative linear-sampling bounds include every texel and mip. A sharp
/// region can use the capture while rough texels retain geometry transport.
fn automatic_material_eligible(draw: &GpuWorldDraw, lighting: &GpuWorldLightingParams) -> bool {
    let p = draw.params;
    if p.style
        .into_iter()
        .chain(p.material0)
        .chain(p.material1)
        .chain(p.material2)
        .chain(p.material4)
        .chain(p.material6)
        .chain(p.material7)
        .chain(p.material8)
        .chain(p.material10)
        .chain(p.material11)
        .chain(p.model)
        .chain(p.actor)
        .chain(p.camera0)
        .chain(p.camera1)
        .chain(p.camera2)
        .chain(p.camera3)
        .chain(p.canvas)
        .chain(p.vegetation)
        .any(|v| !v.is_finite())
    {
        return false;
    }
    let rotation_length = p.actor_rotation.iter().map(|v| v * v).sum::<f32>();
    if !rotation_length.is_finite() || (rotation_length - 1.0).abs() > 0.00001 {
        return false;
    }
    let slab = draw.phase == GpuWorldDrawPhase::Transmissive
        && p.material6[0] > 0.001
        && p.material11[0] <= 0.5;
    let opaque =
        draw.phase == GpuWorldDrawPhase::Opaque && draw.depth_write && p.material6[0] <= 0.001;
    if !(opaque || slab)
        || p.vegetation[0] != 0.0
        || p.model[3] <= 0.0
        || p.material10[0] > 0.0
        || p.material11[0] > 0.5
        || p.material7[3] > 0.0
        || p.material1[3] > 0.0
        || p.material2[3] > 0.0
        || p.style[1] <= 0.0
        || lighting.surface0[0] >= 0.5
        || lighting.reflection0[3] <= 0.5
        || !draw.bone_matrices.iter().all(|m| *m == mat4_identity())
        || p.material4[3] * p.style[0] < 0.999999
        || draw.texture.channel_bounds[3] != [255, 255]
        || draw.vertices.iter().any(|v| {
            v.position
                .iter()
                .chain(&v.normal)
                .chain(&v.color)
                .any(|value| !value.is_finite())
                || v.normal.iter().map(|value| value * value).sum::<f32>() <= 0.000001
                || v.color[3] < 0.999999
        })
    {
        return false;
    }
    let packed = ((p.material8[2] + 0.5) as u32 >> 4) & 15;
    let bounds = draw.metallic_roughness_texture.channel_bounds;
    if bounds.iter().any(|range| range[0] > range[1]) {
        return false;
    }
    let (lo, hi) = if packed & 7 == 4 {
        let weights = [0.2126, 0.7152, 0.0722];
        let endpoint = |i| {
            (0..3)
                .map(|c| bounds[c][i] as f32 / 255.0 * weights[c])
                .sum::<f32>()
        };
        (endpoint(0), endpoint(1))
    } else {
        let channel = (packed & 7) as usize;
        let channel = if channel < 4 { channel } else { 0 };
        (
            bounds[channel][0] as f32 / 255.0,
            bounds[channel][1] as f32 / 255.0,
        )
    };
    let (lo, hi) = if packed & 8 != 0 {
        (1.0 - hi, 1.0 - lo)
    } else {
        (lo, hi)
    };
    let roughness =
        p.material0[1] * if p.material0[1] >= 0.0 { lo } else { hi } + lighting.surface1[2];
    if !roughness.is_finite() || roughness > 0.12 {
        return false;
    }
    let bounds = draw.normal_texture.channel_bounds;
    if bounds.iter().any(|range| range[0] > range[1]) || !p.material0[2].is_finite() {
        return false;
    }
    let z = bounds[2][0] as f32 / 127.5 - 1.0;
    if z <= 0.0 {
        return false;
    }
    let extent = |c: usize| {
        bounds[c]
            .iter()
            .map(|v| (*v as f32 / 127.5 - 1.0).abs())
            .fold(0.0_f32, f32::max)
            * p.material0[2].abs()
    };
    let x = extent(0);
    let y = extent(1);
    // A preview approximation only: prove <0.3 degree for every sampled normal,
    // then accept only <0.1 degree at each full-resolution shading point.
    z / (x * x + y * y + z * z).sqrt() >= 0.9999862
}

fn automatic_view_for_target(
    draw: &GpuWorldDraw,
    lighting: &GpuWorldLightingParams,
    width: u32,
    height: u32,
    limit: u32,
) -> Option<(f32, PlanarView)> {
    if !automatic_material_eligible(draw, lighting) {
        return None;
    }
    let bounds = rigid_vertex_bounds(&draw.vertices)?;
    if !matches!(
        transport_tiles::ScreenBounds::bounds(Some(bounds), draw.params, [width, height]),
        transport_tiles::ScreenBounds::Rect(_)
    ) {
        return None;
    }
    let mut view = view_for_target(draw, 0.25, 0.0, width, height, limit.min(512)).ok()?;
    let p = draw.params;
    let rotation = quat_normalize_xyzw(p.actor_rotation);
    let world = |local: [f32; 3]| {
        let transformed = quat_rotate_vec3(
            rotation,
            std::array::from_fn(|i| (local[i] - p.model[i]) * p.model[3]),
        );
        std::array::from_fn::<_, 3, _>(|i| transformed[i] + p.actor[i])
    };
    let span = (0..3)
        .map(|i| (bounds.1[i] - bounds.0[i]) * p.model[3])
        .fold(0.0_f32, f32::max);
    let tolerance = (span * 0.000001).max(0.000005);
    let plane = view.params.plane;
    let distance = |v: [f32; 3]| (0..3).map(|i| v[i] * plane[i]).sum::<f32>() + plane[3];
    // Captures exclude the target actor. Prove every part stays behind its
    // selected face so excluding it cannot remove reflected foreground.
    if draw
        .vertices
        .iter()
        .any(|v| distance(world(v.position)) > tolerance)
    {
        return None;
    }
    let project = |v: [f32; 3]| -> Option<[f32; 2]> {
        let relative = std::array::from_fn::<_, 3, _>(|i| v[i] - p.camera0[i]);
        let dot = |basis: [f32; 4]| (0..3).map(|i| relative[i] * basis[i]).sum::<f32>();
        let depth = dot(p.camera3);
        if depth <= p.camera1[3] {
            return None;
        }
        let w = if p.camera3[3] > 0.5 { 1.0 } else { depth };
        Some([
            (p.canvas[2] + dot(p.camera1) * p.camera0[3] / w) * width as f32 / p.canvas[0],
            (p.canvas[3] - dot(p.camera2) * p.camera0[3] / w) * height as f32 / p.canvas[1],
        ])
    };
    let mut area = 0.0;
    for face in draw.indices.chunks_exact(3) {
        let vertices = [
            draw.vertices[face[0] as usize],
            draw.vertices[face[1] as usize],
            draw.vertices[face[2] as usize],
        ];
        let positions = vertices.map(|v| world(v.position));
        if positions.iter().any(|v| distance(*v).abs() > tolerance)
            || vertices.iter().any(|v| {
                let n = quat_rotate_vec3(rotation, normalize3(v.normal));
                (0..3).map(|i| n[i] * plane[i]).sum::<f32>() < 0.99995
            })
        {
            continue;
        }
        let Some(a) = project(positions[0]) else {
            continue;
        };
        let Some(b) = project(positions[1]) else {
            continue;
        };
        let Some(c) = project(positions[2]) else {
            continue;
        };
        // Rank projected face coverage, not the thicker actor's bounding box.
        let clamp = |v: [f32; 2]| {
            [
                v[0].clamp(0.0, width as f32),
                v[1].clamp(0.0, height as f32),
            ]
        };
        let a = clamp(a);
        let b = clamp(b);
        let c = clamp(c);
        area += ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() * 0.5;
    }
    if !area.is_finite() || area < 4096.0_f32.max(width as f32 * height as f32 * 0.005) {
        return None;
    }
    view.params.control[3] = tolerance;
    let center = world(std::array::from_fn(|i| (bounds.0[i] + bounds.1[i]) * 0.5));
    let face_center = std::array::from_fn::<_, 3, _>(|i| center[i] - distance(center) * plane[i]);
    let to_camera = std::array::from_fn::<_, 3, _>(|i| p.camera0[i] - face_center[i]);
    let view_length = to_camera.iter().map(|v| v * v).sum::<f32>().sqrt();
    let cosine = ((0..3).map(|i| to_camera[i] * plane[i]).sum::<f32>() / view_length.max(0.000001))
        .clamp(0.0, 1.0);
    // Grazing polished faces carry stronger visible dielectric reflection.
    // Prioritize that contribution without spending slots on a merely larger,
    // front-facing patch with a much weaker reflection response.
    // Experimental slab captures rank the query work they can remove: their
    // reflection is traced even at normal incidence with a weak Fresnel lobe.
    // Keep the established energy-weighted priority for opaque faces.
    let priority = area
        * if draw.phase == GpuWorldDrawPhase::Transmissive {
            25.0
        } else {
            1.0 + 24.0 * (1.0 - cosine).powi(5)
        };
    Some((priority, view))
}

/// The selected front face must also lie on the group's canonical plane.
/// Matching only plane normals would incorrectly merge parallel offset panes.
fn automatic_draw_fits_group_plane(
    draw: &GpuWorldDraw,
    source: &PlanarView,
    group: &PlanarView,
    tolerance: f32,
) -> bool {
    let p = draw.params;
    let rotation = quat_normalize_xyzw(p.actor_rotation);
    let distance = |plane: [f32; 4], position: [f32; 3]| {
        (0..3).map(|i| plane[i] * position[i]).sum::<f32>() + plane[3]
    };
    draw.vertices.iter().all(|vertex| {
        let rotated = quat_rotate_vec3(
            rotation,
            std::array::from_fn(|i| (vertex.position[i] - p.model[i]) * p.model[3]),
        );
        let world = std::array::from_fn::<_, 3, _>(|i| rotated[i] + p.actor[i]);
        let group_distance = distance(group.params.plane, world);
        if !group_distance.is_finite() || group_distance > tolerance {
            return false;
        }
        let normal = quat_rotate_vec3(rotation, normalize3(vertex.normal));
        let source_facing = (0..3)
            .map(|i| normal[i] * source.params.plane[i])
            .sum::<f32>();
        let on_source_face = source_facing >= 0.99995
            && distance(source.params.plane, world).abs() <= source.params.control[3];
        !on_source_face || group_distance.abs() <= tolerance
    })
}

fn automatic_views_coplanar(
    group: &PlanarView,
    candidate: &PlanarView,
    draws: &[GpuWorldDraw],
) -> bool {
    let tolerance = group.params.control[3].min(candidate.params.control[3]);
    if tolerance <= 0.0
        || !tolerance.is_finite()
        || group.width != candidate.width
        || group.height != candidate.height
        || (group.params.plane[3] - candidate.params.plane[3]).abs() > tolerance
        || (0..3)
            .map(|i| group.params.plane[i] * candidate.params.plane[i])
            .sum::<f32>()
            < 0.999999
    {
        return false;
    }
    // In addition to coplanarity, prove the common reflected projection. The
    // small tolerance accommodates transform rounding, not different cameras.
    if (0..3)
        .any(|i| (group.params.camera0[i] - candidate.params.camera0[i]).abs() > tolerance * 2.0)
        || (group.params.camera0[3] - candidate.params.camera0[3]).abs() > 0.000001
        || [
            (group.params.camera1, candidate.params.camera1),
            (group.params.camera2, candidate.params.camera2),
            (group.params.camera3, candidate.params.camera3),
        ]
        .into_iter()
        .any(|(a, b)| (0..4).any(|i| (a[i] - b[i]).abs() > 0.000001))
    {
        return false;
    }
    let Some(canonical_draw) = draws
        .iter()
        .find(|draw| draw.instance_key.actor_id == group.target)
    else {
        return false;
    };
    group.members.iter().chain(&candidate.members).all(|actor| {
        let Some(draw) = draws
            .iter()
            .find(|draw| &draw.instance_key.actor_id == actor)
        else {
            return false;
        };
        draw.params.canvas == canonical_draw.params.canvas
            && automatic_draw_fits_group_plane(
                draw,
                if group.matches_target(actor) {
                    group
                } else {
                    candidate
                },
                group,
                tolerance,
            )
    })
}

fn group_automatic_views(
    mut candidates: Vec<(f32, PlanarView)>,
    draws: &[GpuWorldDraw],
) -> Vec<(f32, PlanarView)> {
    // A stable actor ordering keeps the resource owner deterministic while
    // summed visible reflection contribution determines the capture budget.
    candidates.sort_by(|a, b| a.1.target.cmp(&b.1.target));
    let mut groups: Vec<(f32, PlanarView)> = Vec::new();
    for (priority, candidate) in candidates {
        if let Some((group_priority, group)) = groups
            .iter_mut()
            .find(|(_, group)| automatic_views_coplanar(group, &candidate, draws))
        {
            *group_priority += priority;
            group.params.control[3] = group.params.control[3].min(candidate.params.control[3]);
            group.members.extend(candidate.members);
        } else {
            groups.push((priority, candidate));
        }
    }
    groups.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then_with(|| a.1.target.cmp(&b.1.target))
    });
    groups
}

pub(super) fn closed_mesh(vertices: &[GpuWorldVertex], indices: &[u32]) -> bool {
    // Imported and procedural meshes duplicate vertices at UV/normal seams.
    // Weld by position for topology, leaving the actual render mesh untouched.
    let mut edges = HashMap::<([u32; 3], [u32; 3]), u32>::new();
    let key = |index: u32| {
        vertices[index as usize]
            .position
            .map(|v| if v == 0.0 { 0 } else { v.to_bits() })
    };
    for triangle in indices.chunks_exact(3) {
        let p = [key(triangle[0]), key(triangle[1]), key(triangle[2])];
        if p[0] == p[1] || p[1] == p[2] || p[2] == p[0] {
            continue;
        }
        for (a, b) in [(p[0], p[1]), (p[1], p[2]), (p[2], p[0])] {
            let edge = if a < b { (a, b) } else { (b, a) };
            *edges.entry(edge).or_default() += 1;
        }
    }
    !edges.is_empty() && edges.values().all(|count| *count == 2)
}

impl GpuWorldRenderer {
    fn prepare_planar_resource(&mut self, view: &PlanarView) {
        if self
            .planar_resources
            .get(&view.target)
            .is_none_or(|r| r.color.width() != view.width || r.color.height() != view.height)
        {
            self.planar_resources.insert(
                view.target.clone(),
                GpuPlanarResource {
                    color: Self::make_hdr_texture(&self.device, view.width, view.height),
                    depth: Self::make_depth_texture(&self.device, view.width, view.height),
                    snapshot: Self::make_hdr_texture(&self.device, view.width, view.height),
                    depth_snapshot: Self::make_depth_texture(&self.device, view.width, view.height),
                },
            );
        }
    }

    pub(super) fn prepare_planar_views(
        &mut self,
        draws: &[GpuWorldDraw],
        lighting: &GpuWorldLighting,
    ) -> Result<Vec<PlanarView>, WorldRenderError> {
        let mut views = Vec::new();
        for reflection in &lighting.planar_reflections {
            if views.len() >= lighting.planar_capture_budget as usize {
                break;
            }
            let draw = draws
                .iter()
                .find(|draw| draw.instance_key.actor_id == reflection.target)
                .ok_or_else(|| WorldRenderError::GpuRender {
                    message: format!(
                        "PlanarReflection target '{}' was not submitted",
                        reflection.target
                    ),
                })?;
            let coverage = main_target_bounds(draws, &reflection.target, [self.width, self.height]);
            #[cfg(not(target_arch = "wasm32"))]
            if std::env::var_os("MOTIONLOOM_TRACE_BATCHES").is_some() {
                eprintln!(
                    "motionloom planar target: {} coverage={coverage:?}",
                    reflection.target
                );
            }
            if coverage == transport_tiles::ScreenBounds::Empty {
                // Skip only the target's main-camera visibility. Reflected room
                // objects remain unrestricted by the main camera below.
                continue;
            }
            let view = view_for_target(
                draw,
                reflection.resolution_scale,
                reflection.clip_bias,
                self.width,
                self.height,
                lighting.planar_resolution_limit,
            )?;
            self.prepare_planar_resource(&view);
            views.push(view);
        }
        let quality = lighting.params.reflection0[1];
        if views.len() < lighting.planar_capture_budget as usize
            && self.hybrid_stats.1 >= 32_768
            && quality.is_finite()
            && quality >= 0.5
            && quality < 3.5
            && u64::from(self.width) * u64::from(self.height) > 16_384
        {
            let glass_enabled = automatic_glass_enabled() && self.planar_slab_pipelines.is_some();
            let mut actor_counts = HashMap::<&str, usize>::new();
            for draw in draws {
                *actor_counts.entry(&draw.instance_key.actor_id).or_default() += 1;
            }
            let mut automatic = Vec::new();
            for draw in draws {
                // Current capture bindings and exclusion are actor-wide. Do
                // not infer one material/plane for an imported multi-draw actor.
                if actor_counts[draw.instance_key.actor_id.as_str()] != 1
                    || (draw.phase == GpuWorldDrawPhase::Transmissive && !glass_enabled)
                    || lighting
                        .planar_reflections
                        .iter()
                        .any(|r| r.target == draw.instance_key.actor_id)
                {
                    continue;
                }
                if draw.phase == GpuWorldDrawPhase::Transmissive
                    && self.shared_actor_geometry(draw.vertex_signature,&draw.vertices,&draw.indices).chunks.len() != 1 {
                    continue;
                }
                if let Some(candidate) = automatic_view_for_target(
                    draw,
                    &lighting.params,
                    self.width,
                    self.height,
                    lighting.planar_resolution_limit,
                ) {
                    if draw.phase == GpuWorldDrawPhase::Transmissive
                        && !certified_slab_face_runs(draw,&candidate.1,&lighting.params)
                            .iter().any(|run| run.cached) {
                        continue;
                    }
                    #[cfg(not(target_arch = "wasm32"))]
                    if std::env::var_os("MOTIONLOOM_TRACE_BATCHES").is_some() {
                        eprintln!(
                            "motionloom automatic planar candidate: {} phase={:?} face_priority={:.1}",
                            candidate.1.target, draw.phase, candidate.0
                        );
                    }
                    automatic.push(candidate);
                }
            }
            for (_priority, view) in group_automatic_views(automatic, draws) {
                if views.len() >= lighting.planar_capture_budget as usize {
                    break;
                }
                #[cfg(not(target_arch = "wasm32"))]
                if std::env::var_os("MOTIONLOOM_TRACE_BATCHES").is_some() {
                    eprintln!(
                        "motionloom automatic planar target: {} members={} face_priority={_priority:.1} size={}x{}",
                        view.target,
                        view.members.len(),
                        view.width,
                        view.height
                    );
                }
                self.prepare_planar_resource(&view);
                views.push(view);
            }
        }
        // Automatic targets can change during camera motion. Retain authored
        // resources as before, but bound automatic resources to active captures.
        self.planar_resources.retain(|target, _| {
            views.iter().any(|v| &v.target == target)
                || lighting
                    .planar_reflections
                    .iter()
                    .any(|r| &r.target == target)
        });
        Ok(views)
    }

    fn capture_actor_binding(
        &self,
        draw: &GpuWorldDraw,
        view: &PlanarView,
        jitter: [f32; 2],
    ) -> wgpu::BindGroup {
        let mut p = reflected_draw_params(draw, view, [self.width, self.height], jitter);
        p.material11[1] = self
            .hybrid_object_ids
            .get(&draw.instance_key)
            .copied()
            .unwrap_or(u32::MAX) as f32;
        let id = draw
            .instance_key
            .actor_id
            .split("::")
            .next()
            .unwrap_or(&draw.instance_key.actor_id);
        let flags = self
            .per_light_shadows
            .model_flags
            .get(id)
            .copied()
            .unwrap_or([true, true]);
        p.material10[2] = flags[0] as u8 as f32;
        p.material10[3] = flags[1] as u8 as f32;
        p.motion0 = [draw.bone_matrices.len().max(1) as f32, 0.0, 0.0, 0.0];
        if draw.phase == GpuWorldDrawPhase::Transmissive {
            p.material8[3] = closed_mesh(&draw.vertices, &draw.indices) as u8 as f32;
        }
        let params = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("planar-actor-params"),
            size: std::mem::size_of::<GpuWorldParams>() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue
            .write_buffer(&params, 0, &pack_gpu_world_params(p));
        let bone_bytes = pack_gpu_world_bone_pair(&draw.bone_matrices, &draw.bone_matrices);
        let bones = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("planar-actor-bones"),
            size: bone_bytes.len().max(64) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&bones, 0, &bone_bytes);
        let mut clip = view.params;
        clip.control[0] = 0.0;
        clip.control[1] = 1.0;
        let uniform = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("planar-capture-plane"),
            size: 96,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        self.queue.write_buffer(&uniform, 0, &clip.bytes());
        let r = &self.actor_resource_cache[&draw.resource_key];
        let fallback = self.planar_default_texture.create_view(&Default::default());
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("planar-capture-actor"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: bones.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&r.texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.actor_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&r.normal_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(
                        &r.metallic_roughness_texture.view,
                    ),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: wgpu::BindingResource::TextureView(&r.emissive_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 7,
                    resource: wgpu::BindingResource::TextureView(&r.cel_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::TextureView(&r.occlusion_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: wgpu::BindingResource::TextureView(&fallback),
                },
                wgpu::BindGroupEntry {
                    binding: 10,
                    resource: wgpu::BindingResource::Sampler(&self.planar_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 11,
                    resource: uniform.as_entire_binding(),
                },
            ],
        })
    }

    pub(super) fn encode_planar_captures(
        &self,
        mut encoder: wgpu::CommandEncoder,
        submissions: &mut TransportSubmissions,
        draws: &[GpuWorldDraw],
        views: &[PlanarView],
        lighting: &wgpu::BindGroup,
        layer_budget: u32,
        jitter: [f32; 2],
    ) -> (wgpu::CommandEncoder, usize) {
        let mut submitted = 0usize;
        for view in views {
            let viewport = [view.width, view.height];
            let target_bounds = draws
                .iter()
                .filter(|draw| view.matches_target(&draw.instance_key.actor_id))
                .fold(transport_tiles::ScreenBounds::Empty, |bounds, draw| {
                    bounds.union(draw_screen_bounds(
                        draw,
                        reflected_draw_params(draw, view, [self.width, self.height], jitter),
                        viewport,
                    ))
                });
            // Match the actual reflected-camera lookup rather than merely
            // scaling main pixels: authored thickness and bevels can project
            // differently away from the fitted plane. Include GGX blur taps.
            let halo = ((u64::from(view.width.max(view.height)) * 12).div_ceil(1000) + 4) as u32;
            let Some(mut target_clip) = capture_clip(target_bounds, viewport, halo) else {
                continue;
            };
            #[cfg(not(target_arch = "wasm32"))]
            if std::env::var_os("MOTIONLOOM_TRACE_BATCHES").is_some() {
                eprintln!(
                    "motionloom planar clip initial: {} target_bounds={target_bounds:?} halo={halo} scissor={target_clip:?}",
                    view.target
                );
            }
            let r = &self.planar_resources[&view.target];
            let color = r.color.create_view(&Default::default());
            let depth = r.depth.create_view(&Default::default());
            let bindings: Vec<_> = draws
                .iter()
                .filter_map(|d| {
                    if d.indices.is_empty() || view.matches_target(&d.instance_key.actor_id) {
                        return None;
                    }
                    let geometry = &self.actor_resource_cache[&d.resource_key].geometry;
                    let params = reflected_draw_params(d, view, [self.width, self.height], jitter);
                    let bounds = if d.bone_matrices.iter().all(|m| *m == mat4_identity()) {
                        geometry.rigid_bounds
                    } else {
                        None
                    };
                    let bounds = transport_tiles::ScreenBounds::bounds(
                        bounds,
                        params,
                        [view.width, view.height],
                    );
                    (bounds != transport_tiles::ScreenBounds::Empty)
                        .then(|| (d, self.capture_actor_binding(d, view, jitter), bounds))
                })
                .collect();
            // A slab can sample outside its own pixel rectangle. Inflate the
            // dependency region for each reached pane once, revisiting panes
            // reached by that inflation. Solids trace the complete BVH and do
            // not depend on an underlay snapshot. Uncertain bounds retain Full.
            let mut slab_padding = vec![0; bindings.len()];
            let mut iterations = 0;
            loop {
                iterations += 1;
                if iterations > 32 {
                    target_clip = [0, 0, view.width, view.height];
                    break;
                }
                let mut changed = false;
                for (index, (draw, _, bounds)) in bindings.iter().enumerate() {
                    if draw.phase != GpuWorldDrawPhase::Transmissive
                        || draw.params.material11[0] > 0.5
                        || !bounds.intersects(target_clip)
                    {
                        continue;
                    }
                    let params =
                        reflected_draw_params(draw, view, [self.width, self.height], jitter);
                    let padding = clipped_slab_snapshot_padding(
                        draw,
                        params,
                        viewport,
                        jitter,
                        view.params.plane,
                        view.params.control[2],
                        target_clip,
                    );
                    if padding.is_some_and(|padding| padding <= slab_padding[index]) {
                        continue;
                    }
                    changed = true;
                    let _previous_clip = target_clip;
                    target_clip = match padding {
                        Some(padding) => {
                            let increase = padding - slab_padding[index];
                            slab_padding[index] = padding;
                            inflate_capture_clip(target_clip, viewport, increase)
                        }
                        None => [0, 0, view.width, view.height],
                    };
                    #[cfg(not(target_arch = "wasm32"))]
                    if std::env::var_os("MOTIONLOOM_TRACE_BATCHES").is_some() {
                        eprintln!(
                            "motionloom planar slab dependency: target={} slab={} bounds={bounds:?} padding={padding:?} before={_previous_clip:?} after={target_clip:?} optics={:?}",
                            view.target, draw.instance_key.actor_id, params.material6
                        );
                    }
                    if target_clip == [0, 0, view.width, view.height] {
                        break;
                    }
                }
                if !changed || target_clip == [0, 0, view.width, view.height] {
                    break;
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            if std::env::var_os("MOTIONLOOM_TRACE_BATCHES").is_some() {
                eprintln!(
                    "motionloom planar clip: {} viewport={}x{} scissor={target_clip:?}",
                    view.target, view.width, view.height
                );
            }
            let tiles = capture_tiles(
                target_clip,
                self.hybrid_stats.1 >= 32_768
                    && u64::from(view.width) * u64::from(view.height) > 16_384,
            );
            let bounded = tiles.len() > 1;
            let target = draws
                .iter()
                .find(|d| d.instance_key.actor_id == view.target)
                .expect("validated planar target");
            let background_binding = self.capture_actor_binding(target, view, jitter);
            let extent = wgpu::Extent3d {
                width: view.width,
                height: view.height,
                depth_or_array_layers: 1,
            };
            let snapshot = r.snapshot.create_view(&Default::default());
            let depth_snapshot = r.depth_snapshot.create_view(&Default::default());
            let scene_binding = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("planar-glass-scene"),
                layout: &self.transmission_scene_bind_group_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&snapshot),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.planar_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&depth_snapshot),
                    },
                ],
            });
            {
                // Cheap nearest coverage prevents hidden room geometry from
                // tracing specular rays in the reflected camera as well.
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("planar-opaque-visibility-prepass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &snapshot,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_scissor_rect(
                    target_clip[0],
                    target_clip[1],
                    target_clip[2],
                    target_clip[3],
                );
                pass.set_pipeline(&self.opaque_depth_pipeline);
                pass.set_bind_group(1, lighting, &[]);
                for (draw, binding, _) in bindings.iter().filter(|(draw, _, bounds)| {
                    draw.phase == GpuWorldDrawPhase::Opaque
                        && draw.depth_write
                        && bounds.intersects(target_clip)
                }) {
                    pass.set_bind_group(0, binding, &[]);
                    for chunk in &self.actor_resource_cache[&draw.resource_key]
                        .geometry
                        .chunks
                    {
                        pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            chunk.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..chunk.index_count, 0, 0..1);
                        submitted += 1;
                    }
                }
            }
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &r.depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &r.depth_snapshot,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                extent,
            );
            {
                // Clear once before bounded surface passes. Scissor tiles keep
                // the full reflected viewport and material derivatives.
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("planar-reflected-background"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(0.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_scissor_rect(
                    target_clip[0],
                    target_clip[1],
                    target_clip[2],
                    target_clip[3],
                );
                pass.set_bind_group(1, lighting, &[]);
                pass.set_pipeline(&self.planar_background_pipeline);
                pass.set_bind_group(0, &background_binding, &[]);
                pass.draw(0..3, 0..1);
                submitted += 1;
            }
            if bounded {
                encoder =
                    self.submit_transport_encoder(encoder, submissions, "planar-pre-color-encoder");
            }
            for tile in &tiles {
                if !bindings.iter().any(|(draw, _, bounds)| {
                    draw.phase == GpuWorldDrawPhase::Opaque
                        && draw.depth_write
                        && bounds.intersects(*tile)
                }) {
                    continue;
                }
                // The reflected basis reverses winding. Surface pipelines are
                // uncullled; full-resolution normals and authored order remain.
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("planar-reflected-room-tile"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &color,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_scissor_rect(tile[0], tile[1], tile[2], tile[3]);
                pass.set_bind_group(1, lighting, &[]);
                pass.set_pipeline(&self.capture_opaque_pipeline);
                pass.set_bind_group(2, &scene_binding, &[]);
                submissions.passes[0] += 1;
                for (draw, binding, _) in bindings.iter().filter(|(d, _, bounds)| {
                    d.phase == GpuWorldDrawPhase::Opaque
                        && d.depth_write
                        && bounds.intersects(*tile)
                }) {
                    pass.set_bind_group(0, binding, &[]);
                    for chunk in &self.actor_resource_cache[&draw.resource_key]
                        .geometry
                        .chunks
                    {
                        pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            chunk.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..chunk.index_count, 0, 0..1);
                        submitted += 1;
                        submissions.draws[0] += 1;
                    }
                }
                drop(pass);
                if bounded {
                    encoder = self.submit_transport_encoder(
                        encoder,
                        submissions,
                        "planar-opaque-tile-encoder",
                    );
                }
            }
            let extent = wgpu::Extent3d {
                width: view.width,
                height: view.height,
                depth_or_array_layers: 1,
            };
            encoder.copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &r.depth,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &r.depth_snapshot,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::DepthOnly,
                },
                extent,
            );
            let camera_depth = |d: &GpuWorldDraw| {
                (0..3)
                    .map(|i| (d.params.actor[i] - view.params.camera0[i]) * view.params.camera3[i])
                    .sum::<f32>()
            };
            let mut transparent: Vec<_> = bindings
                .iter()
                .filter(|(d, _, _)| d.phase != GpuWorldDrawPhase::Opaque || !d.depth_write)
                .collect();
            transparent.sort_by(|(a, _, _), (b, _, _)| {
                a.sort_priority
                    .cmp(&b.sort_priority)
                    .then_with(|| camera_depth(b).total_cmp(&camera_depth(a)))
            });
            let mut layers = 0;
            let mut previous: Option<(&GpuWorldInstanceKey, bool)> = None;
            let mut refract = false;
            for (draw, binding, bounds) in transparent {
                let solid = draw.params.material11[0] > 0.5;
                if draw.phase == GpuWorldDrawPhase::Transmissive
                    && previous != Some((&draw.instance_key, solid))
                {
                    previous = Some((&draw.instance_key, solid));
                    // Solid transport owns its complete BVH ray and does not
                    // require a slab snapshot. A pane budget must not replace
                    // its geometry exit and absorption with alpha blending.
                    refract = solid || layers < layer_budget;
                    if refract && !solid {
                        encoder.copy_texture_to_texture(
                            wgpu::TexelCopyTextureInfo {
                                texture: &r.color,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            wgpu::TexelCopyTextureInfo {
                                texture: &r.snapshot,
                                mip_level: 0,
                                origin: wgpu::Origin3d::ZERO,
                                aspect: wgpu::TextureAspect::All,
                            },
                            extent,
                        );
                        layers += 1;
                    }
                }
                // A slab owns one complete underlay snapshot, then every
                // intersecting tile finishes before the next pane can copy it.
                for tile in &tiles {
                    if !bounds.intersects(*tile) {
                        continue;
                    }
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("planar-ordered-glass-tile"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &color,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &depth,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_scissor_rect(tile[0], tile[1], tile[2], tile[3]);
                    pass.set_bind_group(1, lighting, &[]);
                    if draw.phase == GpuWorldDrawPhase::Transmissive && refract {
                        pass.set_pipeline(if draw.depth_write {
                            &self.capture_transmissive_depth_write_pipeline
                        } else {
                            &self.capture_transmissive_pipeline
                        });
                        pass.set_bind_group(2, &scene_binding, &[]);
                    } else {
                        pass.set_pipeline(if draw.depth_write {
                            &self.planar_pipeline
                        } else {
                            &self.capture_transparent_pipeline
                        });
                    }
                    pass.set_bind_group(0, binding, &[]);
                    submissions.passes[1] += 1;
                    for chunk in &self.actor_resource_cache[&draw.resource_key]
                        .geometry
                        .chunks
                    {
                        pass.set_vertex_buffer(0, chunk.vertex_buffer.slice(..));
                        pass.set_index_buffer(
                            chunk.index_buffer.slice(..),
                            wgpu::IndexFormat::Uint32,
                        );
                        pass.draw_indexed(0..chunk.index_count, 0, 0..1);
                        submitted += 1;
                        submissions.draws[1] += 1;
                    }
                    drop(pass);
                    if bounded {
                        encoder = self.submit_transport_encoder(
                            encoder,
                            submissions,
                            "planar-transparent-tile-encoder",
                        );
                    }
                }
            }
        }
        (encoder, submitted)
    }
}
