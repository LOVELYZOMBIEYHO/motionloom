// src/weaver/tests/glass.rs
//! Analytic optical fixtures exercise the actual path kernel, without sibling assets.
use super::super::{backend::wgpu::Gpu, geometry, output, scene::Snapshot};
use crate::experimental::geometry::ResolvedMesh;
use crate::world::gltf_loader::{GlbMaterialData, GlbTextureData};
use crate::world::{WorldCamera, WorldLighting};
use std::sync::Arc;

fn optical_mesh(material: GlbMaterialData, front: f32, back: Option<f32>) -> ResolvedMesh {
    let z = back.unwrap_or(front);
    let vertices = [
        [-100., -100., z],
        [100., -100., z],
        [100., 100., z],
        [-100., 100., z],
        [-100., -100., front],
        [100., -100., front],
        [100., 100., front],
        [-100., 100., front],
    ];
    let faces = [
        (4, 5, 6),
        (4, 6, 7),
        (1, 0, 3),
        (1, 3, 2),
        (0, 4, 7),
        (0, 7, 3),
        (5, 1, 2),
        (5, 2, 6),
        (3, 7, 6),
        (3, 6, 2),
        (0, 1, 5),
        (0, 5, 4),
    ];
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    for &(a, b, c) in faces.iter().take(if back.is_some() { 12 } else { 2 }) {
        let a = vertices[a];
        let b = vertices[b];
        let c = vertices[c];
        let e: [f32; 3] = std::array::from_fn(|i| b[i] - a[i]);
        let f: [f32; 3] = std::array::from_fn(|i| c[i] - a[i]);
        let cross = [
            e[1] * f[2] - e[2] * f[1],
            e[2] * f[0] - e[0] * f[2],
            e[0] * f[1] - e[1] * f[0],
        ];
        let length = cross.iter().map(|v| v * v).sum::<f32>().sqrt();
        let normal = cross.map(|v| v / length);
        positions.extend([a, b, c]);
        normals.extend([normal; 3]);
    }
    let count = positions.len();
    let texture = |rgba: [u8; 4]| GlbTextureData {
        width: 1,
        height: 1,
        rgba: Arc::new(Vec::from(rgba)),
    };
    ResolvedMesh {
        name: "optical-fixture".into(),
        positions,
        normals,
        tangents: vec![[1., 0., 0., 1.]; count],
        uvs: vec![[0.5, 0.5]; count],
        colors: vec![material.base_color_factor; count],
        indices: (0..count as u32).collect(),
        material,
        textures: vec![
            texture([255; 4]),
            texture([128, 128, 255, 255]),
            texture([255, 255, 255, 255]),
            texture([255; 4]),
            texture([255; 4]),
        ],
        uv_source: "fixture".into(),
    }
}
fn snapshot(meshes: Vec<ResolvedMesh>) -> Snapshot {
    let count = meshes.len();
    Snapshot {
        meshes,
        camera: WorldCamera::default(),
        lighting: WorldLighting::default(),
        time_seconds: 0.,
        diagnostics: vec![],
        composition: crate::scene::compositor::SceneCompositionPlan::new([4, 4], [4, 4]),
        composition_layers: vec![],
        primary_camera_visibility: vec![true; count],
    }
}
fn glass(solid: bool, ior: f32, color: [f32; 3]) -> GlbMaterialData {
    GlbMaterialData {
        metallic_factor: 0.,
        roughness_factor: 0.04,
        normal_scale: 0.,
        transmission_factor: 1.,
        ior,
        thickness_factor: 1.,
        attenuation_color: color,
        attenuation_distance: 1.,
        refraction_mode: if solid {
            crate::dsl::MaterialRefractionMode::Solid
        } else {
            crate::dsl::MaterialRefractionMode::Slab
        },
        ..Default::default()
    }
}

#[test]
fn solid_optics_require_closed_oriented_mesh_and_pack_every_lobe() {
    let material = GlbMaterialData {
        clearcoat: 0.6,
        sheen: 0.4,
        sheen_color: [0.8, 0.2, 0.1],
        ..glass(true, 1.52, [0.5, 0.8, 1.])
    };
    let closed = snapshot(vec![optical_mesh(material.clone(), 1., Some(0.))]);
    let packed = geometry::pack(&closed, false, false).unwrap();
    let m = packed.material_offset as usize;
    assert_eq!(packed.data[m + 10], [1., 1., 1., 0.]);
    assert_eq!(packed.data[m + 11], [0.5, 0.8, 1., 1.]);
    assert_eq!(packed.data[m + 12][..3], [0.6, 0.1, 0.4]);
    assert_eq!(packed.data[m + 13][..3], [0.8, 0.2, 0.1]);
    let table = packed.data[m + 14][0].to_bits() as usize;
    assert!(table > m);
    assert_eq!(packed.light_offset as usize, table + 4096);
    let open = snapshot(vec![optical_mesh(material, 1., None)]);
    let error = geometry::pack(&open, false, false).err().unwrap();
    assert!(error.to_string().contains("closed consistently oriented"));
    // An explicit migration stopgap remains opt-in and disables transmission.
    let fallback = geometry::pack(&open, false, true).unwrap();
    assert_eq!(fallback.data[fallback.material_offset as usize + 10][0], 0.);
    let mut winding = snapshot(vec![closed.meshes[0].clone()]);
    winding.meshes[0].indices.swap(0, 1);
    assert!(
        geometry::pack(&winding, false, false)
            .err()
            .unwrap()
            .to_string()
            .contains("winding")
    );
}

fn constant_white_energy(meshes: Vec<ResolvedMesh>, samples: u32) -> [f32; 3] {
    let mut packed = geometry::pack(&snapshot(meshes), false, false).unwrap();
    let env = packed.data.len() as u32;
    packed.data.push([1., 1., 1., 0.]);
    let gpu = pollster::block_on(Gpu::new(&packed)).unwrap();
    let mut p = [[0.; 4]; 26];
    p[0] = [4., 4., 32., f32::from_bits(4395)];
    p[1] = [
        f32::from_bits(packed.triangle_offset),
        f32::from_bits(packed.material_offset),
        f32::from_bits(packed.light_offset),
        0.,
    ];
    p[2] = [0., 0., 3., 0.];
    p[3] = [0., 0., -1., 0.02];
    p[4] = [1., 0., 0., 1.];
    p[5] = [0., 1., 0., 0.];
    p[7] = [samples as f32, samples as f32, 0., 16.];
    p[8] = [16., 12., 16., 16.];
    p[9][3] = 64.;
    p[10] = [f32::from_bits(env), 1., 1., 0.];
    p[11] = [1.; 4];
    p[12] = [0., 0., 4., 4.];
    p[19] = [1.; 4];
    let tile = gpu.tile(
        16,
        &vec![0; 16 * super::super::backend::wgpu::FILM_BYTES_PER_PIXEL],
    );
    let mut raw = Vec::new();
    for _ in 0..samples.div_ceil(32) {
        raw = gpu.batch(&tile, &p).unwrap();
    }
    let film = output::floats(&raw);
    let mut mean = [0.; 3];
    for pixel in film.chunks_exact(super::super::backend::wgpu::FILM_FLOATS_PER_PIXEL) {
        assert!(pixel.iter().all(|v| v.is_finite()));
        assert_eq!(pixel[7], 0., "kernel recorded invalid sample");
        assert_eq!(pixel[3], samples as f32);
        for c in 0..3 {
            mean[c] += pixel[c] / pixel[3] / 16.;
        }
    }
    mean
}

#[test]
#[ignore = "requires native GPU; checks actual Beer absorption, nested media and dielectric energy"]
fn gpu_dielectric_slab_solid_and_nested_absorption() {
    // IOR=1 removes Fresnel/refraction, leaving exact Beer-Lambert predictions.
    let slab = constant_white_energy(
        vec![optical_mesh(glass(false, 1., [0.5, 0.8, 1.]), 1., None)],
        64,
    );
    let solid = constant_white_energy(
        vec![optical_mesh(glass(true, 1., [0.5, 0.8, 1.]), 1., Some(0.))],
        64,
    );
    let nested = constant_white_energy(
        vec![
            optical_mesh(glass(true, 1., [0.5, 0.8, 1.]), 1., Some(-1.)),
            optical_mesh(glass(true, 1., [0.8, 0.5, 1.]), 0.5, Some(-0.5)),
        ],
        64,
    );
    for (actual, expected) in [
        (slab, [0.5, 0.8, 1.]),
        (solid, [0.5, 0.8, 1.]),
        (nested, [0.4, 0.4, 1.]),
    ] {
        for c in 0..3 {
            assert!(
                (actual[c] - expected[c]).abs() < 0.015,
                "Beer energy {actual:?}, expected {expected:?}"
            );
        }
    }
    // White illumination through a non-absorbing rough glass boundary must not
    // create energy. Radiance eta factors cancel on actual entry and exit.
    for ior in [1.3, 1.52, 2.0] {
        let energy = constant_white_energy(
            vec![optical_mesh(glass(true, ior, [1.; 3]), 1., Some(0.))],
            512,
        );
        assert!(
            energy.iter().all(|v| (0.90..=1.08).contains(v)),
            "IOR {ior}: energy {energy:?}"
        );
    }
    eprintln!("slab {slab:?}, solid {solid:?}, nested {nested:?}");
}

#[test]
#[ignore = "requires native GPU; validates clearcoat/Charlie energy on the integrated kernel"]
fn gpu_coat_and_sheen_energy_remain_bounded() {
    for (coat, sheen, rough) in [
        (0., 0., 0.5),
        (1., 0., 0.1),
        (0., 1., 0.5),
        (1., 1., 0.5),
        (1., 1., 1.),
    ] {
        let material = GlbMaterialData {
            base_color_factor: [0.8, 0.8, 0.8, 1.],
            metallic_factor: 0.,
            roughness_factor: rough,
            clearcoat: coat,
            clearcoat_roughness: 0.1,
            sheen,
            sheen_roughness: rough,
            normal_scale: 0.,
            ..Default::default()
        };
        let energy = constant_white_energy(vec![optical_mesh(material, 1., None)], 1024);
        assert!(
            energy.iter().all(|v| *v > 0.4 && *v <= 1.08),
            "coat {coat}, sheen {sheen}, rough {rough}: {energy:?}"
        );
        eprintln!("coat {coat}, sheen {sheen}, rough {rough}: {energy:?}");
    }
}

#[test]
#[ignore = "requires native GPU; deterministic exact Fresnel, Snell and TIR helper verification"]
fn gpu_dielectric_fresnel_snell_tir_and_internal_absorption() {
    let packed = geometry::pack(
        &snapshot(vec![optical_mesh(glass(false, 1.52, [1.; 3]), 1., None)]),
        false,
        false,
    )
    .unwrap();
    let gpu = pollster::block_on(Gpu::new(&packed)).unwrap();
    let mut p = [[0.; 4]; 26];
    p[1] = [
        f32::from_bits(packed.triangle_offset),
        f32::from_bits(packed.material_offset),
        f32::from_bits(packed.light_offset),
        0.,
    ];
    let values = gpu.optical_contract_probe(&p).unwrap();
    let first = &values[..4];
    let second = &values[super::super::backend::wgpu::FILM_FLOATS_PER_PIXEL..][..4];
    assert!(
        (first[0] - 0.042579995).abs() < 1e-5,
        "normal Fresnel {first:?}"
    );
    assert_eq!(first[1], 1., "TIR must reflect all energy");
    assert!((first[2] - 1.).abs() < 1e-5, "nonabsorbing slab energy");
    assert!(
        (first[3] - 0.8660254 / 1.52).abs() < 1e-5,
        "Snell refraction {first:?}"
    );
    assert!(
        (second[0] - 0.8660254).abs() < 1e-5,
        "matched parallel exit direction"
    );
    assert!(
        second[1] > 0.49 && second[1] < 0.53,
        "absorbing round-trip total {second:?}"
    );
    assert!(
        second[2] < 0.5,
        "internal reflection loses additional absorption"
    );
    assert!((second[3] - 0.8122524).abs() < 1e-5, "Beer 0.3m {second:?}");
}
