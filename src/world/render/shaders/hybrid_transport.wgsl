// src/world/render/shaders/hybrid_transport.wgsl
// Portable camera-independent BVH. All radiance remains scene-linear HDR.
const REALTIME_TRANSPORT: bool = false;
// The CPU substitutes false only when every evaluated material is slab
// compatible and none has the mixed path's exact 0.001 transmission boundary.
// Keep medium stacks out of that shader's reachable call graph.
override HYBRID_SOLID_TRANSPORT: bool = true;
// A complete-scene proof can remove casting-glass visibility from primary
// and reflected receivers while retaining opaque shadow and optical paths.
override PRIMARY_GLASS_SHADOWS_ENABLED: bool = true;
// A complete evaluated-scene proof may remove only strict hit-alpha tests.
// Full surface alpha and every optical/shadow path retain their original data.
override HYBRID_ALPHA_EVALUATION_ENABLED: bool = true;
@group(1) @binding(20) var<storage, read> hybrid_scene: array<vec4<f32>>;

var<private> hybrid_reflection_hit_distance: f32;
// One invocation shares work across all lobes, lights, interfaces and bounces.
// Initialization is separate from remaining work: an exhausted zero must never
// reset the budget when a later query starts. This also covers planar captures.
var<private> hybrid_work_initialized: bool;
var<private> hybrid_work_remaining: u32;
var<private> hybrid_work_exhausted: bool;

fn hybrid_consume_node_visit() -> bool {
    if (!hybrid_work_initialized) {
        hybrid_work_initialized = true;
        hybrid_work_remaining = 4096u;
    }
    if (hybrid_work_remaining == 0u) {
        hybrid_work_exhausted = true;
        return false;
    }
    hybrid_work_remaining -= 1u;
    return true;
}

struct HybridHit {
    distance: f32,
    triangle: u32,
    barycentric: vec3<f32>,
    valid: bool,
    exhausted: bool,
};
struct HybridSurface {
    position: vec3<f32>,
    normal: vec3<f32>,
    geometric_normal: vec3<f32>,
    color: vec3<f32>,
    alpha: f32,
    metallic: f32,
    roughness: f32,
    transmission: f32,
    ior: f32,
    emissive: vec3<f32>,
    unlit: f32,
    f0: vec3<f32>,
    attenuation: vec3<f32>,
    attenuation_distance: f32,
    solid: bool,
    closed: bool,
    coat: f32,
    coat_roughness: f32,
    object: u32,
    exposure: f32,
    cast_shadow: bool,
    receive_shadow: bool,
};
struct HybridTransportResult {
    radiance: vec3<f32>,
    confidence: f32,
    distance: f32,
    interface_count: u32,
};
// Visibility needs boundary optics, not a shaded PBR surface. Keeping this
// payload small also avoids carrying unrelated texture/BRDF values per light.
struct HybridVisibilitySurface {
    position: vec3<f32>,
    geometric_normal: vec3<f32>,
    transmission: f32,
    ior: f32,
    attenuation: vec3<f32>,
    attenuation_distance: f32,
    thickness: f32,
    solid: bool,
    closed: bool,
    object: u32,
    cast_shadow: bool,
};
fn hybrid_epsilon(position: vec3<f32>) -> f32 {
    return max(0.00005, max(abs(position.x), max(abs(position.y), abs(position.z))) * 0.000002);
}
fn hybrid_srgb_decode(encoded: vec3<f32>) -> vec3<f32> {
    return select(pow((encoded + 0.055) / 1.055, vec3<f32>(2.4)), encoded / 12.92, encoded <= vec3<f32>(0.04045));
}
fn hybrid_material(triangle: u32, slot: u32) -> vec4<f32> {
    let triangle_base = u32(hybrid_scene[0].z) + triangle * 11u;
    let material_id = u32(hybrid_scene[triangle_base].w);
    return hybrid_scene[u32(hybrid_scene[1].x) + material_id * 10u + slot];
}
fn hybrid_texel(tile: vec4<f32>, x: i32, y: i32, decode_color: bool) -> vec4<f32> {
    let width = max(i32(tile.y), 1);
    let height = max(i32(tile.z), 1);
    // Integer wrapping mirrors the main material sampler, including negative UVs.
    let wrapped_x = ((x % width) + width) % width;
    let wrapped_y = ((y % height) + height) % height;
    let index = u32(wrapped_y * width + wrapped_x);
    let packed = bitcast<u32>(hybrid_scene[u32(hybrid_scene[1].z) + u32(tile.x) + index / 4u][index % 4u]);
    let channels = vec4<f32>(f32(packed & 255u), f32((packed >> 8u) & 255u), f32((packed >> 16u) & 255u), f32(packed >> 24u)) / 255.0;
    return vec4<f32>(select(channels.rgb, hybrid_srgb_decode(channels.rgb), decode_color), channels.a);
}
fn hybrid_texture(tile: vec4<f32>, uv: vec2<f32>, decode_color: bool) -> vec4<f32> {
    if(tile.y==1.0 && tile.z==1.0) {return hybrid_texel(tile,0,0,decode_color);}
    let p = uv * tile.yz - 0.5;
    let pixel = vec2<i32>(floor(p));
    let f = fract(p);
    let a = mix(hybrid_texel(tile, pixel.x, pixel.y, decode_color), hybrid_texel(tile, pixel.x + 1, pixel.y, decode_color), f.x);
    let b = mix(hybrid_texel(tile, pixel.x, pixel.y + 1, decode_color), hybrid_texel(tile, pixel.x + 1, pixel.y + 1, decode_color), f.x);
    return mix(a, b, f.y);
}
fn hybrid_texel_alpha(tile: vec4<f32>, x: i32, y: i32) -> f32 {
    let width = max(i32(tile.y), 1);
    let height = max(i32(tile.z), 1);
    let wrapped_x = ((x % width) + width) % width;
    let wrapped_y = ((y % height) + height) % height;
    let index = u32(wrapped_y * width + wrapped_x);
    let packed = bitcast<u32>(hybrid_scene[u32(hybrid_scene[1].z) + u32(tile.x) + index / 4u][index % 4u]);
    return f32(packed >> 24u) / 255.0;
}
fn hybrid_texture_alpha(tile: vec4<f32>, uv: vec2<f32>) -> f32 {
    if(tile.y==1.0 && tile.z==1.0) {return hybrid_texel_alpha(tile,0,0);}
    let p = uv * tile.yz - 0.5;
    let pixel = vec2<i32>(floor(p));
    let f = fract(p);
    let a = mix(hybrid_texel_alpha(tile,pixel.x,pixel.y),hybrid_texel_alpha(tile,pixel.x+1,pixel.y),f.x);
    let b = mix(hybrid_texel_alpha(tile,pixel.x,pixel.y+1),hybrid_texel_alpha(tile,pixel.x+1,pixel.y+1),f.x);
    return mix(a,b,f.y);
}
fn hybrid_channel(sampled: vec4<f32>, packed: u32) -> f32 {
    let channel = packed & 7u;
    var value = sampled.r;
    if (channel == 1u) { value = sampled.g; }
    if (channel == 2u) { value = sampled.b; }
    if (channel == 3u) { value = sampled.a; }
    if (channel == 4u) { value = dot(sampled.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)); }
    return select(value, 1.0 - value, (packed & 8u) != 0u);
}
fn hybrid_uv(hit: HybridHit) -> vec2<f32> {
    let base = u32(hybrid_scene[0].z) + hit.triangle * 11u;
    let a = hybrid_scene[base + 9u]; let b = hybrid_scene[base + 10u];
    return a.xy * hit.barycentric.x + a.zw * hit.barycentric.y + b.xy * hit.barycentric.z;
}
fn hybrid_alpha(hit: HybridHit) -> f32 {
    let base = u32(hybrid_scene[0].z) + hit.triangle * 11u;
    let color = hybrid_scene[base + 6u] * hit.barycentric.x + hybrid_scene[base + 7u] * hit.barycentric.y + hybrid_scene[base + 8u] * hit.barycentric.z;
    let coverage=color.a*hybrid_material(hit.triangle,0u).a;
    let tile=hybrid_material(hit.triangle,6u);
    // Only base-color tile.w carries the packing-time all-alpha-one proof;
    // metallic/roughness tile.w retains its independent channel descriptor.
    if(tile.w>.5) {return coverage;}
    return hybrid_texture_alpha(tile,hybrid_uv(hit))*coverage;
}
struct HybridRayBox {
    inverse:vec3<f32>,
    parallel:vec3<bool>,
};
fn hybrid_ray_box_query(direction:vec3<f32>) -> HybridRayBox {
    let parallel=abs(direction)<vec3<f32>(0.00000001);
    // Preserve the existing epsilon-based parallel-axis semantics without
    // divisions by zero, and share these reciprocals across the entire query.
    return HybridRayBox(vec3<f32>(1.0)/select(direction,vec3<f32>(1.0),parallel),parallel);
}
fn hybrid_ray_box_near(origin: vec3<f32>, ray:HybridRayBox, minimum: vec3<f32>, maximum: vec3<f32>, max_distance: f32) -> f32 {
    if((ray.parallel.x && (origin.x<minimum.x || origin.x>maximum.x)) ||
        (ray.parallel.y && (origin.y<minimum.y || origin.y>maximum.y)) ||
        (ray.parallel.z && (origin.z<minimum.z || origin.z>maximum.z))) {return -1.0;}
    let a=(minimum-origin)*ray.inverse;let b=(maximum-origin)*ray.inverse;
    let lo=select(min(a,b),vec3<f32>(0.0),ray.parallel);
    let hi=select(max(a,b),vec3<f32>(max_distance),ray.parallel);
    let near=max(0.0,max(lo.x,max(lo.y,lo.z)));
    let far=min(max_distance,min(hi.x,min(hi.y,hi.z)));
    return select(-1.0,near,far>=near);
}
fn hybrid_ray_box(origin: vec3<f32>, ray:HybridRayBox, minimum: vec3<f32>, maximum: vec3<f32>, max_distance: f32) -> bool {
    return hybrid_ray_box_near(origin,ray,minimum,maximum,max_distance)>=0.0;
}
fn hybrid_intersect_triangle(origin: vec3<f32>, direction: vec3<f32>, index: u32, max_distance: f32) -> HybridHit {
    let base = u32(hybrid_scene[0].z) + index * 11u;
    let p0 = hybrid_scene[base].xyz; let p1 = hybrid_scene[base + 1u].xyz; let p2 = hybrid_scene[base + 2u].xyz;
    let e1 = p1 - p0; let e2 = p2 - p0;
    let cross_direction = cross(direction, e2); let determinant = dot(e1, cross_direction);
    if (abs(determinant) < 0.00000001) { return HybridHit(max_distance, index, vec3<f32>(0.0), false, false); }
    let inverse = 1.0 / determinant;
    let delta = origin - p0;
    let u = dot(delta, cross_direction) * inverse;
    let q = cross(delta, e1);
    let v = dot(direction, q) * inverse;
    let distance = dot(e2, q) * inverse;
    let valid = u >= -0.000001 && v >= -0.000001 && u + v <= 1.000001 && distance > hybrid_epsilon(origin) && distance < max_distance;
    return HybridHit(distance, index, vec3<f32>(1.0 - u - v, u, v), valid, false);
}
fn hybrid_intersect(origin: vec3<f32>, direction: vec3<f32>, maximum: f32) -> HybridHit {
    return hybrid_intersect_filtered(origin, direction, maximum, 0u);
}
// Query masks match hybrid.rs: 1 casting transmission, 2 transmissive solid,
// 4 any casting shadow. Zero retains unrestricted reflection intersections.
fn hybrid_triangle_matches(triangle: u32, query_mask: u32) -> bool {
    if (query_mask == 0u) { return true; }
    let base = u32(hybrid_scene[0].z) + triangle * 11u;
    let casting = hybrid_scene[base + 10u].z > 0.5;
    if (query_mask == 4u) { return casting; }
    let transmissive = hybrid_material(triangle, 1u).z > 0.001;
    if (query_mask == 1u) { return casting && transmissive; }
    return transmissive && hybrid_material(triangle, 4u).y > 0.5;
}
fn hybrid_child_near(base:u32, node:u32, origin:vec3<f32>, ray:HybridRayBox, maximum:f32, query_mask:u32) -> f32 {
    let metadata=hybrid_scene[base+node*3u+2u];
    if(query_mask!=0u && (u32(metadata.y)&query_mask)==0u) {return -1.0;}
    let a=hybrid_scene[base+node*3u];let b=hybrid_scene[base+node*3u+1u];
    return hybrid_ray_box_near(origin,ray,a.xyz,b.xyz,maximum);
}
fn hybrid_intersect_filtered(origin: vec3<f32>, direction: vec3<f32>, maximum: f32, query_mask: u32) -> HybridHit {
    var result = HybridHit(maximum, 0u, vec3<f32>(0.0), false, false);
    if (arrayLength(&hybrid_scene) < 2u || hybrid_scene[0].y < 0.5) { return result; }
    var count = u32(hybrid_scene[0].y); var base = u32(hybrid_scene[0].x);
    if (query_mask == 1u) {
        if (arrayLength(&hybrid_scene) < 4u) { return result; }
        base = u32(hybrid_scene[3].x); count = u32(hybrid_scene[3].y);
    }
    if(count==0u) {return result;}
    // Parent/right metadata distinguishes the current tree from retained
    // linear synthetic fixtures, whose escape-only traversal remains valid.
    let near_first=hybrid_scene[base+2u].w>.5;
    var node = 0u;
    var depth=0u;var pending_siblings=0u;var right_first=0u;
    var best_order=0xffffffffu;
    let ray_box=hybrid_ray_box_query(direction);
    // Two ancestor bitsets replace a private array stack. CPU trees retain at
    // most thirty levels; escape-only fixtures do not consume this state.
    var visits = 0u;
    loop {
        if (node >= count || visits >= 2048u) { break; }
        if (!hybrid_consume_node_visit()) { result.exhausted = true; break; }
        visits += 1u;
        let a = hybrid_scene[base + node * 3u]; let b = hybrid_scene[base + node * 3u + 1u];
        let metadata = hybrid_scene[base + node * 3u + 2u];
        let escape = u32(metadata.x);
        // A secondary shadow is zero as soon as any opaque caster lies before
        // the emitter. Keep its full finite ray interval while collecting the
        // nearest glass, so nearer glass cannot prune a farther opaque blocker.
        let search_distance = select(result.distance, maximum, query_mask == 4u);
        let matches=query_mask==0u || (u32(metadata.y)&query_mask)!=0u;
        if(matches && hybrid_ray_box(origin,ray_box,a.xyz,b.xyz,search_distance)) {
            if (b.w > 0.5) {
                let first = u32(a.w); let size = u32(b.w);
                for (var i = 0u; i < size; i += 1u) {
                    let order=first+i;
                    var triangle = order;
                    if (query_mask == 1u) {
                        triangle = u32(hybrid_scene[u32(hybrid_scene[3].z) + triangle / 4u][triangle % 4u]);
                    }
                    if (!hybrid_triangle_matches(triangle, query_mask)) { continue; }
                    // Test against the original strict ray endpoint in near
                    // order, then resolve exact ties by original packed order.
                    // The caster tree uses compact order before its remapping.
                    let triangle_maximum=select(result.distance,maximum,query_mask==4u || near_first);
                    let hit = hybrid_intersect_triangle(origin,direction,triangle,triangle_maximum);
                    if(hit.valid && (query_mask==4u || hit.distance<=result.distance)) {
                        var covered = true;
                        if (HYBRID_ALPHA_EVALUATION_ENABLED) {
                            let cutoff = max(hybrid_material(hit.triangle, 3u).w, 0.001);
                            covered = hybrid_alpha(hit) > cutoff;
                        }
                        if (covered) {
                            if (query_mask == 4u && hybrid_material(hit.triangle, 1u).z <= 0.001) { return hit; }
                            if(hit.distance<result.distance || (near_first && hit.distance==result.distance && order<best_order)) {
                                result=hit;best_order=order;
                            }
                        }
                    }
                }
            } else {
                if(!near_first) {node+=1u;continue;}
                let left=node+1u;let right=u32(metadata.w);
                if(depth>=30u || right<=left || right>=count) {result.exhausted=true;break;}
                let left_near=hybrid_child_near(base,left,origin,ray_box,search_distance,query_mask);
                let right_near=hybrid_child_near(base,right,origin,ray_box,search_distance,query_mask);
                let bit=1u<<depth;
                pending_siblings&=~bit;right_first&=~bit;
                if(left_near>=0.0 || right_near>=0.0) {
                    let choose_right=left_near<0.0 || (right_near>=0.0 && right_near<left_near);
                    if(left_near>=0.0 && right_near>=0.0) {
                        pending_siblings|=bit;
                        if(choose_right) {right_first|=bit;}
                    }
                    node=select(left,right,choose_right);depth+=1u;
                    continue;
                }
            }
        }
        if(!near_first) {node=escape;continue;}
        // A finished/pruned branch climbs until an ancestor has an unvisited
        // sibling. Restore child depth before resuming that pending branch.
        var ascent=metadata;
        loop {
            if(depth==0u) {node=count;break;}
            let encoded_parent=u32(ascent.z);
            if(encoded_parent==0u || encoded_parent>node) {result.exhausted=true;break;}
            let parent=encoded_parent-1u;
            depth-=1u;
            let bit=1u<<depth;
            let parent_metadata=hybrid_scene[base+parent*3u+2u];
            if((pending_siblings&bit)!=0u) {
                let previous_right=(right_first&bit)!=0u;
                pending_siblings&=~bit;right_first&=~bit;
                node=select(u32(parent_metadata.w),parent+1u,previous_right);
                depth+=1u;
                break;
            }
            node=parent;ascent=parent_metadata;
        }
        if(result.exhausted) {break;}
    }
    if (node < count && (visits >= 2048u || result.exhausted)) { result.valid = false; result.exhausted = true; }
    return result;
}
fn hybrid_visibility_surface(hit: HybridHit, origin: vec3<f32>, direction: vec3<f32>) -> HybridVisibilitySurface {
    let base = u32(hybrid_scene[0].z) + hit.triangle * 11u;
    let p0 = hybrid_scene[base].xyz;
    let p1 = hybrid_scene[base + 1u].xyz;
    let p2 = hybrid_scene[base + 2u].xyz;
    let optical = hybrid_material(hit.triangle,1u);
    let attenuation = hybrid_material(hit.triangle,3u);
    let transport = hybrid_material(hit.triangle,4u);
    return HybridVisibilitySurface(origin + direction * hit.distance,
        normalize(cross(p1-p0,p2-p0)),optical.z,optical.w,attenuation.rgb,transport.x,
        hybrid_material(hit.triangle,9u).z,transport.y > 0.5,hybrid_scene[base + 2u].w > 0.5,
        u32(hybrid_scene[base + 1u].w),hybrid_scene[base + 10u].z > 0.5);
}
fn hybrid_surface_roughness(hit:HybridHit) -> f32 {
    let tile=hybrid_material(hit.triangle,7u);
    let sampled=hybrid_texture(tile,hybrid_uv(hit),false);
    let channels=u32(tile.w+.5);
    return clamp(hybrid_material(hit.triangle,1u).y*hybrid_channel(sampled,(channels>>4u)&15u)+lighting.surface1.z,.045,1.0);
}
fn hybrid_surface(hit: HybridHit, origin: vec3<f32>, direction: vec3<f32>) -> HybridSurface {
    let base = u32(hybrid_scene[0].z) + hit.triangle * 11u;
    let p0 = hybrid_scene[base].xyz; let p1 = hybrid_scene[base + 1u].xyz; let p2 = hybrid_scene[base + 2u].xyz;
    let geometric = normalize(cross(p1 - p0, p2 - p0));
    let interpolated = normalize(hybrid_scene[base + 3u].xyz * hit.barycentric.x + hybrid_scene[base + 4u].xyz * hit.barycentric.y + hybrid_scene[base + 5u].xyz * hit.barycentric.z);
    let normal = select(interpolated, -interpolated, dot(interpolated, direction) > 0.0);
    let uv = hybrid_uv(hit);
    let vertex_color = hybrid_scene[base + 6u] * hit.barycentric.x + hybrid_scene[base + 7u] * hit.barycentric.y + hybrid_scene[base + 8u] * hit.barycentric.z;
    let color_sample = hybrid_texture(hybrid_material(hit.triangle, 6u), uv, true);
    let factors = hybrid_material(hit.triangle, 0u);
    var factor_color = vertex_color.rgb * factors.rgb;
    if (lighting.surface0.x >= -0.5) { factor_color = pow(max(factor_color, vec3<f32>(0.0)), vec3<f32>(2.2)); }
    let color = color_sample.rgb * factor_color;
    let mr_tile = hybrid_material(hit.triangle, 7u);
    let mr = hybrid_texture(mr_tile, uv, false);
    let channels = u32(mr_tile.w + 0.5);
    let optical = hybrid_material(hit.triangle, 1u);
    let metallic = clamp(optical.x * hybrid_channel(mr, channels & 15u), 0.0, 1.0);
    let roughness = clamp(optical.y * hybrid_channel(mr, (channels >> 4u) & 15u) + lighting.surface1.z, 0.045, 1.0);
    let emission = hybrid_material(hit.triangle, 2u);
    let attenuation = hybrid_material(hit.triangle, 3u);
    let transport = hybrid_material(hit.triangle, 4u);
    let specular = hybrid_material(hit.triangle, 5u);
    let extra = hybrid_material(hit.triangle, 9u);
    let ratio = (optical.w - 1.0) / (optical.w + 1.0);
    let f0 = mix(specular.rgb * ratio * ratio * extra.x, color, metallic);
    return HybridSurface(origin + direction * hit.distance, normal, geometric, color,
        color_sample.a * vertex_color.a * factors.a, metallic, roughness, optical.z, optical.w,
        hybrid_texture(hybrid_material(hit.triangle, 8u), uv, true).rgb * emission.rgb,
        max(emission.w, specular.w), f0, attenuation.rgb, transport.x, transport.y > 0.5, hybrid_scene[base + 2u].w > 0.5,
        transport.z, transport.w, u32(hybrid_scene[base + 1u].w), extra.w,
        hybrid_scene[base + 10u].z > 0.5,hybrid_scene[base + 10u].w > 0.5);
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
fn hybrid_light_visibility(position: vec3<f32>, direction: vec3<f32>, max_distance: f32) -> vec3<f32> {
    // Full geometry remains the fallback when a reflected receiver or any
    // required caster interval is outside the retained opaque shadow maps.
    if (!PRIMARY_GLASS_SHADOWS_ENABLED) {
        // The complete-scene casting-glass proof also applies to secondary
        // receivers. Every matching caster is opaque, so the existing finite
        // any-blocker query owns visibility without boundary or medium state.
        if (hybrid_work_exhausted) { return vec3<f32>(0.0); }
        let origin = position + direction * hybrid_epsilon(position) * 3.0;
        let hit = hybrid_intersect_filtered(origin, direction, max_distance, 4u);
        return select(vec3<f32>(1.0), vec3<f32>(0.0), hit.valid || hit.exhausted);
    }
    return hybrid_trace_visibility(position, direction, max_distance, true);
}
fn hybrid_coat_base(s: HybridSurface, incoming: vec3<f32>) -> f32 {
    let cosine = clamp(abs(dot(s.geometric_normal, -incoming)), 0.0, 1.0);
    return 1.0 - s.coat * (0.04 + 0.96 * pow(1.0 - cosine, 5.0));
}
fn hybrid_direct_brdf(s: HybridSurface, view: vec3<f32>, light: vec3<f32>, radiance: vec3<f32>) -> vec3<f32> {
    let no_l = max(dot(s.normal, light), 0.0); let no_v = max(dot(s.normal, view), 0.0);
    if (no_l <= 0.0 || no_v <= 0.0) { return vec3<f32>(0.0); }
    let half_direction = normalize(light + view);
    let fresnel = fresnel_schlick(max(dot(view, half_direction), 0.0), s.f0);
    let distribution = distribution_ggx(s.normal, half_direction, s.roughness);
    let geometry = geometry_smith(s.normal, view, light, s.roughness);
    let specular = distribution * geometry * fresnel / max(4.0 * no_l * no_v, 0.0001);
    let diffuse = (vec3<f32>(1.0) - fresnel) * s.color * (1.0 - s.metallic) * (1.0 - s.transmission) / 3.14159265;
    return (diffuse + specular) * hybrid_coat_base(s, -view) * radiance * no_l;
}
fn hybrid_hit_local(s: HybridSurface, incoming: vec3<f32>) -> vec3<f32> {
    if (s.unlit > 0.5) { return (s.color + s.emissive) * s.exposure; }
    let view = -incoming;
    let irradiance = sample_environment_irradiance(s.normal) * lighting.environment0.x * lighting.environment1.z / 3.14159265;
    let baked = sample_baked_irradiance(s.position, s.normal);
    var lit = s.color * (1.0 - s.metallic) * (1.0 - s.transmission) *
        hybrid_coat_base(s, incoming) * mix(irradiance, baked.rgb / 3.14159265, baked.a);
    let count = min(u32(lighting.environment2.y + 0.5), 8u);
    for (var i = 0u; i < count; i += 1u) {
        let emitter = lighting.lights[i];
        if(emitter.color_intensity.w<=0.0 || dot(s.normal,view)<=0.0) {continue;}
        let area=emitter.position_kind.w>2.5;
        let requested=select(4.0,lighting.preview2.w,lighting.preview2.w>=.5);
        let area_count=clamp(u32(select(requested,emitter_shadows.control.z,emitter_shadows.control.x>.5)),1u,4u);
        let sample_count=select(1u,area_count,area);
        var shadow_strength = 0.0;
        if (emitter_shadows.control.x > 0.5) { shadow_strength = emitter_shadows.lights[i].views.w; }
        else if (f32(i) == lighting.render_compat.x) { shadow_strength = lighting.color1.w; }
        for(var sample=0u;sample<4u;sample++) {
            if(sample>=sample_count) {break;}
            var direction_attenuation=authored_light_radiance(emitter,s.position);
            var emitter_position=emitter.position_kind.xyz;
            if(area) {
                // Match the retained shadow-map slots exactly: centre, diagonal
                // pair, or all four authored quadrature positions and weights.
                var quadrature=sample;
                if(sample_count==1u) {quadrature=4u;}
                if(sample_count==2u) {quadrature=sample*3u;}
                emitter_position=authored_area_light_position(emitter,quadrature);
                direction_attenuation=authored_area_light_radiance(emitter,s.position,quadrature);
                direction_attenuation.w*=4.0/f32(sample_count);
            }
            // These lobes are exactly zero before visibility. Avoid a full
            // shadow traversal for emitters behind the reflected surface.
            if(direction_attenuation.w<=0.0 || dot(s.normal,direction_attenuation.xyz)<=0.0) {continue;}
            let maximum=select(1000000.0,length(emitter_position-s.position)-hybrid_epsilon(s.position)*3.0,emitter.position_kind.w>.5);
            var visibility=vec3<f32>(1.0);
            if(s.receive_shadow && shadow_strength>0.0) {
                let position=s.position+s.normal*hybrid_epsilon(s.position);
                let opaque=secondary_emitter_opaque_evidence(i,sample,s.position,s.normal);
                if(opaque.valid) {
                    var mapped_visibility=vec3<f32>(0.0);
                    if(opaque.visibility>0.0) {
                        mapped_visibility=opaque.visibility*hybrid_transmission_visibility(position,direction_attenuation.xyz,maximum);
                    }
                    visibility=mix(vec3<f32>(1.0),mapped_visibility,shadow_strength);
                } else {
                    visibility=mix(vec3<f32>(1.0),hybrid_light_visibility(position,direction_attenuation.xyz,maximum),shadow_strength);
                }
            }
            lit+=hybrid_direct_brdf(s,view,direction_attenuation.xyz,
                emitter.color_intensity.rgb*emitter.color_intensity.w*direction_attenuation.w*visibility);
        }
    }
    if (count == 0u && lighting.environment0.w < 0.5) {
        lit += hybrid_direct_brdf(s,view,normalize(vec3<f32>(-0.42,0.78,0.47)),vec3<f32>(4.2,4.0,3.75));
        lit += hybrid_direct_brdf(s,view,normalize(vec3<f32>(0.68,0.28,0.51)),vec3<f32>(1.25,1.45,1.75));
    }
    return (lit + s.emissive) * s.exposure;
}
fn hybrid_specular_weight(s: HybridSurface, incoming: vec3<f32>) -> vec3<f32> {
    let no_v = clamp(dot(s.normal, -incoming), 0.0, 1.0);
    let lut = textureSampleLevel(environment_brdf_texture, environment_brdf_sampler, vec2<f32>(no_v, s.roughness), 0.0).rg;
    var base_response = s.f0 * lut.x + lut.y;
    if (s.solid && s.transmission > 0.001) {
        let inside = dot(s.geometric_normal, incoming) > 0.0;
        let eta_i = select(1.0, s.ior, inside);
        let eta_t = select(s.ior, 1.0, inside);
        base_response = vec3<f32>(hybrid_fresnel(no_v, eta_i, eta_t));
    }
    let coat_base = hybrid_coat_base(s, incoming);
    let base = base_response * coat_base;
    let coat = 1.0 - coat_base;
    return clamp(base + vec3<f32>(coat), vec3<f32>(0.0), vec3<f32>(1.0));
}
fn hybrid_trace_reflection_ray(origin: vec3<f32>, direction: vec3<f32>, roughness: f32, max_bounces: u32) -> vec4<f32> {
    if (hybrid_work_exhausted) { return vec4<f32>(0.0); }
    var ray_origin = origin; var ray_direction = normalize(direction);
    var throughput = vec3<f32>(1.0); var result = vec3<f32>(0.0);
    var confidence = 0.0;
    let bounce_limit = clamp(max_bounces, 1u, 2u);
    for (var bounce = 0u; bounce < 2u; bounce += 1u) {
        if (bounce >= bounce_limit) { break; }
        let hit = hybrid_intersect(ray_origin, ray_direction, 1000000.0);
        if (hit.exhausted) { return vec4<f32>(0.0); }
        if (!hit.valid) {
            if (bounce > 0u) { result += throughput * hybrid_environment(ray_origin, ray_direction, roughness); }
            break;
        }
        confidence = 1.0;
        if (bounce == 0u) {
            hybrid_reflection_hit_distance = select(hit.distance,min(hybrid_reflection_hit_distance,hit.distance),hybrid_reflection_hit_distance > 0.0);
        }
        let s = hybrid_surface(hit, ray_origin, ray_direction);
        result += throughput * hybrid_hit_local(s, ray_direction);
        if (hybrid_work_exhausted) { return vec4<f32>(0.0); }
        if (s.transmission > 0.001) {
            let transmitted = hybrid_trace_transmission_core(s.position, ray_direction, s.geometric_normal,
                s.ior, s.attenuation, s.attenuation_distance, s.object, hit.distance,
                s.solid, s.closed, hybrid_material(hit.triangle,9u).z);
            if (transmitted.confidence <= 0.0) { return vec4<f32>(0.0); }
            result += throughput * s.transmission * hybrid_coat_base(s, ray_direction) *
                transmitted.radiance * transmitted.confidence;
            if (hybrid_work_exhausted) { return vec4<f32>(0.0); }
        }
        if (s.unlit > 0.5) { break; }
        let reflected = reflect(ray_direction, s.normal);
        let weight = hybrid_specular_weight(s, ray_direction) * s.exposure;
        if (bounce + 1u >= bounce_limit || s.roughness > 0.65 || max(weight.x, max(weight.y, weight.z)) < 0.001) {
            result += throughput * weight * hybrid_environment(s.position, reflected, s.roughness);
            break;
        }
        throughput *= weight;
        ray_origin = s.position + reflected * hybrid_epsilon(s.position) * 3.0;
        ray_direction = reflected;
    }
    // High-roughness primary lobes retain probes instead of showing a crisp ray.
    confidence *= 1.0 - smoothstep(0.5, 0.75, roughness);
    return vec4<f32>(max(result, vec3<f32>(0.0)), confidence);
}

fn hybrid_trace_transmission_core(origin: vec3<f32>, incident: vec3<f32>, entry_normal: vec3<f32>, ior: f32,
    attenuation: vec3<f32>, attenuation_distance: f32, object_id: u32, initial_inside_distance: f32,
    initial_solid: bool, initial_closed: bool, initial_thickness: f32) -> HybridTransportResult {
    if (!HYBRID_SOLID_TRANSPORT) {
        return hybrid_trace_slab_transmission(origin, incident, entry_normal, ior,
            attenuation, attenuation_distance, object_id, initial_closed, initial_thickness);
    }
    return hybrid_trace_mixed_transmission(origin, incident, entry_normal, ior,
        attenuation, attenuation_distance, object_id, initial_inside_distance,
        initial_solid, initial_closed, initial_thickness);
}

// This path uses the same paired-interface approximation as the mixed path,
// including closed-slab exit suppression, without any nested-medium state.
fn hybrid_trace_slab_transmission(origin: vec3<f32>, incident: vec3<f32>, entry_normal: vec3<f32>, ior: f32,
    attenuation: vec3<f32>, attenuation_distance: f32, object_id: u32,
    initial_closed: bool, initial_thickness: f32) -> HybridTransportResult {
    if (hybrid_work_exhausted) { return HybridTransportResult(vec3<f32>(0.0),0.0,0.0,0u); }
    let outward_exit = dot(entry_normal, incident) > 0.0;
    let normal = select(normalize(entry_normal), -normalize(entry_normal), outward_exit);
    let entry_direction = refract(normalize(incident), normal, 1.0 / ior);
    if (dot(entry_direction, entry_direction) < 0.000001) { return HybridTransportResult(vec3<f32>(0.0), 1.0, 0.0, 0u); }
    var throughput = vec3<f32>(1.0 - hybrid_fresnel(dot(-incident, normal), 1.0, ior));
    let slab_path = max(initial_thickness,0.0) / max(abs(dot(normal,entry_direction)),0.0001);
    throughput *= (1.0 - hybrid_fresnel(dot(-incident,normal),1.0,ior)) *
        hybrid_beer(attenuation,slab_path,attenuation_distance);
    var optical_distance = slab_path;
    var interface_count = 2u;
    let ray_direction = normalize(incident);
    var ray_origin = origin + normalize(entry_direction) * slab_path + ray_direction * hybrid_epsilon(origin) * 3.0;
    var result = vec3<f32>(0.0);
    var slab_objects: array<u32,4>; var slab_count = 0u;
    if (initial_closed && !outward_exit) { slab_objects[0] = object_id; slab_count = 1u; }
    for (var interface_index = 0u; interface_index < 12u; interface_index += 1u) {
        let hit = hybrid_intersect(ray_origin, ray_direction, 1000000.0);
        if (hit.exhausted) { return HybridTransportResult(vec3<f32>(0.0),0.0,optical_distance,interface_count); }
        if (!hit.valid) {
            result += throughput * hybrid_environment(ray_origin, ray_direction, 0.0);
            return HybridTransportResult(result, 1.0, optical_distance, interface_count);
        }
        let s = hybrid_visibility_surface(hit, ray_origin, ray_direction);
        if (s.transmission > 0.001) {
            if (s.closed) {
                let entering_slab = dot(ray_direction,s.geometric_normal) < 0.0;
                var paired_exit = false;
                if (!entering_slab) {
                    for (var j = 0u; j < 4u; j += 1u) {
                        if (j < slab_count && slab_objects[j] == s.object) {
                            slab_objects[j] = slab_objects[slab_count-1u]; slab_count -= 1u;
                            paired_exit = true; break;
                        }
                    }
                } else {
                    if (slab_count >= 4u) { return HybridTransportResult(vec3<f32>(0.0),0.0,optical_distance,interface_count); }
                    slab_objects[slab_count] = s.object; slab_count += 1u;
                }
                if (paired_exit) {
                    ray_origin = s.position + ray_direction * hybrid_epsilon(s.position) * 3.0;
                    continue;
                }
            }
            let slab_normal = select(s.geometric_normal,-s.geometric_normal,dot(ray_direction,s.geometric_normal) > 0.0);
            let slab_inside = refract(ray_direction,slab_normal,1.0/s.ior);
            let path = s.thickness / max(abs(dot(slab_normal,slab_inside)),0.0001);
            let slab_fresnel = hybrid_fresnel(abs(dot(-ray_direction,slab_normal)),1.0,s.ior);
            let slab_roughness = hybrid_surface_roughness(hit);
            result += throughput * slab_fresnel * hybrid_environment(s.position,reflect(ray_direction,slab_normal),slab_roughness);
            throughput *= s.transmission * (1.0-slab_fresnel) * (1.0-slab_fresnel) * hybrid_beer(s.attenuation,path,s.attenuation_distance);
            optical_distance += path; interface_count += 2u;
            ray_origin = s.position + slab_inside * path + ray_direction * hybrid_epsilon(s.position) * 3.0;
            continue;
        }
        let shaded = hybrid_surface(hit,ray_origin,ray_direction);
        let reflected = reflect(ray_direction, shaded.normal);
        let fallback = hybrid_environment(shaded.position, reflected, shaded.roughness);
        result += throughput * (hybrid_hit_local(shaded, ray_direction) + hybrid_specular_weight(shaded, ray_direction) * fallback);
        if (hybrid_work_exhausted) { return HybridTransportResult(vec3<f32>(0.0),0.0,optical_distance,interface_count); }
        return HybridTransportResult(result, 1.0, optical_distance, interface_count);
    }
    return HybridTransportResult(vec3<f32>(0.0), 0.0, optical_distance, interface_count);
}

fn hybrid_trace_mixed_transmission(origin: vec3<f32>, incident: vec3<f32>, entry_normal: vec3<f32>, ior: f32,
    attenuation: vec3<f32>, attenuation_distance: f32, object_id: u32, initial_inside_distance: f32,
    initial_solid: bool, initial_closed: bool, initial_thickness: f32) -> HybridTransportResult {
    if (hybrid_work_exhausted) { return HybridTransportResult(vec3<f32>(0.0),0.0,0.0,0u); }
    // Initial incident is in air. The raster pass owns entry reflection; this
    // function includes entry/exit transmission and any internal Fresnel loss.
    let outward_exit = dot(entry_normal, incident) > 0.0;
    let starts_inside = initial_solid && outward_exit;
    let normal = select(normalize(entry_normal), -normalize(entry_normal), outward_exit);
    let eta_i_initial = select(1.0, ior, starts_inside);
    let eta_t_initial = select(ior, 1.0, starts_inside);
    let entry_direction = refract(normalize(incident), normal, eta_i_initial / eta_t_initial);
    if (dot(entry_direction, entry_direction) < 0.000001) { return HybridTransportResult(vec3<f32>(0.0), 1.0, 0.0, 0u); }
    var throughput = vec3<f32>(1.0 - hybrid_fresnel(dot(-incident, normal), eta_i_initial, eta_t_initial));
    var ray_direction = normalize(entry_direction);
    var ray_origin = origin + ray_direction * hybrid_epsilon(origin) * 3.0;
    var object_stack: array<u32, 4>;
    var ior_stack: array<f32, 4>;
    var attenuation_stack: array<vec4<f32>, 4>;
    var stack_size = select(0u, 1u, initial_solid && !starts_inside);
    object_stack[0] = object_id; ior_stack[0] = ior;
    attenuation_stack[0] = vec4<f32>(attenuation, attenuation_distance);
    var optical_distance = 0.0;
    if (starts_inside) {
        // The caller owns the incident segment. A secondary ray must never
        // use the main camera's position as its absorption origin.
        optical_distance = max(initial_inside_distance, 0.0);
        throughput *= hybrid_beer(attenuation, optical_distance, attenuation_distance);
    }
    var result = vec3<f32>(0.0);
    var interface_count = 1u;
    var exited_primary = starts_inside || !initial_solid;
    var slab_objects: array<u32,4>; var slab_count = 0u;
    if (!initial_solid) {
        // A reflected slab remains a virtual pair of parallel interfaces.
        // Its air ray exits parallel to the incident ray with the authored
        // lateral offset and optical path, then sees offscreen geometry.
        let slab_path = max(initial_thickness,0.0) / max(abs(dot(normal,entry_direction)),0.0001);
        throughput *= (1.0 - hybrid_fresnel(dot(-incident,normal),1.0,ior)) *
            hybrid_beer(attenuation,slab_path,attenuation_distance);
        optical_distance = slab_path; interface_count = 2u;
        ray_direction = normalize(incident);
        ray_origin = origin + normalize(entry_direction) * slab_path + ray_direction * hybrid_epsilon(origin) * 3.0;
        if (initial_closed && !outward_exit) { slab_objects[0] = object_id; slab_count = 1u; }
    }
    // A nested medium stack handles up to four overlapping closed volumes.
    // TIR changes direction without changing the medium stack.
    for (var interface_index = 0u; interface_index < 12u; interface_index += 1u) {
        let hit = hybrid_intersect(ray_origin, ray_direction, 1000000.0);
        if (hit.exhausted) { return HybridTransportResult(vec3<f32>(0.0),0.0,optical_distance,interface_count); }
        if (!hit.valid) {
            if (stack_size > 0u || !exited_primary) { return HybridTransportResult(vec3<f32>(0.0), 0.0, optical_distance, interface_count); }
            result += throughput * hybrid_environment(ray_origin, ray_direction, 0.0);
            return HybridTransportResult(result, 1.0, optical_distance, interface_count);
        }
        if (stack_size > 0u) {
            let medium = attenuation_stack[stack_size - 1u];
            throughput *= hybrid_beer(medium.rgb, hit.distance, medium.a);
            optical_distance += hit.distance;
        }
        // Intermediate boundaries require geometric optics only. Keep shaded
        // color/MR/emission payloads for the final opaque radiance endpoint.
        let s = hybrid_visibility_surface(hit, ray_origin, ray_direction);
        if (s.transmission > 0.001 && !s.solid) {
            if (s.closed) {
                let entering_slab = dot(ray_direction,s.geometric_normal) < 0.0;
                var paired_exit = false;
                if (!entering_slab) {
                    for (var j = 0u; j < 4u; j += 1u) {
                        if (j < slab_count && slab_objects[j] == s.object) {
                            slab_objects[j] = slab_objects[slab_count-1u]; slab_count -= 1u;
                            paired_exit = true; break;
                        }
                    }
                } else {
                    if (slab_count >= 4u) { return HybridTransportResult(vec3<f32>(0.0),0.0,optical_distance,interface_count); }
                    slab_objects[slab_count] = s.object; slab_count += 1u;
                }
                if (paired_exit) {
                    ray_origin = s.position + ray_direction * hybrid_epsilon(s.position) * 3.0;
                    continue;
                }
            }
            // Thin-slab encounters keep their explicit authored approximation:
            // parallel exit interface, prescribed thickness, unchanged air ray.
            let slab_normal = select(s.geometric_normal,-s.geometric_normal,dot(ray_direction,s.geometric_normal) > 0.0);
            let slab_inside = refract(ray_direction,slab_normal,1.0/s.ior);
            let slab_path = s.thickness / max(abs(dot(slab_normal,slab_inside)),0.0001);
            let slab_fresnel = hybrid_fresnel(abs(dot(-ray_direction,slab_normal)),1.0,s.ior);
            let slab_roughness=hybrid_surface_roughness(hit);
            result += throughput * slab_fresnel * hybrid_environment(s.position,reflect(ray_direction,slab_normal),slab_roughness);
            throughput *= s.transmission * (1.0-slab_fresnel) * (1.0-slab_fresnel) * hybrid_beer(s.attenuation,slab_path,s.attenuation_distance);
            optical_distance += slab_path; interface_count += 2u;
            ray_origin = s.position + slab_inside * slab_path + ray_direction * hybrid_epsilon(s.position) * 3.0;
            continue;
        }
        if (s.transmission < 0.001) {
            let shaded=hybrid_surface(hit,ray_origin,ray_direction);
            let reflected = reflect(ray_direction, shaded.normal);
            let fallback = hybrid_environment(shaded.position, reflected, shaded.roughness);
            result += throughput * (hybrid_hit_local(shaded, ray_direction) + hybrid_specular_weight(shaded, ray_direction) * fallback);
            if (hybrid_work_exhausted) { return HybridTransportResult(vec3<f32>(0.0),0.0,optical_distance,interface_count); }
            return HybridTransportResult(result, 1.0, optical_distance, interface_count);
        }
        // Boundary orientation is geometric, never a normal-map perturbation.
        let entering = dot(ray_direction, s.geometric_normal) < 0.0;
        let boundary_normal = select(-s.geometric_normal, s.geometric_normal, entering);
        let eta_i = select(1.0, ior_stack[max(stack_size, 1u) - 1u], stack_size > 0u);
        var eta_t = s.ior;
        var exit_index = stack_size;
        if (!entering) {
            for (var j = 0u; j < 4u; j += 1u) {
                if (j < stack_size && object_stack[j] == s.object) { exit_index = j; }
            }
            if (exit_index >= stack_size) { return HybridTransportResult(vec3<f32>(0.0), 0.0, optical_distance, interface_count); }
            eta_t = select(1.0, ior_stack[max(exit_index, 1u) - 1u], exit_index > 0u);
            if (exit_index + 1u < stack_size) { eta_t = eta_i; }
        }
        let outgoing = refract(ray_direction, boundary_normal, eta_i / eta_t);
        let fresnel = hybrid_fresnel(dot(-ray_direction, boundary_normal), eta_i, eta_t);
        interface_count += 1u;
        if (dot(outgoing, outgoing) < 0.000001) {
            ray_direction = reflect(ray_direction, boundary_normal);
        } else {
            throughput *= (1.0 - fresnel) * s.transmission;
            ray_direction = normalize(outgoing);
            if (entering) {
                if (stack_size >= 4u) { return HybridTransportResult(vec3<f32>(0.0), 0.0, optical_distance, interface_count); }
                object_stack[stack_size] = s.object; ior_stack[stack_size] = s.ior;
                attenuation_stack[stack_size] = vec4<f32>(s.attenuation, s.attenuation_distance);
                stack_size += 1u;
            } else {
                // Remove only the exiting volume; nested volumes above it retain
                // their medium identity when partially overlapping solids exit.
                for (var j = 0u; j < 3u; j += 1u) {
                    if (j >= exit_index && j + 1u < stack_size) {
                        object_stack[j] = object_stack[j + 1u]; ior_stack[j] = ior_stack[j + 1u];
                        attenuation_stack[j] = attenuation_stack[j + 1u];
                    }
                }
                stack_size -= 1u;
                if (s.object == object_id) { exited_primary = true; }
            }
        }
        ray_origin = s.position + ray_direction * hybrid_epsilon(s.position) * 3.0;
        if (max(throughput.x, max(throughput.y, throughput.z)) < 0.00001) {
            return HybridTransportResult(vec3<f32>(0.0), 1.0, optical_distance, interface_count);
        }
    }
    // Budget exhaustion is explicitly unresolved, not silently a thin slab.
    return HybridTransportResult(vec3<f32>(0.0), 0.0, optical_distance, interface_count);
}

fn trace_geometry_transmission(origin: vec3<f32>, incident: vec3<f32>, entry_normal: vec3<f32>, ior: f32,
    attenuation: vec3<f32>, attenuation_distance: f32, object_id: u32, initial_inside_distance: f32) -> HybridTransportResult {
    return hybrid_trace_transmission_core(origin, incident, entry_normal, ior, attenuation, attenuation_distance,
        object_id, initial_inside_distance, true, true, 0.0);
}

fn trace_geometry_reflection(origin: vec3<f32>, direction: vec3<f32>, roughness: f32, max_bounces: u32) -> vec4<f32> {
    hybrid_reflection_hit_distance = 0.0;
    if (hybrid_work_exhausted) { return vec4<f32>(0.0); }
    if (roughness >= 0.75) { return vec4<f32>(0.0); }
    let ray = normalize(direction);
    let axis = select(vec3<f32>(0.0,1.0,0.0), vec3<f32>(1.0,0.0,0.0), abs(ray.y) > 0.9);
    let tangent = normalize(cross(axis,ray)); let bitangent = cross(ray,tangent);
    let sample_count = select(1u, select(2u,4u,lighting.reflection0.y > 2.5), roughness > 0.12 && lighting.reflection0.y > 1.5);
    let offsets = array<vec2<f32>,4>(vec2<f32>(-0.333,-0.333),vec2<f32>(0.333,0.333),vec2<f32>(-0.333,0.333),vec2<f32>(0.333,-0.333));
    var accumulated = vec3<f32>(0.0); var weight = 0.0;
    for (var i = 0u; i < 4u; i += 1u) {
        if (i >= sample_count) { break; }
        let offset = select(vec2<f32>(0.0),offsets[i],sample_count > 1u) * roughness * roughness * 1.4;
        let sample_direction = normalize(ray + tangent * offset.x + bitangent * offset.y);
        let traced = hybrid_trace_reflection_ray(origin,sample_direction,roughness,max_bounces);
        if (hybrid_work_exhausted) { return vec4<f32>(0.0); }
        accumulated += traced.rgb * traced.a; weight += traced.a;
    }
    if (weight <= 0.00001) { return vec4<f32>(0.0); }
    return vec4<f32>(accumulated / weight, weight / f32(sample_count));
}

// Opaque shadow maps own opaque blockers. This additional straight-through
// term handles only glass Fresnel and Beer loss; it does not bend shadow rays
// or produce focused caustics. Closed solids use actual entry/exit distance.
fn hybrid_transmission_visibility(position: vec3<f32>, direction: vec3<f32>, maximum: f32) -> vec3<f32> {
    // Compile out the colored-shadow call graph when its casting tree is
    // provably empty; reflection and refraction still retain all scene glass.
    if (!PRIMARY_GLASS_SHADOWS_ENABLED) { return vec3<f32>(1.0); }
    if (arrayLength(&hybrid_scene) < 3u) { return vec3<f32>(1.0); }
    if (hybrid_scene[2].x < 0.5) { return vec3<f32>(1.0); }
    return hybrid_trace_visibility(position, direction, maximum, false);
}
fn hybrid_trace_visibility(position: vec3<f32>, direction: vec3<f32>, maximum: f32, include_opaque: bool) -> vec3<f32> {
    if (!HYBRID_SOLID_TRANSPORT) {
        return hybrid_trace_slab_visibility(position, direction, maximum, include_opaque);
    }
    return hybrid_trace_mixed_visibility(position, direction, maximum, include_opaque);
}

fn hybrid_trace_slab_visibility(position: vec3<f32>, direction: vec3<f32>, maximum: f32, include_opaque: bool) -> vec3<f32> {
    if (hybrid_work_exhausted) { return vec3<f32>(0.0); }
    var result = vec3<f32>(1.0); var origin = position + direction * hybrid_epsilon(position) * 3.0;
    var remaining = maximum;
    var slab_objects: array<u32,4>; var slab_count = 0u;
    for (var interface_index = 0u; interface_index < 12u; interface_index += 1u) {
        let query_mask = select(1u, 4u, include_opaque);
        let hit = hybrid_intersect_filtered(origin,direction,remaining,query_mask);
        if (hit.exhausted) { return vec3<f32>(0.0); }
        if (!hit.valid) { return result; }
        let s = hybrid_visibility_surface(hit,origin,direction);
        if (s.transmission <= 0.001 && s.cast_shadow && include_opaque) { return vec3<f32>(0.0); }
        if (s.transmission > 0.001 && s.cast_shadow) {
            let cosine = max(abs(dot(direction,s.geometric_normal)),0.0001);
            var paired_exit = false;
            if (s.closed) {
                let entering = dot(direction,s.geometric_normal) < 0.0;
                if (!entering) {
                    for (var j = 0u; j < 4u; j += 1u) {
                        if (j < slab_count && slab_objects[j] == s.object) {
                            slab_objects[j] = slab_objects[slab_count-1u];
                            slab_count -= 1u; paired_exit = true; break;
                        }
                    }
                } else {
                    if (slab_count >= 4u) { return vec3<f32>(0.0); }
                    slab_objects[slab_count] = s.object; slab_count += 1u;
                }
            }
            if (!paired_exit) {
                let fresnel = hybrid_fresnel(cosine,1.0,s.ior);
                let thickness = s.thickness / cosine;
                result *= s.transmission * (1.0 - fresnel) * (1.0 - fresnel) * hybrid_beer(s.attenuation,thickness,s.attenuation_distance);
            }
        }
        remaining -= hit.distance;
        if (remaining <= hybrid_epsilon(s.position)) { return result; }
        origin = s.position + direction * hybrid_epsilon(s.position) * 3.0;
    }
    return vec3<f32>(0.0);
}

fn hybrid_trace_mixed_visibility(position: vec3<f32>, direction: vec3<f32>, maximum: f32, include_opaque: bool) -> vec3<f32> {
    if (hybrid_work_exhausted) { return vec3<f32>(0.0); }
    var result = vec3<f32>(1.0); var origin = position + direction * hybrid_epsilon(position) * 3.0;
    var remaining = maximum;
    var objects: array<u32,4>; var media: array<vec4<f32>,4>; var iors: array<f32,4>; var stack_size = 0u;
    var slab_objects: array<u32,4>; var slab_count = 0u;
    for (var interface_index = 0u; interface_index < 12u; interface_index += 1u) {
        // Opaque blockers are already owned by primary shadow maps. Exclude
        // them during traversal so they cannot consume glass interface work.
        let query_mask = select(1u, 4u, include_opaque);
        let hit = hybrid_intersect_filtered(origin,direction,remaining,query_mask);
        if (hit.exhausted) { return vec3<f32>(0.0); }
        if (!hit.valid) {
            // A finite emitter may be inside the last entered volume.
            if (stack_size > 0u) {
                let medium = media[stack_size - 1u]; result *= hybrid_beer(medium.rgb,max(remaining,0.0),medium.a);
            }
            return result;
        }
        if (stack_size > 0u) {
            let medium = media[stack_size - 1u]; result *= hybrid_beer(medium.rgb,hit.distance,medium.a);
        }
        let s = hybrid_visibility_surface(hit,origin,direction);
        if (s.transmission <= 0.001 && s.cast_shadow && include_opaque) { return vec3<f32>(0.0); }
        if (s.transmission > 0.001 && s.cast_shadow) {
            let cosine = max(abs(dot(direction,s.geometric_normal)),0.0001);
            if (s.solid) {
                let entering = dot(direction,s.geometric_normal) < 0.0;
                let eta_i = select(1.0,iors[max(stack_size,1u)-1u],stack_size > 0u);
                var eta_t = s.ior;
                var exit_index = stack_size;
                if (!entering) {
                    for (var j = 0u; j < 4u; j += 1u) { if (j < stack_size && objects[j] == s.object) { exit_index = j; } }
                    eta_t = select(1.0,iors[max(exit_index,1u)-1u],exit_index > 0u);
                    if (exit_index + 1u < stack_size) { eta_t = eta_i; }
                }
                let fresnel = hybrid_fresnel(cosine,eta_i,eta_t);
                result *= s.transmission * (1.0 - fresnel);
                if (entering) {
                    if (stack_size >= 4u) { return vec3<f32>(0.0); }
                    objects[stack_size] = s.object; media[stack_size] = vec4<f32>(s.attenuation,s.attenuation_distance);
                    iors[stack_size] = s.ior; stack_size += 1u;
                } else if (!entering) {
                    if (exit_index < stack_size) {
                        for (var j = 0u; j < 3u; j += 1u) { if (j >= exit_index && j + 1u < stack_size) {
                            objects[j] = objects[j+1u]; media[j] = media[j+1u]; iors[j] = iors[j+1u];
                        } }
                        stack_size -= 1u;
                    }
                }
            } else {
                var paired_exit = false;
                if (s.closed) {
                    let entering = dot(direction,s.geometric_normal) < 0.0;
                    if (!entering) {
                        for (var j = 0u; j < 4u; j += 1u) {
                            if (j < slab_count && slab_objects[j] == s.object) {
                                slab_objects[j] = slab_objects[slab_count-1u];
                                slab_count -= 1u; paired_exit = true; break;
                            }
                        }
                    } else {
                        if (slab_count >= 4u) { return vec3<f32>(0.0); }
                        slab_objects[slab_count] = s.object; slab_count += 1u;
                    }
                }
                if (!paired_exit) {
                    let fresnel = hybrid_fresnel(cosine,1.0,s.ior);
                    let thickness = s.thickness / cosine;
                    result *= s.transmission * (1.0 - fresnel) * (1.0 - fresnel) * hybrid_beer(s.attenuation,thickness,s.attenuation_distance);
                }
            }
        }
        remaining -= hit.distance;
        if (remaining <= hybrid_epsilon(s.position)) { return result; }
        origin = s.position + direction * hybrid_epsilon(s.position) * 3.0;
    }
    // Exhausted visibility budgets conservatively block rather than leaking
    // an untested emitter through an incomplete closed-volume sequence.
    return vec3<f32>(0.0);
}

// The nearest solid owns this pixel's complete transmission ray. Selecting
// each object's boundary independently would let object-centre sorting
// overwrite a nearer entry where rotated/overlapping solids cross in depth.
fn primary_solid_interface_visible(camera: vec3<f32>, position: vec3<f32>, direction: vec3<f32>, object_id: u32) -> bool {
    let ray = normalize(direction);
    let camera_distance = max(dot(position-camera,ray),hybrid_epsilon(position));
    let origin = position - ray * camera_distance;
    let maximum = camera_distance + hybrid_epsilon(position) * 8.0;
    var nearest = maximum;
    var nearest_object = 0xffffffffu;
    let tie_epsilon = hybrid_epsilon(position) * 2.0;
    if (arrayLength(&hybrid_scene) < 2u || hybrid_scene[0].y < 0.5) { return false; }
    let count = u32(hybrid_scene[0].y); let base = u32(hybrid_scene[0].x);
    let ray_box=hybrid_ray_box_query(ray);
    var node = 0u; var visits = 0u;
    loop {
        if (node >= count || visits >= 2048u) { break; }
        if (!hybrid_consume_node_visit()) { return false; }
        visits += 1u;
        let a = hybrid_scene[base+node*3u]; let b = hybrid_scene[base+node*3u+1u];
        let metadata = hybrid_scene[base+node*3u+2u];
        let escape = u32(metadata.x);
        if ((u32(metadata.y) & 2u) == 0u) { node=escape;continue; }
        if (!hybrid_ray_box(origin,ray_box,a.xyz,b.xyz,min(maximum,nearest+tie_epsilon))) { node=escape;continue; }
        if (b.w > 0.5) {
            let first=u32(a.w);let size=u32(b.w);
            for (var i=0u;i<size;i+=1u) {
                let triangle_base=u32(hybrid_scene[0].z)+(first+i)*11u;
                let triangle_object = u32(hybrid_scene[triangle_base+1u].w);
                let optical = hybrid_material(first+i,1u);
                if (optical.z <= 0.001 || hybrid_material(first+i,4u).y < 0.5) { continue; }
                let hit=hybrid_intersect_triangle(origin,ray,first+i,min(maximum,nearest+tie_epsilon));
                if (hit.valid) {
                    var covered = true;
                    if (HYBRID_ALPHA_EVALUATION_ENABLED) {
                        covered = hybrid_alpha(hit) > max(hybrid_material(hit.triangle,3u).w,0.001);
                    }
                    if (covered) {
                        if (hit.distance < nearest-tie_epsilon ||
                            (abs(hit.distance-nearest) <= tie_epsilon && triangle_object < nearest_object)) {
                            nearest=hit.distance; nearest_object=triangle_object;
                        }
                    }
                }
            }
            node=escape;
        } else { node+=1u; }
    }
    if (node < count) { return false; }
    return nearest_object == object_id && abs(camera_distance-nearest) <= hybrid_epsilon(position)*8.0;
}
