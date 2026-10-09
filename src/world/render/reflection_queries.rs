// src/world/render/reflection_queries.rs
//! Bounded full-resolution inputs and compacted work for primary reflection queries.

pub(super) const INPUT_FORMATS: [wgpu::TextureFormat; 2] = [
    wgpu::TextureFormat::Rgba32Float,
    wgpu::TextureFormat::Rgba32Uint,
];
const MAX_QUERY_PIXELS: u64 = 2_097_152;
const INPUT_BYTES_PER_PIXEL: u64 = 32;
const WORKLIST_HEADER_BYTES: u64 = 4;
const WORKLIST_INDEX_BYTES: u64 = 4;
const OUTPUT_BYTES_PER_PIXEL: u64 = 48;
const WORKGROUP_SIZE: u32 = 64;
const CHUNK_PIXELS: u32 = 65_536;
const DISPATCH_ROWS: u32 = 32;
const DISPATCH_UNIFORM_BYTES: u64 = 16;

pub(super) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 4] {
    let visibility = wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE;
    let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let buffer = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: false },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    [
        texture(6, wgpu::TextureSampleType::Float { filterable: false }),
        texture(7, wgpu::TextureSampleType::Uint),
        buffer(8),
        buffer(9),
    ]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QueryAllocation {
    worklist_bytes: u64,
    output_bytes: u64,
    total_bytes: u64,
}

fn allocation(limits: &wgpu::Limits, width: u32, height: u32) -> Option<QueryAllocation> {
    // Fragment layouts expose one actor, four lighting/transport, and two query
    // storage buffers. Compute also adds one dynamic uniform to two lighting uniforms.
    if width == 0
        || height == 0
        || width > limits.max_texture_dimension_2d
        || height > limits.max_texture_dimension_2d
        || limits.max_sampled_textures_per_shader_stage < 23
        || limits.max_bind_groups < 4
        || limits.max_bindings_per_bind_group < 10
        || limits.max_color_attachments < 2
        || limits.max_color_attachment_bytes_per_sample < 32
        || limits.max_storage_buffers_per_shader_stage < 7
        || limits.max_compute_workgroup_size_x < WORKGROUP_SIZE
        || limits.max_compute_invocations_per_workgroup < WORKGROUP_SIZE
        || limits.max_dynamic_uniform_buffers_per_pipeline_layout < 1
        || limits.max_uniform_buffers_per_shader_stage < 3
        || u64::from(limits.max_uniform_buffer_binding_size) < DISPATCH_UNIFORM_BYTES
    {
        return None;
    }
    let pixels = u64::from(width).checked_mul(u64::from(height))?;
    // This experiment has a fixed memory ceiling independent of adapter limits.
    if pixels > MAX_QUERY_PIXELS {
        return None;
    }
    if pixels.div_ceil(u64::from(WORKGROUP_SIZE))
        > u64::from(limits.max_compute_workgroups_per_dimension)
    {
        return None;
    }
    let (_, dispatch_bytes) = dispatch_uniform_layout(limits.min_uniform_buffer_offset_alignment)?;
    if dispatch_bytes > limits.max_buffer_size {
        return None;
    }
    let worklist_bytes = pixels
        .checked_mul(WORKLIST_INDEX_BYTES)?
        .checked_add(WORKLIST_HEADER_BYTES)?;
    let output_bytes = pixels.checked_mul(OUTPUT_BYTES_PER_PIXEL)?;
    let buffer_limit =
        u64::from(limits.max_storage_buffer_binding_size).min(limits.max_buffer_size);
    if worklist_bytes > buffer_limit || output_bytes > buffer_limit {
        return None;
    }
    let total_bytes = pixels
        .checked_mul(INPUT_BYTES_PER_PIXEL)?
        .checked_add(worklist_bytes)?
        .checked_add(output_bytes)?;
    Some(QueryAllocation {
        worklist_bytes,
        output_bytes,
        total_bytes,
    })
}

pub(super) fn supports(limits: &wgpu::Limits, width: u32, height: u32) -> bool {
    allocation(limits, width, height).is_some()
}

pub(super) struct QueryTargets {
    pub(super) width: u32,
    pub(super) height: u32,
    _input0: wgpu::Texture,
    _input1: wgpu::Texture,
    pub(super) input0_view: wgpu::TextureView,
    pub(super) input1_view: wgpu::TextureView,
    pub(super) worklist: wgpu::Buffer,
    pub(super) output: wgpu::Buffer,
    allocation: QueryAllocation,
}

impl QueryTargets {
    /// Unsupported dimensions or device limits allocate no GPU resources.
    pub(super) fn new(device: &wgpu::Device, width: u32, height: u32) -> Option<Self> {
        let allocation = allocation(&device.limits(), width, height)?;
        let texture = |label, format| {
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
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        };
        // The caller clears both attachments before collecting visible queries.
        let input0 = texture(
            "primary-reflection-query-position-roughness",
            INPUT_FORMATS[0],
        );
        let input1 = texture(
            "primary-reflection-query-normals-owner-flags",
            INPUT_FORMATS[1],
        );
        let input0_view = input0.create_view(&Default::default());
        let input1_view = input1.create_view(&Default::default());
        let buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        // The worklist starts with an atomic count, followed by packed pixel indices.
        let worklist = buffer(
            "primary-reflection-query-worklist",
            allocation.worklist_bytes,
        );
        let output = buffer("primary-reflection-query-output", allocation.output_bytes);
        Some(Self {
            width,
            height,
            _input0: input0,
            _input1: input1,
            input0_view,
            input1_view,
            worklist,
            output,
            allocation,
        })
    }

    /// Append these resources to the existing six entries in evidence group 3.
    pub(super) fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 4] {
        [
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&self.input0_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&self.input1_view),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: self.worklist.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: self.output.as_entire_binding(),
            },
        ]
    }

    pub(super) fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    /// Inputs, count/index worklist, and three 16-byte output records per pixel.
    pub(super) fn bytes(&self) -> u64 {
        self.allocation.total_bytes
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct DispatchChunk {
    first_pixel: u32,
    workgroups: u32,
}

fn dispatch_chunks(pixels: u32) -> Vec<DispatchChunk> {
    (0..pixels.div_ceil(CHUNK_PIXELS))
        .map(|chunk| {
            let first_pixel = chunk * CHUNK_PIXELS;
            DispatchChunk {
                first_pixel,
                workgroups: (pixels - first_pixel)
                    .min(CHUNK_PIXELS)
                    .div_ceil(WORKGROUP_SIZE),
            }
        })
        .collect()
}

fn dispatch_uniform_layout(alignment: u32) -> Option<(u32, u64)> {
    let alignment = u64::from(alignment.max(1));
    let stride = DISPATCH_UNIFORM_BYTES
        .div_ceil(alignment)
        .checked_mul(alignment)?;
    let bytes = stride.checked_mul(u64::from(DISPATCH_ROWS))?;
    let stride = u32::try_from(stride).ok()?;
    // The dynamic offsets themselves are u32, even when buffers accept u64 sizes.
    stride.checked_mul(DISPATCH_ROWS - 1)?;
    Some((stride, bytes))
}

fn dispatch_uniform_rows(alignment: u32) -> Option<(u32, Vec<u8>)> {
    let (stride, bytes) = dispatch_uniform_layout(alignment)?;
    let mut rows = vec![0; usize::try_from(bytes).ok()?];
    for row in 0..DISPATCH_ROWS {
        let offset = usize::try_from(u64::from(row) * u64::from(stride)).ok()?;
        rows[offset..offset + 4].copy_from_slice(&(row * CHUNK_PIXELS).to_le_bytes());
    }
    Some((stride, rows))
}

pub(super) struct QueryPipelines {
    size: [u32; 2],
    compact: wgpu::ComputePipeline,
    trace: wgpu::ComputePipeline,
    empty_bind_group: wgpu::BindGroup,
    _dispatch_buffer: wgpu::Buffer,
    dispatch_bind_group: wgpu::BindGroup,
    dispatch_stride: u32,
    chunks: Vec<DispatchChunk>,
}

impl QueryPipelines {
    /// Construct only after proving the device and output support query targets.
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        shader: &wgpu::ShaderModule,
        _actor_layout: &wgpu::BindGroupLayout,
        lighting_layout: &wgpu::BindGroupLayout,
        rough_layout: &wgpu::BindGroupLayout,
        width: u32,
        height: u32,
    ) -> Self {
        assert!(
            supports(&device.limits(), width, height),
            "unsupported query dimensions"
        );
        let pixels = width
            .checked_mul(height)
            .expect("bounded query pixel count");
        let limits = device.limits();
        assert!(pixels.div_ceil(WORKGROUP_SIZE) <= limits.max_compute_workgroups_per_dimension);
        let (dispatch_stride, rows) =
            dispatch_uniform_rows(limits.min_uniform_buffer_offset_alignment)
                .expect("bounded query dispatch alignment");
        assert!(rows.len() as u64 <= limits.max_buffer_size);
        // Compute cannot reach actor resources. An explicit empty group avoids
        // requiring a material bind group solely to satisfy pipeline-layout slots.
        let empty_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("primary-reflection-query-empty-layout"),
            entries: &[],
        });
        let empty_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("primary-reflection-query-empty-bind-group"),
            layout: &empty_layout,
            entries: &[],
        });
        let dispatch_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("primary-reflection-query-dispatch-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(DISPATCH_UNIFORM_BYTES),
                },
                count: None,
            }],
        });
        let dispatch_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("primary-reflection-query-dispatch-rows"),
            size: rows.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        // Rows never change between queued passes or frames. Every trace chunk
        // selects its own immutable first-index row through a dynamic offset.
        queue.write_buffer(&dispatch_buffer, 0, &rows);
        let dispatch_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("primary-reflection-query-dispatch-bind-group"),
            layout: &dispatch_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &dispatch_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(DISPATCH_UNIFORM_BYTES),
                }),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("primary-reflection-query-compute-layout"),
            bind_group_layouts: &[
                &empty_layout,
                lighting_layout,
                &dispatch_layout,
                rough_layout,
            ],
            push_constant_ranges: &[],
        });
        let pipeline = |label, entry| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                module: shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            size: [width, height],
            compact: pipeline(
                "primary-reflection-query-compact",
                "cs_compact_reflection_queries",
            ),
            trace: pipeline(
                "primary-reflection-query-trace",
                "cs_trace_reflection_queries",
            ),
            empty_bind_group,
            _dispatch_buffer: dispatch_buffer,
            dispatch_bind_group,
            dispatch_stride,
            chunks: dispatch_chunks(pixels),
        }
    }

    /// Separate passes order count reset, compaction and bounded tracing without readback.
    pub(super) fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        lighting_bind_group: &wgpu::BindGroup,
        rough_bind_group: &wgpu::BindGroup,
        targets: &QueryTargets,
    ) {
        assert_eq!(
            self.size,
            targets.size(),
            "query pipeline and target sizes differ"
        );
        encoder.clear_buffer(&targets.worklist, 0, Some(WORKLIST_HEADER_BYTES));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("primary-reflection-query-compaction-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.compact);
            pass.set_bind_group(0, &self.empty_bind_group, &[]);
            pass.set_bind_group(1, lighting_bind_group, &[]);
            pass.set_bind_group(2, &self.dispatch_bind_group, &[0]);
            pass.set_bind_group(3, rough_bind_group, &[]);
            pass.dispatch_workgroups(
                (targets.width * targets.height).div_ceil(WORKGROUP_SIZE),
                1,
                1,
            );
        }
        for (index, chunk) in self.chunks.iter().enumerate() {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("primary-reflection-query-trace-pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.trace);
            pass.set_bind_group(0, &self.empty_bind_group, &[]);
            pass.set_bind_group(1, lighting_bind_group, &[]);
            pass.set_bind_group(
                2,
                &self.dispatch_bind_group,
                &[index as u32 * self.dispatch_stride],
            );
            pass.set_bind_group(3, rough_bind_group, &[]);
            pass.dispatch_workgroups(chunk.workgroups, 1, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capable_limits() -> wgpu::Limits {
        wgpu::Limits {
            max_sampled_textures_per_shader_stage: 23,
            max_bind_groups: 4,
            max_bindings_per_bind_group: 10,
            max_color_attachments: 2,
            max_color_attachment_bytes_per_sample: 32,
            max_storage_buffers_per_shader_stage: 7,
            max_storage_buffer_binding_size: 128 * 1024 * 1024,
            max_buffer_size: 128 * 1024 * 1024,
            max_texture_dimension_2d: 8192,
            ..wgpu::Limits::default()
        }
    }

    #[test]
    fn full_hd_fits_the_explicit_allocation_budget() {
        let limits = capable_limits();
        let pixels = 1920 * 1080_u64;
        let allocation = allocation(&limits, 1920, 1080).expect("1080p query allocation");
        assert!(supports(&limits, 1920, 1080));
        assert_eq!(allocation.worklist_bytes, 4 + 4 * pixels);
        assert_eq!(allocation.output_bytes, 48 * pixels);
        assert_eq!(allocation.total_bytes, 84 * pixels + 4);
        assert_eq!(allocation.total_bytes, 174_182_404);
    }

    #[test]
    fn pixel_ceiling_accepts_its_boundary_and_refuses_four_k() {
        let limits = capable_limits();
        let maximum = allocation(&limits, 2048, 1024).expect("exact pixel ceiling");
        assert_eq!(maximum.total_bytes, 84 * MAX_QUERY_PIXELS + 4);
        assert!(!supports(&limits, 2048, 1025));
        assert!(!supports(&limits, 3840, 2160));
        assert!(!supports(&limits, u32::MAX, u32::MAX));
    }

    #[test]
    fn both_storage_allocations_must_fit_each_device_buffer_limit() {
        let mut limits = capable_limits();
        let required_output = 48 * 1920 * 1080;
        limits.max_storage_buffer_binding_size = required_output;
        limits.max_buffer_size = u64::from(required_output);
        assert!(supports(&limits, 1920, 1080));
        limits.max_storage_buffer_binding_size -= 1;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_storage_buffer_binding_size += 1;
        limits.max_buffer_size -= 1;
        assert!(!supports(&limits, 1920, 1080));

        // At one pixel the 48-byte output, rather than the 8-byte worklist, dominates.
        limits.max_storage_buffer_binding_size = 47;
        limits.max_buffer_size = 8192;
        assert!(!supports(&limits, 1, 1));
        limits.max_storage_buffer_binding_size = 48;
        assert!(supports(&limits, 1, 1));
    }

    #[test]
    fn two_attachments_require_exactly_thirty_two_color_bytes() {
        let mut limits = capable_limits();
        assert_eq!(INPUT_FORMATS[0], wgpu::TextureFormat::Rgba32Float);
        assert_eq!(INPUT_FORMATS[1], wgpu::TextureFormat::Rgba32Uint);
        assert!(supports(&limits, 1920, 1080));
        limits.max_color_attachment_bytes_per_sample = 31;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_color_attachment_bytes_per_sample = 32;
        limits.max_color_attachments = 1;
        assert!(!supports(&limits, 1920, 1080));
    }

    #[test]
    fn resource_bindings_require_the_complete_stage_budget() {
        let limits = capable_limits();
        let mut insufficient = limits.clone();
        insufficient.max_sampled_textures_per_shader_stage = 22;
        assert!(!supports(&insufficient, 1920, 1080));
        insufficient = limits.clone();
        insufficient.max_bind_groups = 3;
        assert!(!supports(&insufficient, 1920, 1080));
        insufficient = limits.clone();
        insufficient.max_bindings_per_bind_group = 9;
        assert!(!supports(&insufficient, 1920, 1080));
        insufficient = limits.clone();
        insufficient.max_storage_buffers_per_shader_stage = 6;
        assert!(!supports(&insufficient, 1920, 1080));
    }

    #[test]
    fn compaction_workgroups_must_fit_the_device_before_pipeline_construction() {
        let mut limits = capable_limits();
        limits.max_compute_workgroups_per_dimension = 32_400;
        assert!(supports(&limits, 1920, 1080));
        limits.max_compute_workgroups_per_dimension -= 1;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_compute_workgroups_per_dimension += 1;
        limits.max_compute_workgroup_size_x = 63;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_compute_workgroup_size_x = 64;
        limits.max_compute_invocations_per_workgroup = 63;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_compute_invocations_per_workgroup = 64;
        assert!(supports(&limits, 1920, 1080));
    }

    #[test]
    fn compute_uniform_and_dynamic_binding_budgets_are_required() {
        let mut limits = capable_limits();
        limits.max_dynamic_uniform_buffers_per_pipeline_layout = 0;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_dynamic_uniform_buffers_per_pipeline_layout = 1;
        limits.max_uniform_buffers_per_shader_stage = 2;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_uniform_buffers_per_shader_stage = 3;
        limits.max_uniform_buffer_binding_size = 15;
        assert!(!supports(&limits, 1920, 1080));
        limits.max_uniform_buffer_binding_size = 16;
        assert!(supports(&limits, 1920, 1080));
    }

    #[test]
    fn immutable_uniform_rows_must_fit_the_buffer_and_dynamic_offset_ranges() {
        let mut limits = capable_limits();
        // Tiny targets still require the complete immutable dispatch-row buffer.
        limits.max_storage_buffer_binding_size = 48;
        limits.min_uniform_buffer_offset_alignment = 256;
        limits.max_buffer_size = 8191;
        assert!(!supports(&limits, 1, 1));
        limits.max_buffer_size = 8192;
        assert!(supports(&limits, 1, 1));
        limits.min_uniform_buffer_offset_alignment = 512;
        assert!(!supports(&limits, 1, 1));
        limits.max_buffer_size = 16_384;
        assert!(supports(&limits, 1, 1));
        limits.min_uniform_buffer_offset_alignment = u32::MAX;
        limits.max_buffer_size = u64::MAX;
        assert!(!supports(&limits, 1, 1));
    }

    #[test]
    fn texture_dimensions_and_nonempty_extent_are_required() {
        let mut limits = capable_limits();
        assert!(!supports(&limits, 0, 1080));
        assert!(!supports(&limits, 1920, 0));
        limits.max_texture_dimension_2d = 1919;
        assert!(!supports(&limits, 1920, 1080));
        assert!(!supports(&limits, 1080, 1920));
        limits.max_texture_dimension_2d = 1920;
        assert!(supports(&limits, 1920, 1080));
    }

    #[test]
    fn query_layout_exposes_only_the_added_four_resources_to_compute() {
        let entries = layout_entries();
        assert_eq!(entries.map(|entry| entry.binding), [6, 7, 8, 9]);
        for entry in entries {
            assert_eq!(
                entry.visibility,
                wgpu::ShaderStages::FRAGMENT | wgpu::ShaderStages::COMPUTE
            );
        }
        assert!(matches!(
            entries[0].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                ..
            }
        ));
        assert!(matches!(
            entries[1].ty,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                ..
            }
        ));
        for entry in &entries[2..] {
            assert!(matches!(
                entry.ty,
                wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: false },
                    has_dynamic_offset: false,
                    ..
                }
            ));
        }
    }

    #[test]
    fn tracing_chunks_cover_the_pixel_range_without_exceeding_a_chunk() {
        let chunks = dispatch_chunks(1920 * 1080);
        assert_eq!(chunks.len(), 32);
        assert_eq!(
            chunks[0],
            DispatchChunk {
                first_pixel: 0,
                workgroups: 1024
            }
        );
        assert_eq!(
            chunks[31],
            DispatchChunk {
                first_pixel: 2_031_616,
                workgroups: 656
            }
        );
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.first_pixel, index as u32 * CHUNK_PIXELS);
            assert!(chunk.workgroups <= 1024);
        }
        let maximum = dispatch_chunks(MAX_QUERY_PIXELS as u32);
        assert_eq!(maximum.len(), DISPATCH_ROWS as usize);
        assert!(maximum.iter().all(|chunk| chunk.workgroups == 1024));
        assert_eq!(dispatch_chunks(65_537).last().unwrap().workgroups, 1);
    }

    #[test]
    fn immutable_dispatch_rows_are_aligned_and_retain_distinct_first_indices() {
        for alignment in [1, 16, 64, 256, 1024] {
            let (stride, rows) = dispatch_uniform_rows(alignment).expect("valid dispatch rows");
            assert_eq!(stride % alignment, 0);
            assert!(u64::from(stride) >= DISPATCH_UNIFORM_BYTES);
            assert_eq!(rows.len(), (stride * DISPATCH_ROWS) as usize);
            for row in 0..DISPATCH_ROWS {
                let offset = (row * stride) as usize;
                assert_eq!(
                    u32::from_le_bytes(rows[offset..offset + 4].try_into().unwrap()),
                    row * CHUNK_PIXELS
                );
                assert!(
                    rows[offset + 4..offset + 16]
                        .iter()
                        .all(|value| *value == 0)
                );
            }
        }
        assert!(dispatch_uniform_rows(u32::MAX).is_none());
    }
}
