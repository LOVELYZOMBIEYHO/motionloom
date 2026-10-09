// src/world/render/shaders/reflection_query_compute.wgsl
// Compact nearest, fully covered primary reflection requests before tracing.
// Material/route/visibility ownership remains in the producing raster pass.
override REFLECTION_QUERY_ENABLED: bool = false;

struct ReflectionQueryWorklist {
    count: atomic<u32>,
    indices: array<u32>,
};

struct ReflectionQueryOutput {
    base: vec4<f32>,
    coat: vec4<f32>,
    // Independent base/coat hit distances, bitcast draw owner, completed marker.
    // A completed miss has marker one and zero confidence in its radiance.
    distances: vec4<f32>,
};

// These bindings follow the ordinary evidence textures in assembled modules.
// Position.xyz/base roughness retain float32. Integer words contain signed
// oct16 mapped normal, signed oct16 geometric normal, bitcast coat roughness,
// and draw owner/lobe flags (low two bits base/coat, higher bits draw owner).
@group(3) @binding(6) var reflection_query_positions: texture_2d<f32>;
@group(3) @binding(7) var reflection_query_normals: texture_2d<u32>;
@group(3) @binding(8) var<storage, read_write> reflection_query_worklist: ReflectionQueryWorklist;
@group(3) @binding(9) var<storage, read_write> reflection_query_outputs: array<ReflectionQueryOutput>;
// Compute uses its own group-two layout; fragment entries never reference this
// uniform, and compute entries never read the scene texture at this binding.
@group(2) @binding(0) var<uniform> reflection_query_dispatch: vec4<u32>;

struct ReflectionQueryInputs {
    @location(0) position_roughness: vec4<f32>,
    @location(1) normals_owner: vec4<u32>,
};

struct ReflectionQueryMaterialSample {
    normal: vec3<f32>,
    geometric_normal: vec3<f32>,
    roughness: f32,
};

fn reflection_query_finite(value: vec4<f32>) -> bool {
    return all((bitcast<vec4<u32>>(value) & vec4<u32>(0x7f800000u)) != vec4<u32>(0x7f800000u));
}

fn encode_reflection_query_normal(value: vec3<f32>) -> u32 {
    var normal = value / max(abs(value.x) + abs(value.y) + abs(value.z), 0.000001);
    if (normal.z < 0.0) {
        let octant = select(vec2<f32>(-1.0), vec2<f32>(1.0), normal.xy >= vec2<f32>(0.0));
        normal = vec3<f32>((vec2<f32>(1.0) - abs(normal.yx)) * octant, normal.z);
    }
    return pack2x16snorm(normal.xy);
}

fn decode_reflection_query_normal(packed: u32) -> vec3<f32> {
    let xy = unpack2x16snorm(packed);
    var normal = vec3<f32>(xy, 1.0 - abs(xy.x) - abs(xy.y));
    if (normal.z < 0.0) {
        let octant = select(vec2<f32>(-1.0), vec2<f32>(1.0), normal.xy >= vec2<f32>(0.0));
        normal = vec3<f32>((vec2<f32>(1.0) - abs(normal.yx)) * octant, normal.z);
    }
    return normalize(normal);
}

fn reflection_query_invocation_index(workgroup: vec3<u32>, groups: vec3<u32>, local: u32) -> u32 {
    // Linearize workgroups rather than global x so 2D/3D dispatches retain one
    // invocation per pixel without exceeding the device's x workgroup limit.
    return ((workgroup.z * groups.y + workgroup.y) * groups.x + workgroup.x) * 64u + local;
}

@compute @workgroup_size(64)
fn cs_compact_reflection_queries(
    @builtin(workgroup_id) workgroup: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    let pixel = reflection_query_invocation_index(workgroup, groups, local);
    let size = textureDimensions(reflection_query_positions);
    if (pixel >= min(size.x * size.y, arrayLength(&reflection_query_outputs))) {
        return;
    }
    // Clear completion for every visited pixel before testing eligibility.
    // Old radiance is harmless until this frame writes the completion marker.
    reflection_query_outputs[pixel].distances = vec4<f32>(0.0);
    if (any(size != textureDimensions(reflection_query_normals))) { return; }
    let coordinate = vec2<i32>(i32(pixel % size.x), i32(pixel / size.x));
    if ((textureLoad(reflection_query_normals, coordinate, 0).w & 3u) == 0u) { return; }
    let index = atomicAdd(&reflection_query_worklist.count, 1u);
    // The host allocates one index per output pixel and resets count before
    // dispatch. A smaller binding cannot cause an out-of-range storage write;
    // requests that do not fit remain invalid and require raster fallback.
    if (index < arrayLength(&reflection_query_worklist.indices)) {
        reflection_query_worklist.indices[index] = pixel;
    }
}

@compute @workgroup_size(64)
fn cs_trace_reflection_queries(
    @builtin(workgroup_id) workgroup: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
    @builtin(local_invocation_index) local: u32,
) {
    let index = reflection_query_dispatch.x + reflection_query_invocation_index(workgroup, groups, local);
    let count = min(atomicLoad(&reflection_query_worklist.count), arrayLength(&reflection_query_worklist.indices));
    if (index >= count) { return; }
    let pixel = reflection_query_worklist.indices[index];
    let size = textureDimensions(reflection_query_positions);
    if (pixel >= min(size.x * size.y, arrayLength(&reflection_query_outputs)) ||
        any(size != textureDimensions(reflection_query_normals))) {
        return;
    }
    let coordinate = vec2<i32>(i32(pixel % size.x), i32(pixel / size.x));
    let position_roughness = textureLoad(reflection_query_positions, coordinate, 0);
    let packed = textureLoad(reflection_query_normals, coordinate, 0);
    let coat_roughness = bitcast<f32>(packed.z);
    let base_active = (packed.w & 1u) != 0u;
    let coat_active = (packed.w & 2u) != 0u;
    if (!base_active && !coat_active) { return; }
    // A corrupt/stale producer payload remains unresolved instead of silently
    // turning an invalid query into completed black reflection evidence.
    if (!reflection_query_finite(position_roughness) ||
        !reflection_query_finite(vec4<f32>(coat_roughness)) ||
        !reflection_query_finite(lighting.camera0) ||
        !reflection_query_finite(lighting.camera3) ||
        !(lighting.reflection0.x == 1.0 || lighting.reflection0.x == 2.0)) { return; }
    if (base_active && (position_roughness.w < 0.045 || position_roughness.w > 1.0)) { return; }
    if (coat_active && (coat_roughness < 0.045 || coat_roughness > 1.0)) { return; }
    let camera_delta = lighting.camera0.xyz - position_roughness.xyz;
    if (!reflection_query_finite(vec4<f32>(camera_delta, 0.0))) { return; }
    let orthographic = lighting.camera3.w < 0.0;
    if ((!orthographic && dot(camera_delta, camera_delta) <= 0.000000000001) ||
        (orthographic && dot(lighting.camera3.xyz, lighting.camera3.xyz) <= 0.000000000001)) { return; }
    let view = select(normalize(camera_delta), -lighting.camera3.xyz, orthographic);
    if (!reflection_query_finite(vec4<f32>(view, 0.0)) || dot(view, view) <= 0.000000000001) { return; }

    var output = ReflectionQueryOutput(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    let bounces = u32(lighting.reflection0.x);
    // Packed normals add explicit oct16 quantization. World position and the
    // shared main-camera view retain float32; use the same reflect expression
    // and epsilon offset as shade_surface after normal decoding.
    // Private hybrid work state starts once per invocation, then remains shared
    // across both lobes, every cone sample, secondary shadow and bounce.
    if (base_active) {
        let direction = reflect(-view, decode_reflection_query_normal(packed.x));
        output.base = trace_geometry_reflection(
            position_roughness.xyz + direction * hybrid_epsilon(position_roughness.xyz) * 3.0,
            direction, position_roughness.w, bounces);
        output.distances.x = hybrid_reflection_hit_distance;
    }
    if (coat_active) {
        let direction = reflect(-view, decode_reflection_query_normal(packed.y));
        output.coat = trace_geometry_reflection(
            position_roughness.xyz + direction * hybrid_epsilon(position_roughness.xyz) * 3.0,
            direction, coat_roughness, bounces);
        output.distances.y = hybrid_reflection_hit_distance;
    }
    // Storage stays float32: weak BRDF response may produce a finite surface
    // even when its incoming incident radiance exceeds the half-float range.
    output.distances.z = bitcast<f32>(packed.w >> 2u);
    output.distances.w = 1.0;
    reflection_query_outputs[pixel] = output;
}

fn reflection_query_material_sample(input: VertexOut) -> ReflectionQueryMaterialSample {
    let mr = textureSampleGrad(metallic_roughness_texture, actor_sampler, input.uv,
        surface_gradient_x, surface_gradient_y);
    let channels = material_channels(mr);
    var roughness = clamp(params.material0.y * channels.y + lighting.surface1.z, 0.045, 1.0);
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x > 2.5 && lighting.surface0.x < 3.5) { roughness = clay_roughness(); }
    }
    let view = select(normalize(params.camera0.xyz - input.world_position),
        -params.camera3.xyz, params.camera3.w > 0.5);
    var geometric_normal = normalize(input.normal);
    if ((params.material8.y > 0.5 || params.material11.x > 0.5) && dot(geometric_normal, view) < 0.0) {
        geometric_normal = -geometric_normal;
    }
    let tangent = normalize(input.tangent - geometric_normal * dot(input.tangent, geometric_normal));
    let bitangent_sign = select(-1.0, 1.0, dot(cross(geometric_normal, tangent), input.bitangent) >= 0.0);
    let bitangent = normalize(cross(geometric_normal, tangent)) * bitangent_sign;
    let sampled_normal = textureSampleGrad(normal_texture, actor_sampler, input.uv,
        surface_gradient_x, surface_gradient_y).xyz * 2.0 - 1.0;
    let tangent_normal = normalize(vec3<f32>(sampled_normal.xy * params.material0.z, sampled_normal.z));
    let normal = normalize(tangent * tangent_normal.x + bitangent * tangent_normal.y +
        geometric_normal * tangent_normal.z);
    return ReflectionQueryMaterialSample(normal, geometric_normal, roughness);
}

fn reflection_query_primary_allowed() -> bool {
    return REFLECTION_QUERY_ENABLED && GEOMETRY_REFLECTION_ENABLED &&
        lighting.reflection0.w > 0.5 && params.material2.w < 0.5 &&
        lighting.surface0.x < 0.5 && params.style.y > 0.0 &&
        params.material6.x <= 0.001 && planar.control.y < 0.5 &&
        (lighting.reflection0.x == 1.0 || lighting.reflection0.x == 2.0) &&
        reflection_query_finite(vec4<f32>(params.material11.w)) &&
        params.material11.w >= 0.0 && params.material11.w < 16777216.0 &&
        all(params.camera0.xyz == lighting.camera0.xyz) &&
        all(params.camera3.xyz == lighting.camera3.xyz) &&
        ((params.camera3.w > 0.5) == (lighting.camera3.w < 0.0));
}

@fragment
fn fs_reflection_query_inputs(input: VertexOut) -> ReflectionQueryInputs {
    params = instance_params[input.instance_id];
    initialize_surface_gradients(input);
    let view = select(normalize(params.camera0.xyz - input.world_position),
        -params.camera3.xyz, params.camera3.w > 0.5);
    let transmission = clamp(params.material6.x, 0.0, 1.0);
    if (transmission > 0.001 && params.material8.w > 0.5 && params.material11.x < 0.5 &&
        dot(normalize(input.normal), view) <= 0.0) { discard; }
    if (planar.control.y > 0.5 && dot(planar.plane, vec4<f32>(input.world_position, 1.0)) < planar.control.z) { discard; }
    if (input.hidden_weight > 0.01) { discard; }
    let sampled = textureSampleGrad(actor_texture, actor_sampler, input.uv,
        surface_gradient_x, surface_gradient_y);
    let alpha = sampled.a * input.color.a * params.material4.a * params.style.x;
    if (alpha <= 0.001 || (params.material7.w > 0.0 && alpha < params.material7.w)) { discard; }
    let inactive = ReflectionQueryInputs(vec4<f32>(0.0), vec4<u32>(0u));
    // An ineligible nearest winner still writes depth and inactive evidence.
    // Discard here would expose another object's payload through that winner.
    if (alpha < 0.999999 || transmission > 0.001 || !reflection_query_primary_allowed() ||
        sample_primary_reflection_route(input) == 1u) { return inactive; }
    let material = reflection_query_material_sample(input);
    let base_active = material.roughness < 0.75;
    let coat_active = params.material10.x > 0.0 && params.material10.y < 0.75;
    if ((!base_active && !coat_active) ||
        !reflection_query_finite(vec4<f32>(input.world_position, material.roughness)) ||
        !reflection_query_finite(vec4<f32>(material.normal, 0.0)) ||
        !reflection_query_finite(vec4<f32>(material.geometric_normal, params.material10.y))) { return inactive; }
    let coat_roughness = select(-1.0, params.material10.y, coat_active);
    let flags = select(0u, 1u, base_active) | select(0u, 2u, coat_active) |
        (u32(params.material11.w) << 2u);
    return ReflectionQueryInputs(vec4<f32>(input.world_position, material.roughness),
        vec4<u32>(encode_reflection_query_normal(material.normal),
            encode_reflection_query_normal(material.geometric_normal), bitcast<u32>(coat_roughness), flags));
}

fn sample_completed_reflection_query(input: VertexOut, normal: vec3<f32>,
    geometric_normal: vec3<f32>, roughness: f32, coat_lobe: bool) -> RoughReflectionSample {
    let invalid = RoughReflectionSample(vec4<f32>(0.0), 0.0, false);
    if (!reflection_query_primary_allowed()) { return invalid; }
    let size = textureDimensions(reflection_query_positions);
    if (any(size != textureDimensions(reflection_query_normals)) ||
        any(size != textureDimensions(opaque_scene_depth))) { return invalid; }
    let coordinate = vec2<i32>(input.pos.xy);
    if (any(coordinate < vec2<i32>(0)) || any(coordinate >= vec2<i32>(size))) { return invalid; }
    let pixel = u32(coordinate.y) * size.x + u32(coordinate.x);
    if (pixel >= arrayLength(&reflection_query_outputs)) { return invalid; }
    let output = reflection_query_outputs[pixel];
    let owner = u32(params.material11.w);
    if (output.distances.w != 1.0 || bitcast<u32>(output.distances.z) != owner) { return invalid; }
    let position = textureLoad(reflection_query_positions, coordinate, 0);
    let packed = textureLoad(reflection_query_normals, coordinate, 0);
    let active_bit = select(1u, 2u, coat_lobe);
    if ((packed.w & active_bit) == 0u || (packed.w >> 2u) != owner ||
        !reflection_query_finite(position) ||
        !reflection_query_finite(vec4<f32>(input.world_position, roughness)) ||
        !reflection_query_finite(vec4<f32>(normal, 0.0)) ||
        !reflection_query_finite(vec4<f32>(geometric_normal, 0.0))) { return invalid; }
    // A few float32 ulps cover repeated raster interpolation without allowing
    // a neighbor, another plane, or a differently layered draw to borrow light.
    let tolerance = max(vec3<f32>(0.000001),
        max(abs(position.xyz), abs(input.world_position)) * 0.0000005);
    if (any(abs(position.xyz - input.world_position) > tolerance)) { return invalid; }
    let saved_roughness = select(position.w, bitcast<f32>(packed.z), coat_lobe);
    if (!reflection_query_finite(vec4<f32>(saved_roughness)) ||
        abs(saved_roughness - roughness) > 0.000001 ||
        dot(normal, decode_reflection_query_normal(packed.x)) < 0.999999 ||
        dot(geometric_normal, decode_reflection_query_normal(packed.y)) < 0.999999) { return invalid; }
    // Completed misses deliberately remain valid: their zero confidence keeps
    // the existing probe fallback without retracing identical geometry work.
    if (coat_lobe) { return RoughReflectionSample(output.coat, output.distances.y, true); }
    return RoughReflectionSample(output.base, output.distances.x, true);
}

fn completed_reflection_query_route(input: VertexOut) -> u32 {
    if (!REFLECTION_QUERY_ENABLED) { return 0u; }
    params = instance_params[input.instance_id];
    if (!reflection_query_primary_allowed()) { return 0u; }
    let sampled = textureSampleGrad(actor_texture, actor_sampler, input.uv,
        surface_gradient_x, surface_gradient_y);
    let alpha = sampled.a * input.color.a * params.material4.a * params.style.x;
    if (alpha < 0.999999 || (params.material7.w > 0.0 && alpha < params.material7.w)) { return 0u; }
    let material = reflection_query_material_sample(input);
    let base_active = material.roughness < 0.75;
    let coat_active = params.material10.x > 0.0 && params.material10.y < 0.75;
    if (!base_active && !coat_active) { return 0u; }
    if (base_active && !sample_completed_reflection_query(input, material.normal,
        material.geometric_normal, material.roughness, false).valid) { return 0u; }
    if (coat_active && !sample_completed_reflection_query(input, material.normal,
        material.geometric_normal, params.material10.y, true).valid) { return 0u; }
    return 1u;
}
