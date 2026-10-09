// src/world/render/reflection_history.rs
//! Reflection-only temporal evidence, independent of display TAA and grading.

use std::sync::Arc;
use wgpu::util::DeviceExt;

const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

fn shader_source() -> String {
    // Share the exact Lighting declaration with the main surface shader. Avoid
    // another manually maintained uniform layout when lighting fields change.
    let bindings = include_str!("shaders/bindings.wgsl");
    let start = bindings.find("struct Light {").expect("Light declaration");
    let lighting = bindings
        .find("struct Lighting {")
        .expect("Lighting declaration");
    let end = lighting + bindings[lighting..].find("};").expect("Lighting end") + 2;
    format!(
        "{}\n{}",
        &bindings[start..end],
        include_str!("shaders/reflection_history.wgsl")
    )
}

struct HistoryTargets {
    hdr: Arc<wgpu::Texture>,
    reflection: Arc<wgpu::Texture>,
    metadata: wgpu::Texture,
}

pub(super) struct ReflectionHistory {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    targets: [HistoryTargets; 2],
    width: u32,
    height: u32,
    latest: usize,
    valid: bool,
}

impl ReflectionHistory {
    pub(super) fn new(device: &wgpu::Device, width: u32, height: u32) -> Self {
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reflection-history-layout"),
            entries: &[
                uniform(0),
                texture(1),
                texture(2),
                texture(3),
                texture(4),
                texture(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                texture(7),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                uniform(9),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reflection-history-shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source().into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("reflection-history-pipeline-layout"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        let target = || {
            Some(wgpu::ColorTargetState {
                format: TARGET_FORMAT,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("reflection-history-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_reflection_history"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_reflection_history"),
                targets: &[target(), target(), target()],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reflection-history-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let width = width.max(1);
        let height = height.max(1);
        Self {
            layout,
            pipeline,
            sampler,
            targets: Self::targets(device, width, height),
            width,
            height,
            latest: 0,
            valid: false,
        }
    }

    fn targets(device: &wgpu::Device, width: u32, height: u32) -> [HistoryTargets; 2] {
        let target = |label| {
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
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        };
        std::array::from_fn(|_| HistoryTargets {
            hdr: Arc::new(target("reflection-history-hdr")),
            reflection: Arc::new(target("reflection-history-radiance")),
            metadata: target("reflection-history-surface"),
        })
    }

    pub(super) fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        if (self.width, self.height) != (width, height) {
            self.targets = Self::targets(device, width, height);
            self.width = width;
            self.height = height;
            self.latest = 0;
            self.valid = false;
        }
    }

    pub(super) fn invalidate(&mut self) {
        self.valid = false;
    }

    pub(super) fn bytes(&self) -> u64 {
        u64::from(self.width) * u64::from(self.height) * 48
    }

    /// The filtered lobe must accompany the returned HDR when SSR later replaces it.
    pub(super) fn reflection_texture(&self) -> Arc<wgpu::Texture> {
        Arc::clone(&self.targets[self.latest].reflection)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        hdr: &wgpu::Texture,
        reflection: &wgpu::Texture,
        gbuffer: &wgpu::Texture,
        material: &wgpu::Texture,
        depth: &wgpu::Texture,
        lighting: &wgpu::Buffer,
        history_valid: bool,
    ) -> Arc<wgpu::Texture> {
        self.resize(device, hdr.width(), hdr.height());
        debug_assert_eq!(
            (gbuffer.width(), gbuffer.height()),
            (self.width, self.height)
        );
        debug_assert_eq!(
            (reflection.width(), reflection.height()),
            (self.width, self.height)
        );
        let write = 1 - self.latest;
        let view = |texture: &wgpu::Texture| texture.create_view(&Default::default());
        let hdr_view = view(hdr);
        let reflection_view = view(reflection);
        let gbuffer_view = view(gbuffer);
        let material_view = view(material);
        let depth_view = view(depth);
        let history_view = view(&self.targets[self.latest].reflection);
        let metadata_view = view(&self.targets[self.latest].metadata);
        let output_hdr = view(&self.targets[write].hdr);
        let output_reflection = view(&self.targets[write].reflection);
        let output_metadata = view(&self.targets[write].metadata);
        let enabled = (history_valid && self.valid) as u8 as f32;
        let control: Vec<_> = [enabled, 0.0, 0.0, 0.0]
            .into_iter()
            .flat_map(f32::to_ne_bytes)
            .collect();
        // This per-encode uniform cannot be overwritten by another island
        // before the caller submits a shared command encoder.
        let control = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reflection-history-control"),
            contents: &control,
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let texture_entry = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reflection-history-bind-group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lighting.as_entire_binding(),
                },
                texture_entry(1, &hdr_view),
                texture_entry(2, &reflection_view),
                texture_entry(3, &gbuffer_view),
                texture_entry(4, &history_view),
                texture_entry(5, &metadata_view),
                texture_entry(6, &depth_view),
                texture_entry(7, &material_view),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 9,
                    resource: control.as_entire_binding(),
                },
            ],
        });
        let attachment = |view| {
            Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })
        };
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("reflection-history-pass"),
                color_attachments: &[
                    attachment(&output_hdr),
                    attachment(&output_reflection),
                    attachment(&output_metadata),
                ],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        self.latest = write;
        self.valid = true;
        Arc::clone(&self.targets[write].hdr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::naga;

    #[test]
    fn reflection_history_shader_validates_and_matches_shared_uniform_layout() {
        let module = naga::front::wgsl::parse_str(&shader_source()).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        let lighting = module
            .types
            .iter()
            .find_map(|(_, ty)| {
                if ty.name.as_deref() == Some("Lighting") {
                    if let naga::TypeInner::Struct { span, .. } = ty.inner {
                        return Some(span);
                    }
                }
                None
            })
            .unwrap();
        assert_eq!(
            lighting as usize,
            std::mem::size_of::<super::super::GpuWorldLightingParams>()
        );
    }

    #[test]
    fn reflection_history_reprojection_preserves_static_jitter_and_physical_motion() {
        let uv = [0.5_f32, 0.25];
        let current_jitter = [0.375, -0.25];
        let previous_jitter = [-0.125, 0.25];
        let size = [1920.0, 1080.0];
        let velocity = [0.02, -0.01];
        let previous = std::array::from_fn::<_, 2, _>(|i| {
            uv[i] - velocity[i] + (previous_jitter[i] - current_jitter[i]) / size[i]
        });
        let stable_current =
            std::array::from_fn::<_, 2, _>(|i| uv[i] - current_jitter[i] / size[i]);
        let stable_previous =
            std::array::from_fn::<_, 2, _>(|i| previous[i] - previous_jitter[i] / size[i]);
        for i in 0..2 {
            assert!((stable_previous[i] - (stable_current[i] - velocity[i])).abs() < 1e-6);
        }
    }

    #[test]
    fn reflection_history_delta_composition_preserves_direct_energy() {
        let direct = [4.0_f32, 2.0, 0.5];
        let current = [0.8, 0.2, 0.1];
        let filtered = [0.7, 0.25, 0.1];
        for i in 0..3 {
            let hdr = direct[i] + current[i];
            let result = hdr + filtered[i] - current[i];
            assert!((result - (direct[i] + filtered[i])).abs() < 1e-6);
        }
    }

    /// Isolate the transport-history pass from display TAA: feed known HDR
    /// lobes through several actual GPU frames and read the linear result.
    #[test]
    #[ignore = "requires a native GPU adapter"]
    fn reflection_history_gpu_sequence_filters_and_rejects_stale_paths() {
        use crate::common::gpu_async::{
            BufferMapAsyncFuture, DevicePoller, request_adapter_async, request_device_async,
        };
        const SIZE: u32 = 16;
        const PADDED_ROW: u32 = 256;

        pollster::block_on(async {
            let instance = wgpu::Instance::new(&Default::default());
            let adapter = request_adapter_async(&instance, &Default::default())
                .await
                .expect("native reflection-history adapter");
            let (device, queue) = request_device_async(
                &adapter,
                &wgpu::DeviceDescriptor {
                    label: Some("reflection-history-sequence-device"),
                    required_features: wgpu::Features::empty(),
                    required_limits: adapter.limits(),
                    memory_hints: wgpu::MemoryHints::Performance,
                    trace: wgpu::Trace::Off,
                },
            )
            .await
            .expect("reflection-history device");
            let device = Arc::new(device);
            let poller = DevicePoller::start(Arc::clone(&device));
            let extent = wgpu::Extent3d {
                width: SIZE,
                height: SIZE,
                depth_or_array_layers: 1,
            };
            let texture = |label, format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: extent,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let input = |label| {
                texture(
                    label,
                    TARGET_FORMAT,
                    wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                )
            };
            let hdr = input("history-fixture-hdr");
            let reflection = input("history-fixture-reflection");
            let gbuffer = input("history-fixture-gbuffer");
            let material = input("history-fixture-material");
            let depth = texture(
                "history-fixture-depth",
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let write = |target: &wgpu::Texture, pixels: &[[f32; 4]]| {
                let bytes: Vec<_> = pixels
                    .iter()
                    .flatten()
                    .flat_map(|v| half::f16::from_f32(*v).to_bits().to_ne_bytes())
                    .collect();
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: target,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &bytes,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(SIZE * 8),
                        rows_per_image: Some(SIZE),
                    },
                    extent,
                );
            };
            write(
                &gbuffer,
                &vec![[0.5, 0.5, 0.0, 0.0]; (SIZE * SIZE) as usize],
            );
            write(
                &material,
                &vec![[0.1, 0.0, 1.0, 0.0]; (SIZE * SIZE) as usize],
            );

            // Set only the camera/transport fields used by this pass. Obtain
            // offsets from the shared WGSL contract instead of duplicating a
            // second Rust uniform structure in this fixture.
            let module = naga::front::wgsl::parse_str(&shader_source()).unwrap();
            let (span, members) = module
                .types
                .iter()
                .find_map(|(_, ty)| match &ty.inner {
                    naga::TypeInner::Struct { span, members }
                        if ty.name.as_deref() == Some("Lighting") =>
                    {
                        Some((*span, members))
                    }
                    _ => None,
                })
                .unwrap();
            let mut lighting_bytes = vec![0; span as usize];
            let set_vector = |bytes: &mut [u8], name: &str, vector: [f32; 4]| {
                let offset = members
                    .iter()
                    .find(|m| m.name.as_deref() == Some(name))
                    .unwrap()
                    .offset as usize;
                for (index, value) in vector.into_iter().enumerate() {
                    bytes[offset + index * 4..offset + index * 4 + 4]
                        .copy_from_slice(&value.to_ne_bytes());
                }
            };
            for (name, vector) in [
                ("camera0", [0.0, 0.0, 0.0, 16.0]),
                ("camera1", [1.0, 0.0, 0.0, 0.1]),
                ("camera2", [0.0, 1.0, 0.0, 100.0]),
                ("camera3", [0.0, 0.0, 1.0, 1.0]),
                ("previous_camera0", [0.0, 0.0, 0.0, 16.0]),
                ("previous_camera1", [1.0, 0.0, 0.0, 0.1]),
                ("previous_camera2", [0.0, 1.0, 0.0, 100.0]),
                ("previous_camera3", [0.0, 0.0, 1.0, 1.0]),
            ] {
                set_vector(&mut lighting_bytes, name, vector);
            }
            let lighting = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("history-fixture-lighting"),
                contents: &lighting_bytes,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
            let mut depth_encoder = device.create_command_encoder(&Default::default());
            {
                let depth_view = depth.create_view(&Default::default());
                let _pass = depth_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("history-fixture-flat-surface"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth_view,
                        depth_ops: Some(wgpu::Operations {
                            // Reversed perspective depth for a surface at z=3.
                            load: wgpu::LoadOp::Clear((0.1 * 100.0 / 3.0 - 0.1) / 99.9),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
            }
            queue.submit([depth_encoder.finish()]);
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("history-fixture-readback"),
                size: u64::from(PADDED_ROW * SIZE),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut history = ReflectionHistory::new(&device, SIZE, SIZE);
            let center = (SIZE * (SIZE / 2) + SIZE / 2) as usize;
            for frame in 0..6 {
                if frame >= 3 {
                    // Prime a visibly distinct lobe before each independent
                    // rejection. A test with identical old/current colors
                    // would pass even if invalidation did nothing.
                    write(
                        &reflection,
                        &vec![[0.95, 0.95, 0.95, -8.0]; (SIZE * SIZE) as usize],
                    );
                    write(&hdr, &vec![[4.95, 2.95, 1.45, 0.4]; (SIZE * SIZE) as usize]);
                    write(
                        &material,
                        &vec![[0.1, 0.0, 1.0, 0.0]; (SIZE * SIZE) as usize],
                    );
                    set_vector(&mut lighting_bytes, "reflection0", [1.0, 0.0, 0.0, 0.0]);
                    queue.write_buffer(&lighting, 0, &lighting_bytes);
                    history.invalidate();
                    let mut encoder = device.create_command_encoder(&Default::default());
                    history.encode(
                        &device,
                        &mut encoder,
                        &hdr,
                        &reflection,
                        &gbuffer,
                        &material,
                        &depth,
                        &lighting,
                        false,
                    );
                    queue.submit([encoder.finish()]);
                }
                // Frame 1 has mild lobe variation with an unchanged path.
                // Later frames must reject a path jump, cut, scene revision,
                // or roughness change even when the old lobe is nearby.
                let current = if frame == 0 { 0.95 } else { 0.90 };
                let path = if frame < 2 { -5.0 } else { -8.0 };
                let mut lobes = vec![[0.98, 0.98, 0.98, path]; (SIZE * SIZE) as usize];
                lobes[center] = [current, current, current, path];
                if frame == 0 {
                    lobes.fill([current, current, current, path]);
                }
                let colors: Vec<_> = lobes
                    .iter()
                    .map(|lobe| [4.0 + lobe[0], 2.0 + lobe[1], 0.5 + lobe[2], 0.4])
                    .collect();
                write(&reflection, &lobes);
                write(&hdr, &colors);
                if frame == 3 {
                    history.invalidate();
                }
                if frame == 4 {
                    set_vector(&mut lighting_bytes, "reflection0", [1.0, 0.0, 1.0, 0.0]);
                } else {
                    set_vector(&mut lighting_bytes, "reflection0", [1.0, 0.0, 0.0, 0.0]);
                }
                queue.write_buffer(&lighting, 0, &lighting_bytes);
                if frame == 5 {
                    write(
                        &material,
                        &vec![[0.3, 0.0, 1.0, 0.0]; (SIZE * SIZE) as usize],
                    );
                }
                let mut encoder = device.create_command_encoder(&Default::default());
                let output = history.encode(
                    &device,
                    &mut encoder,
                    &hdr,
                    &reflection,
                    &gbuffer,
                    &material,
                    &depth,
                    &lighting,
                    true,
                );
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &output,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(PADDED_ROW),
                            rows_per_image: Some(SIZE),
                        },
                    },
                    extent,
                );
                queue.submit([encoder.finish()]);
                BufferMapAsyncFuture::new(&poller, &readback).await.unwrap();
                let result = {
                    let mapped = readback.slice(..).get_mapped_range();
                    let offset = ((SIZE / 2) * PADDED_ROW + (SIZE / 2) * 8) as usize;
                    std::array::from_fn::<_, 4, _>(|index| {
                        half::f16::from_bits(u16::from_ne_bytes([
                            mapped[offset + index * 2],
                            mapped[offset + index * 2 + 1],
                        ]))
                        .to_f32()
                    })
                };
                readback.unmap();
                assert!(
                    (result[3] - 0.4).abs() < 0.001,
                    "frame {frame}: alpha changed"
                );
                assert!(
                    (result[0] - result[1] - 2.0).abs() < 0.006,
                    "frame {frame}: direct HDR energy changed: {result:?}"
                );
                let filtered = result[0] - 4.0;
                if frame == 1 {
                    assert!(
                        filtered > 0.925 && filtered < 0.955,
                        "stable mild reflection noise was not filtered: {result:?}"
                    );
                } else {
                    assert!(
                        (filtered - current).abs() < 0.006,
                        "frame {frame}: stale reflection survived rejection: {result:?}"
                    );
                }
            }
        });
    }
}
