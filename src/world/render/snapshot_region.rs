//! Conservative screen-space slab read footprints; SSR keeps a full snapshot.
use super::transport_tiles::ScreenBounds;
use super::{GpuWorldParams, mat4_identity, quat_normalize_xyzw, quat_rotate_vec3};

/// Returns a half-open rectangle in the original full-resolution texture.
/// Unknown/deformed bounds or near-plane crossings retain the complete image.
pub(super) fn slab_region(
    bounds: Option<([f32; 3], [f32; 3])>,
    params: GpuWorldParams,
    size: [u32; 2],
    screen: ScreenBounds,
    screen_reflections: bool,
    rigid: bool,
) -> [u32; 4] {
    let full = [0, 0, size[0], size[1]];
    let ScreenBounds::Rect(rect) = screen else {
        return full;
    };
    if screen_reflections || !rigid {
        return full;
    }
    let Some((minimum, maximum)) = bounds else {
        return full;
    };
    let thickness = params.material6[2];
    let focal = params.camera0[3];
    if !thickness.is_finite() || thickness < 0.0 || !focal.is_finite() || focal <= 0.0 {
        return full;
    }
    let rotation = quat_normalize_xyzw(params.actor_rotation);
    let mut nearest = f32::INFINITY;
    for corner in 0..8 {
        let local = std::array::from_fn(|axis| {
            let coordinate = if corner & (1 << axis) == 0 {
                minimum[axis]
            } else {
                maximum[axis]
            };
            (coordinate - params.model[axis]) * params.model[3]
        });
        let world = quat_rotate_vec3(rotation, local);
        let depth = (0..3)
            .map(|axis| {
                (world[axis] + params.actor[axis] - params.camera0[axis]) * params.camera3[axis]
            })
            .sum::<f32>();
        nearest = nearest.min(depth);
    }
    // WGSL clamps the interface cosine to .05, hence a path <= 20*thickness.
    // A valid refracted sample is behind the exit. Rejected rays sample the
    // original pixel instead. Reserve precision at the nearest exit plane.
    let path = thickness * 20.0;
    let exit_depth = nearest - path - 0.0001;
    if !exit_depth.is_finite() || exit_depth <= params.camera1[3].max(0.0001) {
        return full;
    }
    let orthographic = params.camera3[3] > 0.5;
    let ray = [0, 1].map(|axis| {
        if orthographic {
            0.0
        } else {
            let centre = params.canvas[axis + 2];
            // Raster jitter and pixel-centre rounding are included conservatively.
            ((centre.abs() + 2.0).max((size[axis] as f32 - centre).abs() + 2.0)) / focal
        }
    });
    // The shader also clamps the incident view cosine to .05. If it can reach
    // that clamp, the perspective displacement derivation no longer applies.
    if !orthographic && 1.0 / (1.0 + ray[0] * ray[0] + ray[1] * ray[1]).sqrt() <= 0.05 {
        return full;
    }
    let margin = [0, 1].map(|axis| {
        let displacement =
            focal * path * (1.0 + ray[axis]) / if orthographic { 1.0 } else { exit_depth };
        // Roughness is clamped to <=1: the largest blur tap is .006 UV.
        (displacement + size[axis] as f32 * 0.006 + 4.0).ceil()
    });
    if margin
        .iter()
        .any(|v| !v.is_finite() || *v >= u32::MAX as f32)
    {
        return full;
    }
    let margin = margin.map(|v| v as u32);
    [
        rect[0].saturating_sub(margin[0]),
        rect[1].saturating_sub(margin[1]),
        rect[2].saturating_add(margin[0]).min(size[0]),
        rect[3].saturating_add(margin[1]).min(size[1]),
    ]
}

pub(super) fn rigid(bones: &[[f32; 16]]) -> bool {
    bones.iter().all(|matrix| *matrix == mat4_identity())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn params() -> GpuWorldParams {
        let mut p = GpuWorldParams::default();
        p.camera0 = [0.0, 0.0, 0.0, 500.0];
        p.camera1 = [1.0, 0.0, 0.0, 0.01];
        p.camera3 = [0.0, 0.0, 1.0, 0.0];
        p.canvas = [1000.0, 600.0, 500.0, 300.0];
        p.model[3] = 1.0;
        p.actor_rotation = [0.0, 0.0, 0.0, 1.0];
        p
    }
    #[test]
    fn slab_copy_keeps_original_coordinates_and_blur_border() {
        let p = params();
        assert_eq!(
            slab_region(
                Some(([-1.0, -1.0, 4.0], [1.0, 1.0, 4.1])),
                p,
                [1000, 600],
                ScreenBounds::Rect([400, 200, 600, 400]),
                false,
                true
            ),
            [390, 192, 610, 408]
        );
    }
    #[test]
    fn unsupported_optics_keep_full_snapshot() {
        let p = params();
        let bounds = Some(([-1.0, -1.0, 0.0], [1.0, 1.0, 1.0]));
        for (ssr, rigid) in [(true, true), (false, false), (false, true)] {
            assert_eq!(
                slab_region(
                    bounds,
                    p,
                    [1000, 600],
                    ScreenBounds::Rect([400, 200, 600, 400]),
                    ssr,
                    rigid
                ),
                [0, 0, 1000, 600]
            );
        }
    }
    #[test]
    fn thickness_expands_copy_and_near_exit_falls_back() {
        let mut p = params();
        let bounds = Some(([-1.0, -1.0, 4.0], [1.0, 1.0, 4.1]));
        let zero = slab_region(
            bounds,
            p,
            [1000, 600],
            ScreenBounds::Rect([400, 200, 600, 400]),
            false,
            true,
        );
        p.material6[2] = 0.012;
        let thick = slab_region(
            bounds,
            p,
            [1000, 600],
            ScreenBounds::Rect([400, 200, 600, 400]),
            false,
            true,
        );
        assert!(thick[0] < zero[0] && thick[2] > zero[2]);
        p.material6[2] = 0.2;
        assert_eq!(
            slab_region(
                bounds,
                p,
                [1000, 600],
                ScreenBounds::Rect([400, 200, 600, 400]),
                false,
                true
            ),
            [0, 0, 1000, 600]
        );
    }
}
