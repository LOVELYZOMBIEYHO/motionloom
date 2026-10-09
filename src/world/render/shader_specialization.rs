// src/world/render/shader_specialization.rs
//! Conservative static shader selection; never remove a reachable optical path.

use super::{GpuWorldDraw, GpuWorldInstanceKey, GpuWorldLightingParams};
use std::collections::HashMap;

/// Physical contraction is valid only for the exact evaluated physical style.
/// Neighboring values, filmic glTF and nonfinite styles keep every shading path.
pub(super) fn physical_style_proven(style: f32) -> bool {
    style.is_finite() && style == 0.0
}

/// Diagnostic switches accept only exact 0/1. Unknown values follow the normal
/// policy, so a misspelled value cannot silently select a different shader.
#[cfg(any(not(target_arch = "wasm32"), test))]
fn diagnostic_switch(value: Option<&str>) -> Option<bool> {
    match value {
        Some("0") => Some(false),
        Some("1") => Some(true),
        _ => None,
    }
}

/// Return the actual module key: (physical-only, alpha evaluation enabled).
/// Native defaults contract both paths together only when both scene proofs
/// hold. Independent diagnostic overrides never bypass their respective proof.
#[cfg(any(not(target_arch = "wasm32"), test))]
fn shader_variant_policy(
    style: f32,
    requires_alpha: bool,
    native: bool,
    coverage_override: Option<bool>,
    physical_override: Option<bool>,
) -> (bool, bool) {
    if !native {
        return (false, true);
    }
    let physical = physical_style_proven(style);
    let joint = physical && !requires_alpha;
    (
        physical && physical_override.unwrap_or(joint),
        requires_alpha || !coverage_override.unwrap_or(joint),
    )
}

/// Ask for coverage only when its result can affect either selected module.
/// Coverage=0 fixes alpha evaluation on; with an explicit physical override,
/// neither output then depends on vertex/texture coverage. Nonphysical styles
/// need this proof only for the independent coverage=1 investigation.
#[cfg(any(not(target_arch = "wasm32"), test))]
fn shader_variant_selection_with_proof(
    style: f32,
    native: bool,
    coverage_override: Option<bool>,
    physical_override: Option<bool>,
    requires_alpha: impl FnOnce() -> bool,
) -> (bool, bool) {
    let needs_proof = native
        && match coverage_override {
            Some(true) => true,
            Some(false) => physical_style_proven(style) && physical_override.is_none(),
            None => physical_style_proven(style),
        };
    // An omitted proof is always represented conservatively as requiring
    // evaluation; the policy outputs are independent of coverage in this case.
    let requires_alpha = !needs_proof || requires_alpha();
    shader_variant_policy(
        style,
        requires_alpha,
        native,
        coverage_override,
        physical_override,
    )
}

/// Select once before renderer reuse/construction. In particular, constructors
/// must not reread environment variables and change this already chosen key.
pub(super) fn shader_variant_selection(style: f32, draws: &[GpuWorldDraw]) -> (bool, bool) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let coverage = std::env::var("MOTIONLOOM_TRACE_COMPLETE_COVERAGE_QUERY").ok();
        let physical = std::env::var("MOTIONLOOM_TRACE_PHYSICAL_SHADER").ok();
        shader_variant_selection_with_proof(
            style,
            true,
            diagnostic_switch(coverage.as_deref()),
            diagnostic_switch(physical.as_deref()),
            || requires_alpha_evaluation(draws),
        )
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (style, draws);
        (false, true)
    }
}

pub(super) fn physical_style_shader_source(source: &str, physical_only: bool) -> String {
    if physical_only {
        source.replace(
            "const PHYSICAL_STYLE_ONLY: bool = false;",
            "const PHYSICAL_STYLE_ONLY: bool = true;",
        )
    } else {
        source.to_owned()
    }
}

/// Imported primitive chunks can share a transport object. Reuse coating
/// radiance only if every chunk has a coating with the same authored roughness.
pub(super) fn certify_coating_roughness(
    draws: &[GpuWorldDraw],
    objects: &HashMap<GpuWorldInstanceKey, u32>,
    count: usize,
) -> Vec<bool> {
    let mut roughness = vec![None; count];
    let mut compatible = vec![true; count];
    for draw in draws {
        let Some(&object) = objects.get(&draw.instance_key) else {
            continue;
        };
        let Some(valid) = compatible.get_mut(object as usize) else {
            continue;
        };
        let coating = draw.params.material10;
        if coating[0] <= 0.0 || !coating[0].is_finite() || !coating[1].is_finite() {
            *valid = false;
        }
        let bits = coating[1].to_bits();
        let previous = &mut roughness[object as usize];
        if previous.is_some_and(|value| value != bits) {
            *valid = false;
        }
        *previous = Some(bits);
    }
    compatible
        .into_iter()
        .zip(roughness)
        .map(|(valid, value)| valid && value.is_some())
        .collect()
}

pub(super) fn requires_solid_transport(draws: &[GpuWorldDraw]) -> bool {
    draws
        .iter()
        .any(|draw| draw.params.material11[0] > 0.5 || draw.params.material6[0] == 0.001)
}

/// Every accepted hit has unit texture/vertex/material coverage and a cutoff
/// below its conservative floating-point bound. Include off-camera draws: all
/// optical and shadow queries share their geometry. A valid hit's barycentrics
/// lie near the unit triangle; eight f32 epsilons bound unit-alpha interpolation
/// error below 0.000001, safely above the maximum certified cutoff 0.9999.
#[cfg(any(not(target_arch = "wasm32"), test))]
pub(super) fn requires_alpha_evaluation(draws: &[GpuWorldDraw]) -> bool {
    draws.iter().any(|draw| {
        let material_alpha = draw.params.material4[3];
        let actor_alpha = draw.params.style[0];
        let cutoff = draw.params.material7[3];
        let texture_bytes = (draw.texture.width as usize)
            .checked_mul(draw.texture.height as usize)
            .and_then(|pixels| pixels.checked_mul(4));
        !material_alpha.is_finite()
            || !actor_alpha.is_finite()
            || material_alpha * actor_alpha != 1.0
            || !cutoff.is_finite()
            || cutoff.max(0.001) > 0.9999
            || texture_bytes.is_none_or(|bytes| bytes == 0 || bytes != draw.texture.rgba.len())
            || draw.texture.channel_bounds[3] != [255, 255]
            || draw.vertices.iter().any(|vertex| vertex.color[3] != 1.0)
    })
}

/// Primary opaque occlusion already comes from shadow maps. Retain the glass
/// visibility traversal only when the scene can populate its casting-glass
/// tree, including off-camera chunks and the legacy default-casting mode.
pub(super) fn requires_primary_glass_shadows(
    draws: &[GpuWorldDraw],
    flags: &HashMap<String, [bool; 2]>,
    per_light: bool,
) -> bool {
    draws.iter().any(|draw| {
        draw.params.material6[0] > 0.001 && super::hybrid::shadow_flags(draw, flags, per_light)[0]
    })
}

pub(super) fn draw_requires_geometry_reflection(
    draw: &GpuWorldDraw,
    lighting: &GpuWorldLightingParams,
) -> bool {
    let params = &draw.params;
    if lighting.reflection0[3] <= 0.5
        || params.material2[3] >= 0.5
        || lighting.surface0[0] >= 0.5
        || params.style[1] <= 0.0
    {
        return false;
    }
    if params.material10[0] > 0.0 {
        return true;
    }
    // Linear sampling and mip generation remain inside these channel bounds.
    // Luminance bounds are deliberately conservative across independent RGB.
    let packed = ((params.material8[2] + 0.5) as u32 >> 4) & 15;
    let bounds = draw.metallic_roughness_texture.channel_bounds;
    let channel = (packed & 7) as usize;
    let (minimum, maximum) = if channel == 4 {
        let coefficients = [0.2126, 0.7152, 0.0722];
        let endpoint = |index: usize| {
            (0..3)
                .map(|c| bounds[c][index] as f32 / 255.0 * coefficients[c])
                .sum::<f32>()
        };
        (endpoint(0), endpoint(1))
    } else {
        let c = if channel < 4 { channel } else { 0 };
        (bounds[c][0] as f32 / 255.0, bounds[c][1] as f32 / 255.0)
    };
    let (sampled_minimum, sampled_maximum) = if packed & 8 != 0 {
        (1.0 - maximum, 1.0 - minimum)
    } else {
        (minimum, maximum)
    };
    let sampled_minimum = if params.material0[1] < 0.0 {
        sampled_maximum
    } else {
        sampled_minimum
    };
    let roughness_minimum = params.material0[1] * sampled_minimum + lighting.surface1[2];
    // A margin covers texture conversion and floating-point interpolation.
    !roughness_minimum.is_finite() || roughness_minimum < 0.7501
}

#[cfg(test)]
mod tests {
    use super::super::{
        GpuWorldDrawKey, GpuWorldDrawPhase, GpuWorldInstanceKey, GpuWorldParams,
        GpuWorldResourceKey, GpuWorldTexture, PerspectiveCameraView, WorldLighting,
    };
    use super::*;
    use std::{path::PathBuf, sync::Arc};

    fn lighting() -> GpuWorldLightingParams {
        let camera = PerspectiveCameraView {
            orthographic: false,
            eye: [0.0, 0.0, -4.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, 1.0],
            focal_px: 900.0,
            near: 0.1,
            far: 100.0,
            aspect: 16.0 / 9.0,
            optics: [0.0; 4],
        };
        let mut lighting =
            GpuWorldLightingParams::from_world(&WorldLighting::default(), camera, false, 1);
        lighting.reflection0 = [2.0, 0.0, 0.0, 1.0];
        lighting.surface0[0] = 0.0;
        lighting.surface1[2] = 0.0;
        lighting
    }

    #[test]
    fn joint_shader_policy_requires_both_proofs_and_restores_universal_on_transitions() {
        for native in [false, true] {
            for requires_alpha in [false, true] {
                for style in [
                    0.0,
                    -0.0,
                    f32::from_bits(1),
                    -f32::from_bits(1),
                    -1.0,
                    1.0,
                    2.0,
                    3.0,
                    4.0,
                    f32::NAN,
                    f32::INFINITY,
                    f32::NEG_INFINITY,
                ] {
                    let joint = native && style.is_finite() && style == 0.0 && !requires_alpha;
                    assert_eq!(
                        shader_variant_policy(style, requires_alpha, native, None, None),
                        (joint, !joint),
                        "style={style}, requires_alpha={requires_alpha}, native={native}"
                    );
                }
            }
        }
        let mut draw = fully_covered_draw();
        let key = |style, draw: &GpuWorldDraw| {
            shader_variant_policy(
                style,
                requires_alpha_evaluation(std::slice::from_ref(draw)),
                true,
                None,
                None,
            )
        };
        assert_eq!(key(0.0, &draw), (true, false));
        draw.params.style[0] = 0.5;
        assert_eq!(key(0.0, &draw), (false, true));
        draw.params.style[0] = 1.0;
        assert_eq!(key(4.0, &draw), (false, true));
        assert_eq!(key(-1.0, &draw), (false, true));
        assert_eq!(key(0.0, &draw), (true, false));
        draw.params.material7[3] = 1.0;
        assert_eq!(key(0.0, &draw), (false, true));
        draw.params.material7[3] = 0.0;
        assert_eq!(key(0.0, &draw), (true, false));
    }

    #[test]
    fn joint_shader_diagnostics_are_independent_and_never_bypass_proofs() {
        for native in [false, true] {
            for style in [0.0, -1.0, 4.0, f32::NAN] {
                for requires_alpha in [false, true] {
                    for coverage in [None, Some(false), Some(true)] {
                        for physical in [None, Some(false), Some(true)] {
                            let joint = style.is_finite() && style == 0.0 && !requires_alpha;
                            let expected = if native {
                                (
                                    style.is_finite() && style == 0.0 && physical.unwrap_or(joint),
                                    requires_alpha || !coverage.unwrap_or(joint),
                                )
                            } else {
                                (false, true)
                            };
                            assert_eq!(
                                shader_variant_policy(
                                    style,
                                    requires_alpha,
                                    native,
                                    coverage,
                                    physical
                                ),
                                expected
                            );
                        }
                    }
                }
            }
        }
        // An explicit 0 disables that contraction independently of the other.
        assert_eq!(
            shader_variant_policy(0.0, false, true, Some(false), None),
            (true, true)
        );
        assert_eq!(
            shader_variant_policy(0.0, false, true, None, Some(false)),
            (false, false)
        );
        // An explicit 1 can investigate either proven path on its own.
        assert_eq!(
            shader_variant_policy(-1.0, false, true, Some(true), None),
            (false, false)
        );
        assert_eq!(
            shader_variant_policy(0.0, true, true, None, Some(true)),
            (true, true)
        );
    }

    #[test]
    fn joint_shader_unknown_diagnostic_values_follow_normal_policy() {
        assert_eq!(diagnostic_switch(Some("0")), Some(false));
        assert_eq!(diagnostic_switch(Some("1")), Some(true));
        for value in [
            None,
            Some(""),
            Some("true"),
            Some("false"),
            Some("2"),
            Some(" 1"),
            Some("1 "),
            Some("01"),
        ] {
            assert_eq!(diagnostic_switch(value), None);
            assert_eq!(
                shader_variant_policy(
                    0.0,
                    false,
                    true,
                    diagnostic_switch(value),
                    diagnostic_switch(value)
                ),
                (true, false)
            );
            assert_eq!(
                shader_variant_policy(
                    0.0,
                    true,
                    true,
                    diagnostic_switch(value),
                    diagnostic_switch(value)
                ),
                (false, true)
            );
        }
    }

    #[test]
    fn joint_shader_coverage_proof_shortcuts_preserve_every_policy_selection() {
        for native in [false, true] {
            for style in [
                0.0,
                -0.0,
                f32::from_bits(1),
                -f32::from_bits(1),
                -1.0,
                1.0,
                2.0,
                3.0,
                4.0,
                f32::NAN,
                f32::INFINITY,
                f32::NEG_INFINITY,
            ] {
                for requires_alpha in [false, true] {
                    for coverage in [None, Some(false), Some(true)] {
                        for physical in [None, Some(false), Some(true)] {
                            let calls = std::cell::Cell::new(0);
                            let selected = shader_variant_selection_with_proof(
                                style,
                                native,
                                coverage,
                                physical,
                                || {
                                    calls.set(calls.get() + 1);
                                    requires_alpha
                                },
                            );
                            assert_eq!(
                                selected,
                                shader_variant_policy(
                                    style,
                                    requires_alpha,
                                    native,
                                    coverage,
                                    physical
                                ),
                                "style={style}, native={native}, requires_alpha={requires_alpha}, coverage={coverage:?}, physical={physical:?}"
                            );
                            assert!(calls.get() <= 1, "a frame must never rescan coverage");
                            if !native {
                                assert_eq!(calls.get(), 0, "WASM never scans coverage");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn joint_shader_irrelevant_coverage_does_not_scan_vertices() {
        // Fixed diagnostic results and nonphysical defaults must not invoke an
        // expensive complete-scene scan. Panic would expose any actual scan.
        for (style, coverage, physical, expected) in [
            (0.0, Some(false), Some(false), (false, true)),
            (0.0, Some(false), Some(true), (true, true)),
            (-1.0, None, None, (false, true)),
            (4.0, Some(false), None, (false, true)),
            (f32::NAN, None, Some(true), (false, true)),
        ] {
            assert_eq!(
                shader_variant_selection_with_proof(style, true, coverage, physical, || panic!(
                    "coverage cannot affect this selected shader"
                )),
                expected
            );
        }
        for (style, coverage, physical) in [
            (0.0, None, None),
            (0.0, Some(false), None),
            (-1.0, Some(true), Some(false)),
        ] {
            let calls = std::cell::Cell::new(0);
            shader_variant_selection_with_proof(style, true, coverage, physical, || {
                calls.set(calls.get() + 1);
                false
            });
            assert_eq!(
                calls.get(),
                1,
                "a relevant coverage proof must be evaluated"
            );
        }
        assert_eq!(
            shader_variant_selection_with_proof(0.0, false, Some(true), Some(true), || panic!(
                "WASM retains both full paths without a coverage scan"
            )),
            (false, true)
        );
    }

    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn hybrid_retained_renderer_restores_shader_variants_after_coverage_and_style_transitions() {
        pollster::block_on(async {
            let mut renderer = super::super::WorldFrameRenderer::new();
            let covered = fully_covered_draw();
            let mut partial = covered.clone();
            partial.params.style[0] = 0.5;
            let cases = [
                (0.0, &covered, None, None, (true, false)),
                (0.0, &covered, None, None, (true, false)),
                (0.0, &partial, None, None, (false, true)),
                (4.0, &covered, None, None, (false, true)),
                (-1.0, &covered, None, None, (false, true)),
                (0.0, &covered, None, None, (true, false)),
                // Exercise an alpha-only key change and a physical-only change.
                (0.0, &partial, None, Some(true), (true, true)),
                (0.0, &partial, None, Some(false), (false, true)),
                (0.0, &covered, Some(true), Some(false), (false, false)),
                (0.0, &covered, None, None, (true, false)),
                (0.0, &covered, None, None, (true, false)),
            ];
            let mut previous_key = None;
            let mut previous_target = None;
            for (style, draw, coverage, physical, expected) in cases {
                let key = shader_variant_policy(
                    style,
                    requires_alpha_evaluation(std::slice::from_ref(draw)),
                    true,
                    coverage,
                    physical,
                );
                assert_eq!(key, expected);
                // The selected booleans are immutable constructor inputs. No
                // process-wide environment mutation is needed in parallel tests.
                if let Some((device, _)) = &renderer.gpu_device_queue {
                    device.push_error_scope(wgpu::ErrorFilter::Validation);
                }
                renderer
                    .ensure_gpu_renderer(16, 16, false, true, false, false, key.0, key.1, false)
                    .await
                    .expect("retained native shader variant transition");
                let gpu = renderer.gpu_renderer.as_ref().unwrap();
                assert_eq!(
                    (gpu.physical_style_only, gpu.alpha_evaluation_enabled),
                    expected
                );
                if let Some(target) = &previous_target {
                    assert_eq!(
                        Arc::ptr_eq(target, &gpu.hdr_target),
                        previous_key == Some(key),
                        "reuse must compare the actual shader key across style/coverage transitions"
                    );
                }
                if let Some((device, _)) = &renderer.gpu_device_queue {
                    assert!(
                        device.pop_error_scope().await.is_none(),
                        "transition shader pipeline validation"
                    );
                }
                let device = Arc::clone(&gpu.device);
                let queue = gpu.queue.clone();
                previous_target = Some(Arc::clone(&gpu.hdr_target));
                previous_key = Some(key);
                // Keep one adapter/device/queue throughout all backend rebuilds.
                renderer.gpu_device_queue = Some((device, queue));
            }
        });
    }

    fn draw(pixels: &[[u8; 4]], roughness_channel: u32) -> GpuWorldDraw {
        let key = GpuWorldResourceKey {
            model_path: PathBuf::from("shader-specialization-test"),
            binding_actor: None,
            draw_key: GpuWorldDrawKey {
                material: None,
                texture: None,
                mesh: None,
                mesh_node: None,
            },
        };
        let texture = Arc::new(GpuWorldTexture::new(
            pixels.len() as u32,
            1,
            pixels.iter().flatten().copied().collect::<Vec<_>>(),
        ));
        GpuWorldDraw {
            instance_key: GpuWorldInstanceKey {
                actor_id: "rough-instance".into(),
                resource_key: key.clone(),
            },
            resource_key: key,
            vertices: Arc::new(Vec::new()),
            indices: Arc::new(Vec::new()),
            vertex_signature: 0,
            texture: texture.clone(),
            normal_texture: texture.clone(),
            metallic_roughness_texture: texture.clone(),
            emissive_texture: texture.clone(),
            occlusion_texture: texture.clone(),
            cel_texture: texture,
            bone_matrices: Vec::new(),
            params: GpuWorldParams {
                style: [1.0; 4],
                material0: [0.0, 1.0, 1.0, 1.0],
                // Only the second packed nibble selects roughness. Metallic
                // and AO use deliberately different channels in this fixture.
                material8: [0.0, 0.0, (3 | roughness_channel << 4 | 2 << 8) as f32, 0.0],
                ..Default::default()
            },
            phase: GpuWorldDrawPhase::Opaque,
            depth_write: true,
            sort_priority: 0,
            camera_depth: 0.0,
        }
    }

    fn fully_covered_draw() -> GpuWorldDraw {
        let mut value = draw(&[[255; 4]], 1);
        value.params.material4 = [1.0; 4];
        value.params.style[0] = 1.0;
        value.vertices = Arc::new(vec![super::super::GpuWorldVertex {
            position: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0],
            bitangent: [0.0, 1.0, 0.0],
            outline_normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            color: [1.0; 4],
            joints: [0.0; 4],
            weights: [0.0; 4],
        }]);
        value
    }

    #[test]
    fn complete_coverage_proof_checks_off_camera_members_texture_and_every_vertex() {
        let covered = fully_covered_draw();
        assert!(!requires_alpha_evaluation(&[]));
        assert!(!requires_alpha_evaluation(&[
            covered.clone(),
            covered.clone()
        ]));
        let mut off_camera = covered.clone();
        off_camera.camera_depth = -1000.0;
        off_camera.params.actor[0] = 1000.0;
        // An unused vertex still conservatively prevents specialization.
        let mut unused = off_camera.vertices[0];
        unused.color[3] = 0.99999;
        Arc::make_mut(&mut off_camera.vertices).push(unused);
        assert!(requires_alpha_evaluation(&[covered.clone(), off_camera]));
        for alpha in [0.0, 0.5, 0.99999, f32::NAN, f32::INFINITY] {
            let mut partial = covered.clone();
            Arc::make_mut(&mut partial.vertices)[0].color[3] = alpha;
            assert!(requires_alpha_evaluation(&[covered.clone(), partial]));
        }
        let mut textured = covered.clone();
        textured.texture = Arc::new(GpuWorldTexture::new(
            2,
            1,
            vec![255, 255, 255, 255, 255, 255, 255, 254],
        ));
        assert!(requires_alpha_evaluation(&[covered.clone(), textured]));
        let mut empty_texture = covered.clone();
        empty_texture.texture = Arc::new(GpuWorldTexture::new(0, 0, Vec::new()));
        assert!(requires_alpha_evaluation(&[empty_texture]));
        for (width, height, pixels) in [
            (0, 0, vec![255; 4]),
            (2, 1, vec![255; 4]),
            (1, 1, vec![255; 5]),
            (u32::MAX, u32::MAX, vec![255; 4]),
        ] {
            let mut invalid = covered.clone();
            invalid.texture = Arc::new(GpuWorldTexture::new(width, height, pixels));
            assert!(requires_alpha_evaluation(&[invalid]));
        }
    }

    #[test]
    fn complete_coverage_proof_uses_exact_packed_alpha_and_strict_cutoff_reserve() {
        let covered = fully_covered_draw();
        for (material_alpha, actor_alpha, requires) in [
            (1.0, 1.0, false),
            (2.0, 0.5, false),
            (0.5, 2.0, false),
            (1.0, 0.99999, true),
            (0.99999, 1.0, true),
            (0.0, 1.0, true),
            (f32::NAN, 1.0, true),
            (1.0, f32::NAN, true),
            (f32::INFINITY, 0.0, true),
            (0.0, f32::INFINITY, true),
        ] {
            let mut value = covered.clone();
            value.params.material4[3] = material_alpha;
            value.params.style[0] = actor_alpha;
            assert_eq!(requires_alpha_evaluation(&[value]), requires);
        }
        for (cutoff, requires) in [
            (-1.0, false),
            (0.0, false),
            (0.001, false),
            (0.9999, false),
            (0.99995, true),
            (1.0, true),
            (f32::NAN, true),
            (f32::INFINITY, true),
            (f32::NEG_INFINITY, true),
        ] {
            let mut value = covered.clone();
            value.params.material7[3] = cutoff;
            assert_eq!(
                requires_alpha_evaluation(&[value]),
                requires,
                "cutoff {cutoff} was incorrectly certified"
            );
        }
        assert!(1.0 - 8.0 * f32::EPSILON > 0.9999_f32);
    }

    // Evaluate sampled RGBA directly, independently of texture extrema. This
    // models channel remapping after bilinear/trilinear interpolation.
    fn sampled_roughness(rgba: [f32; 4], encoded: u32) -> f32 {
        let value = match encoded & 7 {
            1 => rgba[1],
            2 => rgba[2],
            3 => rgba[3],
            4 => rgba[0] * 0.2126 + rgba[1] * 0.7152 + rgba[2] * 0.0722,
            _ => rgba[0],
        };
        if encoded & 8 != 0 { 1.0 - value } else { value }
    }

    #[test]
    fn solid_specialization_preserves_every_scene_member_and_boundary_value() {
        let mut slab = draw(&[[255; 4]], 1);
        slab.params.material6[0] = 0.95;
        assert!(!requires_solid_transport(&[]));
        assert!(!requires_solid_transport(std::slice::from_ref(&slab)));

        for transmission in [0.0, 0.0009, 0.001, 0.95] {
            let mut solid = slab.clone();
            solid.params.material11[0] = 1.0;
            solid.params.material6[0] = transmission;
            // An off-camera solid may still be hit by a reflected ray.
            solid.camera_depth = -1000.0;
            solid.params.actor[0] = 1000.0;
            assert!(requires_solid_transport(&[slab.clone(), solid]));
        }

        slab.params.material11[0] = 0.5;
        for transmission in [0.0, 0.0009, 0.0011, 0.95] {
            slab.params.material6[0] = transmission;
            assert!(!requires_solid_transport(std::slice::from_ref(&slab)));
        }
        slab.params.material6[0] = 0.001;
        assert!(requires_solid_transport(std::slice::from_ref(&slab)));
        slab.params.material6[0] = 0.0;
        slab.params.material11[0] = 0.5001;
        assert!(requires_solid_transport(std::slice::from_ref(&slab)));
    }

    #[test]
    fn primary_glass_specialization_uses_the_packed_transmission_boundary() {
        let mut glass = draw(&[[255; 4]], 1);
        let flags = HashMap::new();
        assert!(!requires_primary_glass_shadows(&[], &flags, true));
        let boundary = 0.001_f32;
        for transmission in [
            0.0,
            0.0009,
            f32::from_bits(boundary.to_bits() - 1),
            boundary,
        ] {
            glass.params.material6[0] = transmission;
            assert!(!requires_primary_glass_shadows(
                std::slice::from_ref(&glass),
                &flags,
                true
            ));
        }
        for transmission in [f32::from_bits(boundary.to_bits() + 1), 0.95] {
            glass.params.material6[0] = transmission;
            // Slab and solid materials populate the same primary glass tree.
            for solid in [0.0, 1.0] {
                glass.params.material11[0] = solid;
                assert!(requires_primary_glass_shadows(
                    std::slice::from_ref(&glass),
                    &flags,
                    true
                ));
            }
        }
    }

    #[test]
    fn primary_glass_specialization_preserves_shadow_flag_defaults_and_imported_chunks() {
        let mut glass = draw(&[[255; 4]], 1);
        glass.params.material6[0] = 0.95;
        glass.instance_key.actor_id = "window::primitive-2::material-1".into();
        // A pane behind the camera can shadow a directly visible receiver.
        glass.camera_depth = -1000.0;
        glass.params.actor[0] = 1000.0;
        let flags = HashMap::from([("window".into(), [false, true])]);
        assert!(!requires_primary_glass_shadows(
            std::slice::from_ref(&glass),
            &flags,
            true
        ));
        // Authored per-model flags apply only to per-light shadows. The
        // established compatibility mode treats every draw as casting.
        assert!(requires_primary_glass_shadows(
            std::slice::from_ref(&glass),
            &flags,
            false
        ));
        assert!(requires_primary_glass_shadows(
            std::slice::from_ref(&glass),
            &HashMap::new(),
            true
        ));
        let receive_only_disabled = HashMap::from([("window".into(), [true, false])]);
        assert!(requires_primary_glass_shadows(
            std::slice::from_ref(&glass),
            &receive_only_disabled,
            true
        ));

        let mut other = glass.clone();
        other.instance_key.actor_id = "unlisted-window::primitive-0".into();
        // Every evaluated chunk participates; one noncasting pane cannot
        // certify a scene containing another pane with the default flags.
        assert!(requires_primary_glass_shadows(
            &[glass, other],
            &flags,
            true
        ));
    }

    #[test]
    fn geometry_specialization_honors_channels_inversion_and_luminance() {
        let lighting = lighting();
        let pixels = [[255, 190, 255, 255], [255, 250, 255, 255]];
        assert!(!draw_requires_geometry_reflection(
            &draw(&pixels, 0),
            &lighting
        ));
        assert!(draw_requires_geometry_reflection(
            &draw(&pixels, 1),
            &lighting
        ));
        assert!(!draw_requires_geometry_reflection(
            &draw(&pixels, 2),
            &lighting
        ));
        assert!(!draw_requires_geometry_reflection(
            &draw(&pixels, 3),
            &lighting
        ));
        assert!(draw_requires_geometry_reflection(
            &draw(&pixels, 9),
            &lighting
        ));

        let low_red = [[10, 255, 255, 255], [20, 255, 255, 255]];
        // Unknown packed channels follow the shader's red-channel fallback.
        for encoded in [0, 5, 6, 7] {
            assert!(draw_requires_geometry_reflection(
                &draw(&low_red, encoded),
                &lighting
            ));
            assert!(!draw_requires_geometry_reflection(
                &draw(&low_red, encoded | 8),
                &lighting
            ));
        }
        assert!(!draw_requires_geometry_reflection(
            &draw(&low_red, 4),
            &lighting
        ));
        assert!(draw_requires_geometry_reflection(
            &draw(&low_red, 12),
            &lighting
        ));

        // Independent component extrema can only make the proof stricter:
        // neither pixel has the combined minimum luminance of these bounds.
        let anticorrelated = [[255, 170, 255, 255], [170, 255, 255, 255]];
        assert!(draw_requires_geometry_reflection(
            &draw(&anticorrelated, 4),
            &lighting
        ));
    }

    #[test]
    fn cheap_geometry_shader_never_removes_a_reachable_interpolated_rough_lobe() {
        let pixels = [[255, 12, 210, 255], [34, 240, 25, 128], [140, 110, 250, 0]];
        let mut eligible_count = 0;
        for encoded in 0..16 {
            for factor in [-1.0, 0.0, 0.8, 1.0, 1.4] {
                for bias in [-0.4, 0.0, 0.25, 1.0, 2.0] {
                    let mut draw = draw(&pixels, encoded);
                    draw.params.material0[1] = factor;
                    let mut lighting = lighting();
                    lighting.surface1[2] = bias;
                    if draw_requires_geometry_reflection(&draw, &lighting) {
                        continue;
                    }
                    eligible_count += 1;
                    // Convex interpolation covers original texels, edge
                    // filtering and interpolated mip levels without assuming
                    // that the minimum lies in the green channel.
                    for first in pixels {
                        for second in pixels {
                            for weight in [0.0, 0.125, 0.5, 0.875, 1.0] {
                                let rgba = std::array::from_fn(|channel| {
                                    (first[channel] as f32 * (1.0 - weight)
                                        + second[channel] as f32 * weight)
                                        / 255.0
                                });
                                let roughness = (sampled_roughness(rgba, encoded) * factor + bias)
                                    .clamp(0.04, 1.0);
                                assert!(
                                    roughness >= 0.75,
                                    "removed reachable lobe: channel={encoded}, factor={factor}, bias={bias}, roughness={roughness}"
                                );
                            }
                        }
                    }
                }
            }
        }
        assert!(
            eligible_count > 100,
            "fixture must exercise cheap shader selection"
        );
    }

    #[test]
    fn geometry_specialization_keeps_clearcoat_bias_and_instance_variation() {
        let mut lighting = lighting();
        let rough = draw(&[[255; 4]], 1);
        assert!(!draw_requires_geometry_reflection(&rough, &lighting));
        let mut glossy_instance = rough.clone();
        glossy_instance.instance_key.actor_id = "glossy-instance".into();
        glossy_instance.params.material0[1] = 0.6;
        assert!(draw_requires_geometry_reflection(
            &glossy_instance,
            &lighting
        ));
        assert_eq!(rough.resource_key, glossy_instance.resource_key);
        assert!(
            [rough.clone(), glossy_instance]
                .iter()
                .any(|instance| draw_requires_geometry_reflection(instance, &lighting))
        );

        lighting.surface1[2] = -0.3;
        assert!(draw_requires_geometry_reflection(&rough, &lighting));
        lighting.surface1[2] = 0.0;
        let mut coated = rough.clone();
        coated.params.material10 = [0.00001, 0.04, 0.0, 0.0];
        assert!(draw_requires_geometry_reflection(&coated, &lighting));
        for roughness in [0.75, 0.75005, f32::NAN, f32::INFINITY] {
            let mut uncertain = rough.clone();
            uncertain.params.material0[1] = roughness;
            assert!(draw_requires_geometry_reflection(&uncertain, &lighting));
        }
    }

    #[test]
    fn scalar_texture_bounds_include_transparent_texels() {
        let draw = draw(&[[240, 250, 255, 255], [120, 100, 20, 0]], 1);
        assert_eq!(
            draw.metallic_roughness_texture.channel_bounds,
            [[120, 240], [100, 250], [20, 255], [0, 255]]
        );
        assert!(draw_requires_geometry_reflection(&draw, &lighting()));
    }

    #[test]
    fn coating_certification_checks_every_chunk_of_each_transport_object() {
        let mut first = draw(&[[255; 4]], 1);
        first.params.material10 = [0.2, 0.28, 0.0, 0.0];
        let mut second = first.clone();
        second.resource_key.draw_key.mesh = Some(1);
        second.instance_key.resource_key = second.resource_key.clone();
        second.params.material10[0] = 0.8;
        let mut independent = first.clone();
        independent.instance_key.actor_id = "independent-coated-object".into();
        independent.params.material10 = [0.6, 0.52, 0.0, 0.0];
        let mut unmapped = first.clone();
        unmapped.instance_key.actor_id = "unmapped-object".into();
        unmapped.params.material10[0] = 0.0;
        let mut out_of_range = first.clone();
        out_of_range.instance_key.actor_id = "unknown-object-id".into();
        let mut unseen_key = first.instance_key.clone();
        unseen_key.actor_id = "mapped-object-with-no-draws".into();
        let objects = HashMap::from([
            (first.instance_key.clone(), 1),
            (second.instance_key.clone(), 1),
            (independent.instance_key.clone(), 3),
            (out_of_range.instance_key.clone(), u32::MAX),
            (unseen_key, 5),
        ]);
        let certify = |second: GpuWorldDraw| {
            certify_coating_roughness(
                &[
                    first.clone(),
                    second,
                    independent.clone(),
                    unmapped.clone(),
                    out_of_range.clone(),
                ],
                &objects,
                6,
            )
        };
        // Different positive factors share the same geometric coating lobe;
        // full-resolution material shading still applies their own factors.
        assert_eq!(
            certify(second.clone()),
            [false, true, false, true, false, false]
        );
        assert_eq!(certify_coating_roughness(&[], &objects, 6), [false; 6]);

        let mut uncoated_offscreen = second.clone();
        uncoated_offscreen.params.material10[0] = 0.0;
        uncoated_offscreen.camera_depth = -1000.0;
        uncoated_offscreen.params.actor[0] = 1000.0;
        // A chunk outside the direct view can still supply a neighboring
        // cache sample or a reflected hit. It must participate in the proof.
        let reversed = [
            uncoated_offscreen.clone(),
            first.clone(),
            independent.clone(),
        ];
        assert_eq!(
            certify_coating_roughness(&reversed, &objects, 6),
            [false, false, false, true, false, false]
        );
        assert_eq!(
            certify(uncoated_offscreen),
            [false, false, false, true, false, false]
        );
        let mut different_roughness = second.clone();
        different_roughness.params.material10[1] = 0.2801;
        assert_eq!(
            certify(different_roughness),
            [false, false, false, true, false, false]
        );

        for (factor, roughness) in [
            (-0.1, 0.28),
            (f32::NAN, 0.28),
            (f32::INFINITY, 0.28),
            (f32::NEG_INFINITY, 0.28),
            (0.8, f32::NAN),
            (0.8, f32::INFINITY),
            (0.8, f32::NEG_INFINITY),
        ] {
            let mut invalid = second.clone();
            invalid.params.material10[0] = factor;
            invalid.params.material10[1] = roughness;
            assert_eq!(
                certify(invalid),
                [false, false, false, true, false, false],
                "invalid coating certificate: factor={factor}, roughness={roughness}"
            );
        }
    }
}
