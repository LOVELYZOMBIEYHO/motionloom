// =========================================
// src/lighting_ibl.rs

//! Deterministic, renderer-independent preprocessing of linear latlong lighting.
//!
//! Diffuse coefficients contain irradiance E, not outgoing diffuse radiance;
//! the material shader applies its Lambertian albedo / PI. Specular mips use
//! GGX importance sampling and the accompanying correlated-Smith split-sum LUT.
//! Source/background mips are separate so roughness filtering never changes
//! the visible sky. Hosts retain this output by source/options fingerprint.

use std::f32::consts::{PI, TAU};

use half::f16;
use thiserror::Error;

/// Real SH order: 00, y, z, x, xy, yz, 3z²-1, xz, x²-y².
/// RGB stores the coefficient; the fourth component is reserved and zero.
pub type IrradianceSh9 = [[f32; 4]; 9];

/// Finite, nonnegative linear radiance. Latlong +Y is the north pole, +X is
/// u=0.5, and +Z is u=0.75, matching the immediate renderer's environment map.
#[derive(Clone, Debug)]
pub struct LinearEnvironment {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}

/// Explicit bounds keep a cache miss finite on both native and browser hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IblPreprocessOptions {
    pub max_specular_width: u32,
    pub specular_samples: u32,
    pub brdf_lut_size: u32,
    pub brdf_samples: u32,
}

impl Default for IblPreprocessOptions {
    fn default() -> Self {
        Self {
            max_specular_width: 512,
            specular_samples: 64,
            brdf_lut_size: 64,
            brdf_samples: 128,
        }
    }
}

#[derive(Clone, Debug)]
pub struct BrdfIntegrationLut {
    /// Square LUT, x=NoV and y=perceptual roughness, sampled at texel centres.
    pub size: u32,
    pub pixels: Vec<[f32; 2]>,
}

#[derive(Clone, Debug)]
pub struct PreprocessedEnvironment {
    pub diffuse_sh: IrradianceSh9,
    pub specular_mips: Vec<LinearEnvironment>,
    pub background_mips: Vec<LinearEnvironment>,
    pub brdf_lut: BrdfIntegrationLut,
}

#[derive(Debug, Error)]
pub enum IblPreprocessError {
    #[error("environment dimensions must be nonzero, received {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("environment has {actual} pixels, expected {expected}")]
    PixelCount { expected: usize, actual: usize },
    #[error("environment exceeds the 32-megapixel preprocessing limit")]
    SourceTooLarge,
    #[error(
        "environment radiance at pixel {pixel}, channel {channel}, must be finite and nonnegative"
    )]
    InvalidRadiance { pixel: usize, channel: usize },
    #[error("invalid IBL option {name}: {value}")]
    InvalidOption { name: &'static str, value: u32 },
    #[error("failed to decode environment image: {0}")]
    Decode(#[from] image::ImageError),
}

impl LinearEnvironment {
    pub fn new(width: u32, height: u32, pixels: Vec<[f32; 3]>) -> Result<Self, IblPreprocessError> {
        let result = Self {
            width,
            height,
            pixels,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn from_encoded_bytes(bytes: &[u8]) -> Result<Self, IblPreprocessError> {
        Self::from_image(&image::load_from_memory(bytes)?)
    }

    /// Float HDR/EXR pixels are already linear; ordinary images use exact sRGB.
    /// Negative EXR reconstruction values are clamped as nonphysical radiance.
    pub fn from_image(image: &image::DynamicImage) -> Result<Self, IblPreprocessError> {
        let linear = matches!(
            image.color(),
            image::ColorType::Rgb32F | image::ColorType::Rgba32F
        );
        let rgba = image.to_rgba32f();
        let pixels = rgba
            .pixels()
            .map(|pixel| {
                std::array::from_fn(|channel| {
                    let original = pixel[channel];
                    let value = if original.is_finite() {
                        original.max(0.0)
                    } else {
                        original
                    };
                    if linear { value } else { srgb_to_linear(value) }
                })
            })
            .collect();
        Self::new(rgba.width(), rgba.height(), pixels)
    }

    pub fn validate(&self) -> Result<(), IblPreprocessError> {
        if self.width == 0 || self.height == 0 {
            return Err(IblPreprocessError::InvalidDimensions {
                width: self.width,
                height: self.height,
            });
        }
        let count = u64::from(self.width) * u64::from(self.height);
        if count > 32 * 1024 * 1024 {
            return Err(IblPreprocessError::SourceTooLarge);
        }
        if count as usize != self.pixels.len() {
            return Err(IblPreprocessError::PixelCount {
                expected: count as usize,
                actual: self.pixels.len(),
            });
        }
        for (pixel, rgb) in self.pixels.iter().enumerate() {
            for (channel, value) in rgb.iter().enumerate() {
                if !value.is_finite() || *value < 0.0 {
                    return Err(IblPreprocessError::InvalidRadiance { pixel, channel });
                }
            }
        }
        Ok(())
    }

    /// Bilinear latlong sampling wraps longitude and clamps only the poles.
    pub fn sample_latlong(&self, direction: [f32; 3]) -> [f32; 3] {
        let direction = normalize(direction);
        let u = 0.5 + direction[2].atan2(direction[0]) / TAU;
        let v = direction[1].clamp(-1.0, 1.0).acos() / PI;
        self.sample_uv(u, v)
    }

    fn sample_uv(&self, u: f32, v: f32) -> [f32; 3] {
        let width = self.width.max(1) as i64;
        let height = self.height.max(1) as i64;
        let x = u * width as f32 - 0.5;
        let y = v.clamp(0.0, 1.0) * height as f32 - 0.5;
        let x0 = x.floor() as i64;
        let y0 = y.floor() as i64;
        let fx = x - x.floor();
        let fy = y - y.floor();
        let pixel = |px: i64, py: i64| {
            let index = (py.clamp(0, height - 1) * width + px.rem_euclid(width)) as usize;
            self.pixels.get(index).copied().unwrap_or([0.0; 3])
        };
        let top = lerp3(pixel(x0, y0), pixel(x0 + 1, y0), fx);
        let bottom = lerp3(pixel(x0, y0 + 1), pixel(x0 + 1, y0 + 1), fx);
        lerp3(top, bottom, fy)
    }

    /// Rgba16Float upload bytes. CPU radiance remains f32; upload saturation at
    /// 65504 is explicit because infinities cannot be filtered by the GPU.
    pub fn rgba16f_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 8);
        for pixel in &self.pixels {
            for value in pixel.iter().copied().chain(std::iter::once(1.0)) {
                bytes.extend_from_slice(&f16::from_f32(value.min(65504.0)).to_bits().to_ne_bytes());
            }
        }
        bytes
    }
}

impl BrdfIntegrationLut {
    /// Existing GGX RG stays byte-exact; B is Charlie + Neubelt directional albedo.
    pub fn rgba16f_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 8);
        for (index, pixel) in self.pixels.iter().enumerate() {
            let size = self.size.max(1);
            let n_dot_v = ((index as u32 % size) as f32 + 0.5) / size as f32;
            let roughness = ((index as u32 / size) as f32 + 0.5) / size as f32;
            for value in [
                pixel[0],
                pixel[1],
                crate::material_layers::directional_albedo(n_dot_v, roughness),
                0.0,
            ] {
                bytes.extend_from_slice(&f16::from_f32(value).to_bits().to_ne_bytes());
            }
        }
        bytes
    }

    pub fn rg16f_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.pixels.len() * 4);
        for pixel in &self.pixels {
            for value in pixel {
                bytes.extend_from_slice(&f16::from_f32(*value).to_bits().to_ne_bytes());
            }
        }
        bytes
    }

    pub fn sample(&self, n_dot_v: f32, roughness: f32) -> [f32; 2] {
        let size = self.size.max(1) as i64;
        let x = n_dot_v.clamp(0.0, 1.0) * size as f32 - 0.5;
        let y = roughness.clamp(0.0, 1.0) * size as f32 - 0.5;
        let x0 = x.floor() as i64;
        let y0 = y.floor() as i64;
        let fx = x - x.floor();
        let fy = y - y.floor();
        let pixel = |px: i64, py: i64| {
            self.pixels[(py.clamp(0, size - 1) * size + px.clamp(0, size - 1)) as usize]
        };
        let a = pixel(x0, y0);
        let b = pixel(x0 + 1, y0);
        let c = pixel(x0, y0 + 1);
        let d = pixel(x0 + 1, y0 + 1);
        std::array::from_fn(|i| {
            (a[i] + (b[i] - a[i]) * fx) * (1.0 - fy) + (c[i] + (d[i] - c[i]) * fx) * fy
        })
    }
}

/// Lambertian convolution factors for radiance SH coefficients, yielding E.
pub fn lambert_sh_factor(coefficient: usize) -> f32 {
    match coefficient {
        0 => PI,
        1..=3 => 2.0 * PI / 3.0,
        4..=8 => PI / 4.0,
        _ => 0.0,
    }
}

pub fn sh_basis(direction: [f32; 3]) -> [f32; 9] {
    let [x, y, z] = normalize(direction);
    [
        0.282_094_8,
        0.488_602_52 * y,
        0.488_602_52 * z,
        0.488_602_52 * x,
        1.092_548_5 * x * y,
        1.092_548_5 * y * z,
        0.315_391_57 * (3.0 * z * z - 1.0),
        1.092_548_5 * x * z,
        0.546_274_24 * (x * x - y * y),
    ]
}

/// Integrate piecewise-constant latlong texels exactly over SH basis cells.
/// Unlike centre-sample equal-area guesses, even a 1x1 constant source has
/// exactly zero directional coefficients and irradiance PI * radiance.
pub fn irradiance_sh9(environment: &LinearEnvironment) -> IrradianceSh9 {
    let width = environment.width.max(1) as usize;
    let height = environment.height.max(1) as usize;
    let longitude: Vec<_> = (0..width)
        .map(|x| {
            let a = x as f64 / width as f64 * std::f64::consts::TAU - std::f64::consts::PI;
            let b = (x + 1) as f64 / width as f64 * std::f64::consts::TAU - std::f64::consts::PI;
            let delta = b - a;
            [
                delta,
                b.sin() - a.sin(),
                a.cos() - b.cos(),
                (b.sin().powi(2) - a.sin().powi(2)) * 0.5,
                delta * 0.5 - ((2.0 * b).sin() - (2.0 * a).sin()) * 0.25,
                delta * 0.5 + ((2.0 * b).sin() - (2.0 * a).sin()) * 0.25,
            ]
        })
        .collect();
    let mut coefficients = [[0.0f64; 3]; 9];
    for y in 0..height {
        let a = y as f64 / height as f64 * std::f64::consts::PI;
        let b = (y + 1) as f64 / height as f64 * std::f64::consts::PI;
        let s = a.cos() - b.cos();
        let s2 = (b - a) * 0.5 - ((2.0 * b).sin() - (2.0 * a).sin()) * 0.25;
        let cs = (a.cos().powi(2) - b.cos().powi(2)) * 0.5;
        let s2c = (b.sin().powi(3) - a.sin().powi(3)) / 3.0;
        let s3 = (-b.cos() + b.cos().powi(3) / 3.0) - (-a.cos() + a.cos().powi(3) / 3.0);
        let c2s = (a.cos().powi(3) - b.cos().powi(3)) / 3.0;
        for (x, p) in longitude.iter().enumerate() {
            let weights = [
                0.282_094_791_773_878_14 * s * p[0],
                0.488_602_511_902_919_9 * cs * p[0],
                0.488_602_511_902_919_9 * s2 * p[2],
                0.488_602_511_902_919_9 * s2 * p[1],
                1.092_548_430_592_079_2 * s2c * p[1],
                1.092_548_430_592_079_2 * s2c * p[2],
                0.315_391_565_252_520_05 * (3.0 * s3 * p[4] - s * p[0]),
                1.092_548_430_592_079_2 * s3 * p[3],
                0.546_274_215_296_039_6 * (s3 * p[5] - c2s * p[0]),
            ];
            let rgb = environment
                .pixels
                .get(y * width + x)
                .copied()
                .unwrap_or([0.0; 3]);
            for (coefficient, weight) in coefficients.iter_mut().zip(weights) {
                for channel in 0..3 {
                    coefficient[channel] += rgb[channel] as f64 * weight;
                }
            }
        }
    }
    std::array::from_fn(|i| {
        std::array::from_fn(|channel| {
            if channel == 3 {
                0.0
            } else {
                (coefficients[i][channel] * lambert_sh_factor(i) as f64) as f32
            }
        })
    })
}

pub fn evaluate_irradiance_sh9(coefficients: &IrradianceSh9, normal: [f32; 3]) -> [f32; 3] {
    let basis = sh_basis(normal);
    std::array::from_fn(|channel| {
        coefficients
            .iter()
            .zip(basis)
            .map(|(coefficient, value)| coefficient[channel] * value)
            .sum::<f32>()
            .max(0.0)
    })
}

pub fn preprocess_environment(
    environment: &LinearEnvironment,
    options: IblPreprocessOptions,
) -> Result<PreprocessedEnvironment, IblPreprocessError> {
    environment.validate()?;
    validate_options(options)?;
    let background_mips = background_mips(environment);
    let diffuse_sh = irradiance_sh9(environment);
    let specular_mips = prefilter_from_background(environment, &background_mips, options);
    let brdf_lut = build_brdf_lut(options.brdf_lut_size, options.brdf_samples);
    Ok(PreprocessedEnvironment {
        diffuse_sh,
        specular_mips,
        background_mips,
        brdf_lut,
    })
}

/// Filter a local reflection capture without rebuilding the shared BRDF LUT.
pub fn prefilter_specular_mips(
    environment: &LinearEnvironment,
    options: IblPreprocessOptions,
) -> Result<Vec<LinearEnvironment>, IblPreprocessError> {
    environment.validate()?;
    validate_options(options)?;
    let background = background_mips(environment);
    Ok(prefilter_from_background(environment, &background, options))
}

/// A shared LUT is independent of the environment or local capture contents.
pub fn build_brdf_integration_lut(
    size: u32,
    samples: u32,
) -> Result<BrdfIntegrationLut, IblPreprocessError> {
    validate_options(IblPreprocessOptions {
        brdf_lut_size: size,
        brdf_samples: samples,
        ..Default::default()
    })?;
    Ok(build_brdf_lut(size, samples))
}

fn prefilter_from_background(
    environment: &LinearEnvironment,
    background_mips: &[LinearEnvironment],
    options: IblPreprocessOptions,
) -> Vec<LinearEnvironment> {
    let width = floor_power_of_two(environment.width.min(options.max_specular_width));
    let height = (width / 2).max(1);
    let levels = width.ilog2() + 1;
    let mut specular_mips = Vec::with_capacity(levels as usize);
    for level in 0..levels {
        let mip_width = (width >> level).max(1);
        let mip_height = (height >> level).max(1);
        let roughness = if levels <= 1 {
            0.0
        } else {
            level as f32 / (levels - 1) as f32
        };
        let samples = (0..options.specular_samples)
            .map(|i| importance_sample_ggx(hammersley(i, options.specular_samples), roughness))
            .collect::<Vec<_>>();
        let mut pixels = Vec::with_capacity((mip_width * mip_height) as usize);
        for y in 0..mip_height {
            for x in 0..mip_width {
                let direction = latlong_direction(
                    (x as f32 + 0.5) / mip_width as f32,
                    (y as f32 + 0.5) / mip_height as f32,
                );
                let value = if level == 0 {
                    // Preserve the sharp lobe; use source downsample mips only
                    // to antialias the capped specular base resolution.
                    let lod = (environment.width as f32 / width as f32).log2().max(0.0);
                    sample_mip_chain(background_mips, direction, lod)
                } else {
                    prefilter_direction(background_mips, direction, roughness, &samples)
                };
                pixels.push(value);
            }
        }
        specular_mips.push(LinearEnvironment {
            width: mip_width,
            height: mip_height,
            pixels,
        });
    }
    specular_mips
}

/// Correlated Smith GGX split-sum coefficients A and B: specular = F0*A+B.
/// The same perceptual-roughness convention is used by the prefilter and WGSL.
pub fn integrate_brdf(n_dot_v: f32, roughness: f32, samples: u32) -> [f32; 2] {
    let n_dot_v = n_dot_v.clamp(0.0001, 1.0);
    let roughness = roughness.clamp(0.001, 1.0);
    let view = [(1.0 - n_dot_v * n_dot_v).sqrt(), 0.0, n_dot_v];
    let mut result = [0.0; 2];
    let count = samples.clamp(1, 4096);
    for i in 0..count {
        let half = importance_sample_ggx(hammersley(i, count), roughness);
        let v_dot_h = dot(view, half).max(0.0);
        let light = sub3(scale3(half, 2.0 * v_dot_h), view);
        let n_dot_l = light[2].max(0.0);
        let n_dot_h = half[2].max(0.000001);
        if n_dot_l > 0.0 {
            let geometry = geometry_smith_correlated(n_dot_v, n_dot_l, roughness);
            let weight = geometry * v_dot_h / (n_dot_h * n_dot_v);
            let fresnel = (1.0 - v_dot_h).clamp(0.0, 1.0).powi(5);
            result[0] += (1.0 - fresnel) * weight;
            result[1] += fresnel * weight;
        }
    }
    result = result.map(|value| (value / count as f32).max(0.0));
    // Finite-sample quadrature can slightly exceed unity at grazing angles.
    // Bound the single-scattering response instead of injecting extra energy.
    let total = result[0] + result[1];
    if total > 1.0 {
        result = result.map(|value| value / total);
    }
    result
}

fn validate_options(options: IblPreprocessOptions) -> Result<(), IblPreprocessError> {
    for (name, value, valid) in [
        (
            "max_specular_width",
            options.max_specular_width,
            (1..=1024).contains(&options.max_specular_width)
                && options.max_specular_width.is_power_of_two(),
        ),
        (
            "specular_samples",
            options.specular_samples,
            (1..=256).contains(&options.specular_samples),
        ),
        (
            "brdf_lut_size",
            options.brdf_lut_size,
            (4..=128).contains(&options.brdf_lut_size) && options.brdf_lut_size.is_power_of_two(),
        ),
        (
            "brdf_samples",
            options.brdf_samples,
            (1..=1024).contains(&options.brdf_samples),
        ),
    ] {
        if !valid {
            return Err(IblPreprocessError::InvalidOption { name, value });
        }
    }
    Ok(())
}

fn build_brdf_lut(size: u32, samples: u32) -> BrdfIntegrationLut {
    let mut pixels = Vec::with_capacity((size * size) as usize);
    for y in 0..size {
        for x in 0..size {
            pixels.push(integrate_brdf(
                (x as f32 + 0.5) / size as f32,
                (y as f32 + 0.5) / size as f32,
                samples,
            ));
        }
    }
    BrdfIntegrationLut { size, pixels }
}

fn background_mips(environment: &LinearEnvironment) -> Vec<LinearEnvironment> {
    let mut result = vec![environment.clone()];
    while result
        .last()
        .is_some_and(|mip| mip.width > 1 || mip.height > 1)
    {
        let previous = result.last().expect("source mip exists");
        let width = (previous.width / 2).max(1);
        let height = (previous.height / 2).max(1);
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            for x in 0..width {
                let mut value = [0.0; 3];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let sx = (x * 2 + dx).min(previous.width - 1);
                        let sy = (y * 2 + dy).min(previous.height - 1);
                        let pixel = previous.pixels[(sy * previous.width + sx) as usize];
                        for channel in 0..3 {
                            value[channel] += pixel[channel] * 0.25;
                        }
                    }
                }
                pixels.push(value);
            }
        }
        result.push(LinearEnvironment {
            width,
            height,
            pixels,
        });
    }
    result
}

fn prefilter_direction(
    mips: &[LinearEnvironment],
    normal: [f32; 3],
    roughness: f32,
    half_samples: &[[f32; 3]],
) -> [f32; 3] {
    let up = if normal[2].abs() < 0.999 {
        [0.0, 0.0, 1.0]
    } else {
        [1.0, 0.0, 0.0]
    };
    let tangent = normalize(cross(up, normal));
    let bitangent = cross(normal, tangent);
    let mut result = [0.0; 3];
    let mut total = 0.0;
    for local_half in half_samples {
        let half = add3(
            add3(
                scale3(tangent, local_half[0]),
                scale3(bitangent, local_half[1]),
            ),
            scale3(normal, local_half[2]),
        );
        let n_dot_h = dot(normal, half).max(0.0);
        let light = normalize(sub3(scale3(half, 2.0 * n_dot_h), normal));
        let n_dot_l = dot(normal, light).max(0.0);
        if n_dot_l > 0.0 {
            let alpha2 = roughness.powi(4);
            let denominator = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
            let distribution = alpha2 / (PI * denominator * denominator).max(0.000000000001);
            let pdf = (distribution * n_dot_h / (4.0 * n_dot_h).max(0.000001)).max(0.000001);
            let sample_solid_angle = 1.0 / (half_samples.len() as f32 * pdf);
            let texel_solid_angle = latlong_texel_solid_angle(&mips[0], light);
            let lod = (0.5 * (sample_solid_angle / texel_solid_angle).log2()).max(0.0);
            let sampled = sample_mip_chain(mips, light, lod);
            for channel in 0..3 {
                result[channel] += sampled[channel] * n_dot_l;
            }
            total += n_dot_l;
        }
    }
    if total <= 0.000001 {
        mips[0].sample_latlong(normal)
    } else {
        result.map(|value| value / total)
    }
}

fn sample_mip_chain(mips: &[LinearEnvironment], direction: [f32; 3], lod: f32) -> [f32; 3] {
    let lod = lod.clamp(0.0, mips.len().saturating_sub(1) as f32);
    let lower = lod.floor() as usize;
    let upper = (lower + 1).min(mips.len() - 1);
    lerp3(
        mips[lower].sample_latlong(direction),
        mips[upper].sample_latlong(direction),
        lod - lower as f32,
    )
}

fn latlong_texel_solid_angle(environment: &LinearEnvironment, direction: [f32; 3]) -> f32 {
    let polar = direction[1].clamp(-1.0, 1.0).acos();
    let row = (polar / PI * environment.height as f32)
        .floor()
        .clamp(0.0, environment.height.saturating_sub(1) as f32);
    let top = row * PI / environment.height as f32;
    let bottom = (row + 1.0) * PI / environment.height as f32;
    (TAU / environment.width as f32 * (top.cos() - bottom.cos())).max(0.00000001)
}

fn geometry_smith_correlated(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    let alpha2 = roughness.powi(4);
    let gv = n_dot_l * (n_dot_v * n_dot_v * (1.0 - alpha2) + alpha2).sqrt();
    let gl = n_dot_v * (n_dot_l * n_dot_l * (1.0 - alpha2) + alpha2).sqrt();
    2.0 * n_dot_v * n_dot_l / (gv + gl).max(0.000000000001)
}

fn importance_sample_ggx(xi: [f32; 2], roughness: f32) -> [f32; 3] {
    let alpha2 = roughness.max(0.001).powi(4);
    let longitude = TAU * xi[0];
    let cos_theta = ((1.0 - xi[1]) / (1.0 + (alpha2 - 1.0) * xi[1]))
        .max(0.0)
        .sqrt();
    let sin_theta = (1.0 - cos_theta * cos_theta).max(0.0).sqrt();
    [
        longitude.cos() * sin_theta,
        longitude.sin() * sin_theta,
        cos_theta,
    ]
}

fn hammersley(index: u32, count: u32) -> [f32; 2] {
    [
        index as f32 / count as f32,
        index.reverse_bits() as f32 * 2.328_306_4e-10,
    ]
}

fn latlong_direction(u: f32, v: f32) -> [f32; 3] {
    let longitude = (u - 0.5) * TAU;
    let polar = v * PI;
    [
        polar.sin() * longitude.cos(),
        polar.cos(),
        polar.sin() * longitude.sin(),
    ]
}

fn floor_power_of_two(value: u32) -> u32 {
    1 << value.max(1).ilog2()
}
fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}
fn scale3(a: [f32; 3], scale: f32) -> [f32; 3] {
    a.map(|value| value * scale)
}
fn add3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn lerp3(a: [f32; 3], b: [f32; 3], factor: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * factor)
}
fn normalize(value: [f32; 3]) -> [f32; 3] {
    let length = dot(value, value).sqrt();
    if length.is_finite() && length > 0.0000001 {
        scale3(value, 1.0 / length)
    } else {
        [0.0, 1.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn sheen_lut_addition_preserves_existing_ggx_half_bytes() {
        let lut = super::BrdfIntegrationLut {
            size: 2,
            pixels: vec![[0.1, 0.2], [0.3, 0.4], [0.5, 0.6], [0.7, 0.8]],
        };
        let rg = lut.rg16f_bytes();
        let rgba = lut.rgba16f_bytes();
        assert_eq!(rgba.len(), rg.len() * 2);
        for (a, b) in rg.chunks_exact(4).zip(rgba.chunks_exact(8)) {
            assert_eq!(a, &b[..4]);
            let energy =
                half::f16::from_bits(u16::from_ne_bytes(b[4..6].try_into().unwrap())).to_f32();
            assert!((0.0..=1.0).contains(&energy));
        }
    }

    use super::*;

    fn assert_close(actual: f32, expected: f32, tolerance: f32) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual} != {expected} ±{tolerance}"
        );
    }

    fn small_options() -> IblPreprocessOptions {
        IblPreprocessOptions {
            max_specular_width: 32,
            specular_samples: 64,
            brdf_lut_size: 16,
            brdf_samples: 128,
        }
    }

    #[test]
    fn constant_hdr_irradiance_is_pi_radiance_at_every_direction() {
        for (width, height) in [(1, 1), (4, 2), (64, 32)] {
            let radiance = [2.0, 4.0, 8.0];
            let environment =
                LinearEnvironment::new(width, height, vec![radiance; (width * height) as usize])
                    .unwrap();
            let sh = irradiance_sh9(&environment);
            for direction in [
                [1.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, -1.0, 0.0],
                [0.0, 0.0, 1.0],
            ] {
                let actual = evaluate_irradiance_sh9(&sh, direction);
                for channel in 0..3 {
                    assert_close(actual[channel], radiance[channel] * PI, 0.00001);
                }
            }
            for coefficient in &sh[1..] {
                for component in coefficient {
                    assert!(component.abs() < 0.00001);
                }
            }
        }
    }

    #[test]
    fn hemispherical_source_retains_direction_and_correct_solid_angle() {
        let (width, height) = (32, 16);
        let pixels = (0..width * height)
            .map(|index| {
                if index / width < height / 2 {
                    [1.0; 3]
                } else {
                    [0.0; 3]
                }
            })
            .collect();
        let sh = irradiance_sh9(&LinearEnvironment::new(width, height, pixels).unwrap());
        let up = evaluate_irradiance_sh9(&sh, [0.0, 1.0, 0.0])[0];
        let side = evaluate_irradiance_sh9(&sh, [1.0, 0.0, 0.0])[0];
        let down = evaluate_irradiance_sh9(&sh, [0.0, -1.0, 0.0])[0];
        assert_close(up, PI, 0.00001);
        assert_close(side, PI / 2.0, 0.00001);
        assert_close(down, 0.0, 0.00001);
    }

    #[test]
    fn latlong_sampling_wraps_seam_without_clamping_longitude() {
        let environment = LinearEnvironment::new(
            4,
            2,
            vec![
                [2.0, 0.0, 0.0],
                [0.0; 3],
                [0.0; 3],
                [0.0, 0.0, 2.0],
                [2.0, 0.0, 0.0],
                [0.0; 3],
                [0.0; 3],
                [0.0, 0.0, 2.0],
            ],
        )
        .unwrap();
        let a = environment.sample_latlong([-1.0, 0.0, 0.0000001]);
        let b = environment.sample_latlong([-1.0, 0.0, -0.0000001]);
        for channel in 0..3 {
            assert_close(a[channel], b[channel], 0.00001);
        }
        assert_close(a[0], 1.0, 0.00001);
        assert_close(a[2], 1.0, 0.00001);
    }

    #[test]
    fn ggx_mips_preserve_constant_hdr_and_original_background() {
        let radiance = [1.5, 3.0, 6.0];
        let source = LinearEnvironment::new(64, 32, vec![radiance; 64 * 32]).unwrap();
        let processed = preprocess_environment(&source, small_options()).unwrap();
        assert_eq!(
            (
                processed.specular_mips[0].width,
                processed.specular_mips[0].height
            ),
            (32, 16)
        );
        assert_eq!(
            (
                processed.background_mips[0].width,
                processed.background_mips[0].height
            ),
            (64, 32)
        );
        assert_eq!(processed.background_mips[0].pixels, source.pixels);
        for mip in &processed.specular_mips {
            for pixel in &mip.pixels {
                for channel in 0..3 {
                    assert_close(pixel[channel], radiance[channel], 0.00001);
                }
            }
        }
        let half_bytes = processed.specular_mips[0].rgba16f_bytes();
        assert_eq!(half_bytes.len(), 32 * 16 * 8);
        let blue = f16::from_bits(u16::from_ne_bytes([half_bytes[4], half_bytes[5]])).to_f32();
        assert_eq!(blue, 6.0);
    }

    #[test]
    fn ggx_high_roughness_spreads_a_local_highlight() {
        let mut pixels = vec![[0.01; 3]; 32 * 16];
        for y in 6..10 {
            for x in 14..18 {
                pixels[y * 32 + x] = [20.0; 3];
            }
        }
        let source = LinearEnvironment::new(32, 16, pixels).unwrap();
        let processed = preprocess_environment(&source, small_options()).unwrap();
        let sharp = processed.specular_mips[0]
            .pixels
            .iter()
            .map(|p| p[0])
            .fold(0.0, f32::max);
        let rough = processed.specular_mips[3]
            .pixels
            .iter()
            .map(|p| p[0])
            .fold(0.0, f32::max);
        assert!(sharp > 10.0);
        assert!(
            rough > 0.01 && rough < sharp * 0.75,
            "rough={rough}, sharp={sharp}"
        );
        assert!(
            processed
                .specular_mips
                .iter()
                .flat_map(|m| &m.pixels)
                .flatten()
                .all(|v| v.is_finite())
        );
    }

    #[test]
    fn brdf_mirror_limit_matches_schlick_f0_and_f90() {
        for n_dot_v in [0.05, 0.25, 0.5, 1.0] {
            let [a, b] = integrate_brdf(n_dot_v, 0.0, 512);
            let expected_fresnel = 0.04 + 0.96 * (1.0 - n_dot_v).powi(5);
            assert_close(0.04 * a + b, expected_fresnel, 0.002);
            assert_close(a + b, 1.0, 0.002);
        }
    }

    #[test]
    fn brdf_single_scattering_energy_is_bounded_and_roughness_dependent() {
        for n_dot_v in [0.01, 0.1, 0.3, 0.6, 1.0] {
            for roughness in [0.0, 0.1, 0.25, 0.5, 0.75, 1.0] {
                let [a, b] = integrate_brdf(n_dot_v, roughness, 1024);
                assert!(a.is_finite() && b.is_finite() && a >= 0.0 && b >= 0.0);
                assert!(a + b <= 1.000001);
                for f0 in [0.0, 0.04, 0.5, 1.0] {
                    assert!(f0 * a + b <= 1.000001);
                }
            }
        }
        let smooth = integrate_brdf(1.0, 0.05, 1024);
        let rough = integrate_brdf(1.0, 1.0, 1024);
        assert!(rough[0] < smooth[0] * 0.5);
    }

    #[test]
    fn preprocessing_is_deterministic_and_rejects_invalid_inputs() {
        assert!(matches!(
            LinearEnvironment::new(0, 1, vec![]),
            Err(IblPreprocessError::InvalidDimensions { .. })
        ));
        assert!(matches!(
            LinearEnvironment::new(2, 1, vec![[1.0; 3]]),
            Err(IblPreprocessError::PixelCount { .. })
        ));
        assert!(matches!(
            LinearEnvironment::new(1, 1, vec![[f32::NAN, 0.0, 0.0]]),
            Err(IblPreprocessError::InvalidRadiance { .. })
        ));
        let source = LinearEnvironment::new(4, 2, vec![[2.0; 3]; 8]).unwrap();
        let options = small_options();
        let a = preprocess_environment(&source, options).unwrap();
        let b = preprocess_environment(&source, options).unwrap();
        assert_eq!(a.diffuse_sh, b.diffuse_sh);
        assert_eq!(
            a.specular_mips[1].rgba16f_bytes(),
            b.specular_mips[1].rgba16f_bytes()
        );
        assert_eq!(a.brdf_lut.rg16f_bytes(), b.brdf_lut.rg16f_bytes());
        assert!(
            preprocess_environment(
                &source,
                IblPreprocessOptions {
                    max_specular_width: 511,
                    ..options
                }
            )
            .is_err()
        );
    }

    #[test]
    fn ldr_and_float_decoding_use_their_correct_color_spaces() {
        let ldr = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            1,
            1,
            image::Rgb([128, 128, 128]),
        ));
        let linear = LinearEnvironment::from_image(&ldr).unwrap();
        assert_close(linear.pixels[0][0], 0.2158605, 0.00001);
        let hdr = image::DynamicImage::ImageRgb32F(image::Rgb32FImage::from_pixel(
            1,
            1,
            image::Rgb([2.0, 4.0, 8.0]),
        ));
        assert_eq!(
            LinearEnvironment::from_image(&hdr).unwrap().pixels[0],
            [2.0, 4.0, 8.0]
        );
    }
}
