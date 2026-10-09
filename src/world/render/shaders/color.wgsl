// =========================================
// =========================================
// src/world/render/shaders/color.wgsl

fn white_balance(color: vec3<f32>, kelvin: f32) -> vec3<f32> {
    let temperature = clamp((kelvin - 6500.0) / 6500.0, -0.75, 0.75);
    return color * vec3<f32>(1.0 + temperature * 0.16, 1.0, 1.0 - temperature * 0.16);
}
fn inverse_tone_curve(color: vec3<f32>) -> vec3<f32> {
    let y = pow(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(2.2));
    if (lighting.color0.w > 1.5) {
        let a = vec3<f32>(2.51) - y * 2.43;
        let b = vec3<f32>(0.03) - y * 0.59;
        return (-b + sqrt(b * b + 4.0 * a * y * 0.14)) / (2.0 * a);
    }
    if (lighting.color0.w > 0.5) { return y / max(vec3<f32>(1.0) - y, vec3<f32>(0.0001)); }
    return y;
}

// Display-locked colors enter HDR before the final scene grade. Undo that
// grade too, so ordinary exposure/white balance does not tint fixed colors.
fn inverse_display_curve(color: vec3<f32>) -> vec3<f32> {
    var linear = inverse_tone_curve(color);
    let saturation = max(lighting.surface1.w, 0.0001);
    let luminance = dot(linear, vec3<f32>(0.2126,0.7152,0.0722));
    linear = (linear - vec3<f32>(luminance) * (1.0 - saturation)) / saturation;
    linear = (linear - vec3<f32>(0.18)) / max(lighting.color0.z, 0.0001) + vec3<f32>(0.18);
    let temperature = clamp((lighting.color0.y - 6500.0) / 6500.0, -0.75, 0.75);
    return linear / max(lighting.color0.x, 0.0001) /
        vec3<f32>(1.0 + temperature * 0.16, 1.0, 1.0 - temperature * 0.16);
}

fn display_transform(color: vec3<f32>) -> vec3<f32> {
    var adjusted = white_balance(max(color * lighting.color0.x, vec3<f32>(0.0)), lighting.color0.y);
    adjusted = (adjusted - vec3<f32>(0.18)) * lighting.color0.z + vec3<f32>(0.18);
    if (lighting.surface1.w != 1.0) {
        adjusted = mix(vec3<f32>(dot(adjusted, vec3<f32>(0.2126,0.7152,0.0722))), adjusted, lighting.surface1.w);
    }
    return max(adjusted, vec3<f32>(0.0));
}
