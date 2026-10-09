// Reflection-only HDR history. Shared Light/Lighting declarations are prepended.
struct ReflectionHistoryControl { value: vec4<f32>, };
@group(0) @binding(0) var<uniform> lighting: Lighting;
@group(0) @binding(1) var current_hdr: texture_2d<f32>;
@group(0) @binding(2) var current_reflection: texture_2d<f32>;
@group(0) @binding(3) var current_gbuffer: texture_2d<f32>;
@group(0) @binding(4) var previous_reflection: texture_2d<f32>;
@group(0) @binding(5) var previous_metadata: texture_2d<f32>;
@group(0) @binding(6) var current_depth: texture_depth_2d;
@group(0) @binding(7) var current_material: texture_2d<f32>;
@group(0) @binding(8) var history_sampler: sampler;
@group(0) @binding(9) var<uniform> control: ReflectionHistoryControl;

struct ReflectionHistoryVertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
struct ReflectionHistoryOutput {
    @location(0) hdr: vec4<f32>,
    @location(1) reflection: vec4<f32>,
    // Octahedral normal, roughness, view distance; zero distance means no surface.
    @location(2) metadata: vec4<f32>,
};

@vertex
fn vs_reflection_history(@builtin(vertex_index) index: u32) -> ReflectionHistoryVertex {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var output: ReflectionHistoryVertex;
    output.position = vec4<f32>(x*2.0-1.0, 1.0-y*2.0, 0.0, 1.0);
    output.uv = vec2<f32>(x,y);
    return output;
}

fn history_normal(encoded: vec2<f32>) -> vec3<f32> {
    let xy = encoded*2.0-1.0;
    var normal = vec3<f32>(xy,1.0-abs(xy.x)-abs(xy.y));
    if (normal.z < 0.0) {
        normal = vec3<f32>((vec2<f32>(1.0)-abs(normal.yx))*sign(normal.xy),normal.z);
    }
    return normalize(normal);
}

fn history_view_distance(depth: f32) -> f32 {
    let near = lighting.camera1.w;
    let far = max(lighting.camera2.w,near+0.001);
    if (lighting.camera3.w < 0.0) { return far-depth*(far-near); }
    return near*far/max(near+depth*(far-near),0.000001);
}

fn history_expected_previous_distance(uv: vec2<f32>, distance: f32, size: vec2<f32>) -> f32 {
    let stable_uv = uv-lighting.preview1.xy/size;
    let ndc = vec2<f32>(stable_uv.x*2.0-1.0,1.0-stable_uv.y*2.0);
    let projection_distance = select(distance,1.0,lighting.camera3.w < 0.0);
    let position = lighting.camera0.xyz+lighting.camera3.xyz*distance
        +lighting.camera1.xyz*ndc.x*projection_distance*size.x/max(2.0*lighting.camera0.w,0.000001)
        +lighting.camera2.xyz*ndc.y*projection_distance*size.y/max(2.0*lighting.camera0.w,0.000001);
    return dot(position-lighting.previous_camera0.xyz,lighting.previous_camera3.xyz);
}

fn history_path_class(encoded: f32) -> u32 {
    if (encoded < -1.5) { return 2u; }
    if (encoded < -0.5) { return 1u; }
    return 0u;
}

fn history_path_matches(current: f32, previous: f32) -> bool {
    let kind = history_path_class(current);
    if (kind != history_path_class(previous)) { return false; }
    if (kind == 2u) {
        let distance = max(-current-2.0,0.0);
        let old_distance = max(-previous-2.0,0.0);
        return abs(distance-old_distance) <= max(0.04,distance*0.04);
    }
    return abs(current-previous) <= 0.15;
}

@fragment
fn fs_reflection_history(input: ReflectionHistoryVertex) -> ReflectionHistoryOutput {
    let dimensions_i = textureDimensions(current_hdr);
    let dimensions = vec2<f32>(dimensions_i);
    let pixel = vec2<i32>(clamp(input.uv*dimensions,vec2<f32>(0.0),dimensions-1.0));
    let hdr = textureLoad(current_hdr,pixel,0);
    let reflection = textureLoad(current_reflection,pixel,0);
    let gbuffer = textureLoad(current_gbuffer,pixel,0);
    let material = textureLoad(current_material,pixel,0);
    let depth = textureLoad(current_depth,pixel,0);
    let distance = select(0.0,history_view_distance(depth),depth > 0.000001);
    var output: ReflectionHistoryOutput;
    output.hdr = hdr;
    output.reflection = reflection;
    output.metadata = vec4<f32>(gbuffer.xy,material.x,min(distance,65500.0));
    if (control.value.x < 0.5 || lighting.reflection0.z > 0.5 || distance <= 0.0
        || hdr.a < 0.00001 || length(reflection.rgb) < 0.000001) { return output; }

    // Our history is on the raster grid. Velocities contain physical motion,
    // so add the previous-minus-current jitter exactly once for reprojecting it.
    let velocity = gbuffer.zw;
    let previous_uv = input.uv-velocity+(lighting.preview1.zw-lighting.preview1.xy)/dimensions;
    if (any(previous_uv <= vec2<f32>(0.001)) || any(previous_uv >= vec2<f32>(0.999))) { return output; }
    let previous_pixel = vec2<i32>(clamp(previous_uv*dimensions,vec2<f32>(0.0),dimensions-1.0));
    let metadata = textureLoad(previous_metadata,previous_pixel,0);
    let previous_identity = textureLoad(previous_reflection,previous_pixel,0);
    let expected = history_expected_previous_distance(input.uv,distance,dimensions);
    if (metadata.w <= 0.0 || expected <= 0.0
        || abs(metadata.w-expected) > max(expected*0.018,0.012)
        || dot(history_normal(gbuffer.xy),history_normal(metadata.xy)) < 0.92
        || abs(material.x-metadata.z) > 0.06
        || !history_path_matches(reflection.a,previous_identity.a)) { return output; }

    let previous = textureSampleLevel(previous_reflection,history_sampler,previous_uv,0.0).rgb;
    var minimum = reflection.rgb;
    var maximum = reflection.rgb;
    for (var y=-1; y<=1; y=y+1) {
        for (var x=-1; x<=1; x=x+1) {
            let sample_pixel = clamp(pixel+vec2<i32>(x,y),vec2<i32>(0),vec2<i32>(dimensions_i)-1);
            let sample = textureLoad(current_reflection,sample_pixel,0);
            let sample_gbuffer = textureLoad(current_gbuffer,sample_pixel,0);
            if (history_path_matches(reflection.a,sample.a)
                && dot(history_normal(gbuffer.xy),history_normal(sample_gbuffer.xy)) > 0.9) {
                minimum = min(minimum,sample.rgb);
                maximum = max(maximum,sample.rgb);
            }
        }
    }
    let padding = vec3<f32>(0.002)+maximum*0.02;
    let clamped = clamp(previous,minimum-padding,maximum+padding);
    let current_luma = dot(reflection.rgb,vec3<f32>(0.2126,0.7152,0.0722));
    let old_luma = dot(previous,vec3<f32>(0.2126,0.7152,0.0722));
    let lighting_change = abs(current_luma-old_luma)/max(max(current_luma,old_luma),0.02);
    let speed = length(velocity*dimensions);
    // Geometry changes/cuts reject the frame globally; local light/reactive
    // changes and motion lower confidence independently of display TAA.
    let weight = mix(0.78,0.12,smoothstep(0.25,8.0,speed))
        *(1.0-clamp(material.w,0.0,1.0))
        *(1.0-smoothstep(0.10,0.30,lighting_change));
    let filtered = mix(reflection.rgb,clamped,weight);
    output.hdr = vec4<f32>(max(hdr.rgb+filtered-reflection.rgb,vec3<f32>(0.0)),hdr.a);
    output.reflection = vec4<f32>(filtered,reflection.a);
    return output;
}
