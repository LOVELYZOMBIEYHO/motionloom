// src/world/render/rough_reflections.rs
//! Coarse primary-surface reflection evidence with full-resolution material response.

use super::reflection_queries::{self, QueryTargets};

const COLOR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const OBJECT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Uint;
const ROUTE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Uint;
const TARGET_FORMATS: [wgpu::TextureFormat; 5] = [
    COLOR_FORMAT,
    NORMAL_FORMAT,
    COLOR_FORMAT,
    OBJECT_FORMAT,
    COLOR_FORMAT,
];
const BYTES_PER_PIXEL: u64 = 36;
const MIN_TRIANGLES: usize = 32_768;
const MIN_OUTPUT_PIXELS: u64 = 16_384;
const MAX_PACKED_OBJECTS: usize = 65_535;

pub(super) fn supports_evidence(limits: &wgpu::Limits) -> bool {
    // WebGPU attachment cost differs from texel storage size: Rgba8Unorm
    // costs eight attachment bytes, so these five outputs require 36.
    let attachment_bytes: u32 = TARGET_FORMATS
        .iter()
        .map(|format| format.target_pixel_byte_cost().expect("renderable evidence format"))
        .sum();
    limits.max_sampled_textures_per_shader_stage >= 21
        && limits.max_bind_groups >= 4
        && limits.max_color_attachments >= 5
        && limits.max_color_attachment_bytes_per_sample >= attachment_bytes
}

/// The viewport is smaller; the shader retains the authored full-view projection.
fn evidence_dimensions(
    width: u32,
    height: u32,
    triangles: usize,
    objects: usize,
    quality: f32,
) -> Option<[u32; 2]> {
    if width == 0
        || height == 0
        || triangles < MIN_TRIANGLES
        || objects >= MAX_PACKED_OBJECTS
        || u64::from(width) * u64::from(height) <= MIN_OUTPUT_PIXELS
        || !quality.is_finite()
        || quality < 0.5
        || quality >= 3.5
    {
        return None;
    }
    // The direct World path has no host profile (zero); Ultra retains per-pixel
    // transport. Cinematic keeps a denser grid than Portable and Balanced.
    let divisor = if quality > 2.5 { 4 } else { 8 };
    Some([
        width.div_ceil(divisor).max(1),
        height.div_ceil(divisor).max(1),
    ])
}

pub(super) struct RoughReflectionTargets {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) route_width: u32,
    pub(super) route_height: u32,
    _radiance: wgpu::Texture,
    _normals: wgpu::Texture,
    _world_position: wgpu::Texture,
    pub(super) object_id: wgpu::Texture,
    _depth: wgpu::Texture,
    _coat_radiance: wgpu::Texture,
    _route: wgpu::Texture,
    pub(super) radiance_view: wgpu::TextureView,
    pub(super) normals_view: wgpu::TextureView,
    pub(super) world_position_view: wgpu::TextureView,
    pub(super) object_id_view: wgpu::TextureView,
    pub(super) depth_view: wgpu::TextureView,
    pub(super) coat_radiance_view: wgpu::TextureView,
    pub(super) route_view: wgpu::TextureView,
    pub(super) bind_group: wgpu::BindGroup,
    pub(super) classification_bind_group: wgpu::BindGroup,
    pub(super) input_bind_group: Option<wgpu::BindGroup>,
    pub(super) query: Option<QueryTargets>,
}

impl RoughReflectionTargets {
    fn new(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        dimensions: [u32; 2],
        route_dimensions: [u32; 2],
        classification_route_view: Option<&wgpu::TextureView>,
        compaction_enabled: bool,
        query_defaults: Option<&QueryTargets>,
    ) -> Self {
        let [width, height] = dimensions;
        let [route_width, route_height] = route_dimensions;
        let texture = |label, format, [width, height]: [u32; 2]| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let radiance = texture("rough-reflection-radiance", COLOR_FORMAT, dimensions);
        let normals = texture("rough-reflection-normals", NORMAL_FORMAT, dimensions);
        let world_position = texture("rough-reflection-world-position", COLOR_FORMAT, dimensions);
        // Lower 16 bits identify the object; upper 16 bits retain float16 hit distance.
        let object_id = texture(
            "rough-reflection-object-and-distance",
            OBJECT_FORMAT,
            dimensions,
        );
        let depth = texture(
            "rough-reflection-depth",
            wgpu::TextureFormat::Depth32Float,
            dimensions,
        );
        let coat_radiance = texture("rough-reflection-coat-radiance", COLOR_FORMAT, dimensions);
        let route = texture(
            "rough-reflection-primary-route",
            ROUTE_FORMAT,
            route_dimensions,
        );
        let view = |texture: &wgpu::Texture| texture.create_view(&Default::default());
        let radiance_view = view(&radiance);
        let normals_view = view(&normals);
        let world_position_view = view(&world_position);
        let object_id_view = view(&object_id);
        let depth_view = view(&depth);
        let coat_radiance_view = view(&coat_radiance);
        let route_view = view(&route);
        let entry = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let query = compaction_enabled
            .then(|| QueryTargets::new(device, route_width, route_height))
            .flatten();
        let active_query = query.as_ref().or(query_defaults);
        let inactive_query = query_defaults.or(query.as_ref());
        let bind_group = |label, route, query: Option<&QueryTargets>| {
            let mut entries = vec![
                entry(0, &radiance_view),
                entry(1, &normals_view),
                entry(2, &world_position_view),
                entry(3, &object_id_view),
                entry(4, &coat_radiance_view),
                entry(5, route),
            ];
            if let Some(query) = query {
                entries.extend(query.entries());
            }
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout,
                entries: &entries,
            })
        };
        let surface_bind_group = bind_group(
            "rough-reflection-evidence-bind-group",
            &route_view,
            active_query,
        );
        // The classifier writes the active route attachment. Bind a different
        // default route texture to avoid an attachment/read feedback hazard.
        let classification_bind_group = bind_group(
            "rough-reflection-classification-bind-group",
            classification_route_view.unwrap_or(&route_view),
            inactive_query,
        );
        // The input producer reads the active route, but its query bindings must
        // reference defaults rather than the attachments it is about to write.
        let input_bind_group = compaction_enabled.then(|| {
            bind_group(
                "rough-reflection-query-input-bind-group",
                &route_view,
                inactive_query,
            )
        });
        Self {
            width,
            height,
            route_width,
            route_height,
            _radiance: radiance,
            _normals: normals,
            _world_position: world_position,
            object_id,
            _depth: depth,
            _coat_radiance: coat_radiance,
            _route: route,
            radiance_view,
            normals_view,
            world_position_view,
            object_id_view,
            depth_view,
            coat_radiance_view,
            route_view,
            bind_group: surface_bind_group,
            classification_bind_group,
            input_bind_group,
            query,
        }
    }
}

pub(super) struct RoughReflections {
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) pipeline: Option<wgpu::RenderPipeline>,
    pub(super) route_pipeline: Option<wgpu::RenderPipeline>,
    pub(super) default_bind_group: wgpu::BindGroup,
    pub(super) targets: Option<RoughReflectionTargets>,
    pub(super) compaction_enabled: bool,
    _defaults: RoughReflectionTargets,
}

impl RoughReflections {
    /// Evidence generation cannot read group 3: these textures are its attachments.
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shader: &wgpu::ShaderModule,
        actor_layout: &wgpu::BindGroupLayout,
        lighting_layout: &wgpu::BindGroupLayout,
        scene_layout: &wgpu::BindGroupLayout,
        geometry_transport_enabled: bool,
        compaction_enabled: bool,
    ) -> Self {
        let entry = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let floating = wgpu::TextureSampleType::Float { filterable: false };
        let compaction_enabled =
            compaction_enabled && reflection_queries::supports(&device.limits(), 1, 1);
        let mut entries = vec![
            entry(0, floating),
            entry(1, floating),
            entry(2, floating),
            entry(3, wgpu::TextureSampleType::Uint),
            entry(4, floating),
            entry(5, wgpu::TextureSampleType::Uint),
        ];
        if compaction_enabled {
            entries.extend(reflection_queries::layout_entries());
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rough-reflection-evidence-layout"),
            entries: &entries,
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rough-reflection-evidence-pipeline-layout"),
            bind_group_layouts: &[actor_layout, lighting_layout, scene_layout],
            push_constant_ranges: &[],
        });
        let target = |format| {
            Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        // Small default textures remain valid on constrained adapters; only
        // capable devices construct the layered evidence render pipeline.
        let (pipeline, route_pipeline) = if geometry_transport_enabled && supports_evidence(&device.limits()) {
            let evidence_targets = TARGET_FORMATS.map(target);
            let route_targets = [target(ROUTE_FORMAT)];
            let mut descriptor = wgpu::RenderPipelineDescriptor {
                label: Some("rough-reflection-evidence-pipeline"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 116,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &[
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 12,
                                shader_location: 1,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 24,
                                shader_location: 2,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 40,
                                shader_location: 3,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x2,
                                offset: 56,
                                shader_location: 4,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 64,
                                shader_location: 5,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 80,
                                shader_location: 6,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 92,
                                shader_location: 7,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x3,
                                offset: 104,
                                shader_location: 8,
                            },
                        ],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_rough_reflection_evidence"),
                    compilation_options: Default::default(),
                    targets: &evidence_targets,
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Greater,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            };
            let pipeline = device.create_render_pipeline(&descriptor);
            let route_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("rough-reflection-route-pipeline-layout"),
                bind_group_layouts: &[actor_layout, lighting_layout, scene_layout, &layout],
                push_constant_ranges: &[],
            });
            descriptor.label = Some("rough-reflection-route-pipeline");
            descriptor.layout = Some(&route_layout);
            let fragment = descriptor
                .fragment
                .as_mut()
                .expect("evidence has a fragment stage");
            fragment.entry_point = Some("fs_reflection_route");
            fragment.compilation_options = Default::default();
            fragment.targets = &route_targets;
            let route_pipeline = device.create_render_pipeline(&descriptor);
            (Some(pipeline), Some(route_pipeline))
        } else {
            (None, None)
        };
        let defaults = RoughReflectionTargets::new(
            device,
            &layout,
            [1, 1],
            [1, 1],
            None,
            compaction_enabled,
            None,
        );
        // Zero-initialized radiance is a legitimate miss. An impossible ID is
        // necessary so inactive evidence never impersonates scene object zero.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &defaults.object_id,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &u32::MAX.to_le_bytes(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        // Full is zero, including every unsupported or inactive path.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &defaults._route,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let default_bind_group = defaults.bind_group.clone();
        Self {
            layout,
            pipeline,
            route_pipeline,
            default_bind_group,
            targets: None,
            compaction_enabled,
            _defaults: defaults,
        }
    }

    /// Clear and populate active targets before binding them to surface shading.
    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        triangles: usize,
        objects: usize,
        quality: f32,
    ) -> bool {
        if self.pipeline.is_none() || self.route_pipeline.is_none() {
            self.targets = None;
            return false;
        }
        let Some([evidence_width, evidence_height]) =
            evidence_dimensions(width, height, triangles, objects, quality)
        else {
            self.targets = None;
            return false;
        };
        if !self.targets.as_ref().is_some_and(|targets| {
            targets.width == evidence_width
                && targets.height == evidence_height
                && targets.route_width == width
                && targets.route_height == height
        }) {
            self.targets = Some(RoughReflectionTargets::new(
                device,
                &self.layout,
                [evidence_width, evidence_height],
                [width, height],
                Some(&self._defaults.route_view),
                self.compaction_enabled,
                self._defaults.query.as_ref(),
            ));
        }
        true
    }

    pub(super) fn bind_group(&self) -> &wgpu::BindGroup {
        self.targets
            .as_ref()
            .map_or(&self.default_bind_group, |targets| &targets.bind_group)
    }

    pub(super) fn bind_group_for_classification(&self) -> &wgpu::BindGroup {
        self.targets
            .as_ref()
            .map_or(&self._defaults.classification_bind_group, |targets| {
                &targets.classification_bind_group
            })
    }

    pub(super) fn input_bind_group(&self) -> Option<&wgpu::BindGroup> {
        self.targets
            .as_ref()
            .and_then(|targets| targets.input_bind_group.as_ref())
    }

    pub(super) fn bytes(&self) -> u64 {
        // Coarse evidence costs 36 B/pixel; routing costs one byte for each
        // full-resolution output pixel. Include each retained default once.
        BYTES_PER_PIXEL
            + 1
            + self._defaults.query.as_ref().map_or(0, QueryTargets::bytes)
            + self.targets.as_ref().map_or(0, |targets| {
                u64::from(targets.width) * u64::from(targets.height) * BYTES_PER_PIXEL
                    + u64::from(targets.route_width) * u64::from(targets.route_height)
                    + targets.query.as_ref().map_or(0, QueryTargets::bytes)
            })
    }

    pub(super) fn size(&self) -> Option<[u32; 2]> {
        self.targets
            .as_ref()
            .map(|targets| [targets.width, targets.height])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constrained_devices_never_construct_layered_evidence_pipelines() {
        let mut exact = wgpu::Limits::default();
        exact.max_sampled_textures_per_shader_stage = 21;
        exact.max_bind_groups = 4;
        exact.max_color_attachments = 5;
        exact.max_color_attachment_bytes_per_sample = 36;
        assert!(supports_evidence(&exact));
        assert!(!supports_evidence(&wgpu::Limits::downlevel_defaults()));
        for missing in 0..4 {
            let mut limited = exact.clone();
            match missing {
                0 => limited.max_sampled_textures_per_shader_stage = 20,
                1 => limited.max_bind_groups = 3,
                2 => limited.max_color_attachments = 4,
                _ => limited.max_color_attachment_bytes_per_sample = 35,
            }
            assert!(!supports_evidence(&limited));
        }
    }

    #[test]
    fn layered_evidence_accounts_for_webgpu_attachment_cost_separately_from_storage() {
        let color_bytes: u32 = TARGET_FORMATS
            .iter()
            .map(|format| format.block_copy_size(None).unwrap())
            .sum();
        assert_eq!(color_bytes, 32);
        let attachment_bytes: u32 = TARGET_FORMATS
            .iter()
            .map(|format| format.target_pixel_byte_cost().unwrap())
            .sum();
        assert_eq!(attachment_bytes, 36);
        assert!(attachment_bytes > wgpu::Limits::default().max_color_attachment_bytes_per_sample);
        assert!(TARGET_FORMATS.len() <= wgpu::Limits::default().max_color_attachments as usize);
        let depth_bytes = wgpu::TextureFormat::Depth32Float
            .block_copy_size(Some(wgpu::TextureAspect::DepthOnly))
            .unwrap();
        assert_eq!(u64::from(color_bytes + depth_bytes), BYTES_PER_PIXEL);
        assert_eq!(ROUTE_FORMAT.block_copy_size(None), Some(1));
    }

    #[test]
    fn evidence_requires_large_profiled_scenes_and_retains_ultra() {
        assert_eq!(evidence_dimensions(1920, 1080, 32_767, 770, 3.0), None);
        assert_eq!(evidence_dimensions(128, 128, 32_768, 770, 3.0), None);
        assert_eq!(evidence_dimensions(0, 1080, 32_768, 770, 3.0), None);
        assert_eq!(evidence_dimensions(1920, 1080, 32_768, 65_535, 3.0), None);
        assert_eq!(
            evidence_dimensions(1920, 1080, 32_768, usize::MAX, 3.0),
            None
        );
        for quality in [0.0, 4.0, f32::NAN, f32::INFINITY] {
            assert_eq!(evidence_dimensions(1920, 1080, 32_768, 770, quality), None);
        }
    }

    #[test]
    fn profile_grids_cover_odd_sizes_and_bound_reflection_pixels() {
        assert_eq!(
            evidence_dimensions(1920, 1080, 32_768, 770, 3.0),
            Some([480, 270])
        );
        for quality in [1.0, 2.0] {
            assert_eq!(
                evidence_dimensions(1920, 1080, 32_768, 770, quality),
                Some([240, 135])
            );
        }
        assert_eq!(
            evidence_dimensions(1921, 1081, 32_768, 770, 3.0),
            Some([481, 271])
        );
        assert_eq!(
            evidence_dimensions(1, 16_385, 32_768, 770, 1.0),
            Some([1, 2049])
        );
        assert_eq!(
            evidence_dimensions(1920, 1080, 32_768, 65_534, 3.0),
            Some([480, 270])
        );
    }
}
