// Complex raster islands may reject hidden pixels using current-frame depth.
override RASTER_VISIBILITY_ENABLED: bool = false;
// =========================================
// =========================================
// src/world/render/shaders/surface.wgsl

fn authored_light_radiance(light: Light, world_position: vec3<f32>) -> vec4<f32> {
    let kind = light.position_kind.w;
    var direction = normalize(-light.direction_range.xyz);
    var attenuation = 1.0;
    if (kind > 0.5) {
        let delta = light.position_kind.xyz - world_position;
        let distance = max(length(delta), 0.001);
        direction = delta / distance;
        let normalized_distance = distance / max(light.direction_range.w, 0.001);
        attenuation = pow(clamp(1.0 - pow(normalized_distance, 4.0), 0.0, 1.0), 2.0) /
            max(distance * distance, 0.25);
        if (kind > 1.5 && kind < 2.5) {
            let cone = dot(normalize(-direction), normalize(light.direction_range.xyz));
            attenuation *= smoothstep(light.spot_area.y, light.spot_area.x, cone);
        }
    }
    return vec4<f32>(direction, attenuation);
}

// A regular four-point emitter is retained as the reference; distant pixels
// use its centre and nearby pixels use a diagonal pair to limit BRDF work.
fn authored_area_light_position(light: Light, sample_index: u32) -> vec3<f32> {
    let emitter_normal = normalize(light.direction_range.xyz);
    let reference = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(emitter_normal.y) > 0.95);
    let right = normalize(cross(reference, emitter_normal));
    let up = normalize(cross(emitter_normal, right));
    let center = sample_index == 4u;
    let x = select(select(-0.288675, 0.288675, (sample_index & 1u) != 0u), 0.0, center);
    let y = select(select(-0.288675, 0.288675, (sample_index & 2u) != 0u), 0.0, center);
    let width = max(light.spot_area.z, 0.001);
    let height = max(light.spot_area.w, 0.001);
    return light.position_kind.xyz + right * x * width + up * y * height;
}
fn authored_area_light_radiance(light: Light, world_position: vec3<f32>, sample_index: u32) -> vec4<f32> {
    let emitter_normal = normalize(light.direction_range.xyz);
    let width = max(light.spot_area.z, 0.001);
    let height = max(light.spot_area.w, 0.001);
    let sample_position = authored_area_light_position(light, sample_index);
    let delta = sample_position - world_position;
    let distance = max(length(delta), 0.001);
    let direction = delta / distance;
    let facing = max(dot(emitter_normal, -direction), 0.0);
    var attenuation = width * height * facing / (distance * distance);
    if (light.direction_range.w > 0.001) {
        let normalized_distance = distance / light.direction_range.w;
        attenuation *= pow(clamp(1.0 - pow(normalized_distance, 4.0), 0.0, 1.0), 2.0);
    }
    return vec4<f32>(direction, attenuation * 0.25);
}

fn surface_caustic_pattern(world: vec3<f32>) -> f32 {
    let p = world.xz / max(lighting.caustics0.y, 0.0001);
    let time = params.vegetation.w * lighting.caustics0.z;
    let a = sin(p.x * 1.31 + p.y * 0.77 + time);
    let b = sin(p.x * -0.63 + p.y * 1.67 - time * 1.23);
    let c = sin(p.x * 1.91 - p.y * 0.41 + time * 0.71);
    return pow(clamp((a + b + c) * 0.1667 + 0.5, 0.0, 1.0), 5.0);
}

fn material_channel(sample: vec4<f32>, encoded: u32) -> f32 {
    let channel = encoded & 7u;
    var value = sample.r;
    if (channel == 1u) {
        value = sample.g;
    } else if (channel == 2u) {
        value = sample.b;
    } else if (channel == 3u) {
        value = sample.a;
    } else if (channel == 4u) {
        value = dot(sample.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    }
    return select(value, 1.0 - value, (encoded & 8u) != 0u);
}

fn material_channels(sample: vec4<f32>) -> vec3<f32> {
    let packed = u32(params.material8.z + 0.5);
    return vec3<f32>(
        material_channel(sample, packed & 15u),
        material_channel(sample, (packed >> 4u) & 15u),
        material_channel(sample, (packed >> 8u) & 15u),
    );
}

// Realtime assembly disables geometry transport. Retained reference modules
// can also remove it when CPU texture bounds prove the rough-lobe fallback.
override GEOMETRY_REFLECTION_ENABLED: bool = true;
override COARSE_ROUTE_FAST_ONLY: bool = false;
override COARSE_ROUTE_FULL_ONLY: bool = false;
var<private> surface_reflection: vec4<f32>;
var<private> surface_geometry_reflection: f32;
var<private> surface_reflection_distance: f32;
var<private> surface_planar_reflection: bool;
var<private> surface_history_normal: vec3<f32>;
var<private> surface_history_material: vec3<f32>;
// Glass resolves its own interface against the current underlay snapshot.
// Keep RGB response separate from the opaque post-pass scalar payload.
var<private> surface_screen_reflection_lobe: vec3<f32>;
var<private> surface_screen_reflection_response: vec3<f32>;
var<private> surface_gradient_x: vec2<f32>;
var<private> surface_gradient_y: vec2<f32>;

fn initialize_surface_gradients(input: VertexOut) {
    // Derivatives belong to the original raster quad, before visibility/route
    // branches. Explicit gradients retain mip selection at rejected neighbors.
    surface_gradient_x = dpdx(input.uv);
    surface_gradient_y = dpdy(input.uv);
}

// Positive control.w belongs to an internal automatic face capture. Authored
// planar reflections retain their established projection and material response.
fn planar_surface_reuse_allowed(input: VertexOut, normal: vec3<f32>, geometric_normal: vec3<f32>, roughness: f32) -> bool {
    if (planar.control.w <= 0.0) { return true; }
    return roughness <= 0.12 && params.material10.x <= 0.0 &&
        params.material11.x <= 0.5 &&
        dot(normal,geometric_normal) >= 0.9999985 &&
        dot(geometric_normal,planar.plane.xyz) >= 0.99995 &&
        abs(dot(planar.plane,vec4<f32>(input.world_position,1.0))) <= planar.control.w;
}

fn surface_geometric_normal(input: VertexOut, view: vec3<f32>) -> vec3<f32> {
    var normal = normalize(input.normal);
    if ((params.material8.y > 0.5 || params.material11.x > 0.5) && dot(normal,view) < 0.0) {
        normal = -normal;
    }
    return normal;
}

fn surface_mapped_normal(input: VertexOut, geometric_normal: vec3<f32>) -> vec3<f32> {
    let tangent = normalize(input.tangent - geometric_normal * dot(input.tangent,geometric_normal));
    let sign = select(-1.0,1.0,dot(cross(geometric_normal,tangent),input.bitangent) >= 0.0);
    let bitangent = normalize(cross(geometric_normal,tangent)) * sign;
    let sampled = textureSampleGrad(normal_texture,actor_sampler,input.uv,surface_gradient_x,surface_gradient_y).xyz * 2.0 - 1.0;
    let mapped = normalize(vec3<f32>(sampled.xy * params.material0.z,sampled.z));
    return normalize(tangent * mapped.x + bitangent * mapped.y + geometric_normal * mapped.z);
}

// The routed slab entries and surface shading use one projection proof, so a
// cached pixel cannot lose its reflection through a different route predicate.
// xy is the projected UV; z records that the full surface would use the cache.
fn surface_planar_projection(input: VertexOut, normal: vec3<f32>, geometric_normal: vec3<f32>, roughness: f32) -> vec3<f32> {
    if (planar.control.x > 0.5 && dot(geometric_normal,planar.plane.xyz) > 0.85 &&
        planar_surface_reuse_allowed(input,normal,geometric_normal,roughness)) {
        let rel = input.world_position - planar.camera0.xyz;
        let z = dot(rel,planar.camera3.xyz);
        let projection_w = select(z,1.0,planar.camera3.w > 0.5);
        let dimensions = vec2<f32>(textureDimensions(planar_texture));
        let uv = vec2<f32>(params.canvas.z / params.canvas.x,params.canvas.w / params.canvas.y) +
            vec2<f32>(dot(rel,planar.camera1.xyz),-dot(rel,planar.camera2.xyz)) *
            planar.camera0.w / (max(projection_w,0.0001) * dimensions);
        let valid = z > planar.camera1.w && all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
        return vec3<f32>(uv,select(0.0,1.0,valid));
    }
    return vec3<f32>(0.0);
}

fn transmissive_planar_cached(input: VertexOut) -> bool {
    if (planar.control.w <= 0.0) { return false; }
    let view = select(normalize(params.camera0.xyz-input.world_position),-params.camera3.xyz,params.camera3.w > 0.5);
    let geometric_normal = surface_geometric_normal(input,view);
    let normal = surface_mapped_normal(input,geometric_normal);
    let material_samples = material_channels(textureSampleGrad(metallic_roughness_texture,
        actor_sampler,input.uv,surface_gradient_x,surface_gradient_y));
    var roughness = clamp(params.material0.y * material_samples.y + lighting.surface1.z,0.045,1.0);
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x > 2.5 && lighting.surface0.x < 3.5) { roughness = clay_roughness(); }
    }
    return surface_planar_projection(input,normal,geometric_normal,roughness).z > 0.5;
}

// Opaque occlusion remains in shadow maps. This factor follows transparent
// interfaces and their Beer absorption on the finite emitter segment.
fn surface_glass_visibility(light_index: u32, world: vec3<f32>, direction: vec3<f32>, distance: f32) -> vec3<f32> {
    if (!PRIMARY_GLASS_SHADOWS_ENABLED) { return vec3<f32>(1.0); }
    if (params.material10.w < 0.5 || lighting.reflection0.w < 0.5) { return vec3<f32>(1.0); }
    var strength = 1.0;
    if (emitter_shadows.control.x > 0.5) {
        strength = emitter_shadows.lights[light_index].views.w;
    } else if (lighting.surface0.x < -0.5 && f32(light_index) != lighting.render_compat.x) {
        return vec3<f32>(1.0);
    }
    if (strength <= 0.0) { return vec3<f32>(1.0); }
    return mix(vec3<f32>(1.0), hybrid_transmission_visibility(world, direction, distance), strength);
}

fn surface_primary_visible(input: VertexOut) -> bool {
    if (params.material6.x > 0.001 && params.material8.w > 0.5 && (REALTIME_TRANSPORT || params.material11.x < 0.5)) {
        let closed_view = select(normalize(params.camera0.xyz-input.world_position),
            -params.camera3.xyz,params.camera3.w > 0.5);
        if (dot(normalize(input.normal),closed_view) <= 0.0) { return false; }
    }
    if (planar.control.y > 0.5 && dot(planar.plane,vec4<f32>(input.world_position,1.0)) < planar.control.z) { return false; }
    if (input.hidden_weight > 0.01) { return false; }
    let alpha = textureSampleGrad(actor_texture,actor_sampler,input.uv,surface_gradient_x,surface_gradient_y).a * input.color.a * params.material4.a * params.style.x;
    return alpha > 0.001 && (params.material7.w <= 0.0 || alpha >= params.material7.w);
}

fn shade_surface(input: VertexOut) -> vec4<f32> {
    // The full-size nearest-winner mask partitions pixels, not individual
    // draws. Every original candidate retains its order within its route.
    var accepted = true;
    if (COARSE_ROUTE_FAST_ONLY || COARSE_ROUTE_FULL_ONLY) {
        let route = /* sample reflection route */sample_primary_reflection_route(input);
        accepted = !(COARSE_ROUTE_FAST_ONLY && route != 1u) && !(COARSE_ROUTE_FULL_ONLY && route == 1u);
    }
    // Keep expensive calls in the accepted control-flow branch. A backend
    // can retain discarded fragment helpers for derivatives; discard alone
    // must not be relied on to suppress lighting or transport work.
    // Keep shading inside the accepted branch, while providing a reachable
    // value return for both browser WGSL and native Naga validation.
    var output = vec4<f32>(0.0);
    if (accepted && surface_primary_visible(input)) {
        output = shade_surface_accepted(input);
    } else {
        discard;
    }
    return output;
}

fn shade_surface_accepted(input: VertexOut) -> vec4<f32> {
    surface_reflection = vec4<f32>(0.0);
    surface_geometry_reflection = 0.0;
    surface_reflection_distance = 0.0;
    surface_planar_reflection = false;
    surface_history_normal = normalize(input.normal);
    surface_history_material = vec3<f32>(1.0, 0.0, 1.0);
    surface_screen_reflection_lobe = vec3<f32>(0.0);
    surface_screen_reflection_response = vec3<f32>(0.0);
    if (params.material6.x > 0.001 && params.material8.w > 0.5 && (REALTIME_TRANSPORT || params.material11.x < 0.5)) {
        let closed_view = select(normalize(params.camera0.xyz - input.world_position),
            -params.camera3.xyz, params.camera3.w > 0.5);
        if (dot(normalize(input.normal),closed_view) <= 0.0) { discard; }
    }
    if (planar.control.y > 0.5 && dot(planar.plane, vec4<f32>(input.world_position, 1.0)) < planar.control.z) {
        discard;
    }
    if (input.hidden_weight > 0.01) {
        discard;
    }
    let uv = input.uv;
    let sampled = textureSampleGrad(actor_texture, actor_sampler, uv,surface_gradient_x,surface_gradient_y);
    let alpha = sampled.a * input.color.a * params.material4.a * params.style.x;
    if (alpha <= 0.001 || (params.material7.w > 0.0 && alpha < params.material7.w)) {
        discard;
    }
    // The sRGB texture format decodes before filtering. Undo it only for
    // display-locked materials; PBR uses the sampled linear radiance directly.
    let encoded_sample = select(1.055 * pow(max(sampled.rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055,
        sampled.rgb * 12.92, sampled.rgb <= vec3<f32>(0.0031308));
    let base_srgb = clamp(
        encoded_sample * input.color.rgb * params.material4.rgb,
        vec3<f32>(0.0), vec3<f32>(1.0)
    );
    var base_color = sampled.rgb * pow(clamp(input.color.rgb * params.material4.rgb,
        vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(2.2));
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x < -0.5) {
            // glTF factors and vertex colors are linear, unlike legacy display factors.
            base_color = sampled.rgb * input.color.rgb * params.material4.rgb;
        }
    }
    if (lighting.surface3.x != 1.0) {
        base_color = max(mix(vec3<f32>(dot(base_color, vec3<f32>(0.2126,0.7152,0.0722))), base_color, lighting.surface3.x), vec3<f32>(0.0));
    }
    let mr_sample = textureSampleGrad(metallic_roughness_texture, actor_sampler, uv,surface_gradient_x,surface_gradient_y);
    let material_samples = material_channels(mr_sample);
    var metallic = clamp(params.material0.x * material_samples.x, 0.0, 1.0);
    var roughness = clamp(params.material0.y * material_samples.y + lighting.surface1.z, 0.045, 1.0);
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x > 2.5 && lighting.surface0.x < 3.5) {
            base_color = clay_base_color();
            metallic = clay_metallic();
            roughness = clay_roughness();
        }
    }

    // glTF double-sided materials must light backfaces with the front side's
    // normal; orient the geometric normal toward the viewer before the tangent
    // frame so the normal map and direct lighting agree.
    let view = select(normalize(params.camera0.xyz - input.world_position), -params.camera3.xyz, params.camera3.w > 0.5);
    let geometric_normal = surface_geometric_normal(input,view);
    material_geometric_normal = geometric_normal;
    let tangent = normalize(input.tangent - geometric_normal * dot(input.tangent,geometric_normal));
    let normal = surface_mapped_normal(input,geometric_normal);

    let authored_ior = clamp(params.material6.y, 1.0, 3.0);
    let ior_ratio = (authored_ior - 1.0) / (authored_ior + 1.0);
    let dielectric_f0 = vec3<f32>(ior_ratio * ior_ratio) *
        params.material0.w * params.material2.rgb;
    let f0 = mix(dielectric_f0, base_color, metallic);
    let transmission = clamp(params.material6.x, 0.0, 1.0);
    // Transmitted energy belongs to the scene behind the surface, not Lambert
    // diffuse. Preserve the existing opaque BRDF when transmission is zero.
    let diffuse_color = base_color * (1.0 - transmission);
    var lit = vec3<f32>(0.0);
    let light_count = u32(lighting.environment2.y + 0.5);
    var face_band = -1.0;
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x > 3.5 && params.cel_material0.y > 2.5) {
            var key = normalize(vec3<f32>(-0.42, 0.78, 0.47));
            if (light_count > 0u) { key = authored_light_radiance(lighting.lights[0], input.world_position).xyz; }
            let forward = normalize(input.face_forward);
            let right = normalize(input.face_right);
            let lateral = dot(key, right);
            let frontal = dot(key, forward);
            let planar = max(length(vec2<f32>(lateral, frontal)), 0.0001);
            let threshold = 1.0 - clamp(frontal / planar, 0.0, 1.0);
            let face_uv = vec2<f32>(select(uv.x, 1.0 - uv.x, lateral < 0.0), uv.y);
            let sdf = textureSampleLevel(cel_texture, actor_sampler, face_uv, 0.0).g;
            face_band = smoothstep(threshold - lighting.cel0.y, threshold + lighting.cel0.y, sdf);
            if (frontal <= 0.0) { face_band = 0.0; }
        }
    }
    for (var light_index = 0u; light_index < light_count; light_index = light_index + 1u) {
        let authored = lighting.lights[light_index];
        // Local attenuation is exactly zero beyond the light range. For
        // area lights, include the farthest emitter sample in the bound.
        if (authored.position_kind.w > 0.5 && authored.direction_range.w > 0.001) {
            let emitter_radius = select(
                0.0,
                0.288675 * length(authored.spot_area.zw),
                authored.position_kind.w > 2.5,
            );
            let cutoff = authored.direction_range.w + emitter_radius;
            let delta = authored.position_kind.xyz - input.world_position;
            if (dot(delta, delta) >= cutoff * cutoff) {
                continue;
            }
        }
        let direction_attenuation = authored_light_radiance(authored, input.world_position);
        var radiance = authored.color_intensity.rgb * authored.color_intensity.w * direction_attenuation.w;
        if (authored.position_kind.w <= 2.5) {
            if (direction_attenuation.w <= 0.0 || authored.color_intensity.w <= 0.0) { continue; }
            // Unwrapped physical lobes contribute no energy behind the surface.
            if (lighting.surface0.x < 0.5 && lighting.surface0.z == 0.0 &&
                dot(normal, direction_attenuation.xyz) <= 0.0 &&
                (params.material10.x <= 0.0 || dot(material_geometric_normal, direction_attenuation.xyz) <= 0.0)) { continue; }
            let emitter_distance = select(1e6, length(authored.position_kind.xyz - input.world_position), authored.position_kind.w > 0.5);
            var opaque_visibility = 1.0;
            if (emitter_shadows.control.x > 0.5) {
                opaque_visibility = emitter_visibility(light_index, 0u, input.world_position, normal);
            } else if (lighting.surface0.x < -0.5) {
                // The selected shadow owner's index is packed independently of kind.
                if (f32(light_index) == lighting.render_compat.x) { opaque_visibility = sample_shadow(input.world_position, normal); }
            }
            // A completely occluded emitter contributes no direct energy;
            // avoid tracing its glass path through the rest of the room.
            if (opaque_visibility <= 0.0) { continue; }
            radiance *= opaque_visibility * surface_glass_visibility(light_index, input.world_position, direction_attenuation.xyz, emitter_distance);
        }
        if (authored.position_kind.w > 2.5) {
            // A direct World render leaves this slot zero and keeps the
            // original four-sample result; Scene preview sets its budget.
            let requested_count = u32(select(4.0, lighting.preview2.w, lighting.preview2.w >= 0.5));
            // At distances larger than the emitter's width/height, its
            // quadrature directions converge; balanced previews use the centre.
            let area_distance = length(authored.position_kind.xyz - input.world_position);
            let far_area = requested_count < 4u &&
                area_distance > max(authored.spot_area.z, authored.spot_area.w);
            let area_count = select(select(requested_count, 1u, far_area), u32(emitter_shadows.control.z), emitter_shadows.control.x > 0.5);
            for (var area_sample = 0u; area_sample < area_count; area_sample = area_sample + 1u) {
                var sample_index = area_sample;
                if (area_count == 1u) { sample_index = 4u; }
                if (area_count == 2u) { sample_index = area_sample * 3u; }
                let area_direction_attenuation = authored_area_light_radiance(authored, input.world_position, sample_index);
                if (area_direction_attenuation.w <= 0.0 || authored.color_intensity.w <= 0.0) { continue; }
                if (lighting.surface0.x < 0.5 && lighting.surface0.z == 0.0 &&
                    dot(normal, area_direction_attenuation.xyz) <= 0.0 &&
                    (params.material10.x <= 0.0 || dot(material_geometric_normal, area_direction_attenuation.xyz) <= 0.0)) { continue; }

                let sample_weight = 4.0 / f32(area_count);
                var area_radiance = authored.color_intensity.rgb * authored.color_intensity.w * area_direction_attenuation.w * sample_weight;
                // Match the exact emitter quadrature direction used by the BRDF.
                let area_distance = length(authored_area_light_position(authored, sample_index) - input.world_position);
                var opaque_visibility = 1.0;
                if (emitter_shadows.control.x > 0.5) {
                    opaque_visibility = emitter_visibility(light_index, area_sample, input.world_position, normal);
                } else if (lighting.surface0.x < -0.5 && f32(light_index) == lighting.render_compat.x) {
                    opaque_visibility = sample_shadow(input.world_position, normal);
                }
                if (opaque_visibility <= 0.0) { continue; }
                area_radiance *= opaque_visibility * surface_glass_visibility(light_index, input.world_position, area_direction_attenuation.xyz, area_distance);
                lit += direct_pbr(
                    normal, view, area_direction_attenuation.xyz, area_radiance,
                    diffuse_color, metallic, roughness, f0, face_band
                );
            }
        } else {
            if (!PHYSICAL_STYLE_ONLY) {
                if (lighting.surface0.x > 3.5 && light_index > 0u) {
                    // Only the first authored light shapes cel bands; others provide soft fill.
                    lit += diffuse_color * radiance * max(dot(normal, direction_attenuation.xyz), 0.0) * 0.15 / 3.14159265;
                    continue;
                }
            }
            lit += direct_pbr(
                normal, view, direction_attenuation.xyz, radiance,
                diffuse_color, metallic, roughness, f0, face_band
            );
        }
    }
    // Scenes without authored lighting retain the earlier studio setup.
    if (light_count == 0u && lighting.environment0.w < 0.5) {
        lit += direct_pbr(
            normal, view, normalize(vec3<f32>(-0.42, 0.78, 0.47)), vec3<f32>(4.2, 4.0, 3.75),
            diffuse_color, metallic, roughness, f0, face_band
        );
        lit += direct_pbr(
            normal, view, normalize(vec3<f32>(0.68, 0.28, 0.51)), vec3<f32>(1.25, 1.45, 1.75),
            diffuse_color, metallic, roughness, f0, face_band
        );
    }
    if (lighting.surface0.x >= -0.5 && emitter_shadows.control.x < 0.5) { lit *= sample_shadow(input.world_position, normal); }
    let n_dot_v = max(dot(normal, view), 0.0);
    var environment_fresnel = fresnel_schlick(n_dot_v, f0);
    let inside_solid = params.material11.x > 0.5 && dot(normalize(input.normal), view) < 0.0;
    if (params.material11.x > 0.5 && transmission > 0.001) {
        let eta_i = select(1.0, authored_ior, inside_solid);
        let eta_t = select(authored_ior, 1.0, inside_solid);
        environment_fresnel = vec3<f32>(hybrid_fresnel(n_dot_v, eta_i, eta_t));
    }
    let diffuse_environment = sample_environment_irradiance(normal) / 3.14159265 *
        lighting.environment0.x * lighting.environment1.z;
    let baked_irradiance = sample_baked_irradiance(input.world_position, normal);
    let baked_diffuse = baked_irradiance.rgb / 3.14159265;
    let reflected = reflect(-view, normal);
    var specular_environment = sample_environment_specular(reflected, roughness, f0, n_dot_v) *
        lighting.environment0.x * lighting.environment1.w;
    let local_specular = sample_local_specular(input.world_position, reflected, roughness, f0, n_dot_v);
    specular_environment = mix(specular_environment, local_specular.rgb, local_specular.a);
    if (params.material11.x > 0.5 && transmission > 0.001) {
        let approximate = max(fresnel_schlick(n_dot_v, f0), vec3<f32>(0.0001));
        specular_environment *= environment_fresnel / approximate;
    }
    var specular_replacement = local_specular.a;
    let sheen_base = material_sheen_scale(n_dot_v, n_dot_v);
    let coat_no_v = max(dot(geometric_normal, view), 0.0);
    let coat_base = 1.0 - params.material10.x * material_coat_fresnel(coat_no_v);
    let layer_base = sheen_base * coat_base;
    let sheen_environment = material_sheen_environment(input.world_position, normal, view);
    var coat_environment = vec3<f32>(0.0);
    var coat_replacement = 0.0;
    if (params.material10.x > 0.0) {
        let coat_reflected = reflect(-view, geometric_normal);
        let coat_global = sample_environment_specular(coat_reflected, params.material10.y,
            vec3<f32>(0.04), coat_no_v) * lighting.environment0.x * lighting.environment1.w;
        let coat_local = sample_local_specular(input.world_position, coat_reflected,
            params.material10.y, vec3<f32>(0.04), coat_no_v);
        coat_environment = mix(coat_global, coat_local.rgb, coat_local.a) * params.material10.x;
        coat_replacement = coat_local.a;
    }
    // A valid reflected-camera capture replaces both GGX lobes below. Resolve
    // its projection first so those pixels do not trace discarded BVH rays.
    // Backfaces and points outside the capture retain geometry transport.
    let planar_sample = surface_planar_projection(input,normal,geometric_normal,roughness);
    let planar_projection = planar_sample.xy;
    surface_planar_reflection = planar_sample.z > 0.5;
    // Geometry transport supplies camera-independent radiance for each GGX
    // lobe. A miss leaves its existing room/global fallback intact.
    // A reflected-camera capture already spends the primary mirror bounce.
    let geometry_bounces = u32(max(lighting.reflection0.x - select(0.0, 1.0, planar.control.y > 0.5), 0.0));
    if (GEOMETRY_REFLECTION_ENABLED && lighting.reflection0.w > 0.5 && params.material2.w < 0.5 &&
        lighting.surface0.x < 0.5 && params.style.y > 0.0 && !surface_planar_reflection && geometry_bounces > 0u) {
        var coarse = RoughReflectionSample(vec4<f32>(0.0),0.0,false);
        if (COARSE_REFLECTION_ENABLED && planar.control.y < 0.5 && transmission <= 0.001 &&
            alpha >= 0.999999 && roughness < 0.75) {
            coarse = /* sample rough evidence */sample_rough_reflection_evidence(input,normal,geometric_normal,roughness,false);
        }
        var geometry_radiance = coarse.radiance;
        if (coarse.valid) {
            hybrid_reflection_hit_distance = coarse.distance;
        } else {
            geometry_radiance = /* primary reflection query */trace_geometry_reflection(
                input.world_position + reflected * hybrid_epsilon(input.world_position) * 3.0,
                reflected, roughness, geometry_bounces);
        }
        let response_brdf = textureSampleLevel(environment_brdf_texture, environment_brdf_sampler,
            vec2<f32>(n_dot_v,roughness),0.0).rg;
        let response = select(f0 * response_brdf.x + response_brdf.y,
            environment_fresnel, params.material11.x > 0.5 && transmission > 0.001);
        specular_environment = mix(specular_environment,
            geometry_radiance.rgb * response, geometry_radiance.a);
        specular_replacement = max(specular_replacement, geometry_radiance.a);
        surface_geometry_reflection = geometry_radiance.a;
        surface_reflection_distance = hybrid_reflection_hit_distance;
        if (params.material10.x > 0.0) {
            let coat_direction = reflect(-view, geometric_normal);
            var coarse_coat = RoughReflectionSample(vec4<f32>(0.0),0.0,false);
            if (COARSE_REFLECTION_ENABLED && planar.control.y < 0.5 && transmission <= 0.001 &&
                alpha >= 0.999999 && params.material10.y < 0.75) {
                coarse_coat = /* sample rough evidence */sample_rough_reflection_evidence(input,normal,geometric_normal,params.material10.y,true);
            }
            var coat_geometry = coarse_coat.radiance;
            if (coarse_coat.valid) {
                hybrid_reflection_hit_distance = coarse_coat.distance;
            } else {
                coat_geometry = /* primary reflection query */trace_geometry_reflection(
                    input.world_position + coat_direction * hybrid_epsilon(input.world_position) * 3.0,
                    coat_direction, params.material10.y, geometry_bounces);
            }
            let coat_brdf = textureSampleLevel(environment_brdf_texture, environment_brdf_sampler,
                vec2<f32>(coat_no_v,params.material10.y),0.0).rg;
            coat_environment = mix(coat_environment,
                coat_geometry.rgb * (vec3<f32>(0.04) * coat_brdf.x + coat_brdf.y) * params.material10.x,
                coat_geometry.a);
            coat_replacement = max(coat_replacement, coat_geometry.a);
            surface_geometry_reflection = max(surface_geometry_reflection, coat_geometry.a);
            if (hybrid_reflection_hit_distance > 0.0) {
                surface_reflection_distance = select(hybrid_reflection_hit_distance,
                    min(surface_reflection_distance, hybrid_reflection_hit_distance), surface_reflection_distance > 0.0);
            }
        }
    }
    // Planar radiance comes from the actual reflected room. Only the target's
    // viewer-facing plane replaces the BRDF-weighted environment fallback.
    if (surface_planar_reflection) {
        let projected = planar_projection;
        let radius = roughness * roughness * 0.012;
        let reflected_room = textureSampleLevel(planar_texture, planar_sampler, projected,0.0).rgb * 0.4 +
            textureSampleLevel(planar_texture, planar_sampler, projected + vec2<f32>(radius,0.0),0.0).rgb * 0.15 +
            textureSampleLevel(planar_texture, planar_sampler, projected - vec2<f32>(radius,0.0),0.0).rgb * 0.15 +
            textureSampleLevel(planar_texture, planar_sampler, projected + vec2<f32>(0.0,radius),0.0).rgb * 0.15 +
            textureSampleLevel(planar_texture, planar_sampler, projected - vec2<f32>(0.0,radius),0.0).rgb * 0.15;
        if (planar.control.w > 0.0) {
            // Automatic captures replace incident radiance only. Retain the
            // primary material's full-resolution integrated GGX response.
            let planar_brdf = textureSampleLevel(environment_brdf_texture,environment_brdf_sampler,
                vec2<f32>(n_dot_v,roughness),0.0).rg;
            specular_environment = reflected_room * (f0 * planar_brdf.x + planar_brdf.y);
        } else {
            specular_environment = reflected_room * environment_fresnel;
        }
        specular_replacement = 1.0;
        if (params.material10.x > 0.0) {
            coat_environment = reflected_room * params.material10.x * material_coat_fresnel(coat_no_v);
            coat_replacement = 1.0;
        }
    }
    let ao = clamp(1.0 - lighting.environment2.z * (1.0 - max(normal.y, 0.0)) * 0.35, 0.15, 1.0);
    let contact = 1.0 - lighting.color1.x *
        (1.0 - smoothstep(0.0, max(lighting.color1.y, 0.001), max(input.world_position.y, 0.0))) *
        (0.45 + 0.55 * (1.0 - lighting.color1.z));
    var diffuse_ambient = diffuse_color * (1.0 - metallic) * mix(
        diffuse_environment * lighting.surface2.rgb * lighting.surface2.w,
        baked_diffuse, baked_irradiance.a);
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x < -0.5) {
            // Environment irradiance remains available when hemisphere fill is zero.
            diffuse_ambient = diffuse_color * (1.0 - metallic) * mix(
                diffuse_environment, baked_diffuse, baked_irradiance.a);
        }
    }
    diffuse_ambient *= layer_base;
    let specular_ambient = specular_environment * lighting.surface1.y * layer_base;
    let sheen_ambient = sheen_environment.rgb * lighting.surface1.y * coat_base;
    let coat_ambient = coat_environment * lighting.surface1.y;
    let specular_occlusion = mix(ao * contact, 1.0, specular_replacement);
    let material_ao = material_channels(textureSampleGrad(occlusion_texture, actor_sampler, uv,surface_gradient_x,surface_gradient_y)).z;
    surface_history_normal = normal;
    surface_history_material = vec3<f32>(roughness, metallic, material_ao);
    // Baked transport and local/planar captures already contain room occlusion.
    // Retain material micro-occlusion, without a second hemisphere/floor shadow.
    lit += (diffuse_ambient * mix(ao * contact, 1.0, baked_irradiance.a) +
        specular_ambient * specular_occlusion +
        sheen_ambient * mix(ao * contact, 1.0, sheen_environment.a) +
        coat_ambient * mix(ao * contact, 1.0, coat_replacement)) * material_ao;
    if (lighting.caustics1.w > 0.5 && params.material8.x > 0.5) {
        let depth_attenuation = exp(-max(0.0, lighting.fog4.y - input.world_position.y) * lighting.caustics0.w);
        let facing = max(normal.y, 0.0);
        lit += base_color * lighting.caustics1.rgb * surface_caustic_pattern(input.world_position)
            * lighting.caustics0.x * depth_attenuation * facing;
    }
    lit += base_color * lighting.surface0.w * pow(1.0 - n_dot_v, lighting.surface1.x);
    // Tangent-aligned hair highlights are optional and do not change PBR materials.
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x > 3.5 && params.cel_material0.y > 1.5 && params.cel_material0.y < 2.5) {
            let key = select(normalize(vec3<f32>(-0.42,0.78,0.47)), authored_light_radiance(lighting.lights[0], input.world_position).xyz, light_count > 0u);
            let half_vector = normalize(view + key);
            let strand = sqrt(max(0.0, 1.0 - pow(dot(tangent, half_vector), 2.0)));
            lit += base_color * smoothstep(0.96, 0.99, strand) * params.cel_material0.z
                * textureSampleLevel(cel_texture, actor_sampler, uv, 0.0).b;
        }
    }

    let emissive_sample = textureSampleGrad(emissive_texture, actor_sampler, uv,surface_gradient_x,surface_gradient_y).rgb;
    lit += emissive_sample * params.material1.rgb * coat_base;
    let force_unlit = max(params.material1.w, params.material2.w);
    let lighting_mix = params.style.y * (1.0 - force_unlit);
    let shaded = mix(base_color, lit, lighting_mix);
    let surface_exposure = mix(params.style.z, 1.0, params.material2.w);
    let exposed = shaded * surface_exposure;
    var display = exposed;
    if (params.material2.w > 0.5) { display = inverse_display_curve(base_srgb); }
    let fog_amount = atmosphere_medium_amount(input.world_position);
    let fog_radiance = lighting.fog1.rgb;
    let indirect_specular = (specular_ambient * specular_occlusion +
        sheen_ambient * mix(ao * contact, 1.0, sheen_environment.a) +
        coat_ambient * mix(ao * contact, 1.0, coat_replacement)) * material_ao * lighting_mix * surface_exposure;
    let specular_display = indirect_specular * (1.0 - fog_amount);
    let brdf = textureSampleLevel(environment_brdf_texture,environment_brdf_sampler,
        vec2<f32>(n_dot_v,roughness),0.0).rg;
    var response_rgb = (f0 * brdf.x + vec3<f32>(brdf.y)) *
        lighting.surface1.y * specular_occlusion * material_ao * lighting_mix * surface_exposure * (1.0 - fog_amount);
    var reflection_lobe = specular_display;
    if (REALTIME_TRANSPORT) {
        // One screen ray owns the mapped-normal base GGX lobe. Retain sheen
        // and differently shaped coat fallback instead of subtracting their
        // light with a response that cannot reconstruct those layers.
        reflection_lobe = specular_ambient * specular_occlusion *
            material_ao * lighting_mix * surface_exposure * (1.0 - fog_amount);
        response_rgb *= layer_base;
        surface_screen_reflection_response = response_rgb;
        if (params.material11.x > 0.5 && transmission > 0.001) {
            // The solid approximation's environment base uses exact entry
            // Fresnel; the screen replacement must retain that same response.
            surface_screen_reflection_response *= environment_fresnel /
                max(fresnel_schlick(n_dot_v, f0), vec3<f32>(0.0001));
        }
        if (params.material10.x > 0.0 && dot(normal, geometric_normal) >= 0.999 &&
            abs(roughness - params.material10.y) <= 0.03) {
            // Matching lobes may share one screen query. Each still keeps its
            // own BRDF and occlusion, so resolve replaces rather than adds.
            let coat_brdf = textureSampleLevel(environment_brdf_texture,environment_brdf_sampler,
                vec2<f32>(coat_no_v,params.material10.y),0.0).rg;
            let coat_scale = lighting.surface1.y * params.material10.x *
                mix(ao * contact, 1.0, coat_replacement) * material_ao * lighting_mix *
                surface_exposure * (1.0 - fog_amount);
            response_rgb += (vec3<f32>(0.04) * coat_brdf.x + vec3<f32>(coat_brdf.y)) * coat_scale;
            surface_screen_reflection_response +=
                (vec3<f32>(0.04) * coat_brdf.x + vec3<f32>(coat_brdf.y)) * coat_scale;
            reflection_lobe += coat_ambient * mix(ao * contact, 1.0, coat_replacement) *
                material_ao * lighting_mix * surface_exposure * (1.0 - fog_amount);
        }
        surface_screen_reflection_lobe = reflection_lobe;
    }
    // Screen-space resolve and geometry transport both sample scene-linear radiance.
    var response = dot(response_rgb,vec3<f32>(0.2126,0.7152,0.0722));
    if (REALTIME_TRANSPORT) {
        // The opaque MRT carries one scalar response. A tinted GGX lobe
        // cannot be reconstructed from it: retain its RGB probe/environment
        // fallback instead of replacing colored metal with a neutral lobe.
        let response_max = max(response_rgb.r, max(response_rgb.g, response_rgb.b));
        let response_min = min(response_rgb.r, min(response_rgb.g, response_rgb.b));
        if (response_max - response_min > max(0.00001, response_max * 0.02)) { response = -1.0; }
    }
    if (params.material2.w > 0.5) { response = 0.0; }
    if (planar.control.x > 0.5) { response = -1.0; }
    // Reference transport resolves layered GGX independently. Realtime uses
    // the selected base/matching-coat payload above and retains other layers.
    if (!REALTIME_TRANSPORT && (any(params.material9.rgb > vec3<f32>(0.0)) || params.material10.x > 0.0)) { response = -1.0; }
    if (surface_geometry_reflection > 0.0) { response = -(2.0 + surface_reflection_distance); }
    let fog_color = fog_radiance;
    display = mix(display, fog_color, fog_amount);
    var output_alpha = alpha;
    if (transmission > 0.001) {
        // Reflection is an entry-interface lobe and is not Beer-Lambert
        // attenuated. The ordered transmission path attenuates only the ray
        // through the volume; fallback alpha retains Fresnel coverage.
        let fresnel_strength = max(environment_fresnel.r,
            max(environment_fresnel.g, environment_fresnel.b));
        output_alpha *= clamp(
            (1.0 - transmission) + fresnel_strength + roughness * 0.08,
            0.015,
            1.0
        );
    }
    // The HDR attachment uses alpha blending, while this lobe attachment has
    // no blend state. Store the same covered energy for temporal/SSR deltas.
    surface_reflection = vec4<f32>(reflection_lobe * output_alpha,
        select(response * output_alpha, response, response < 0.0));
    return vec4<f32>(display, output_alpha);
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    params = instance_params[input.instance_id];
    initialize_surface_gradients(input);
    return shade_surface(input);
}

@fragment
fn fs_capture_opaque(input: VertexOut) -> @location(0) vec4<f32> {
    params = instance_params[input.instance_id];
    initialize_surface_gradients(input);
    let size = vec2<i32>(textureDimensions(opaque_scene_depth));
    let pixel = clamp(vec2<i32>(input.pos.xy), vec2<i32>(0), size - vec2<i32>(1));
    let depth = textureLoad(opaque_scene_depth, pixel, 0);
    let coverage = textureLoad(opaque_scene_texture, pixel, 0).a;
    var output = vec4<f32>(0.0);
    if (!(coverage >= 0.999999 && input.pos.z + 0.000002 < depth)) {
        output = shade_surface(input);
    } else {
        discard;
    }
    return output;
}

// This cheap pass uses the same vertex deformation and coverage predicates as
// shade_surface. Its strict depth comparison records the first nearest surface
// and its coverage, including coplanar surfaces with different authored alpha.
@fragment
fn fs_depth_prepass(input: VertexOut) -> @location(0) vec4<f32> {
    params = instance_params[input.instance_id];
    if (params.material6.x > 0.001 && params.material8.w > 0.5 && (REALTIME_TRANSPORT || params.material11.x < 0.5)) {
        let closed_view = select(normalize(params.camera0.xyz - input.world_position),
            -params.camera3.xyz, params.camera3.w > 0.5);
        if (dot(normalize(input.normal),closed_view) <= 0.0) { discard; }
    }
    if (planar.control.y > 0.5 && dot(planar.plane, vec4<f32>(input.world_position, 1.0)) < planar.control.z) {
        discard;
    }
    if (input.hidden_weight > 0.01) { discard; }
    let sampled = textureSample(actor_texture, actor_sampler, input.uv);
    let alpha = sampled.a * input.color.a * params.material4.a * params.style.x;
    if (alpha <= 0.001 || (params.material7.w > 0.0 && alpha < params.material7.w)) { discard; }
    return vec4<f32>(0.0, 0.0, 0.0, alpha);
}

struct SurfaceGbufferOutput {
    @location(0) color: vec4<f32>,
    @location(1) geometry: vec4<f32>,
    @location(2) material: vec4<f32>,
    @location(3) reflection: vec4<f32>,
};

fn encode_gbuffer_normal(value: vec3<f32>) -> vec2<f32> {
    var normal = value / max(abs(value.x) + abs(value.y) + abs(value.z), 0.000001);
    if (normal.z < 0.0) {
        normal = vec3<f32>((vec2<f32>(1.0) - abs(normal.yx)) * sign(normal.xy), normal.z);
    }
    return normal.xy * 0.5 + 0.5;
}

@fragment
fn fs_main_gbuffer(input: VertexOut) -> SurfaceGbufferOutput {
    params = instance_params[input.instance_id];
    initialize_surface_gradients(input);
    // Small raster islands keep ordinary depth testing without snapshot reads.
    // Only a current-frame prepass can provide valid occlusion evidence.
    if (REALTIME_TRANSPORT && !RASTER_VISIBILITY_ENABLED) { return shade_gbuffer_accepted(input); }
    let depth_size = vec2<i32>(textureDimensions(opaque_scene_depth));
    let depth_pixel = clamp(vec2<i32>(input.pos.xy), vec2<i32>(0), depth_size - vec2<i32>(1));
    let nearest_depth = textureLoad(opaque_scene_depth, depth_pixel, 0);
    let nearest_coverage = textureLoad(opaque_scene_texture, depth_pixel, 0).a;
    // The independent snapshot uses the identical projection and coverage.
    // Keep a conservative reverse-Z tolerance and the original Greater test,
    // so coplanar authored ordering remains owned by the main depth buffer.
    // Fractional nearest coverage retains every original colour contribution.
    var output = SurfaceGbufferOutput(vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0));
    if (!(nearest_coverage >= 0.999999 && input.pos.z + 0.000002 < nearest_depth)) {
        output = shade_gbuffer_accepted(input);
    } else {
        discard;
    }
    return output;
}

fn shade_gbuffer_accepted(input: VertexOut) -> SurfaceGbufferOutput {
    let color = shade_surface(input);
    // Reuse the actual shaded normal and channels in every preview profile.
    // Reflection history needs these identities even when SSR is disabled.
    let normal = surface_history_normal;
    let roughness = surface_history_material.x;
    let metallic = surface_history_material.y;
    let ao = surface_history_material.z;

    var velocity = vec2<f32>(0.0);
    if (params.motion0.y > 0.5
        && input.current_clip.w > 0.0001
        && input.previous_clip.w > 0.0001) {
        let current_ndc = input.current_clip.xy / input.current_clip.w;
        let current_uv = vec2<f32>(current_ndc.x * 0.5 + 0.5, 0.5 - current_ndc.y * 0.5);
        let previous_ndc = input.previous_clip.xy / input.previous_clip.w;
        let previous_uv = vec2<f32>(previous_ndc.x * 0.5 + 0.5, 0.5 - previous_ndc.y * 0.5);
        if (all(previous_uv >= vec2<f32>(-0.05)) && all(previous_uv <= vec2<f32>(1.05))) {
            // Store physical motion on the stable output grid. Projection
            // jitter is a sampling pattern, never object or shutter motion.
            velocity = current_uv - previous_uv
                - (lighting.preview1.xy - lighting.preview1.zw) / params.canvas.xy;
        }
    }

    let emissive_factor = length(params.material1.rgb) * params.material1.w;
    let transport_reflection = max(surface_geometry_reflection,
        select(0.0, 1.0, surface_planar_reflection));
    let reactive = clamp(max(max(params.material6.x, emissive_factor * 0.25),
        transport_reflection * lighting.reflection0.z), 0.0, 1.0);
    return SurfaceGbufferOutput(
        color,
        vec4<f32>(encode_gbuffer_normal(normal), velocity),
        vec4<f32>(roughness, metallic, ao, reactive),
        surface_reflection,
    );
}

fn transmission_scene_view_depth(uv: vec2<f32>) -> f32 {
    let size = vec2<i32>(textureDimensions(opaque_scene_depth));
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), size - vec2<i32>(1));
    let depth = textureLoad(opaque_scene_depth, pixel, 0);
    let near = params.camera1.w;
    let far = params.camera2.w;
    return select(near * far / max(near + depth * (far - near), 0.00001),
        far - depth * (far - near), params.camera3.w > 0.5);
}

fn transmission_project(world: vec3<f32>) -> vec2<f32> {
    let rel = world - params.camera0.xyz;
    let depth = dot(rel, params.camera3.xyz);
    let w = select(max(depth, 0.00001), 1.0, params.camera3.w > 0.5);
    return (params.canvas.zw + lighting.preview1.xy) / params.canvas.xy +
        vec2<f32>(dot(rel, params.camera1.xyz), -dot(rel, params.camera2.xyz)) * params.camera0.w / (params.canvas.xy * w);
}

fn transmissive_snapshot_visible(input: VertexOut) -> bool {
    let depth_size = vec2<i32>(textureDimensions(opaque_scene_depth));
    let depth_pixel = clamp(vec2<i32>(input.pos.xy), vec2<i32>(0), depth_size - vec2<i32>(1));
    let nearest_depth = textureLoad(opaque_scene_depth, depth_pixel, 0);
    let nearest_coverage = textureLoad(opaque_scene_texture, depth_pixel, 0).a;
    // The current pane's independent underlay retains the opaque reverse-Z
    // depth. Keep transport in the accepted branch, with the same conservative
    // depth/coverage tolerance as opaque shading and gradients from its quad.
    return !(nearest_coverage >= 0.999999 && input.pos.z + 0.000002 < nearest_depth);
}

fn shade_transmissive(input: VertexOut) -> vec4<f32> {
    params = instance_params[input.instance_id];
    initialize_surface_gradients(input);
    var output = vec4<f32>(0.0);
    if (transmissive_snapshot_visible(input) && surface_primary_visible(input)) {
        output = shade_transmissive_accepted(input);
    } else {
        discard;
    }
    return output;
}

fn shade_transmissive_planar_route(input: VertexOut, cached: bool) -> vec4<f32> {
    params = instance_params[input.instance_id];
    initialize_surface_gradients(input);
    var output = vec4<f32>(0.0);
    if (transmissive_snapshot_visible(input) && surface_primary_visible(input)) {
        // These two entries partition one original draw. Their underlay and
        // opaque depth are copied once before either entry runs. Edges, mapped
        // normals and failed projections retain the complete transport shader.
        if (transmissive_planar_cached(input) == cached) {
            output = shade_transmissive_accepted(input);
        } else {
            discard;
        }
    } else {
        discard;
    }
    return output;
}

fn transmission_snapshot_sample(uv: vec2<f32>, center_uv: vec2<f32>, exit_depth: f32) -> vec4<f32> {
    var sample_uv = uv;
    if (REALTIME_TRANSPORT) {
        let border = vec2<f32>(0.5) / params.canvas.xy;
        // Every blur tap must respect the pane's depth, not only its centre.
        // Reuse the valid centre at borders and foreground silhouettes.
        if (any(uv < border) || any(uv > vec2<f32>(1.0) - border)) {
            sample_uv = center_uv;
        } else if (transmission_scene_view_depth(uv) < exit_depth) {
            sample_uv = center_uv;
        }
    }
    return textureSampleLevel(opaque_scene_texture, opaque_scene_sampler, sample_uv,0.0);
}

fn glass_snapshot_depth(uv: vec2<f32>) -> f32 {
    let size = vec2<i32>(textureDimensions(opaque_scene_depth));
    let pixel = clamp(vec2<i32>(uv * vec2<f32>(size)), vec2<i32>(0), size - vec2<i32>(1));
    return textureLoad(opaque_scene_depth, pixel, 0);
}

fn glass_snapshot_world(uv: vec2<f32>, distance: f32) -> vec3<f32> {
    let projection_distance = select(distance, 1.0, params.camera3.w > 0.5);
    let screen = uv * params.canvas.xy - params.canvas.zw - lighting.preview1.xy;
    let view_xy = screen * projection_distance / max(params.camera0.w, 0.000001);
    return params.camera0.xyz + params.camera3.xyz * distance +
        params.camera1.xyz * view_xy.x - params.camera2.xyz * view_xy.y;
}

fn glass_reflection_snapshot_sample(uv: vec2<f32>, center: vec2<f32>, distance: f32, tolerance: f32) -> vec3<f32> {
    var accepted = center;
    let border = vec2<f32>(1.5) / params.canvas.xy;
    if (all(uv > border) && all(uv < vec2<f32>(1.0) - border)) {
        if (glass_snapshot_depth(uv) > 0.000001 &&
            abs(transmission_scene_view_depth(uv) - distance) <= tolerance) {
            accepted = uv;
        }
    }
    return textureSampleLevel(opaque_scene_texture, opaque_scene_sampler, accepted, 0.0).rgb;
}

// One bounded screen ray follows the actual glass interface, never the opaque
// underlay's normal. Only panes with a current profile-budgeted snapshot reach
// this helper; a miss keeps room/probe radiance and the glass TAA marker intact.
fn glass_screen_reflection(position: vec3<f32>, normal: vec3<f32>, view: vec3<f32>, roughness: f32) -> vec4<f32> {
    if (!REALTIME_TRANSPORT || lighting.preview0.y < 0.5 || lighting.preview2.y < 1.0 ||
        lighting.surface0.x >= 0.5 || planar.control.y > 0.5 || surface_planar_reflection ||
        roughness >= 0.5 || max(surface_screen_reflection_response.r,
            max(surface_screen_reflection_response.g, surface_screen_reflection_response.b)) <= 0.00001) {
        return vec4<f32>(0.0);
    }
    let reflected = normalize(reflect(-view, normal));
    let distance = dot(position - params.camera0.xyz, params.camera3.xyz);
    let stride = max(distance * 0.035, 0.025);
    // Glass never inherits an unbounded cost from a high quality profile.
    // Browser profiles further reduce this through the shared step uniform.
    let steps = u32(clamp(lighting.preview2.y, 1.0, 24.0));
    let origin_uv = transmission_project(position);
    let border = vec2<f32>(1.5) / params.canvas.xy;
    var previous = 0.0;
    for (var step = 1u; step <= 24u; step += 1u) {
        if (step > steps) { break; }
        let ray_distance = stride * f32(step);
        let ray_world = position + reflected * ray_distance;
        let ray_depth = dot(ray_world - params.camera0.xyz, params.camera3.xyz);
        if (ray_depth <= params.camera1.w) { break; }
        let uv = transmission_project(ray_world);
        if (any(uv <= border) || any(uv >= vec2<f32>(1.0) - border)) { break; }
        if (glass_snapshot_depth(uv) > 0.000001) {
            let scene_distance = transmission_scene_view_depth(uv);
            let thickness = max(stride * 0.6, scene_distance * 0.004);
            if (ray_depth >= scene_distance - thickness) {
                var low = previous;
                var high = ray_distance;
                for (var refine = 0u; refine < 4u; refine += 1u) {
                    let middle = (low + high) * 0.5;
                    let middle_world = position + reflected * middle;
                    let middle_depth = dot(middle_world - params.camera0.xyz, params.camera3.xyz);
                    let middle_uv = transmission_project(middle_world);
                    if (all(middle_uv > border) && all(middle_uv < vec2<f32>(1.0) - border) &&
                        glass_snapshot_depth(middle_uv) > 0.000001 &&
                        middle_depth >= transmission_scene_view_depth(middle_uv) - thickness) {
                        high = middle;
                    } else { low = middle; }
                }
                let hit_world = position + reflected * high;
                let hit_uv = transmission_project(hit_world);
                if (any(hit_uv <= border) || any(hit_uv >= vec2<f32>(1.0) - border)) { break; }
                if (length((hit_uv - origin_uv) * params.canvas.xy) < 2.0) {
                    previous = ray_distance;
                    continue;
                }
                let hit_distance = transmission_scene_view_depth(hit_uv);
                let hit_ray_depth = dot(hit_world - params.camera0.xyz, params.camera3.xyz);
                if (glass_snapshot_depth(hit_uv) <= 0.000001 ||
                    abs(hit_ray_depth - hit_distance) > thickness * 1.5) { break; }
                // Depth-derived receiver normal rejects backfaces and depth
                // silhouettes without importing another G-buffer binding.
                let texel = vec2<f32>(1.0) / params.canvas.xy;
                let right_uv = hit_uv + vec2<f32>(texel.x, 0.0);
                let down_uv = hit_uv + vec2<f32>(0.0, texel.y);
                let right_distance = transmission_scene_view_depth(right_uv);
                let down_distance = transmission_scene_view_depth(down_uv);
                let normal_tolerance = max(stride * 2.0, hit_distance * 0.025);
                if (glass_snapshot_depth(right_uv) <= 0.000001 || glass_snapshot_depth(down_uv) <= 0.000001 ||
                    abs(right_distance - hit_distance) > normal_tolerance ||
                    abs(down_distance - hit_distance) > normal_tolerance) { break; }
                let center_world = glass_snapshot_world(hit_uv, hit_distance);
                let raw_normal = cross(glass_snapshot_world(right_uv, right_distance) - center_world,
                    glass_snapshot_world(down_uv, down_distance) - center_world);
                if (dot(raw_normal, raw_normal) < 0.000000000001) { break; }
                var hit_normal = normalize(raw_normal);
                if (dot(hit_normal, params.camera0.xyz - center_world) < 0.0) { hit_normal = -hit_normal; }
                if (dot(hit_normal, -reflected) <= 0.03) { break; }
                let blur = (0.5 + roughness * 2.5) * texel;
                let blur_tolerance = max(thickness * 2.0, hit_distance * 0.01);
                let radiance = glass_reflection_snapshot_sample(hit_uv,hit_uv,hit_distance,blur_tolerance) * 0.5 +
                    glass_reflection_snapshot_sample(hit_uv + vec2<f32>(blur.x,0.0),hit_uv,hit_distance,blur_tolerance) * 0.125 +
                    glass_reflection_snapshot_sample(hit_uv - vec2<f32>(blur.x,0.0),hit_uv,hit_distance,blur_tolerance) * 0.125 +
                    glass_reflection_snapshot_sample(hit_uv + vec2<f32>(0.0,blur.y),hit_uv,hit_distance,blur_tolerance) * 0.125 +
                    glass_reflection_snapshot_sample(hit_uv - vec2<f32>(0.0,blur.y),hit_uv,hit_distance,blur_tolerance) * 0.125;
                let edge = min(min(hit_uv.x, 1.0-hit_uv.x), min(hit_uv.y, 1.0-hit_uv.y));
                let confidence = (1.0 - f32(step) / (f32(steps) + 1.0)) *
                    smoothstep(0.005,0.06,edge) * pow(1.0-roughness,2.0);
                return vec4<f32>(radiance,confidence);
            }
        }
        previous = ray_distance;
    }
    return vec4<f32>(0.0);
}

fn shade_transmissive_accepted(input: VertexOut) -> vec4<f32> {
    let view = select(normalize(params.camera0.xyz - input.world_position), -params.camera3.xyz, params.camera3.w > 0.5);
    let original_normal = normalize(input.normal);
    // Keep the query inside the static branch. A call in an && condition can
    // be lowered before the condition and retain BVH private storage even in
    // a scene whose solid-transport proof specializes this constant to false.
    if (HYBRID_SOLID_TRANSPORT) {
        if (params.material11.x > 0.5) {
            if (!primary_solid_interface_visible(params.camera0.xyz,input.world_position,
                -view,u32(params.material11.y))) { discard; }
        }
    }
    // Closed volumes contribute the entry interface once. Otherwise the back
    // face would sample and apply the same slab attenuation a second time.
    if (params.material8.w > 0.5 && dot(original_normal, view) <= 0.0 && (REALTIME_TRANSPORT || params.material11.x < 0.5)) { discard; }
    let surface = shade_surface(input);
    var geometric_normal = original_normal;
    if (dot(geometric_normal, view) < 0.0) { geometric_normal = -geometric_normal; }
    let tangent = normalize(input.tangent - geometric_normal * dot(input.tangent, geometric_normal));
    let sign = select(-1.0, 1.0, dot(cross(geometric_normal, tangent), input.bitangent) >= 0.0);
    let bitangent = normalize(cross(geometric_normal, tangent)) * sign;
    // The shared vertex shader has already applied the material UV transform.
    let sample_normal = textureSampleGrad(normal_texture, actor_sampler, input.uv,surface_gradient_x,surface_gradient_y).xyz * 2.0 - 1.0;
    let normal = normalize(tangent * sample_normal.x * params.material0.z +
        bitangent * sample_normal.y * params.material0.z + geometric_normal * sample_normal.z);
    let transmission = clamp(params.material6.x, 0.0, 1.0);
    let ior = clamp(params.material6.y, 1.0, 3.0);
    let incident = -view;
    if (HYBRID_SOLID_TRANSPORT && params.material11.x > 0.5) {
        let transported = trace_geometry_transmission(input.world_position, incident,
            original_normal, ior, params.material7.rgb, params.material6.w, u32(params.material11.y),
            select(0.0, length(input.world_position - params.camera0.xyz), dot(original_normal, incident) > 0.0));
        let layer_transmission = material_sheen_scale(max(dot(normal,view),0.0),max(dot(normal,view),0.0)) *
            (1.0 - params.material10.x * material_coat_fresnel(max(dot(geometric_normal,view),0.0)));
        var transmitted = transported.radiance;
        if (transported.confidence < 1.0) {
            let eta = select(1.0 / ior, ior, dot(original_normal, incident) > 0.0);
            var fallback_direction = refract(incident, normal, eta);
            if (dot(fallback_direction, fallback_direction) < 0.000001) {
                fallback_direction = reflect(incident, normal);
            }
            let fallback = hybrid_environment(input.world_position, fallback_direction, params.material0.y);
            transmitted = mix(fallback, transported.radiance, transported.confidence);
        }
        var reflected_surface = surface.rgb;
        if (dot(original_normal, incident) > 0.0) {
            reflected_surface *= hybrid_beer(params.material7.rgb,
                length(input.world_position - params.camera0.xyz), params.material6.w);
        }
        let coverage = textureSampleGrad(actor_texture,actor_sampler,input.uv,surface_gradient_x,surface_gradient_y).a * input.color.a * params.material4.a * params.style.x;
        return vec4<f32>(transmitted * transmission * layer_transmission + reflected_surface, coverage);
    }
    let inside = refract(incident, normal, 1.0 / ior);
    let thickness = max(params.material6.z, 0.0);
    let slab_distance = thickness / max(-dot(inside, geometric_normal), 0.05);
    let exit = input.world_position + inside * slab_distance;
    let screen_uv = input.pos.xy / params.canvas.xy;
    let scene_depth = transmission_scene_view_depth(screen_uv);
    let exit_depth = dot(exit - params.camera0.xyz, params.camera3.xyz);
    // A parallel-sided slab exits along the original incident ray. Intersect
    // that exit ray with the opaque scene's view-depth plane for perspective
    // displacement; camera right/up determine the pixel offset at every angle.
    let behind_distance = max(scene_depth - exit_depth, 0.0) / max(dot(incident, params.camera3.xyz), 0.05);
    var refracted_uv = transmission_project(exit + incident * behind_distance);
    let border = vec2<f32>(0.5) / params.canvas.xy;
    if (any(refracted_uv < border) || any(refracted_uv > vec2<f32>(1.0) - border) ||
        transmission_scene_view_depth(clamp(refracted_uv, border, vec2<f32>(1.0) - border)) < exit_depth) {
        // Offscreen/foreground samples retain the current view instead of
        // stretching edge texels or dragging a foreground object through glass.
        refracted_uv = screen_uv;
    }
    let remapped = material_channels(textureSampleGrad(metallic_roughness_texture, actor_sampler, input.uv,surface_gradient_x,surface_gradient_y));
    let roughness = clamp(params.material0.y * remapped.y + lighting.surface1.z, 0.045, 1.0);
    let blur_radius = roughness * roughness * 0.006;
    // The HDR underlay has one mip. Explicit level zero preserves its filter
    // while avoiding derivative-dependent work in rejected fragment helpers.
    let scene_sample = transmission_snapshot_sample(refracted_uv,refracted_uv,exit_depth) * 0.4 +
        transmission_snapshot_sample(refracted_uv + vec2<f32>(blur_radius,0.0),refracted_uv,exit_depth) * 0.15 +
        transmission_snapshot_sample(refracted_uv - vec2<f32>(blur_radius,0.0),refracted_uv,exit_depth) * 0.15 +
        transmission_snapshot_sample(refracted_uv + vec2<f32>(0.0,blur_radius),refracted_uv,exit_depth) * 0.15 +
        transmission_snapshot_sample(refracted_uv - vec2<f32>(0.0,blur_radius),refracted_uv,exit_depth) * 0.15;
    var scene_color=scene_sample.rgb;
    if (lighting.environment0.w>0.5) {
        // A camera-hidden sky remains visible to transmission rays. Snapshot
        // alpha identifies uncovered sky while retaining previously composed
        // panes and opaque geometry, including at silhouette edges.
        var sky=sample_environment_background(incident,roughness*roughness)*lighting.environment0.x;
        scene_color+=sky*(1.0-clamp(scene_sample.a,0.0,1.0));
    }
    let attenuation = pow(clamp(params.material7.rgb,vec3<f32>(0.0001),vec3<f32>(1.0)),
        vec3<f32>(slab_distance / max(params.material6.w,0.0001)));
    let ratio = (ior - 1.0) / (ior + 1.0);
    let fresnel = ratio * ratio + (1.0 - ratio * ratio) * pow(1.0 - max(dot(normal,view),0.0),5.0);
    // The surface BRDF already includes Fresnel reflection. Transmitted energy
    // is complementary and attenuation uses the longer refracted optical path.
    let layer_transmission = material_sheen_scale(max(dot(normal,view),0.0),max(dot(normal,view),0.0)) *
        (1.0 - params.material10.x * material_coat_fresnel(max(dot(geometric_normal,view),0.0)));
    var reflected_surface = surface.rgb;
    if (REALTIME_TRANSPORT) {
        let screen_reflection = glass_screen_reflection(input.world_position, normal, view, roughness);
        // Replace only the selected indirect lobe. Direct lights, transmission,
        // distinct clearcoat and sheen remain in their original energy paths.
        reflected_surface = max(reflected_surface +
            (screen_reflection.rgb * surface_screen_reflection_response - surface_screen_reflection_lobe) *
            screen_reflection.a, vec3<f32>(0.0));
    }
    let color = scene_color * attenuation * transmission * (1.0 - fresnel) * layer_transmission + reflected_surface;
    let coverage = textureSampleGrad(actor_texture,actor_sampler,input.uv,surface_gradient_x,surface_gradient_y).a * input.color.a * params.material4.a * params.style.x;
    return vec4<f32>(color,coverage);
}


@fragment
fn fs_transmissive(input: VertexOut) -> @location(0) vec4<f32> {
    return shade_transmissive(input);
}

struct TransparentMrtOutput {
    @location(0) color: vec4<f32>,
    @location(1) reflection: vec4<f32>,
};

@fragment
fn fs_main_mrt(input: VertexOut) -> TransparentMrtOutput {
    params = instance_params[input.instance_id];
    initialize_surface_gradients(input);
    return TransparentMrtOutput(shade_surface(input),vec4<f32>(0.0,0.0,0.0,-0.75));
}

@fragment
fn fs_transmissive_mrt(input: VertexOut) -> TransparentMrtOutput {
    return TransparentMrtOutput(shade_transmissive(input),vec4<f32>(0.0,0.0,0.0,-0.75));
}

@fragment
fn fs_transmissive_planar_cached_mrt(input: VertexOut) -> TransparentMrtOutput {
    return TransparentMrtOutput(shade_transmissive_planar_route(input,true),vec4<f32>(0.0,0.0,0.0,-0.75));
}

@fragment
fn fs_transmissive_planar_full_mrt(input: VertexOut) -> TransparentMrtOutput {
    return TransparentMrtOutput(shade_transmissive_planar_route(input,false),vec4<f32>(0.0,0.0,0.0,-0.75));
}

struct BackgroundVertexOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_background(@builtin(vertex_index) index: u32) -> BackgroundVertexOut {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: BackgroundVertexOut;
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_background(input: BackgroundVertexOut) -> @location(0) vec4<f32> {
    if (lighting.environment2.x < 0.5 || lighting.environment0.w < 0.5) {
        discard;
    }
    let ndc = input.uv * 2.0 - vec2<f32>(1.0);
    let aspect = max(abs(lighting.camera3.w), 0.001);
    let direction = normalize(
        lighting.camera3.xyz +
        lighting.camera1.xyz * ndc.x * aspect +
        lighting.camera2.xyz * -ndc.y
    );
    let color = sample_environment_background(direction, lighting.environment1.y) * lighting.environment1.x;
    var display = color;
    if (lighting.fog1.w > 0.5 && lighting.fog2.z > 0.5 && lighting.fog2.w > 0.5) {
        let horizon = pow(clamp(1.0 - abs(direction.y), 0.0, 1.0), 3.0);
        var sky_fog = horizon * clamp(lighting.fog0.x * 8.0, 0.0, 0.75);
        if (lighting.fog3.w > 0.5) {
            let volume_sample = atmosphere_medium_ray_sample(direction, 100000.0, lighting.camera0.y);
            sky_fog = clamp(
                (1.0 - exp(-lighting.fog0.x * volume_sample.x)) * volume_sample.y,
                0.0,
                0.75,
            );
        }
        display = mix(display, lighting.fog1.rgb, sky_fog);
    }
    return vec4<f32>(display, 1.0);
}


@fragment
fn fs_planar_background(input: BackgroundVertexOut) -> @location(0) vec4<f32> {
    params = instance_params[0u];
    let pixel = input.uv * params.canvas.xy - params.canvas.zw - lighting.preview1.xy;
    let direction = normalize(params.camera3.xyz + params.camera1.xyz * pixel.x / max(params.camera0.w,0.0001)
        - params.camera2.xyz * pixel.y / max(params.camera0.w,0.0001));
    let color = sample_environment_background(direction, 0.0) * lighting.environment0.x * lighting.environment1.w;
    return vec4<f32>(color,1.0);
}
