// Portable SH irradiance grids with directional distance-moment visibility.
struct BakedProbeData { vectors: array<vec4<f32>>, };
@group(1) @binding(8) var<storage, read> baked_probes: BakedProbeData;
@group(1) @binding(9) var local_reflection_texture: texture_2d_array<f32>;
@group(1) @binding(10) var local_reflection_sampler: sampler;

// Most shots hold an authored lighting endpoint. Select by index before the
// storage load so those frames do not fetch both complete probe payloads.
fn baked_blended_vector(first: u32, second: u32) -> vec4<f32> {
    if (lighting.baked0.y == 0.0) { return baked_probes.vectors[first]; }
    if (lighting.baked0.y == 1.0) { return baked_probes.vectors[second]; }
    return mix(baked_probes.vectors[first], baked_probes.vectors[second], lighting.baked0.y);
}

fn baked_local_radiance(uv: vec2<f32>, layer: i32, mip: f32) -> vec3<f32> {
    if (lighting.baked0.y == 0.0) {
        return textureSampleLevel(local_reflection_texture, local_reflection_sampler, uv, layer, mip).rgb;
    }
    if (lighting.baked0.y == 1.0) {
        return textureSampleLevel(local_reflection_texture, local_reflection_sampler, uv, layer + 1, mip).rgb;
    }
    return mix(
        textureSampleLevel(local_reflection_texture, local_reflection_sampler, uv, layer, mip).rgb,
        textureSampleLevel(local_reflection_texture, local_reflection_sampler, uv, layer + 1, mip).rgb,
        lighting.baked0.y);
}

fn baked_volume_weight(position: vec3<f32>, minimum: vec3<f32>, maximum: vec3<f32>) -> f32 {
    // Grid endpoints are inset from walls/floors to keep probes out of solids.
    // A small receiver shell reaches those room surfaces; depth moments still
    // reject interpolation through the partition. Vertical padding reaches
    // floors and ceilings without moving a probe into the concrete slab.
    let padding=vec3<f32>(0.12,0.30,0.12);
    if (any(position < minimum-padding) || any(position > maximum+padding)) { return 0.0; }
    let edge = min(position - (minimum-padding), maximum+padding - position);
    return smoothstep(0.0, 0.03, min(edge.x, min(edge.y, edge.z)));
}

fn baked_oct_uv(direction: vec3<f32>) -> vec2<f32> {
    var n = direction / max(dot(abs(direction), vec3<f32>(1.0)), 0.00001);
    if (n.z < 0.0) { n = vec3<f32>((vec2<f32>(1.0) - abs(n.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0)), n.z); }
    return n.xy * 0.5 + 0.5;
}

fn baked_depth_moments(probe: u32, direction: vec3<f32>) -> vec2<f32> {
    // Interpolate raw first/second moments before the visibility test. Nearest
    // angular cells create polygonal illumination bands on smooth room walls.
    let grid=clamp(baked_oct_uv(direction)*8.0-0.5,vec2<f32>(0.0),vec2<f32>(7.0));
    let first=vec2<u32>(floor(grid)); let next=min(first+vec2<u32>(1u),vec2<u32>(7u));
    let alpha=fract(grid);
    var result=vec2<f32>(0.0);
    for (var corner=0u;corner<4u;corner=corner+1u) {
        let delta=vec2<u32>(corner&1u,(corner>>1u)&1u);
        let cell=select(first,next,delta==vec2<u32>(1u));
        let factors=select(vec2<f32>(1.0)-alpha,alpha,delta==vec2<u32>(1u));
        let index=cell.y*8u+cell.x;
        result+=baked_blended_vector(probe+20u+index,probe+84u+index).xy*factors.x*factors.y;
    }
    return result;
}

// The receiver basis is shared by every corner and overlapping room volume.
fn baked_receiver_basis(normal: vec3<f32>) -> array<f32,9> {
    let n = normal;
    return array<f32,9>(0.2820948,0.48860252*n.y,0.48860252*n.z,0.48860252*n.x,
        1.0925485*n.x*n.y,1.0925485*n.y*n.z,0.31539157*(3.0*n.z*n.z-1.0),
        1.0925485*n.x*n.z,0.54627424*(n.x*n.x-n.y*n.y));
}
fn baked_sh(probe: u32, basis: array<f32,9>) -> vec3<f32> {
    var e = vec3<f32>(0.0);
    for (var k=0u; k<9u; k=k+1u) {
        e += baked_blended_vector(probe+2u+k,probe+11u+k).rgb * basis[k];
    }
    return max(e,vec3<f32>(0.0));
}

// RGB=irradiance E, alpha=room coverage. Invalid/occluded probes do not
// brighten the receiver; global IBL is replaced only where the grid is valid.
fn sample_baked_irradiance(position: vec3<f32>, normal: vec3<f32>) -> vec4<f32> {
    if (lighting.baked0.x < 0.5 || lighting.baked0.z <= 0.0) { return vec4<f32>(0.0); }
    let receiver_basis = baked_receiver_basis(normal);
    var result=vec3<f32>(0.0); var volume_total=0.0; var coverage=0.0;
    for (var volume=0u; volume<32u; volume=volume+1u) {
        if (volume>=u32(lighting.baked0.x)) { break; }
        let header=volume*4u;
        let lower=baked_probes.vectors[header]; let upper=baked_probes.vectors[header+1u];
        let room_weight=baked_volume_weight(position,lower.xyz,upper.xyz);
        if (room_weight<=0.0) { continue; }
        let counts=vec3<u32>(baked_probes.vectors[header+2u].xyz);
        let grid=clamp((position-lower.xyz)/(upper.xyz-lower.xyz),vec3<f32>(0.0),vec3<f32>(1.0))*vec3<f32>(counts-vec3<u32>(1u));
        let first=min(vec3<u32>(floor(grid)),counts-vec3<u32>(2u)); let alpha=grid-vec3<f32>(first);
        var irradiance=vec3<f32>(0.0); var total=0.0; var available=0.0; var normal_total=0.0;
        for (var corner=0u;corner<8u;corner=corner+1u) {
            let delta=vec3<u32>(corner&1u,(corner>>1u)&1u,(corner>>2u)&1u);
            let index=first+delta;
            let probe=u32(lower.w)+(index.x+index.y*counts.x+index.z*counts.x*counts.y)*148u;
            let probe_data=baked_blended_vector(probe,probe+1u);
            let valid=probe_data.w;
            let factors=select(vec3<f32>(1.0)-alpha,alpha,delta==vec3<u32>(1u));
            let grid_weight=factors.x*factors.y*factors.z*valid;
            if (grid_weight<=0.000001) { continue; }
            let probe_position=probe_data.xyz;
            // Receiver normal bias and directional Chebyshev visibility stop
            // a bright probe on the other side of a thin interior partition.
            let distance_vector=position+normal*0.035-probe_position;
            let distance=length(distance_vector); let direction=distance_vector/max(distance,0.00001);
            // Terminal transmission is diagnostic; receiver occlusion uses depth.
            let moments=baked_depth_moments(probe,direction);
            let excess=max(distance-moments.x-0.035,0.0);
            let variance=max(moments.y-moments.x*moments.x,0.0004);
            var visible=1.0;
            if (excess>0.0) { visible=pow(variance/(variance+excess*excess),3.0); }
            let normal_weight=pow(max(dot(normal,-direction)*0.5+0.5,0.04),2.0);
            let weight=grid_weight*visible*normal_weight;
            normal_total+=grid_weight*normal_weight;
            irradiance+=baked_sh(probe,receiver_basis)*weight; total+=weight; available+=grid_weight;
        }
        if (available>0.00001) {
            // Completely occluded valid cells stay dark rather than falling
            // back to unoccluded global IBL. A tiny visibility tail cannot
            // normalize into full bright cross-wall radiance.
            let confidence=clamp(total/max(normal_total,0.00001)*4.0,0.0,1.0);
            result+=irradiance/max(total,0.00001)*confidence*room_weight;
            volume_total+=room_weight;
            coverage=max(coverage,room_weight*clamp(available,0.0,1.0));
        }
    }
    if (volume_total<=0.0) { return vec4<f32>(0.0); }
    return vec4<f32>(result/volume_total*max(lighting.baked0.z,1.0),coverage*min(lighting.baked0.z,1.0));
}

fn baked_box_direction(position: vec3<f32>, direction: vec3<f32>, minimum: vec3<f32>, maximum: vec3<f32>, probe: vec3<f32>) -> vec3<f32> {
    let safe=select(vec3<f32>(-0.000001),vec3<f32>(0.000001),direction>=vec3<f32>(0.0));
    let d=select(safe,direction,abs(direction)>vec3<f32>(0.000001));
    let face=select(minimum,maximum,direction>=vec3<f32>(0.0));
    let distances=(face-position)/d;
    let t=max(0.0,min(distances.x,min(distances.y,distances.z)));
    return normalize(position+direction*t-probe);
}

fn sample_local_specular(position: vec3<f32>, reflected: vec3<f32>, roughness: f32, f0: vec3<f32>, no_v: f32) -> vec4<f32> {
    if (lighting.baked0.x<0.5 || lighting.baked0.w<=0.0) { return vec4<f32>(0.0); }
    var radiance=vec3<f32>(0.0); var total=0.0; var coverage=0.0;
    for (var volume=0u; volume<32u;volume=volume+1u) {
        if (volume>=u32(lighting.baked0.x)) { break; }
        let h=volume*4u; let lower=baked_probes.vectors[h]; let upper=baked_probes.vectors[h+1u];
        let capture=baked_probes.vectors[h+3u];
        let weight=baked_volume_weight(position,lower.xyz,upper.xyz)*capture.w;
        if (weight<=0.0) {continue;}
        let d=baked_box_direction(position,reflected,lower.xyz,upper.xyz,capture.xyz);
        let uv=vec2<f32>(0.5+atan2(d.z,d.x)/6.2831853,acos(clamp(d.y,-1.0,1.0))/3.14159265);
        let mip=roughness*f32(textureNumLevels(local_reflection_texture)-1u);
        let layer=i32(upper.w);
        radiance+=baked_local_radiance(uv,layer,mip)*weight; total+=weight;coverage=max(coverage,weight);
    }
    if (total<=0.0) {return vec4<f32>(0.0);}
    let lut=textureSampleLevel(environment_brdf_texture,environment_brdf_sampler,vec2<f32>(clamp(no_v,0.001,0.999),roughness),0.0).rg;
    return vec4<f32>(radiance/total*(f0*lut.x+lut.y)*max(lighting.baked0.w,1.0),coverage*min(lighting.baked0.w,1.0));
}

// Raw radiance for lobes other than GGX. No split-sum or Fresnel is applied.
// lod_fraction selects the existing roughness chain; zero is source radiance.
fn sample_local_radiance(position: vec3<f32>, direction: vec3<f32>, lod_fraction: f32) -> vec4<f32> {
    if (lighting.baked0.x < 0.5 || lighting.baked0.w <= 0.0) { return vec4<f32>(0.0); }
    var radiance = vec3<f32>(0.0); var total = 0.0; var coverage = 0.0;
    for (var volume = 0u; volume < 32u; volume = volume + 1u) {
        if (volume >= u32(lighting.baked0.x)) { break; }
        let h = volume * 4u;
        let lower = baked_probes.vectors[h]; let upper = baked_probes.vectors[h + 1u];
        let capture = baked_probes.vectors[h + 3u];
        let weight = baked_volume_weight(position, lower.xyz, upper.xyz) * capture.w;
        if (weight <= 0.0) { continue; }
        let d = baked_box_direction(position, direction, lower.xyz, upper.xyz, capture.xyz);
        let uv = vec2<f32>(0.5 + atan2(d.z, d.x) / 6.2831853, acos(clamp(d.y, -1.0, 1.0)) / 3.14159265);
        let mip = clamp(lod_fraction, 0.0, 1.0) * f32(textureNumLevels(local_reflection_texture) - 1u);
        let layer = i32(upper.w);
        radiance += baked_local_radiance(uv,layer,mip) * weight;
        total += weight; coverage = max(coverage, weight);
    }
    if (total <= 0.0) { return vec4<f32>(0.0); }
    return vec4<f32>(radiance / total * max(lighting.baked0.w, 1.0), coverage * min(lighting.baked0.w, 1.0));
}
