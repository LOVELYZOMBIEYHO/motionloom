//! Portable bake loading and retained-cache contracts, without a GPU device.
use super::super::baked::{PROBE_STRIDE, load_baked_lighting, validate};
use crate::lighting_bake::*;

fn asset() -> BakedLightingAsset {
    let probes = (0..8)
        .map(|index| IrradianceProbe {
            position: [
                (index & 1) as f32,
                ((index >> 1) & 1) as f32,
                ((index >> 2) & 1) as f32,
            ],
            valid: true,
            irradiance_sh: [[1.0, 2.0, 3.0]; 9],
            depth_moments: vec![[10.0, 100.0]; 64],
            visibility: vec![0.0; 64],
        })
        .collect();
    let volume = BakedLightingVolume {
        id: "room".into(),
        bounds_min: [0.0; 3],
        bounds_max: [1.0; 3],
        counts: [2; 3],
        probes,
        reflections: Vec::new(),
    };
    BakedLightingAsset {
        schema_version: LIGHTING_BAKE_SCHEMA_VERSION,
        baker_version: LIGHTING_BAKER_VERSION.into(),
        scene_id: "room".into(),
        authoring_fingerprint: String::new(),
        fingerprint: BakeFingerprint {
            geometry: "geometry".into(),
            materials: "materials".into(),
            textures: "textures".into(),
            lighting: "lights".into(),
            environments: "environments".into(),
            settings: "settings".into(),
            combined: "combined".into(),
        },
        states: ["day", "dusk"]
            .into_iter()
            .enumerate()
            .map(|(index, name)| BakedLightingState {
                name: name.into(),
                frame: index as u32,
                volumes: vec![volume.clone()],
            })
            .collect(),
        diagnostics: Vec::new(),
    }
}

fn reflection() -> LocalReflectionProbe {
    LocalReflectionProbe {
        position: [0.5; 3],
        bounds_min: [0.0; 3],
        bounds_max: [1.0; 3],
        src: "reflections/room.hdr".into(),
    }
}

#[test]
fn baked_asset_validation_requires_two_compatible_states_and_regular_grids() {
    let valid = asset();
    validate(&valid).unwrap();
    let mut invalid = valid.clone();
    invalid.schema_version += 1;
    assert!(validate(&invalid).is_err());
    let mut invalid = valid.clone();
    invalid.states.pop();
    assert!(validate(&invalid).is_err());
    let mut invalid = valid.clone();
    invalid.states[1].volumes[0].counts[0] = 3;
    assert!(validate(&invalid).is_err());
    for count in [0, 1, 33] {
        let mut invalid = valid.clone();
        for state in &mut invalid.states {
            state.volumes[0].counts[0] = count;
        }
        assert!(validate(&invalid).is_err());
    }
    let mut invalid = valid;
    for state in &mut invalid.states {
        state.volumes[0].bounds_max[1] = 0.0;
    }
    assert!(validate(&invalid).is_err());
}

#[test]
fn baked_probe_payload_rejects_incomplete_or_nonfinite_data() {
    for bad in 0..7 {
        let mut invalid = asset();
        let probe = &mut invalid.states[0].volumes[0].probes[0];
        match bad {
            0 => {
                probe.depth_moments.pop();
            }
            1 => {
                probe.visibility.pop();
            }
            2 => probe.position[0] = f32::NAN,
            3 => probe.irradiance_sh[4][2] = f32::INFINITY,
            4 => probe.depth_moments[0][0] = -1.0,
            5 => probe.depth_moments[0][1] = f32::NAN,
            6 => probe.visibility[0] = 1.01,
            _ => unreachable!(),
        }
        assert!(
            validate(&invalid).is_err(),
            "accepted malformed probe {bad}"
        );
    }
    let mut invalid = asset();
    invalid.states[0].volumes[0].probes.pop();
    assert!(validate(&invalid).is_err());
}

#[test]
fn baked_reflection_v1_requires_one_finite_matching_capture_per_room() {
    let mut valid = asset();
    for state in &mut valid.states {
        state.volumes[0].reflections.push(reflection());
    }
    validate(&valid).unwrap();
    let mut invalid = valid.clone();
    for state in &mut invalid.states {
        state.volumes[0].reflections.push(reflection());
    }
    assert!(validate(&invalid).is_err());
    let mut invalid = valid.clone();
    invalid.states[1].volumes[0].reflections.clear();
    assert!(validate(&invalid).is_err());
    let mut invalid = valid.clone();
    invalid.states[1].volumes[0].reflections[0].position[0] = 0.7;
    assert!(validate(&invalid).is_err());
    let mut invalid = valid.clone();
    for state in &mut invalid.states {
        state.volumes[0].reflections[0].bounds_min[0] = f32::NAN;
    }
    assert!(validate(&invalid).is_err());
    let mut invalid = valid;
    invalid.states[1].volumes[0].reflections[0].src.clear();
    assert!(validate(&invalid).is_err());
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn baked_json_loader_packs_two_states_and_reuses_content_until_asset_changes() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "motionloom-baked-contract-{}-{}.json",
        std::process::id(),
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    ));
    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = RemoveOnDrop(path.clone());
    let mut source = asset();
    for probe in &mut source.states[1].volumes[0].probes {
        probe.irradiance_sh = [[4.0, 5.0, 6.0]; 9];
    }
    std::fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let binding = crate::world::WorldBakedLighting {
        expected_authoring_fingerprint: String::new(),
        src: path.to_string_lossy().into_owned(),
        blend: 0.5,
        intensity: 1.0,
        specular_intensity: 1.0,
    };
    let root = path.parent().unwrap();
    let first = load_baked_lighting(&binding, root, &crate::PathAssetResolver).unwrap();
    assert_eq!(first.volume_count, 1);
    assert_eq!(first.vectors.len(), 4 + 8 * PROBE_STRIDE);
    assert!(first.reflections.is_empty());
    let base = first.vectors[0][3] as usize;
    assert_eq!(first.vectors[base], [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(first.vectors[base + 2], [1.0, 2.0, 3.0, 0.0]);
    assert_eq!(first.vectors[base + 11], [4.0, 5.0, 6.0, 0.0]);
    // Terminal transmission remains diagnostic and is retained losslessly.
    assert_eq!(first.vectors[base + 20], [10.0, 100.0, 0.0, 0.0]);
    assert_eq!(first.vectors[base + PROBE_STRIDE], [1.0, 0.0, 0.0, 1.0]);
    let second = load_baked_lighting(&binding, root, &crate::PathAssetResolver).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let mut stale = binding.clone();
    stale.expected_authoring_fingerprint = "changed source".into();
    assert!(
        load_baked_lighting(&stale, root, &crate::PathAssetResolver).is_err(),
        "warm cache must reject a stale source"
    );
    // Different byte length invalidates even a coarse filesystem timestamp.
    source.diagnostics.push("asset dependency changed".into());
    std::fs::write(&path, serde_json::to_vec(&source).unwrap()).unwrap();
    let changed = load_baked_lighting(&binding, root, &crate::PathAssetResolver).unwrap();
    assert_ne!(first.signature, changed.signature);
    assert!(!Arc::ptr_eq(&first, &changed));
    let stale = crate::world::WorldBakedLighting {
        expected_authoring_fingerprint: "different-scene-source".into(),
        ..binding
    };
    assert!(
        load_baked_lighting(&stale, root, &crate::PathAssetResolver)
            .unwrap_err()
            .to_string()
            .contains("stale"),
        "a cached asset must still reject a changed authoring fingerprint"
    );
}

#[cfg(not(target_arch = "wasm32"))]
fn constant_sh_world_fixture(root: &std::path::Path) -> crate::world::WorldGraph {
    use crate::world::*;
    // Parse the current material/geometry syntax, then use the internal World
    // bridge directly so this deliberately legacy bake has no source guard.
    let parsed = crate::parse_graph_script(r##"<Graph fps="24" duration="1s" size={[64,64]}>
<Assets><MaterialAsset id="receiver_material" baseColor="#FFFFFF" metallic="0" roughness="1" specular="0" />
<GeometryAsset id="receiver_geometry"><Primitive shape="box" size={[0.45,0.45,0.08]} /></GeometryAsset>
<MeshAsset id="receiver_mesh" material="receiver_material" geometry="receiver_geometry" /></Assets>
<Scene id="receiver_source"><Timeline><Track id="receiver_track"><Sequence duration="1s">
<CompositeGroup id="receiver_room" space="3d"><Model asset="receiver_mesh" /></CompositeGroup>
</Sequence></Track></Timeline></Scene><Present from="receiver_source" /></Graph>"##).unwrap();
    let primitive = parsed.assets.iter().find_map(|asset| asset.primitive()).unwrap().clone();
    let actor = WorldActor {
        id: "receiver".into(), model: "receiver_mesh".into(), primitive: Some(primitive),
        cel_materials: Vec::new(), terrain: None, vegetation: None, native_skin: None,
        path_style: WorldPathStyle::Relative, hide_meshes: Vec::new(), hide_materials: Vec::new(),
        camera_hidden_bones: Vec::new(), profile: None, rig: None, retarget: None,
        x: "0.5".into(), y: "0.5".into(), z: "0.5".into(), yaw: "0".into(),
        pitch: "0".into(), roll: "0".into(), rotation_quaternion: None,
        scale: "1".into(), scale_mode: "none".into(), opacity: "1".into(),
        material: Some(WorldMaterial { style: WorldMaterialStyle::Pbr, outline: false,
            outline_width: "0".into(), exposure: "1".into() }),
        play: None, plays: Vec::new(), material_color_overrides: Vec::new(),
    };
    let camera = WorldCamera {
        target_x: "0.5".into(), target_y: "0.5".into(), target_z: "0.5".into(),
        distance: "2.5".into(), ..Default::default()
    };
    let lighting = WorldLighting {
        ao_intensity: 0.0,
        environment: Some(WorldEnvironmentLighting {
            src: root.join("black.png").to_string_lossy().into_owned(), mapping: "equirectangular".into(),
            intensity: 0.0, rotation_y_degrees: 0.0, visible: false, background_intensity: 0.0,
            background_blur: 0.0, diffuse_intensity: 0.0, specular_intensity: 0.0,
        }),
        baked_lighting: Some(WorldBakedLighting {
            src: root.join("constant-sh.json").to_string_lossy().into_owned(),
            expected_authoring_fingerprint: String::new(), blend: 0.0, intensity: 1.0, specular_intensity: 0.0,
        }),
        color_management: WorldColorManagement { tone_mapping: "none".into(), ..Default::default() },
        ..Default::default()
    };
    WorldGraph {
        id: None, version: None, fps: 24.0, duration_ms: 1_000, duration_explicit: true,
        size: (64,64), render_size: None, model_profiles: Vec::new(),
        worlds: vec![WorldNode::new("constant_sh_room",camera,vec![actor])],
        retargets: Vec::new(), actions: Vec::new(), apply_actions: Vec::new(),
        animation_assets: Vec::new(), constraints: Vec::new(), attachments: Vec::new(), lighting,
        present: WorldPresent { from: "constant_sh_room".into() },
    }
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
#[ignore = "requires a native GPU adapter"]
fn baked_gpu_constant_sh_endpoints_and_interior_phases_match_analytic_irradiance() {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT_ID: AtomicU64 = AtomicU64::new(0);
    let _policy = super::super::transport_policy::test_reference_transport(false);
    let root = std::env::temp_dir().join(format!("motionloom-baked-gpu-{}-{}",
        std::process::id(), NEXT_ID.fetch_add(1,Ordering::Relaxed)));
    std::fs::create_dir_all(&root).unwrap();
    struct RemoveOnDrop(std::path::PathBuf);
    impl Drop for RemoveOnDrop {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    let _cleanup = RemoveOnDrop(root.clone());
    image::RgbaImage::from_pixel(2,1,image::Rgba([0,0,0,255])).save(root.join("black.png")).unwrap();
    let irradiance = [[0.8f32,0.2,0.1],[0.1f32,0.2,0.8]];
    let mut baked = asset();
    for (state, expected) in baked.states.iter_mut().zip(irradiance) {
        for probe in &mut state.volumes[0].probes {
            probe.irradiance_sh = [[0.0;3];9];
            // Y00 = 0.2820948. Every valid corner has the same constant
            // irradiance, and all depths exceed the receiver/probe distances.
            probe.irradiance_sh[0] = expected.map(|value| value / 0.2820948);
        }
    }
    validate(&baked).unwrap();
    std::fs::write(root.join("constant-sh.json"),serde_json::to_vec(&baked).unwrap()).unwrap();
    pollster::block_on(async {
        let mut graph = constant_sh_world_fixture(&root);
        let mut renderer = super::super::WorldFrameRenderer::new();
        renderer.set_immediate_preview_settings(crate::ImmediatePreviewSettings {
            profile: crate::ImmediatePreviewProfile::Portable, dynamic_resolution: false,
            min_resolution_scale: 1.0, ..Default::default()
        });
        let mut linear_pixels = Vec::new();
        for phase in [0.0f32,0.25,0.5,1.0] {
            graph.lighting.baked_lighting.as_mut().unwrap().blend = phase;
            let image = renderer.render_frame_gpu(&graph,0,&root).await.unwrap();
            let profile = renderer.last_frame_profile();
            assert!(profile.baked_probe_bytes > 0, "phase {phase}: bake was not bound");
            assert!(!profile.baked_lighting_fallback, "phase {phase}: unexpected stale fallback");
            assert_eq!(profile.hybrid_scene_bytes,0);
            let mut mean = [0.0f32;3];
            for y in 28..36 {
                for x in 28..36 {
                    let pixel = image.get_pixel(x,y).0;
                    assert_eq!(pixel[3],255,"receiver lost coverage at phase {phase}");
                    for channel in 0..3 {
                        mean[channel] += (pixel[channel] as f32 / 255.0).powf(2.2) / 64.0;
                    }
                }
            }
            for channel in 0..3 {
                let expected = (irradiance[0][channel] * (1.0-phase) + irradiance[1][channel] * phase)
                    / std::f32::consts::PI;
                assert!((mean[channel]-expected).abs() < 0.012,
                    "phase {phase}, channel {channel}: decoded {mean:?}, expected linear {expected}");
            }
            linear_pixels.push(mean);
        }
        assert!(linear_pixels[0][0] > linear_pixels[3][0] + 0.15,"day red endpoint missing");
        assert!(linear_pixels[3][2] > linear_pixels[0][2] + 0.15,"dusk blue endpoint missing");
        for channel in 0..3 {
            let middle = (linear_pixels[0][channel] + linear_pixels[3][channel]) * 0.5;
            assert!((linear_pixels[2][channel]-middle).abs() < 0.012,
                "midpoint did not interpolate linear irradiance: {linear_pixels:?}");
        }
    });
}
