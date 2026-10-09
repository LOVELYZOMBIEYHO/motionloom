//! Portable, camera-independent lighting bake asset contract.
use serde::{Deserialize, Serialize};

pub const LIGHTING_BAKE_SCHEMA_VERSION: u32 = 1;
pub const LIGHTING_BAKER_VERSION: &str = "diffuse-probes-v2";
pub const PROBE_VISIBILITY_RESOLUTION: u32 = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BakeVolumeOptions {
    pub id: String,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Probe order is x fastest, then y, then z; endpoints include the bounds.
    /// Every axis has at least two samples for trilinear runtime interpolation.
    pub counts: [u32; 3],
    /// V1 supports zero or one camera-independent local capture per room.
    pub reflection_positions: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LightingBakeOptions {
    pub scene_id: String,
    pub day_frame: u32,
    pub dusk_frame: u32,
    pub volumes: Vec<BakeVolumeOptions>,
    pub rays_per_probe: u32,
    /// Equirectangular width/height; no tone mapping is baked into the data.
    pub specular_resolution: [u32; 2],
    pub specular_samples_per_pixel: u32,
    pub max_bounces: u32,
    pub seed: u64,
    pub max_ray_distance: f32,
}

impl Default for LightingBakeOptions {
    fn default() -> Self {
        Self {
            scene_id: String::new(),
            day_frame: 360,
            dusk_frame: 1032,
            volumes: Vec::new(),
            rays_per_probe: 256,
            specular_resolution: [128, 64],
            specular_samples_per_pixel: 2,
            max_bounces: 3,
            seed: 99,
            max_ray_distance: 100.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BakeFingerprint {
    pub geometry: String,
    pub materials: String,
    pub textures: String,
    pub lighting: String,
    pub environments: String,
    pub settings: String,
    pub combined: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IrradianceProbe {
    pub position: [f32; 3],
    pub valid: bool,
    /// Real SH order: Y00, Y1-1(y), Y10(z), Y11(x), Y2-2(xy),
    /// Y2-1(yz), Y20(3z²-1), Y21(xz), Y22(x²-y²).
    /// Cosine-convolved irradiance E; Lambert shading multiplies albedo/PI.
    pub irradiance_sh: [[f32; 3]; 9],
    /// 8x8 octahedral direction cells, row-major, first/second distance moments.
    pub depth_moments: Vec<[f32; 2]>,
    /// Mean straight-ray RGB transmittance reduced to luminance per cell.
    pub visibility: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReflectionProbe {
    pub position: [f32; 3],
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Relative to this JSON asset, Radiance RGBE equirectangular linear HDR.
    pub src: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BakedLightingVolume {
    pub id: String,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub counts: [u32; 3],
    pub probes: Vec<IrradianceProbe>,
    pub reflections: Vec<LocalReflectionProbe>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BakedLightingState {
    /// First "day", second "dusk"; blend=0 selects day and blend=1 dusk.
    pub name: String,
    pub frame: u32,
    pub volumes: Vec<BakedLightingVolume>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BakedLightingAsset {
    pub schema_version: u32,
    pub baker_version: String,
    pub scene_id: String,
    #[serde(default)]
    pub authoring_fingerprint: String,
    pub fingerprint: BakeFingerprint,
    pub states: Vec<BakedLightingState>,
    pub diagnostics: Vec<String>,
}

/// In-memory linear HDR images are returned without implicit filesystem writes.
#[derive(Debug, Clone)]
pub struct BakedReflectionImage {
    pub src: String,
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}

#[derive(Debug, Clone)]
pub struct LightingBakeBundle {
    pub asset: BakedLightingAsset,
    pub reflection_images: Vec<BakedReflectionImage>,
}
