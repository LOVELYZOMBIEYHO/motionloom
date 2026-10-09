// src/world/render/per_light_shadows.rs
//! Bounded, cached per-emitter depth views. Point lights always use six faces.
use super::*;
use wgpu::util::DeviceExt;
const MAX_VIEWS: usize = 96;
const PARAM_BYTES: u64 = 16 + 8 * 32 + MAX_VIEWS as u64 * 80;
#[derive(Clone, Copy, Debug)]
struct ShadowView {
    vectors: [[f32; 4]; 5],
}
#[derive(Debug)]
pub(super) struct ShadowPlan {
    pub(super) enabled: bool,
    primary: Option<ShadowView>,
    flags: HashMap<String, [bool; 2]>,
    resolution: u32,
    samples: u32,
    lights: [[[f32; 4]; 2]; 8],
    views: Vec<ShadowView>,
}
impl ShadowPlan {
    pub(super) fn secondary_coverage(&self) -> [bool; 8] {
        self.lights.map(|light| light[0][3] > 0.0 && light[1][2] > 0.5)
    }
    pub(super) fn new(lighting: &GpuWorldLighting, fitted: &GpuWorldLightingParams) -> Self {
        let enabled = lighting.per_light_shadows;
        let resolution = if fitted.surface3[2] <= 1024.0 {
            256
        } else {
            512
        };
        let mut samples = fitted.preview2[3].max(1.0).min(4.0) as u32;
        let max_views = if fitted.surface3[2] <= 1536.0 {
            64
        } else {
            MAX_VIEWS
        };
        let count = fitted.environment2[1].max(0.0).min(8.0) as usize;
        let sources = &lighting.shadow_lights[..lighting.shadow_lights.len().min(count)];
        let projected_count = |n: u32| {
            sources
                .iter()
                .enumerate()
                .filter(|(_, l)| l.cast_shadow)
                .map(|(i, l)| match l.kind {
                    WorldLightKind::Directional if i as f32 == fitted.render_compat[0] => 0,
                    WorldLightKind::Directional | WorldLightKind::Spot => 1,
                    WorldLightKind::Point => 6,
                    WorldLightKind::RectArea => n as usize * 6,
                })
                .sum::<usize>()
        };
        while samples > 1 && projected_count(samples) > max_views {
            samples = if samples > 2 { 2 } else { 1 };
        }
        let mut plan = Self {
            enabled,
            primary: None,
            flags: lighting.model_shadow_flags.clone(),
            resolution,
            samples,
            lights: [[[0.; 4]; 2]; 8],
            views: Vec::new(),
        };
        if !enabled {
            return plan;
        }
        // Froxel transport still reads the fitted owner map. Retain that map
        // for a shadowed local volumetric source alongside its surface views.
        if lighting.froxel.as_ref().is_some_and(|f| f.shadowed) && fitted.color1[3] > 0.0 {
            plan.primary = Some(ShadowView {
                vectors: [
                    fitted.shadow0,
                    fitted.shadow1,
                    fitted.shadow2,
                    fitted.shadow3,
                    [0.; 4],
                ],
            });
        }
        for (i, light) in sources.iter().enumerate() {
            if !light.cast_shadow {
                continue;
            }
            let radius = light.source_radius;
            let angle = (light.angular_diameter.to_radians() * 0.5).tan();
            plan.lights[i][1] = [radius, angle, 0., 0.];
            if light.kind == WorldLightKind::Directional && i as f32 == fitted.render_compat[0] {
                plan.lights[i][0] = [-1., 1., 0., light.shadow_strength];
                plan.primary = Some(ShadowView {
                    vectors: [
                        fitted.shadow0,
                        fitted.shadow1,
                        fitted.shadow2,
                        fitted.shadow3,
                        [0.; 4],
                    ],
                });
                continue;
            }
            let first = plan.views.len();
            match light.kind {
                WorldLightKind::Directional => {
                    let forward = normalize3(light.direction);
                    let reference = if forward[1].abs() > 0.95 {
                        [0., 0., 1.]
                    } else {
                        [0., 1., 0.]
                    };
                    let right = normalize3(cross3(reference, forward));
                    let up = normalize3(cross3(forward, right));
                    plan.views.push(ShadowView {
                        vectors: [
                            [right[0], right[1], right[2], fitted.shadow0[3]],
                            [up[0], up[1], up[2], fitted.shadow1[3]],
                            [forward[0], forward[1], forward[2], fitted.shadow2[3]],
                            fitted.shadow3,
                            [0.; 4],
                        ],
                    });
                }
                WorldLightKind::Spot => {
                    let extent = light.outer_cone_degrees.to_radians().tan().max(0.01);
                    plan.views.push(perspective(
                        light.position,
                        light.direction,
                        light.range,
                        extent,
                    ));
                }
                WorldLightKind::Point => plan.views.extend(cube(light.position, light.range)),
                WorldLightKind::RectArea => {
                    let normal = normalize3(light.direction);
                    let reference = if normal[1].abs() > 0.95 {
                        [1., 0., 0.]
                    } else {
                        [0., 1., 0.]
                    };
                    let right = normalize3(cross3(reference, normal));
                    let up = normalize3(cross3(normal, right));
                    for sample in 0..samples {
                        let index = match samples {
                            1 => 4,
                            2 => sample * 3,
                            _ => sample,
                        };
                        let x = if index == 4 {
                            0.
                        } else if index & 1 == 0 {
                            -0.288675
                        } else {
                            0.288675
                        };
                        let y = if index == 4 {
                            0.
                        } else if index & 2 == 0 {
                            -0.288675
                        } else {
                            0.288675
                        };
                        let position = std::array::from_fn(|axis| {
                            light.position[axis]
                                + right[axis] * x * light.width
                                + up[axis] * y * light.height
                        });
                        plan.views.extend(cube(position, light.range));
                    }
                }
            }
            let faces = if matches!(light.kind, WorldLightKind::Point | WorldLightKind::RectArea) {
                6.
            } else {
                1.
            };
            let strength = if light.kind == WorldLightKind::Directional {
                light.shadow_strength
            } else {
                1.
            };
            plan.lights[i][0] = [
                first as f32,
                if light.kind == WorldLightKind::RectArea {
                    samples as f32
                } else {
                    1.
                },
                faces,
                strength,
            ];
        }
        debug_assert!(plan.views.len() <= MAX_VIEWS);
        plan
    }
    /// Only reuse opaque maps for secondary hits when their clipped intervals
    /// cannot omit a caster. Evaluated BVH geometry includes skinning and wind.
    pub(super) fn certify_secondary(&mut self, scene: &hybrid::HybridScene) {
        for light in &mut self.lights {
            light[1][2] = 0.0;
        }
        if !self.enabled || scene.vectors[2][3] < 0.5 {
            return;
        }
        for view in &mut self.views {
            if view.vectors[4][0] < 0.5 {
                fit_directional_depth(&mut view.vectors, scene.opaque_caster_bounds);
            } else {
                view.vectors[4][1] = perspective_near_bounds(*view).is_some_and(|(minimum, maximum)| {
                    !scene.opaque_caster_overlaps(minimum, maximum)
                }) as u8 as f32;
            }
        }
        for light in &mut self.lights {
            if light[0][3] <= 0.0 {
                continue;
            }
            let certified = if light[0][0] < -0.5 {
                self.primary.is_some_and(|view| directional_casters_covered(view, scene.opaque_caster_bounds))
            } else {
                let first = light[0][0] as usize;
                let count = light[0][1] as usize * light[0][2] as usize;
                self.views.get(first..first + count).is_some_and(|views| {
                    views.iter().any(|view| {
                        if view.vectors[4][0] < 0.5 {
                            return directional_casters_covered(*view, scene.opaque_caster_bounds);
                        }
                        view.vectors[4][1] > 0.5
                    })
                })
            };
            // source.z is internal coverage evidence, not a material setting.
            light[1][2] = certified as u8 as f32;
        }
    }
    fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(PARAM_BYTES as usize);
        let control = [
            self.enabled as u8 as f32,
            self.views.len() as f32,
            self.samples as f32,
            self.resolution as f32,
        ];
        for value in control
            .into_iter()
            .chain(self.lights.iter().flatten().flatten().copied())
        {
            bytes.extend_from_slice(&value.to_ne_bytes());
        }
        for i in 0..MAX_VIEWS {
            for value in self
                .views
                .get(i)
                .map_or([[0.; 4]; 5], |v| v.vectors)
                .into_iter()
                .flatten()
            {
                bytes.extend_from_slice(&value.to_ne_bytes());
            }
        }
        bytes
    }
}
fn perspective_near_bounds(view: ShadowView) -> Option<([f32; 3], [f32; 3])> {
    let [right, up, forward, origin, _] = view.vectors;
    let near = origin[3];
    if !(near > 0.0 && forward[3] > near && right[3] > 0.0 && up[3] > 0.0) {
        return None;
    }
    // Enclose this face's clipped pyramid, rather than unrelated directions
    // behind the emitter. The receiver guard contains every PCSS tap in-face.
    let center: [f32; 3] = std::array::from_fn(|axis| origin[axis] + forward[axis] * near * 0.5);
    let extent: [f32; 3] = std::array::from_fn(|axis| {
        forward[axis].abs() * near * 0.5 + right[axis].abs() * near * right[3]
            + up[axis].abs() * near * up[3] + 0.0002
    });
    if !center.into_iter().chain(extent).all(f32::is_finite) { return None; }
    Some((std::array::from_fn(|axis| center[axis] - extent[axis]),
          std::array::from_fn(|axis| center[axis] + extent[axis])))
}
/// Preserve directional texel density in XY while retaining upstream casters
/// throughout the depth interval. The primary uniform and pass must agree.
pub(super) fn fit_primary_directional_depth(params: &mut GpuWorldLightingParams, scene: &hybrid::HybridScene) {
    let owner = params.render_compat[0];
    if owner < 0.0 || owner >= 8.0 || params.lights[owner as usize][3] > 0.5 {
        return;
    }
    let mut vectors = [params.shadow0, params.shadow1, params.shadow2, params.shadow3, [0.0; 4]];
    fit_directional_depth(&mut vectors, scene.opaque_caster_bounds);
    params.shadow2 = vectors[2];
}
fn fit_directional_depth(vectors: &mut [[f32; 4]; 5], bounds: Option<([f32; 3], [f32; 3])>) {
    let Some((minimum, maximum)) = bounds else { return; };
    let mut radius = vectors[2][3] * 0.5;
    for corner in 0..8 {
        let relative: [f32; 3] = std::array::from_fn(|axis| {
            (if corner & (1 << axis) == 0 { minimum[axis] } else { maximum[axis] }) - vectors[3][axis]
        });
        let projected = (0..3).map(|axis| relative[axis] * vectors[2][axis]).sum::<f32>();
        if !projected.is_finite() { return; }
        radius = radius.max(projected.abs() + 0.001);
    }
    if radius.is_finite() && radius > 0.0 { vectors[2][3] = radius * 2.0; }
}
fn directional_casters_covered(view: ShadowView, bounds: Option<([f32; 3], [f32; 3])>) -> bool {
    let Some((minimum, maximum)) = bounds else { return true; };
    let [right, up, forward, origin, _] = view.vectors;
    if forward[3] > 1_000_000.0 || ![right[3], up[3], forward[3]].into_iter().all(|v| v.is_finite() && v > 0.0) {
        return false;
    }
    (0..8).all(|corner| {
        let relative: [f32; 3] = std::array::from_fn(|axis| {
            (if corner & (1 << axis) == 0 { minimum[axis] } else { maximum[axis] }) - origin[axis]
        });
        // Every represented PCSS tap is parallel to the orthographic forward
        // basis. Off-map XY casters cannot block an interior tap; the shader
        // independently proves that the complete filter footprint stays in XY.
        let projected = (0..3).map(|i| relative[i] * forward[i]).sum::<f32>();
        let extent = forward[3] * 0.5;
        projected.is_finite() && projected >= -extent && projected <= extent
    })
}
fn perspective(origin: [f32; 3], direction: [f32; 3], far: f32, extent: f32) -> ShadowView {
    let forward = normalize3(direction);
    let reference = if forward[1].abs() > 0.95 {
        [0., 0., 1.]
    } else {
        [0., 1., 0.]
    };
    let right = normalize3(cross3(reference, forward));
    let up = normalize3(cross3(forward, right));
    ShadowView {
        vectors: [
            [right[0], right[1], right[2], extent],
            [up[0], up[1], up[2], extent],
            [forward[0], forward[1], forward[2], far.max(0.1)],
            [origin[0], origin[1], origin[2], 0.01],
            [1., 0., 0., 0.],
        ],
    }
}
fn cube(origin: [f32; 3], far: f32) -> [ShadowView; 6] {
    [
        [1., 0., 0.],
        [-1., 0., 0.],
        [0., 1., 0.],
        [0., -1., 0.],
        [0., 0., 1.],
        [0., 0., -1.],
    ]
    .map(|d| perspective(origin, d, far, 1.))
}
pub(super) fn depth_array_layout(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            multisampled: false,
            view_dimension: wgpu::TextureViewDimension::D2Array,
            sample_type: wgpu::TextureSampleType::Depth,
        },
        count: None,
    }
}
pub(super) struct ShadowResources {
    pub(super) texture: wgpu::Texture,
    pub(super) params: wgpu::Buffer,
    resolution: u32,
    layers: u32,
    signatures: Vec<Option<u64>>,
    primary_signature: Option<u64>,
    pub(super) model_flags: HashMap<String, [bool; 2]>,
    pub(super) rendered_views: usize,
    pub(super) cached_views: usize,
    pub(super) view_count: usize,
}
impl ShadowResources {
    pub(super) fn estimated_bytes(&self) -> u64 {
        u64::from(self.resolution) * u64::from(self.resolution) * u64::from(self.layers) * 4
            + PARAM_BYTES
    }
    pub(super) fn new(device: &wgpu::Device, resolution: u32, layers: u32) -> Self {
        Self {
            texture: device.create_texture(&wgpu::TextureDescriptor {
                label: Some("per-emitter-shadow-array"),
                size: wgpu::Extent3d {
                    width: resolution,
                    height: resolution,
                    depth_or_array_layers: layers,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }),
            params: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("per-light-shadow-params"),
                size: PARAM_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            resolution,
            layers,
            signatures: vec![None; layers as usize],
            primary_signature: None,
            model_flags: HashMap::new(),
            rendered_views: 0,
            cached_views: 0,
            view_count: 0,
        }
    }
}
fn caster_signature(calls: &[GpuWorldDraw], flags: &HashMap<String, [bool; 2]>) -> u64 {
    let mut caster_hash = DefaultHasher::new();
    for draw in calls.iter().filter(|d| d.phase == GpuWorldDrawPhase::Opaque) {
        let id = draw.instance_key.actor_id.split("::").next()
            .unwrap_or(&draw.instance_key.actor_id);
        if !flags.get(id).copied().unwrap_or([true, true])[0] {
            continue;
        }
        draw.vertex_signature.hash(&mut caster_hash);
        draw.texture.signature.hash(&mut caster_hash);
        for value in draw.params.model.into_iter()
            .chain(draw.params.actor)
            .chain(draw.params.actor_rotation)
            .chain(draw.params.style)
            .chain(draw.params.material0)
            .chain(draw.params.material3)
            .chain(draw.params.material4)
            .chain(draw.params.material5)
            .chain(draw.params.material7)
        {
            value.to_bits().hash(&mut caster_hash);
        }
        for value in draw.bone_matrices.iter().flatten() {
            value.to_bits().hash(&mut caster_hash);
        }
        if draw.params.vegetation[0] > 0. {
            for value in draw.params.vegetation {
                value.to_bits().hash(&mut caster_hash);
            }
        }
    }
    caster_hash.finish()
}

impl GpuWorldRenderer {
    pub(super) fn prepare_per_light_shadows(&mut self, plan: &ShadowPlan) {
        let layers = plan.views.len().max(1) as u32;
        let resolution = if plan.enabled { plan.resolution } else { 1 };
        if self.per_light_shadows.layers != layers
            || self.per_light_shadows.resolution != resolution
        {
            self.per_light_shadows = ShadowResources::new(&self.device, resolution, layers);
            self.environment_resource = None;
        }
        self.per_light_shadows.rendered_views = 0;
        self.per_light_shadows.model_flags = if plan.enabled {
            plan.flags.clone()
        } else {
            HashMap::new()
        };
        self.per_light_shadows.cached_views = 0;
        self.per_light_shadows.view_count = plan.views.len() + usize::from(plan.primary.is_some());
        self.queue
            .write_buffer(&self.per_light_shadows.params, 0, &plan.bytes());
    }
    /// A camera-only update does not invalidate any retained shadow view.
    /// Off-camera resources may be skipped only after this complete-scene proof.
    pub(super) fn per_light_depth_valid(&self, plan: &ShadowPlan, calls: &[GpuWorldDraw]) -> bool {
        if !plan.enabled { return false; }
        let caster = caster_signature(calls, &plan.flags);
        let signature = |view: &ShadowView, primary: bool| {
            let mut hash = DefaultHasher::new();
            caster.hash(&mut hash);
            if primary { self.shadow_texture.width().hash(&mut hash); }
            for value in view.vectors.iter().flatten() { value.to_bits().hash(&mut hash); }
            hash.finish()
        };
        plan.primary.as_ref().is_none_or(|view|
            self.per_light_shadows.primary_signature == Some(signature(view, true)))
            && plan.views.iter().enumerate().all(|(i, view)|
                self.per_light_shadows.signatures[i] == Some(signature(view, false)))
    }

    pub(super) fn encode_per_light_shadows(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        plan: &ShadowPlan,
        calls: &[GpuWorldDraw],
        draws: &[GpuWorldDrawResources],
    ) {
        if !plan.enabled {
            return;
        }
        let caster_signature = caster_signature(calls, &plan.flags);
        let stride = self
            .device
            .limits()
            .min_uniform_buffer_offset_alignment
            .max(80) as usize;
        let pass_views = plan
            .primary
            .iter()
            .map(|v| (-1i32, *v))
            .chain(plan.views.iter().enumerate().map(|(i, v)| (i as i32, *v)))
            .collect::<Vec<_>>();
        let mut bytes = vec![0u8; stride * pass_views.len()];
        let mut changed = Vec::new();
        for (uniform_index, (layer, view)) in pass_views.iter().enumerate() {
            let mut hash = DefaultHasher::new();
            caster_signature.hash(&mut hash);
            if *layer < 0 {
                self.shadow_texture.width().hash(&mut hash);
            }
            for (j, value) in view.vectors.iter().flatten().enumerate() {
                value.to_bits().hash(&mut hash);
                bytes[uniform_index * stride + j * 4..uniform_index * stride + j * 4 + 4]
                    .copy_from_slice(&value.to_ne_bytes());
            }
            let signature = hash.finish();
            let previous = if *layer < 0 {
                self.per_light_shadows.primary_signature
            } else {
                self.per_light_shadows.signatures[*layer as usize]
            };
            if previous != Some(signature) {
                changed.push((uniform_index, *layer, signature));
            }
        }
        self.per_light_shadows.cached_views = pass_views.len() - changed.len();
        self.per_light_shadows.rendered_views = changed.len();
        if changed.is_empty() {
            return;
        }
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shadow-view-uniforms"),
                contents: &bytes,
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow-view-pass"),
            layout: &self.per_light_shadow_pass_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 13,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(80),
                }),
            }],
        });
        for (uniform_index, layer, signature) in changed {
            let target = if layer < 0 {
                self.shadow_texture.create_view(&Default::default())
            } else {
                self.per_light_shadows
                    .texture
                    .create_view(&wgpu::TextureViewDescriptor {
                        dimension: Some(wgpu::TextureViewDimension::D2),
                        base_array_layer: layer as u32,
                        array_layer_count: Some(1),
                        ..Default::default()
                    })
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("per-emitter-shadow-view"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.per_light_shadow_pipeline);
            pass.set_bind_group(1, &group, &[(uniform_index * stride) as u32]);
            for draw in draws
                .iter()
                .filter(|d| d.phase == GpuWorldDrawPhase::Opaque)
            {
                pass.set_bind_group(0, &draw.bind_group, &[]);
                pass.set_vertex_buffer(0, draw.vertex_buffer.slice(..));
                pass.set_index_buffer(draw.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..draw.index_count, 0, 0..draw.instance_count);
            }
            drop(pass);
            if layer < 0 {
                self.per_light_shadows.primary_signature = Some(signature);
            } else {
                self.per_light_shadows.signatures[layer as usize] = Some(signature);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_light(kind: WorldLightKind) -> WorldLight {
        WorldLight {
            id: Some("fixture-emitter".into()),
            kind,
            position: [1.0, 2.0, 3.0],
            direction: [0.0, -1.0, 0.0],
            color: [1.0; 3],
            intensity: 4.0,
            range: 8.0,
            inner_cone_degrees: 22.0,
            outer_cone_degrees: 35.0,
            width: 0.8,
            height: 0.6,
            cast_shadow: true,
            shadow_strength: 1.0,
            angular_diameter: 0.0,
            source_radius: 0.025,
        }
    }

    fn fixture_lighting(lights: Vec<WorldLight>) -> GpuWorldLighting {
        let mut lighting = GpuWorldLighting::fallback(PerspectiveCameraView {
            orthographic: false,
            eye: [0.0, 2.0, 5.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
            forward: [0.0, 0.0, -1.0],
            focal_px: 64.0,
            near: 0.01,
            far: 100.0,
            aspect: 1.0,
            optics: [0.0; 4],
        });
        lighting.per_light_shadows = true;
        lighting.params.environment2[1] = lights.len() as f32;
        lighting.params.surface3[2] = 2048.0;
        lighting.params.preview2[3] = 4.0;
        lighting.params.render_compat[0] = 0.0;
        lighting.shadow_lights = lights;
        lighting
    }

    #[test]
    fn a_shadowed_volumetric_spot_retains_owner_map_and_its_surface_view() {
        let spot = fixture_light(WorldLightKind::Spot);
        let mut lighting = fixture_lighting(vec![spot.clone()]);
        lighting.params.color1[3] = 1.0;
        lighting.froxel = Some(GpuFroxelSettings {
            tile_size: 8,
            depth_slices: 32,
            density: 0.02,
            scattering: [1.0; 3],
            base_height: 0.0,
            height_falloff: 0.0,
            edge_feather: 0.1,
            affect_environment: false,
            bounds_min: None,
            bounds_max: None,
            light: spot.clone(),
            intensity: 1.0,
            anisotropy: 0.0,
            max_distance: 8.0,
            shadowed: true,
            debug_view: 0,
            caustics: None,
        });
        let plan = ShadowPlan::new(&lighting, &lighting.params);
        let owner = plan
            .primary
            .expect("froxel transport needs its fitted owner depth map");
        assert_eq!(
            owner.vectors,
            [
                lighting.params.shadow0,
                lighting.params.shadow1,
                lighting.params.shadow2,
                lighting.params.shadow3,
                [0.0; 4]
            ]
        );
        assert_eq!(
            plan.views.len(),
            1,
            "surface spot visibility needs a separate perspective view"
        );
        assert_eq!(plan.lights[0][0], [0.0, 1.0, 1.0, 1.0]);
        assert_eq!(
            plan.views[0].vectors[3],
            [spot.position[0], spot.position[1], spot.position[2], 0.01]
        );
        assert_eq!(
            plan.views[0].vectors[4][0], 1.0,
            "surface spot view must remain perspective"
        );

        lighting.froxel.as_mut().unwrap().shadowed = false;
        let no_volume_shadow = ShadowPlan::new(&lighting, &lighting.params);
        assert!(no_volume_shadow.primary.is_none());
        assert_eq!(
            no_volume_shadow.views.len(),
            1,
            "disabling froxel occlusion must keep surface occlusion"
        );
        lighting.froxel.as_mut().unwrap().shadowed = true;
        lighting.params.color1[3] = 0.0;
        assert!(
            ShadowPlan::new(&lighting, &lighting.params)
                .primary
                .is_none(),
            "a disabled fitted owner needs no retained volume map"
        );
    }

    #[test]
    fn eight_shadowed_area_lights_reduce_quadrature_to_the_view_budget() {
        let mut lighting = fixture_lighting(
            (0..8)
                .map(|i| {
                    let mut light = fixture_light(WorldLightKind::RectArea);
                    light.id = Some(format!("area-{i}"));
                    light.position[0] = i as f32;
                    light
                })
                .collect(),
        );
        // Four samples would need 192 cube faces. The 96-view tier must
        // preserve all eight emitters by reducing to two samples each.
        let cinematic = ShadowPlan::new(&lighting, &lighting.params);
        assert_eq!(cinematic.samples, 2);
        assert_eq!(cinematic.views.len(), 96);
        assert!(cinematic.primary.is_none());
        for (i, descriptor) in cinematic.lights.iter().enumerate() {
            assert_eq!(descriptor[0], [(i * 12) as f32, 2.0, 6.0, 1.0]);
        }
        assert_eq!(cinematic.bytes().len(), PARAM_BYTES as usize);

        // The 64-view tier must continue through two to one, yielding 48
        // faces; it must never truncate cube faces or drop the last light.
        lighting.params.surface3[2] = 1536.0;
        let balanced = ShadowPlan::new(&lighting, &lighting.params);
        assert_eq!(balanced.samples, 1);
        assert_eq!(balanced.views.len(), 48);
        for (i, descriptor) in balanced.lights.iter().enumerate() {
            assert_eq!(descriptor[0], [(i * 6) as f32, 1.0, 6.0, 1.0]);
            for face in &balanced.views[i * 6..i * 6 + 6] {
                assert_eq!(&face.vectors[3][..3], &lighting.shadow_lights[i].position);
            }
        }
    }

    #[test]
    fn point_faces_cover_all_axes_and_layout_is_exact() {
        let faces = cube([1., 2., 3.], 5.);
        for (axis, face) in faces.iter().enumerate() {
            assert_eq!(
                face.vectors[2][axis / 2],
                if axis % 2 == 0 { 1. } else { -1. }
            );
        }
        let p = ShadowPlan {
            enabled: false,
            primary: None,
            flags: Default::default(),
            resolution: 1,
            samples: 1,
            lights: [[[0.; 4]; 2]; 8],
            views: vec![],
        };
        assert_eq!(p.bytes().len(), PARAM_BYTES as usize);
    }

    #[test]
    fn directional_certificate_retains_caster_depth_and_preserves_xy_texel_density() {
        let view = ShadowView { vectors: [
            [1.0, 0.0, 0.0, 2.0],
            [0.0, 1.0, 0.0, 3.0],
            [0.0, 0.0, 1.0, 8.0],
            [5.0, 2.0, -1.0, 0.0],
            [0.0; 4],
        ] };
        assert!(directional_casters_covered(view, Some(([3.0, -1.0, -5.0], [7.0, 5.0, 3.0]))));
        assert!(directional_casters_covered(view, Some(([-30.0, -50.0, -5.0], [70.0, 50.0, 3.0]))));
        assert!(!directional_casters_covered(view, Some(([3.0, -1.0, -5.0], [7.0, 5.0, 3.001]))));
        let mut fitted = view;
        fit_directional_depth(&mut fitted.vectors, Some(([-30.0, -50.0, -15.0], [70.0, 50.0, 13.0])));
        assert_eq!(fitted.vectors[0], view.vectors[0]);
        assert_eq!(fitted.vectors[1], view.vectors[1]);
        assert!(directional_casters_covered(fitted, Some(([-30.0, -50.0, -15.0], [70.0, 50.0, 13.0]))));
        let mut invalid = view;
        invalid.vectors[0][3] = f32::NAN;
        assert!(!directional_casters_covered(invalid, Some(([5.0, 2.0, -1.0], [5.0, 2.0, -1.0]))));
    }

    #[test]
    fn local_near_certificate_bounds_cover_each_clipped_pyramid_independently() {
        let origin = [1.0, 2.0, 3.0];
        for view in cube(origin, 8.0) {
            let (minimum, maximum) = perspective_near_bounds(view).unwrap();
            let [right, up, forward, _, _] = view.vectors;
            for distance in [0.0, 0.005, 0.01] {
                for x in [-1.0, 1.0] {
                    for y in [-1.0, 1.0] {
                        for axis in 0..3 {
                            let point = origin[axis] + distance * (forward[axis] + right[axis] * x + up[axis] * y);
                            assert!(point >= minimum[axis] && point <= maximum[axis]);
                        }
                    }
                }
            }
            // Geometry behind this face's emitter does not occupy its near interval.
            let axis = (0..3).find(|&axis| forward[axis].abs() > 0.5).unwrap();
            let behind = origin[axis] - forward[axis] * 0.007;
            assert!(behind < minimum[axis] || behind > maximum[axis]);
        }
    }
}
