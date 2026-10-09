// =========================================
// =========================================
// src/world/render/shaders/environment.wgsl

fn environment_rotated_direction(direction: vec3<f32>) -> vec3<f32> {
    let rotated_x = direction.x * cos(lighting.environment0.y) - direction.z * sin(lighting.environment0.y);
    let rotated_z = direction.x * sin(lighting.environment0.y) + direction.z * cos(lighting.environment0.y);
    return normalize(vec3<f32>(rotated_x, direction.y, rotated_z));
}
fn direction_to_environment_uv(direction: vec3<f32>) -> vec2<f32> {
    let normalized = environment_rotated_direction(direction);
    let u = 0.5 + atan2(normalized.z, normalized.x) / (2.0 * 3.14159265);
    let v = acos(clamp(normalized.y, -1.0, 1.0)) / 3.14159265;
    return vec2<f32>(u, v);
}
fn sample_environment(direction: vec3<f32>, lod: f32) -> vec3<f32> {
    return textureSampleLevel(
        environment_texture,
        environment_sampler,
        direction_to_environment_uv(direction),
        clamp(lod, 0.0, lighting.environment0.z)
    ).rgb;
}

// SH9 stores Lambert-convolved irradiance E. Material shading applies albedo/PI.
// The order and constants match lighting_ibl::sh_basis exactly.
fn sample_environment_irradiance(normal: vec3<f32>) -> vec3<f32> {
    let n = environment_rotated_direction(normal);
    let basis = array<f32, 9>(
        0.2820948,
        0.48860252 * n.y,
        0.48860252 * n.z,
        0.48860252 * n.x,
        1.0925485 * n.x * n.y,
        1.0925485 * n.y * n.z,
        0.31539157 * (3.0 * n.z * n.z - 1.0),
        1.0925485 * n.x * n.z,
        0.54627424 * (n.x * n.x - n.y * n.y)
    );
    var irradiance = vec3<f32>(0.0);
    for (var i = 0u; i < 9u; i = i + 1u) {
        irradiance += lighting.environment_sh[i].rgb * basis[i];
    }
    return max(irradiance, vec3<f32>(0.0));
}

// GGX split sum: the LUT contains A/B, so Fresnel must not be multiplied again.
// Authored environment intensity and specular strength are applied by the caller.
fn sample_environment_specular(
    reflected: vec3<f32>,
    roughness: f32,
    f0: vec3<f32>,
    n_dot_v: f32
) -> vec3<f32> {
    let perceptual_roughness = clamp(roughness, 0.0, 1.0);
    let prefiltered = sample_environment(reflected, perceptual_roughness * lighting.environment0.z);
    let brdf = textureSampleLevel(
        environment_brdf_texture,
        environment_brdf_sampler,
        vec2<f32>(clamp(n_dot_v, 0.0, 1.0), perceptual_roughness),
        0.0
    ).rg;
    return prefiltered * (clamp(f0, vec3<f32>(0.0), vec3<f32>(1.0)) * brdf.x + vec3<f32>(brdf.y));
}

// Visible sky always samples source mips, not the reflection roughness chain.
fn sample_environment_background(direction: vec3<f32>, blur: f32) -> vec3<f32> {
    let max_lod = f32(textureNumLevels(environment_background_texture) - 1u);
    return textureSampleLevel(
        environment_background_texture,
        environment_sampler,
        direction_to_environment_uv(direction),
        clamp(blur, 0.0, 1.0) * max_lod
    ).rgb;
}
