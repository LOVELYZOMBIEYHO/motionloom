// =========================================
// =========================================
// src/world/render/shaders/lighting.wgsl

fn distribution_ggx(normal: vec3<f32>, halfway: vec3<f32>, roughness: f32) -> f32 {
    let alpha = roughness * roughness;
    let alpha2 = alpha * alpha;
    let n_dot_h = max(dot(normal, halfway), 0.0);
    let denominator = n_dot_h * n_dot_h * (alpha2 - 1.0) + 1.0;
    return alpha2 / max(3.14159265 * denominator * denominator, 0.000001);
}

fn geometry_schlick_ggx(n_dot_v: f32, roughness: f32) -> f32 {
    let k = ((roughness + 1.0) * (roughness + 1.0)) / 8.0;
    return n_dot_v / max(n_dot_v * (1.0 - k) + k, 0.000001);
}

fn geometry_smith(normal: vec3<f32>, view: vec3<f32>, light: vec3<f32>, roughness: f32) -> f32 {
    return geometry_schlick_ggx(max(dot(normal, view), 0.0), roughness) *
        geometry_schlick_ggx(max(dot(normal, light), 0.0), roughness);
}

fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

fn direct_base_pbr(
    normal: vec3<f32>,
    view: vec3<f32>,
    light: vec3<f32>,
    radiance: vec3<f32>,
    base_color: vec3<f32>,
    metallic: f32,
    roughness: f32,
    f0: vec3<f32>,
    face_band: f32,
) -> vec3<f32> {
    let halfway = normalize(view + light);
    let n_dot_l = max(dot(normal, light), 0.0);
    let n_dot_v = max(dot(normal, view), 0.0);
    let distribution = distribution_ggx(normal, halfway, roughness);
    let geometry = geometry_smith(normal, view, light, roughness);
    let fresnel = fresnel_schlick(max(dot(halfway, view), 0.0), f0);
    let specular = (distribution * geometry * fresnel) / max(4.0 * n_dot_v * n_dot_l, 0.0001);
    let diffuse_weight = (vec3<f32>(1.0) - fresnel) * (1.0 - metallic);
    let diffuse = diffuse_weight * base_color / 3.14159265;
    // Keep alternate visibility and art-directed shading inside an explicit
    // static branch; uniform conditions alone retain physical-only overhead.
    if (!PHYSICAL_STYLE_ONLY) {
        if (lighting.surface0.x < -0.5) {
            // The filmic path uses correlated Smith visibility and Lambert diffuse for glTF.
            let a2 = pow(roughness, 4.0);
            let gv = n_dot_l * sqrt(n_dot_v * n_dot_v * (1.0 - a2) + a2);
            let gl = n_dot_v * sqrt(n_dot_l * n_dot_l * (1.0 - a2) + a2);
            let visibility = 0.5 / max(gv + gl, 0.000001);
            return (base_color * (1.0 - metallic) / 3.14159265 + distribution * visibility * fresnel) * radiance * n_dot_l;
        }
        if (lighting.surface0.x > 3.5) {
            return shade_cel(
                normal, light, radiance, base_color, specular, n_dot_l, face_band,
            );
        }
        if (lighting.surface0.x > 0.5) {
            var intensity = stylized_intensity(normal, light);
            if (lighting.surface0.x > 1.5 && lighting.surface0.x < 2.5) {
                intensity = toon_intensity(intensity);
            }
            return shade_stylized(base_color, specular, radiance, intensity, n_dot_l);
        }
    }
    if (lighting.surface1.y != 1.0 || lighting.surface0.z != 0.0) {
        return shade_wrapped_physical(normal, light, diffuse, specular, radiance, n_dot_l);
    }
    return shade_physical(diffuse, specular, radiance, n_dot_l);
}

// Clearcoat has its own geometric normal, not the underlying normal map.
var<private> material_geometric_normal: vec3<f32>;

fn sheen_directional_albedo(no_v: f32, roughness: f32) -> f32 {
    return textureSampleLevel(environment_brdf_texture, environment_brdf_sampler,
        vec2<f32>(clamp(no_v, 0.0, 1.0), clamp(roughness, 0.04, 1.0)), 0.0).b;
}

fn sheen_brdf(no_v: f32, no_l: f32, no_h: f32, roughness: f32) -> f32 {
    if (no_v <= 0.0 || no_l <= 0.0) { return 0.0; }
    let inverse_alpha = 1.0 / pow(clamp(roughness, 0.04, 1.0), 2.0);
    let distribution = (2.0 + inverse_alpha) * pow(max(1.0 - no_h * no_h, 0.0),
        0.5 * inverse_alpha) / 6.2831853;
    return distribution / max(4.0 * (no_v + no_l - no_v * no_l), 0.000001);
}

fn material_sheen_scale(no_v: f32, no_l: f32) -> f32 {
    let color_weight = max(params.material9.r, max(params.material9.g, params.material9.b));
    if (color_weight <= 0.0) { return 1.0; }
    return 1.0 - color_weight * max(sheen_directional_albedo(no_v, params.material9.w),
        sheen_directional_albedo(no_l, params.material9.w));
}

fn material_coat_fresnel(cosine: f32) -> f32 {
    return 0.04 + 0.96 * pow(clamp(1.0 - cosine, 0.0, 1.0), 5.0);
}

fn coat_distribution_ggx(normal: vec3<f32>, halfway: vec3<f32>, roughness: f32) -> f32 {
    let a2 = pow(roughness, 4.0);
    let no_h = max(dot(normal, halfway), 0.0);
    let denominator = no_h * no_h * (a2 - 1.0) + 1.0;
    // The base BRDF's legacy floor is retained above for compatibility. A
    // coating needs the narrower valid peak down to authored roughness 0.04.
    return a2 / max(3.14159265 * denominator * denominator, 0.000000000001);
}

fn direct_pbr(normal: vec3<f32>, view: vec3<f32>, light: vec3<f32>, radiance: vec3<f32>,
    base_color: vec3<f32>, metallic: f32, roughness: f32, f0: vec3<f32>, face_band: f32) -> vec3<f32> {
    let base = direct_base_pbr(normal, view, light, radiance, base_color, metallic, roughness, f0, face_band);
    if (all(params.material9.rgb == vec3<f32>(0.0)) && params.material10.x <= 0.0) { return base; }
    let halfway = normalize(view + light);
    let no_v = max(dot(normal, view), 0.0);
    let no_l = max(dot(normal, light), 0.0);
    let no_h = max(dot(normal, halfway), 0.0);
    let specular_strength = select(1.0, lighting.surface1.y, lighting.surface0.x >= -0.5);
    let sheen = params.material9.rgb * sheen_brdf(no_v, no_l, no_h, params.material9.w)
        * radiance * no_l * specular_strength;
    var result = base * material_sheen_scale(no_v, no_l) + sheen;
    if (params.material10.x > 0.0) {
        let coat_normal = material_geometric_normal;
        let coat_v = max(dot(coat_normal, view), 0.0);
        let coat_l = max(dot(coat_normal, light), 0.0);
        let coat_roughness = params.material10.y;
        let a2 = pow(coat_roughness, 4.0);
        let gv = coat_l * sqrt(coat_v * coat_v * (1.0 - a2) + a2);
        let gl = coat_v * sqrt(coat_l * coat_l * (1.0 - a2) + a2);
        let visibility = 0.5 / max(gv + gl, 0.000001);
        let fresnel = material_coat_fresnel(max(dot(halfway, view), 0.0));
        let coat = params.material10.x * fresnel;
        result = result * (1.0 - coat) + radiance * coat_l * coat
            * coat_distribution_ggx(coat_normal, halfway, coat_roughness) * visibility * specular_strength;
    }
    return result;
}

// A bounded deterministic quadrature samples actual source/probe radiance for
// Charlie, rather than applying a GGX-prefiltered sheen approximation. The
// normalization preserves the LUT's energy under constant hemispherical light.
fn material_sheen_environment(position: vec3<f32>, normal: vec3<f32>, view: vec3<f32>) -> vec4<f32> {
    if (all(params.material9.rgb == vec3<f32>(0.0))) { return vec4<f32>(0.0); }
    let no_v = max(dot(normal, view), 0.0);
    let axis = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(normal.y) > 0.9);
    let tangent = normalize(cross(axis, normal));
    let bitangent = cross(normal, tangent);
    var result = vec3<f32>(0.0); var total = 0.0; var local_coverage = 0.0;
    for (var i = 0u; i < 16u; i = i + 1u) {
        let no_l = (f32(i) + 0.5) / 16.0;
        let phi = f32(i) * 2.3999632;
        let radius = sqrt(1.0 - no_l * no_l);
        let light = tangent * (cos(phi) * radius) + bitangent * (sin(phi) * radius) + normal * no_l;
        let halfway = normalize(view + light);
        let weight = sheen_brdf(no_v, no_l, max(dot(normal, halfway), 0.0), params.material9.w) * no_l;
        let local = sample_local_radiance(position, light, 0.0);
        let global = sample_environment(light, 0.0) * lighting.environment0.x * lighting.environment1.w;
        result += mix(global, local.rgb, local.a) * weight;
        local_coverage += local.a * weight; total += weight;
    }
    if (total <= 0.00000001) { return vec4<f32>(0.0); }
    return vec4<f32>(result / total * params.material9.rgb * sheen_directional_albedo(no_v, params.material9.w),
        local_coverage / total);
}
