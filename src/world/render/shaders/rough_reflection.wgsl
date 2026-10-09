// src/world/render/shaders/rough_reflection.wgsl
// Broad opaque base/coating radiance is sampled on a smaller raster grid.
// Full-resolution shading still owns material response, layers and occlusion.
override COARSE_REFLECTION_ENABLED: bool = false;
@group(3) @binding(0) var rough_reflection_radiance: texture_2d<f32>;
@group(3) @binding(1) var rough_reflection_normals: texture_2d<f32>;
@group(3) @binding(2) var rough_reflection_position: texture_2d<f32>;
@group(3) @binding(3) var rough_reflection_identity: texture_2d<u32>;
@group(3) @binding(4) var rough_reflection_coat_radiance: texture_2d<f32>;
@group(3) @binding(5) var primary_reflection_route: texture_2d<u32>;

struct RoughReflectionEvidence {
    @location(0) radiance: vec4<f32>,
    @location(1) normals: vec4<f32>,
    @location(2) position: vec4<f32>,
    @location(3) identity: u32,
    @location(4) coat_radiance: vec4<f32>,
};
struct RoughReflectionSample {
    radiance: vec4<f32>,
    distance: f32,
    valid: bool,
};

// The default shader replaces the marked sampling call with this stub before
// validation, so transparent and planar pipelines retain three bind groups.
fn rough_reflection_unavailable(input: VertexOut, normal: vec3<f32>, geometric_normal: vec3<f32>, roughness: f32, coat_lobe: bool) -> RoughReflectionSample {
    return RoughReflectionSample(vec4<f32>(0.0),-7.0,false);
}

// Ordinary transparent/capture modules replace the marked route read with
// this stub, keeping their established three-bind-group layouts.
fn rough_reflection_default_route(input: VertexOut) -> u32 {
    return 0u;
}

fn sample_primary_reflection_route(input: VertexOut) -> u32 {
    if (/* completed query route */rough_reflection_default_route(input) == 1u) { return 1u; }
    let size = vec2<i32>(textureDimensions(primary_reflection_route));
    let pixel = clamp(vec2<i32>(input.pos.xy),vec2<i32>(0),size-vec2<i32>(1));
    return textureLoad(primary_reflection_route,pixel,0).x;
}

// Fast surface modules replace only the marked primary reflection queries.
// Cached radiance still receives the unchanged full-resolution BRDF response.
fn rough_reflection_unavailable_query(origin: vec3<f32>, direction: vec3<f32>, roughness: f32, max_bounces: u32) -> vec4<f32> {
    hybrid_reflection_hit_distance = 0.0;
    return vec4<f32>(0.0);
}

fn rough_reflection_screen_uv(input: VertexOut) -> vec2<f32> {
    let ndc = input.current_clip.xy / max(input.current_clip.w, 0.000001);
    return vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
}

fn encode_rough_reflection_normal(value: vec3<f32>) -> vec2<f32> {
    var normal = value / max(abs(value.x)+abs(value.y)+abs(value.z),0.000001);
    if (normal.z < 0.0) {
        let octant = select(vec2<f32>(-1.0),vec2<f32>(1.0),normal.xy >= vec2<f32>(0.0));
        normal = vec3<f32>((vec2<f32>(1.0)-abs(normal.yx))*octant,normal.z);
    }
    return normal.xy * 0.5 + 0.5;
}

fn decode_rough_reflection_normal(encoded: vec2<f32>) -> vec3<f32> {
    let xy = encoded * 2.0 - 1.0;
    var normal = vec3<f32>(xy, 1.0 - abs(xy.x) - abs(xy.y));
    if (normal.z < 0.0) {
        let octant = select(vec2<f32>(-1.0),vec2<f32>(1.0),normal.xy >= vec2<f32>(0.0));
        normal = vec3<f32>((vec2<f32>(1.0) - abs(normal.yx)) * octant, normal.z);
    }
    return normalize(normal);
}

// Incident light can exceed half range while the surface's weak BRDF response
// still produces finite HDR color. Such evidence must use the full-pixel path;
// clamping it here would silently remove reflected energy.
fn rough_reflection_radiance_fits_half(value: vec4<f32>) -> bool {
    return all(abs(value) <= vec4<f32>(65504.0,65504.0,65504.0,1.0));
}

fn sample_rough_reflection_evidence(input: VertexOut, normal: vec3<f32>, geometric_normal: vec3<f32>, roughness: f32, coat_lobe: bool) -> RoughReflectionSample {
    let completed = /* sample completed query */rough_reflection_unavailable(input,normal,geometric_normal,roughness,coat_lobe);
    if (completed.valid) { return completed; }
    // Full-pixel query results own their draw and lobes independently. The
    // smaller grid still requires broad roughness and certified coating data.
    if (roughness <= 0.12 || roughness >= 0.75 || (coat_lobe && params.material11.z <= 0.5)) {
        return rough_reflection_unavailable(input,normal,geometric_normal,roughness,coat_lobe);
    }
    let dimensions = vec2<i32>(textureDimensions(rough_reflection_radiance));
    let sample_position = rough_reflection_screen_uv(input) * vec2<f32>(dimensions) - 0.5;
    let center = vec2<i32>(floor(sample_position + 0.5));
    let object = u32(params.material11.y);
    var total_weight = 0.0;
    var confident_weight = 0.0;
    var radiance = vec3<f32>(0.0);
    var selected_weight = 0.0;
    var selected_distance = 0.0;
    // Negative invalid distances identify the last compatibility gate for
    // native diagnostics; shading consumes distance only for valid samples.
    var rejection = -1.0;
    // Camera-relative world vectors retain half precision far from the origin.
    // This tolerance covers that error without accepting a different plane.
    let current_relative = input.world_position - params.camera0.xyz;
    let coordinate_scale = max(abs(current_relative.x), max(abs(current_relative.y), abs(current_relative.z)));
    let plane_tolerance = max(0.0005, coordinate_scale * 0.0015);
    for (var y = -1; y <= 1; y += 1) {
        for (var x = -1; x <= 1; x += 1) {
            let pixel = center + vec2<i32>(x,y);
            if (any(pixel < vec2<i32>(0)) || any(pixel >= dimensions)) { continue; }
            let identity = textureLoad(rough_reflection_identity,pixel,0).x;
            if ((identity & 65535u) != object) { continue; }
            rejection = -2.0;
            let position = textureLoad(rough_reflection_position,pixel,0);
            if (!all(abs(position.xyz) <= vec3<f32>(65504.0))) { continue; }
            rejection = -3.0;
            if (!coat_lobe && (position.w <= 0.12 || position.w >= 0.75 || abs(position.w - roughness) > 0.03)) { continue; }
            rejection = -4.0;
            let encoded_normals = textureLoad(rough_reflection_normals,pixel,0);
            let sample_normal = decode_rough_reflection_normal(encoded_normals.xy);
            let sample_geometric = decode_rough_reflection_normal(encoded_normals.zw);
            if ((!coat_lobe && dot(normal,sample_normal) < 0.985) || dot(geometric_normal,sample_geometric) < 0.995) { continue; }
            rejection = -5.0;
            let delta = position.xyz - current_relative;
            if (abs(dot(delta,geometric_normal)) > plane_tolerance ||
                abs(dot(delta,sample_geometric)) > plane_tolerance) { continue; }
            rejection = -6.0;
            let offset = vec2<f32>(pixel) - sample_position;
            let weight = 1.0 / (0.25 + dot(offset,offset));
            var sample = vec4<f32>(0.0);
            if (coat_lobe) { sample = textureLoad(rough_reflection_coat_radiance,pixel,0); }
            else { sample = textureLoad(rough_reflection_radiance,pixel,0); }
            if (!rough_reflection_radiance_fits_half(sample)) { continue; }
            let confidence_weight = weight * clamp(sample.a,0.0,1.0);
            radiance += sample.rgb * confidence_weight;
            confident_weight += confidence_weight;
            total_weight += weight;
            if (confidence_weight > selected_weight) {
                selected_weight = confidence_weight;
                selected_distance = unpack2x16float(identity & 0xffff0000u).y;
            }
        }
    }
    if (total_weight <= 0.000001) {
        return RoughReflectionSample(vec4<f32>(0.0),rejection,false);
    }
    // Zero confidence is valid cached evidence: retain the original probe
    // fallback rather than retracing the same miss for every output pixel.
    let color = radiance / max(confident_weight,0.000001);
    return RoughReflectionSample(vec4<f32>(color,confident_weight / total_weight),selected_distance,true);
}

// A separate strict-Greater raster records the first nearest authored winner,
// including coplanar triangles and instances. Only that winner can route its
// output pixel to the fast shader. Full pixels retain every original colour
// contribution, including fractional coverage and individually cached layers.
@fragment
fn fs_reflection_route(input: VertexOut) -> @location(0) u32 {
    params = instance_params[input.instance_id];
    // Capture primary raster derivatives before coverage or material returns.
    // Explicit gradients keep later mapped-normal reads valid in WebGPU.
    let gradient_x = dpdx(input.uv);
    let gradient_y = dpdy(input.uv);
    let view = select(normalize(params.camera0.xyz-input.world_position),
        -params.camera3.xyz,params.camera3.w > 0.5);
    let transmission = clamp(params.material6.x,0.0,1.0);
    if (transmission > 0.001 && params.material8.w > 0.5 && params.material11.x < 0.5 &&
        dot(normalize(input.normal),view) <= 0.0) { discard; }
    if (planar.control.y > 0.5 && dot(planar.plane,vec4<f32>(input.world_position,1.0)) < planar.control.z) { discard; }
    if (input.hidden_weight > 0.01) { discard; }
    let sampled = textureSampleGrad(actor_texture,actor_sampler,input.uv,gradient_x,gradient_y);
    let alpha = sampled.a * input.color.a * params.material4.a * params.style.x;
    if (alpha <= 0.001 || (params.material7.w > 0.0 && alpha < params.material7.w)) { discard; }
    if (alpha < 0.999999 || transmission > 0.001 || planar.control.y > 0.5) { return 0u; }
    let geometry_bounces = u32(max(lighting.reflection0.x-select(0.0,1.0,planar.control.y > 0.5),0.0));
    if (!GEOMETRY_REFLECTION_ENABLED || lighting.reflection0.w <= 0.5 || params.material2.w >= 0.5 ||
        lighting.surface0.x >= 0.5 || params.style.y <= 0.0 || geometry_bounces == 0u) { return 1u; }

    // Use primary raster derivatives and the exact same material channels and
    // normal construction as shade_surface. Classification does not shade or
    // trace any lighting, so its entry cannot reach the reflection BVH graph.
    let mr_sample = textureSampleGrad(metallic_roughness_texture,actor_sampler,input.uv,gradient_x,gradient_y);
    let channels = material_channels(mr_sample);
    var roughness = clamp(params.material0.y*channels.y+lighting.surface1.z,0.045,1.0);
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x > 2.5 && lighting.surface0.x < 3.5) { roughness = clay_roughness(); }
    }
    var geometric_normal = normalize(input.normal);
    if ((params.material8.y > 0.5 || params.material11.x > 0.5) && dot(geometric_normal,view) < 0.0) {
        geometric_normal = -geometric_normal;
    }
    let tangent = normalize(input.tangent-geometric_normal*dot(input.tangent,geometric_normal));
    let bitangent_sign = select(-1.0,1.0,dot(cross(geometric_normal,tangent),input.bitangent) >= 0.0);
    let bitangent = normalize(cross(geometric_normal,tangent))*bitangent_sign;
    let sampled_normal = textureSampleGrad(normal_texture,actor_sampler,input.uv,gradient_x,gradient_y).xyz*2.0-1.0;
    let tangent_normal = normalize(vec3<f32>(sampled_normal.xy*params.material0.z,sampled_normal.z));
    let normal = normalize(tangent*tangent_normal.x+bitangent*tangent_normal.y+geometric_normal*tangent_normal.z);

    if (planar.control.x > 0.5 && dot(geometric_normal,planar.plane.xyz) > 0.85 &&
        planar_surface_reuse_allowed(input,normal,geometric_normal,roughness)) {
        let rel = input.world_position-planar.camera0.xyz;
        let z = dot(rel,planar.camera3.xyz);
        let projection_w = select(z,1.0,planar.camera3.w > 0.5);
        let dimensions = vec2<f32>(textureDimensions(planar_texture));
        let projection = params.canvas.zw/params.canvas.xy +
            vec2<f32>(dot(rel,planar.camera1.xyz),-dot(rel,planar.camera2.xyz))*
            planar.camera0.w/(max(projection_w,0.0001)*dimensions);
        if (z > planar.camera1.w && all(projection >= vec2<f32>(0.0)) && all(projection <= vec2<f32>(1.0))) { return 1u; }
    }
    if (roughness < 0.75) {
        if (roughness <= 0.12 || !sample_rough_reflection_evidence(input,normal,geometric_normal,roughness,false).valid) {
            return 0u;
        }
    } else if (!(roughness >= 0.75)) { return 0u; }
    if (params.material10.x > 0.0 && params.material10.y < 0.75) {
        if (params.material11.z <= 0.5 || params.material10.y <= 0.12 ||
            !sample_rough_reflection_evidence(input,normal,geometric_normal,params.material10.y,true).valid) {
            return 0u;
        }
    } else if (params.material10.x > 0.0 && !(params.material10.y >= 0.75)) { return 0u; }
    return 1u;
}

@fragment
fn fs_rough_reflection_evidence(input: VertexOut) -> RoughReflectionEvidence {
    params = instance_params[input.instance_id];
    let screen_uv = rough_reflection_screen_uv(input);
    let full_dimensions = vec2<f32>(textureDimensions(opaque_scene_depth));
    // The lower-grid derivatives span several full-resolution pixels. Scale
    // each raster axis to retain the primary material's mip selection.
    let scale_x = 1.0 / max(abs(dpdx(screen_uv.x)) * full_dimensions.x,0.000001);
    let scale_y = 1.0 / max(abs(dpdy(screen_uv.y)) * full_dimensions.y,0.000001);
    let gradient_x = dpdx(input.uv) * scale_x;
    let gradient_y = dpdy(input.uv) * scale_y;
    let depth_margin = 0.000002 + 0.5 * (abs(dpdx(input.pos.z)) * scale_x + abs(dpdy(input.pos.z)) * scale_y);
    if (params.material6.x > 0.001 && params.material8.w > 0.5 && params.material11.x < 0.5) {
        let closed_view = select(normalize(params.camera0.xyz - input.world_position),
            -params.camera3.xyz, params.camera3.w > 0.5);
        if (dot(normalize(input.normal),closed_view) <= 0.0) { discard; }
    }
    if (planar.control.y > 0.5 && dot(planar.plane, vec4<f32>(input.world_position,1.0)) < planar.control.z) { discard; }
    if (input.hidden_weight > 0.01) { discard; }
    let sampled = textureSampleGrad(actor_texture,actor_sampler,input.uv,gradient_x,gradient_y);
    let alpha = sampled.a * input.color.a * params.material4.a * params.style.x;
    if (alpha <= 0.001 || (params.material7.w > 0.0 && alpha < params.material7.w)) { discard; }
    let depth_size = vec2<i32>(full_dimensions);
    let depth_pixel = clamp(vec2<i32>(screen_uv * full_dimensions),vec2<i32>(0),depth_size-vec2<i32>(1));
    let nearest_depth = textureLoad(opaque_scene_depth,depth_pixel,0);
    let nearest_coverage = textureLoad(opaque_scene_texture,depth_pixel,0).a;
    if (nearest_coverage >= 0.999999 && input.pos.z + depth_margin < nearest_depth) { discard; }
    let mr_sample = textureSampleGrad(metallic_roughness_texture,actor_sampler,input.uv,gradient_x,gradient_y);
    let channels = material_channels(mr_sample);
    var roughness = clamp(params.material0.y * channels.y + lighting.surface1.z,0.045,1.0);
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x > 2.5 && lighting.surface0.x < 3.5) { roughness = clay_roughness(); }
    }
    let view = select(normalize(params.camera0.xyz - input.world_position),-params.camera3.xyz,params.camera3.w > 0.5);
    var geometric_normal = normalize(input.normal);
    if ((params.material8.y > 0.5 || params.material11.x > 0.5) && dot(geometric_normal,view) < 0.0) {
        geometric_normal = -geometric_normal;
    }
    let tangent = normalize(input.tangent - geometric_normal * dot(input.tangent,geometric_normal));
    let bitangent_sign = select(-1.0,1.0,dot(cross(geometric_normal,tangent),input.bitangent) >= 0.0);
    let bitangent = normalize(cross(geometric_normal,tangent)) * bitangent_sign;
    let sampled_normal = textureSampleGrad(normal_texture,actor_sampler,input.uv,gradient_x,gradient_y).xyz * 2.0 - 1.0;
    let tangent_normal = normalize(vec3<f32>(sampled_normal.xy * params.material0.z,sampled_normal.z));
    let normal = normalize(tangent*tangent_normal.x + bitangent*tangent_normal.y + geometric_normal*tangent_normal.z);
    let relative_position = input.world_position - params.camera0.xyz;
    var result = RoughReflectionEvidence(vec4<f32>(0.0),
        vec4<f32>(encode_rough_reflection_normal(normal),encode_rough_reflection_normal(geometric_normal)),
        vec4<f32>(relative_position,roughness),0xffffffffu,vec4<f32>(0.0));
    let geometry_bounces = u32(max(lighting.reflection0.x - select(0.0,1.0,planar.control.y > 0.5),0.0));
    let base_eligible = roughness > 0.12 && roughness < 0.75;
    // Object IDs can span several material chunks. The CPU proves every chunk
    // has coating with the same roughness before their evidence may be shared.
    let coat_eligible = params.material11.z > 0.5 && params.material10.x > 0.0 &&
        params.material10.y > 0.12 && params.material10.y < 0.75;
    if (!GEOMETRY_REFLECTION_ENABLED || lighting.reflection0.w <= 0.5 || params.material2.w >= 0.5 ||
        lighting.surface0.x >= 0.5 || params.style.y <= 0.0 || geometry_bounces == 0u ||
        planar.control.y > 0.5 || params.material6.x > 0.001 || alpha < 0.999999 ||
        !all(abs(relative_position) <= vec3<f32>(65504.0)) ||
        (!base_eligible && !coat_eligible)) { return result; }
    // Valid planar projection owns both lobes and must retain its own capture.
    if (planar.control.x > 0.5 && dot(geometric_normal,planar.plane.xyz) > 0.85 &&
        planar_surface_reuse_allowed(input,normal,geometric_normal,roughness)) {
        let rel = input.world_position - planar.camera0.xyz;
        let z = dot(rel,planar.camera3.xyz);
        let projection_w = select(z,1.0,planar.camera3.w > 0.5);
        let dimensions = vec2<f32>(textureDimensions(planar_texture));
        let projection = params.canvas.zw / params.canvas.xy +
            vec2<f32>(dot(rel,planar.camera1.xyz),-dot(rel,planar.camera2.xyz)) *
            planar.camera0.w / (max(projection_w,0.0001)*dimensions);
        if (z > planar.camera1.w && all(projection >= vec2<f32>(0.0)) && all(projection <= vec2<f32>(1.0))) {
            return result;
        }
    }
    var nearest_distance = 0.0;
    if (base_eligible) {
        let reflected = reflect(-view,normal);
        result.radiance = trace_geometry_reflection(input.world_position + reflected * hybrid_epsilon(input.world_position) * 3.0,
            reflected,roughness,geometry_bounces);
        nearest_distance = hybrid_reflection_hit_distance;
    }
    if (coat_eligible) {
        let reflected = reflect(-view,geometric_normal);
        result.coat_radiance = trace_geometry_reflection(input.world_position + reflected * hybrid_epsilon(input.world_position) * 3.0,
            reflected,params.material10.y,geometry_bounces);
        if (hybrid_reflection_hit_distance > 0.0) {
            nearest_distance = select(hybrid_reflection_hit_distance,min(nearest_distance,hybrid_reflection_hit_distance),nearest_distance > 0.0);
        }
    }
    if (!rough_reflection_radiance_fits_half(result.radiance) ||
        !rough_reflection_radiance_fits_half(result.coat_radiance)) {
        // Retain the impossible identity and avoid writing Inf/NaN to either
        // half target. A lookup with no finite matching evidence retraces.
        result.radiance = vec4<f32>(0.0);
        result.coat_radiance = vec4<f32>(0.0);
        return result;
    }
    result.identity = (u32(params.material11.y) & 65535u) |
        (pack2x16float(vec2<f32>(0.0,clamp(nearest_distance,0.0,65504.0))) & 0xffff0000u);
    return result;
}
