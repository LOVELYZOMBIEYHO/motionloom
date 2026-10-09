// src/world/render/shaders/per_light_shadows.wgsl
struct EmitterShadowView {
    right: vec4<f32>, up: vec4<f32>, forward: vec4<f32>, origin: vec4<f32>, projection: vec4<f32>,
};
struct EmitterShadowLight { views: vec4<f32>, source: vec4<f32>, };
struct EmitterShadowParams {
    control: vec4<f32>, lights: array<EmitterShadowLight,8>, views: array<EmitterShadowView,96>,
};
@group(1) @binding(11) var emitter_shadow_texture: texture_depth_2d_array;
@group(1) @binding(12) var<uniform> emitter_shadows: EmitterShadowParams;
@group(1) @binding(13) var<uniform> emitter_shadow_view: EmitterShadowView;
const SHADOW_POISSON = array<vec2<f32>,12>(
    vec2<f32>(-.326,-.406),vec2<f32>(-.840,-.074),vec2<f32>(-.696,.457),vec2<f32>(-.203,.621),
    vec2<f32>(.962,-.195),vec2<f32>(.473,-.480),vec2<f32>(.519,.767),vec2<f32>(.185,-.893),
    vec2<f32>(.507,.064),vec2<f32>(.896,.412),vec2<f32>(-.322,-.933),vec2<f32>(-.792,-.598));
fn shadow_cube_face(delta:vec3<f32>) -> u32 {
    let d=abs(delta);
    if (d.x>=d.y && d.x>=d.z) {return select(1u,0u,delta.x>=0.0);}
    if (d.y>=d.z) {return select(3u,2u,delta.y>=0.0);}
    return select(5u,4u,delta.z>=0.0);
}
fn emitter_shadow_coordinate(world:vec3<f32>, v:EmitterShadowView) -> vec3<f32> {
    let r=world-v.origin.xyz;
    let z=dot(r,v.forward.xyz);
    if (v.projection.x<.5) {return vec3<f32>(dot(r,v.right.xyz)/v.right.w*.5+.5,.5-dot(r,v.up.xyz)/v.up.w*.5,z/v.forward.w+.5);}
    return vec3<f32>(dot(r,v.right.xyz)/max(z*v.right.w,.000001)*.5+.5,
        .5-dot(r,v.up.xyz)/max(z*v.up.w,.000001)*.5,
        v.forward.w/(v.forward.w-v.origin.w)-v.forward.w*v.origin.w/((v.forward.w-v.origin.w)*max(z,.000001)));
}
fn shadow_distance(depth:f32, v:EmitterShadowView) -> f32 {
    if(v.projection.x<.5) {return (depth-.5)*v.forward.w;}
    return v.forward.w*v.origin.w/max(v.forward.w-depth*(v.forward.w-v.origin.w),.000001);
}
// PCSS is a bounded contact-hardening approximation; source size controls its radius.
fn emitter_visibility(light_index:u32, sample_index:u32, world:vec3<f32>, normal:vec3<f32>) -> f32 {
    if(emitter_shadows.control.x<.5 || params.material10.w<.5) {return 1.0;}
    let l=emitter_shadows.lights[light_index];
    if(l.views.w<=0.0) {return 1.0;}
    return mix(1.0,emitter_opaque_visibility(light_index,sample_index,world,normal),l.views.w);
}
// Raw opaque evidence is independent of the primary fragment's material flags
// and shadow strength. Secondary hits apply their own receive flag and strength.
fn emitter_opaque_visibility(light_index:u32, sample_index:u32, world:vec3<f32>, normal:vec3<f32>) -> f32 {
    let l=emitter_shadows.lights[light_index];
    if(l.views.x<-.5) {return directional_emitter_visibility(world,normal,l.source.y,1.0);}
    let base=u32(l.views.x)+sample_index*u32(l.views.z);
    var index=base;
    if(l.views.z>1.5) {index+=shadow_cube_face(world-emitter_shadows.views[base].origin.xyz);}
    let v=emitter_shadows.views[index];
    let coordinate=emitter_shadow_coordinate(world+normal*.0008,v);
    if(any(coordinate.xy<vec2<f32>(0.0)) || any(coordinate.xy>vec2<f32>(1.0)) || coordinate.z<0.0 || coordinate.z>1.0) {return 1.0;}
    let size=vec2<f32>(textureDimensions(emitter_shadow_texture));
    let facing=abs(dot(normal,normalize(world-v.origin.xyz)));
    let bias=.000001*(1.0+2.0*(1.0-facing));
    let receiver=shadow_distance(coordinate.z,v);
    var filter_radius=.5/size.x;
    if((l.source.x>0.0 || l.source.y>0.0) && lighting.surface3.y<.5) {
        let search_radius=min(.04,select(l.source.x/max(receiver,.01)*.5/max(v.right.w,.001),l.source.y*v.forward.w/(2.0*v.right.w),v.projection.x<.5));
        var blockers=0.0;var blocker_distance=0.0;
        for(var i=0u;i<8u;i++) {
            let uv=clamp(coordinate.xy+SHADOW_POISSON[i]*search_radius,vec2<f32>(0.0),vec2<f32>(.999999));
            let depth=textureLoad(emitter_shadow_texture,vec2<i32>(uv*size),i32(index),0);
            if(depth<coordinate.z-bias) {blockers+=1.0;blocker_distance+=shadow_distance(depth,v);}
        }
        if(blockers>0.0) {
            let blocker=blocker_distance/blockers;
            let penumbra=select(l.source.x*max(receiver-blocker,0.0)/(max(blocker,.01)*max(receiver,.01))*.5/max(v.right.w,.001),
                l.source.y*max(receiver-blocker,0.0)/(2.0*v.right.w),v.projection.x<.5);
            filter_radius=max(filter_radius,min(.04,penumbra));
        }
    }
    if(lighting.surface3.y>.5) {return textureSampleCompareLevel(emitter_shadow_texture,shadow_sampler,coordinate.xy,i32(index),coordinate.z-bias);}
    var visibility=0.0;
    for(var i=0u;i<12u;i++) {
        let uv=coordinate.xy+SHADOW_POISSON[i]*filter_radius;
        // Reproject cube samples across face seams rather than clamping to one face.
        if(l.views.z>1.5) {
            let ray=normalize(v.forward.xyz+v.right.xyz*((uv.x-.5)*2.0*v.right.w)+v.up.xyz*((.5-uv.y)*2.0*v.up.w));
            let adjacent=base+shadow_cube_face(ray);
            let q=emitter_shadow_coordinate(v.origin.xyz+ray*length(world-v.origin.xyz),emitter_shadows.views[adjacent]);
            visibility+=textureSampleCompareLevel(emitter_shadow_texture,shadow_sampler,q.xy,i32(adjacent),q.z-bias);
        } else {visibility+=textureSampleCompareLevel(emitter_shadow_texture,shadow_sampler,uv,i32(index),coordinate.z-bias);}
    }
    return visibility/12.0;
}

struct SecondaryOpaqueShadow {
    visibility:f32,
    valid:bool,
};
fn secondary_shadow_coordinate_covered(coordinate:vec3<f32>, margin:f32) -> bool {
    // Positive comparisons also reject non-finite projected coordinates.
    return all(coordinate.xy>=vec2<f32>(margin)) &&
        all(coordinate.xy<=vec2<f32>(1.0-margin)) && coordinate.z>=0.0 && coordinate.z<=1.0;
}
fn secondary_emitter_opaque_evidence(light_index:u32, sample_index:u32, world:vec3<f32>, normal:vec3<f32>) -> SecondaryOpaqueShadow {
    let invalid=SecondaryOpaqueShadow(1.0,false);
    if(emitter_shadows.control.x<.5 || light_index>=8u) {return invalid;}
    // The depth pass excludes alpha-blended nontransmissive casters. Packing
    // certifies that none are present before opaque map evidence can be reused.
    if(arrayLength(&hybrid_scene)<3u || hybrid_scene[2].w<.5) {return invalid;}
    let l=emitter_shadows.lights[light_index];
    // CPU computes coverage per frame from retained geometry. Directional
    // caster depths fit their volume; local source.z means at least one view
    // has a clear near interval, checked again for the selected face below.
    // Unknown coverage retains the BVH without per-fragment geometry proofs.
    if(l.views.w<=0.0 || l.source.z<.5 || f32(sample_index)>=l.views.y) {return invalid;}
    let primary=l.views.x<-.5;
    var view=EmitterShadowView(lighting.shadow0,lighting.shadow1,lighting.shadow2,lighting.shadow3,vec4<f32>(0.0));
    var coordinate=world_to_shadow(world);
    var margin=.03+.5/f32(textureDimensions(shadow_texture).x);
    if(!primary) {
        let count=min(u32(emitter_shadows.control.y),96u);
        let faces=u32(l.views.z);
        let base=u32(l.views.x)+sample_index*faces;
        if(base>=count || (faces!=1u && faces!=6u) || base+faces>count) {return invalid;}
        var index=base;
        if(faces>1u) {index+=shadow_cube_face(world-emitter_shadows.views[base].origin.xyz);}
        view=emitter_shadows.views[index];
        if(!(view.right.w>0.0 && view.up.w>0.0 && view.forward.w>0.0)) {return invalid;}
        coordinate=emitter_shadow_coordinate(world+normal*.0008,view);
        margin=.04+.5/f32(textureDimensions(emitter_shadow_texture).x);
        if(view.projection.x>.5) {
            if(view.projection.y<.5 || !(view.origin.w>0.0 && view.forward.w>view.origin.w)) {return invalid;}
        }
    } else if(!(lighting.shadow0.w>0.0 && lighting.shadow1.w>0.0 && lighting.shadow2.w>0.0)) {
        return invalid;
    }
    // Leave edge/seam filtering to the complete geometry path. This guard
    // contains every bounded PCSS search and comparison sample in its view.
    if(!secondary_shadow_coordinate_covered(coordinate,margin)) {return invalid;}
    return SecondaryOpaqueShadow(emitter_opaque_visibility(light_index,sample_index,world,normal),true);
}
fn directional_emitter_visibility(world:vec3<f32>,normal:vec3<f32>,angle:f32,strength:f32) -> f32 {
    let coordinate=world_to_shadow(world);
    if(any(coordinate.xy<vec2<f32>(0.0)) || any(coordinate.xy>vec2<f32>(1.0)) || coordinate.z<0.0 || coordinate.z>1.0) {return 1.0;}
    let size=vec2<f32>(textureDimensions(shadow_texture));
    let facing=abs(dot(normalize(normal),normalize(lighting.shadow2.xyz)));
    let bias=.0015/max(lighting.shadow2.w,.01)*(1.0+2.0*(1.0-facing));
    var radius=.5/size.x;
    if(angle>0.0 && lighting.surface3.y<.5) {
        let search=min(.03,max(1.0/size.x,angle*lighting.shadow2.w/(2.0*lighting.shadow0.w)));
        var blockers=0.0;var blocker=0.0;
        for(var i=0u;i<8u;i++) {
            let uv=clamp(coordinate.xy+SHADOW_POISSON[i]*search,vec2<f32>(0.0),vec2<f32>(.999999));
            let depth=textureLoad(shadow_texture,vec2<i32>(uv*size),0);
            if(depth<coordinate.z-bias) {blockers+=1.0;blocker+=depth;}
        }
        if(blockers>0.0) {radius=max(radius,min(.03,angle*max(coordinate.z-blocker/blockers,0.0)*lighting.shadow2.w/(2.0*lighting.shadow0.w)));}
    }
    if(lighting.surface3.y>.5) {return mix(1.0,textureSampleCompareLevel(shadow_texture,shadow_sampler,coordinate.xy,coordinate.z-bias),strength);}
    var visible=0.0;
    for(var i=0u;i<12u;i++) {visible+=textureSampleCompareLevel(shadow_texture,shadow_sampler,coordinate.xy+SHADOW_POISSON[i]*radius,coordinate.z-bias);}
    return mix(1.0,visible/12.0,strength);
}
struct ShadowVertexOut {
    @builtin(position) position:vec4<f32>, @location(0) uv:vec2<f32>, @location(1) @interpolate(flat) instance_id:u32, @location(2) alpha:f32,
};
@vertex fn vs_per_light_shadow(input:VertexIn,@builtin(instance_index) instance_id:u32) -> ShadowVertexOut {
    params=instance_params[instance_id];
    let weights=input.weights;let sum=dot(weights,vec4<f32>(1.0));
    var p=vegetation_deform(input.position);
    if(sum>.000001) {p=(bone_transform(input.joints.x,input.position)*weights.x+bone_transform(input.joints.y,input.position)*weights.y+
        bone_transform(input.joints.z,input.position)*weights.z+bone_transform(input.joints.w,input.position)*weights.w)/sum;}
    let world=params.actor.xyz+actor_rotate((p-params.model.xyz)*params.model.w);
    let v=emitter_shadow_view;let r=world-v.origin.xyz;
    let z=dot(r,v.forward.xyz);var clip=vec4<f32>(dot(r,v.right.xyz)/v.right.w,dot(r,v.up.xyz)/v.up.w,z/v.forward.w+.5,1.0);
    if(v.projection.x>.5) {clip=vec4<f32>(dot(r,v.right.xyz)/v.right.w,dot(r,v.up.xyz)/v.up.w,
        v.forward.w/(v.forward.w-v.origin.w)*z-v.forward.w*v.origin.w/(v.forward.w-v.origin.w),z);}
    let scaled=input.uv*params.material5.xy;
    let rotated=vec2<f32>(scaled.x*params.material3.z-scaled.y*params.material3.w,scaled.x*params.material3.w+scaled.y*params.material3.z);
    var output:ShadowVertexOut;output.position=clip;output.uv=rotated+params.material5.zw+params.material3.xy;output.instance_id=instance_id;output.alpha=input.color.a;return output;
}
@fragment fn fs_per_light_shadow(input:ShadowVertexOut) {
    params=instance_params[input.instance_id];
    let alpha=textureSample(actor_texture,actor_sampler,input.uv).a*input.alpha*params.material4.a*params.style.x;
    if(params.material10.z<.5 || alpha<max(params.material7.w,.001)) {discard;}
}
