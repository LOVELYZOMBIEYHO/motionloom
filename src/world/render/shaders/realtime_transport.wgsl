// src/world/render/shaders/realtime_transport.wgsl
// Raster preview keeps PBR optics while screen-space evidence and probes own
// reflected radiance. This module has no scene-geometry storage or traversal.
const REALTIME_TRANSPORT: bool = true;
const HYBRID_SOLID_TRANSPORT: bool = false;
const PRIMARY_GLASS_SHADOWS_ENABLED: bool = false;
const HYBRID_ALPHA_EVALUATION_ENABLED: bool = true;

var<private> hybrid_reflection_hit_distance: f32;

struct HybridTransportResult {
    radiance: vec3<f32>,
    confidence: f32,
    distance: f32,
    interface_count: u32,
};

fn hybrid_epsilon(position: vec3<f32>) -> f32 {
    return max(0.00005, max(abs(position.x), max(abs(position.y), abs(position.z))) * 0.000002);
}

fn hybrid_environment(position: vec3<f32>, direction: vec3<f32>, roughness: f32) -> vec3<f32> {
    let global = sample_environment(direction, roughness * lighting.environment0.z) * lighting.environment0.x * lighting.environment1.w;
    let local = sample_local_radiance(position, direction, roughness);
    return mix(global, local.rgb, local.a);
}

fn hybrid_fresnel(cosine: f32, eta_i: f32, eta_t: f32) -> f32 {
    let c = clamp(abs(cosine), 0.0, 1.0);
    let eta = eta_i / eta_t;
    let sin2 = eta * eta * (1.0 - c * c);
    if (sin2 >= 1.0) { return 1.0; }
    let ct = sqrt(max(1.0 - sin2, 0.0));
    let rs = (eta_i * c - eta_t * ct) / max(eta_i * c + eta_t * ct, 0.000001);
    let rp = (eta_t * c - eta_i * ct) / max(eta_t * c + eta_i * ct, 0.000001);
    return 0.5 * (rs * rs + rp * rp);
}

fn hybrid_beer(color: vec3<f32>, distance: f32, reference_distance: f32) -> vec3<f32> {
    return exp(log(clamp(color, vec3<f32>(0.0001), vec3<f32>(1.0))) * distance / max(reference_distance, 0.0001));
}

// Shared reference entries remain type-correct, but cannot request geometry
// work in preview. A miss leaves the primary environment/probe lobe intact.
fn trace_geometry_reflection(origin: vec3<f32>, direction: vec3<f32>, roughness: f32, max_bounces: u32) -> vec4<f32> {
    hybrid_reflection_hit_distance = 0.0;
    return vec4<f32>(0.0);
}

fn trace_geometry_transmission(origin: vec3<f32>, incident: vec3<f32>, entry_normal: vec3<f32>, ior: f32,
    attenuation: vec3<f32>, attenuation_distance: f32, object_id: u32, initial_inside_distance: f32) -> HybridTransportResult {
    return HybridTransportResult(vec3<f32>(0.0), 0.0, 0.0, 0u);
}

fn primary_solid_interface_visible(camera: vec3<f32>, position: vec3<f32>, direction: vec3<f32>, object_id: u32) -> bool {
    return true;
}

fn hybrid_transmission_visibility(position: vec3<f32>, direction: vec3<f32>, maximum: f32) -> vec3<f32> {
    return vec3<f32>(1.0);
}
