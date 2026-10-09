//! Shared real-time and diffuse-bake material energy contracts.
//! Charlie distribution uses Neubelt visibility; its own albedo LUT is needed.
use std::{f32::consts::PI, sync::OnceLock};
const LUT_SIZE: usize = 64;
static SHEEN_ALBEDO: OnceLock<Vec<f32>> = OnceLock::new();

pub(crate) fn srgb_decode(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

pub(crate) fn sheen_brdf(n_dot_v: f32, n_dot_l: f32, n_dot_h: f32, roughness: f32) -> f32 {
    if n_dot_v <= 0.0 || n_dot_l <= 0.0 {
        return 0.0;
    }
    let inverse_alpha = 1.0 / roughness.clamp(0.04, 1.0).powi(2);
    let distribution = (2.0 + inverse_alpha)
        * (1.0 - n_dot_h.clamp(0.0, 1.0).powi(2))
            .max(0.0)
            .powf(0.5 * inverse_alpha)
        / (2.0 * PI);
    distribution / (4.0 * (n_dot_v + n_dot_l - n_dot_v * n_dot_l).max(1e-6))
}

fn integrate_albedo(n_dot_v: f32, roughness: f32) -> f32 {
    // Uniform-solid-angle midpoint quadrature keeps bakes deterministic.
    let n_dot_v = n_dot_v.clamp(1e-4, 1.0);
    let v_x = (1.0 - n_dot_v * n_dot_v).sqrt();
    let mut sum = 0.0;
    for z in 0..32 {
        let n_dot_l = (z as f32 + 0.5) / 32.0;
        let l_xy = (1.0 - n_dot_l * n_dot_l).sqrt();
        for p in 0..32 {
            let phi = 2.0 * PI * (p as f32 + 0.5) / 32.0;
            let v_dot_l = v_x * l_xy * phi.cos() + n_dot_v * n_dot_l;
            let n_dot_h = (n_dot_v + n_dot_l) / (2.0 * (1.0 + v_dot_l)).max(1e-8).sqrt();
            sum += sheen_brdf(n_dot_v, n_dot_l, n_dot_h, roughness) * n_dot_l;
        }
    }
    (sum * (2.0 * PI / 1024.0)).clamp(0.0, 1.0)
}

pub(crate) fn directional_albedo(n_dot_v: f32, roughness: f32) -> f32 {
    let table = SHEEN_ALBEDO.get_or_init(|| {
        (0..LUT_SIZE * LUT_SIZE)
            .map(|i| {
                integrate_albedo(
                    ((i % LUT_SIZE) as f32 + 0.5) / LUT_SIZE as f32,
                    ((i / LUT_SIZE) as f32 + 0.5) / LUT_SIZE as f32,
                )
            })
            .collect()
    });
    let x = n_dot_v.clamp(0.0, 1.0) * LUT_SIZE as f32 - 0.5;
    let y = roughness.clamp(0.04, 1.0) * LUT_SIZE as f32 - 0.5;
    let x0 = x.floor() as isize;
    let y0 = y.floor() as isize;
    let fx = x - x.floor();
    let fy = y - y.floor();
    let p = |x: isize, y: isize| {
        table[y.clamp(0, LUT_SIZE as isize - 1) as usize * LUT_SIZE
            + x.clamp(0, LUT_SIZE as isize - 1) as usize]
    };
    let a = p(x0, y0) * (1.0 - fx) + p(x0 + 1, y0) * fx;
    let b = p(x0, y0 + 1) * (1.0 - fx) + p(x0 + 1, y0 + 1) * fx;
    a * (1.0 - fy) + b * fy
}

pub(crate) fn coat_fresnel(n_dot_v: f32) -> f32 {
    0.04 + 0.96 * (1.0 - n_dot_v.clamp(0.0, 1.0)).powi(5)
}

pub(crate) fn layer_base_scale(
    color: [f32; 3],
    sheen_roughness: f32,
    clearcoat: f32,
    n_dot_v: f32,
) -> f32 {
    let sheen_energy = color.into_iter().fold(0.0_f32, f32::max).clamp(0.0, 1.0);
    let cloth = if sheen_energy == 0.0 {
        1.0
    } else {
        1.0 - sheen_energy * directional_albedo(n_dot_v, sheen_roughness)
    };
    cloth * (1.0 - clearcoat.clamp(0.0, 1.0) * coat_fresnel(n_dot_v))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_layers_preserve_base_exactly_and_full_layers_remain_bounded() {
        for n in [0.0, 0.01, 0.2, 0.5, 1.0] {
            for r in [0.04, 0.15, 0.5, 1.0] {
                assert_eq!(layer_base_scale([0.0; 3], r, 0.0, n), 1.0);
                let e = directional_albedo(n, r);
                assert!(e.is_finite() && (0.0..=1.0).contains(&e));
                let b = layer_base_scale([1.0; 3], r, 1.0, n);
                assert!((0.0..=1.0).contains(&b));
                assert!(b + e * (1.0 - coat_fresnel(n)) + coat_fresnel(n) <= 1.00001);
            }
        }
    }
    #[test]
    fn sheen_is_reciprocal_and_roughness_changes_distribution() {
        assert_eq!(
            sheen_brdf(0.2, 0.8, 0.5, 0.5),
            sheen_brdf(0.8, 0.2, 0.5, 0.5)
        );
        assert!(sheen_brdf(0.3, 0.3, 0.01, 0.04) > sheen_brdf(0.3, 0.3, 0.01, 1.0));
        assert!(directional_albedo(0.2, 0.5) > directional_albedo(1.0, 0.5));
        assert!((srgb_decode(0.5) - 0.21404114).abs() < 1e-6);
    }
}
