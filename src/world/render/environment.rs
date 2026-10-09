// =========================================
// =========================================
// src/world/render/environment.rs

//! Decode and preprocess cached diffuse/specular environment lighting.

use super::*;

/// Keep the original background separate from the GGX-filtered reflection map.
/// This runs only on a retained environment-cache miss, never per frame.
pub(super) fn load_environment_image_from_resolved(
    resolved: &ResolvedWorldAsset,
) -> Result<WorldEnvironmentImage, WorldRenderError> {
    let image = load_rgba_image_from_resolved(resolved, |path, source| {
        WorldRenderError::BackgroundImage { path, source }
    })?;
    let source = crate::lighting_ibl::LinearEnvironment::from_image(&image).map_err(|source| {
        WorldRenderError::GpuRender {
            message: format!("invalid environment radiance: {source}"),
        }
    })?;
    let processed = crate::lighting_ibl::preprocess_environment(
        &source,
        crate::lighting_ibl::IblPreprocessOptions::default(),
    )
    .map_err(|source| WorldRenderError::GpuRender {
        message: format!("environment IBL preprocessing failed: {source}"),
    })?;
    let width = processed.specular_mips[0].width;
    let height = processed.specular_mips[0].height;
    let mip_bytes = processed
        .specular_mips
        .iter()
        .map(crate::lighting_ibl::LinearEnvironment::rgba16f_bytes)
        .collect::<Vec<_>>();
    let background_width = source.width;
    let background_height = source.height;
    let background_mip_bytes = processed
        .background_mips
        .iter()
        .map(crate::lighting_ibl::LinearEnvironment::rgba16f_bytes)
        .collect::<Vec<_>>();
    let diffuse_sh = processed.diffuse_sh;
    let brdf_width = processed.brdf_lut.size;
    let brdf_height = processed.brdf_lut.size;
    let brdf_bytes = processed.brdf_lut.rgba16f_bytes();
    let mut hasher = DefaultHasher::new();
    width.hash(&mut hasher);
    height.hash(&mut hasher);
    for bytes in &mip_bytes {
        bytes.hash(&mut hasher);
    }
    background_width.hash(&mut hasher);
    background_height.hash(&mut hasher);
    for bytes in &background_mip_bytes {
        bytes.hash(&mut hasher);
    }
    for coefficient in diffuse_sh {
        for value in coefficient {
            value.to_bits().hash(&mut hasher);
        }
    }
    brdf_bytes.hash(&mut hasher);
    Ok(WorldEnvironmentImage {
        width,
        height,
        mip_bytes,
        background_width,
        background_height,
        background_mip_bytes,
        diffuse_sh,
        brdf_width,
        brdf_height,
        brdf_bytes,
        signature: hasher.finish(),
    })
}
