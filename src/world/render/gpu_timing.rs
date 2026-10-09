//! Nonblocking timestamps around the complete 3D queue interval.
//!
//! A span includes any queue idle gaps between its markers; it is not a GPU
//! utilization percentage. Readbacks are bounded and never add a frame wait.

use std::sync::{Arc, Mutex};

const MARKERS: u32 = 7;
type MapResult = Arc<Mutex<Option<Result<(), wgpu::BufferAsyncError>>>>;

pub(super) struct TimingFrame {
    frame: u32,
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
    result: MapResult,
}

pub(super) struct GpuTiming {
    marker: Option<wgpu::ComputePipeline>,
    inside_encoder: bool,
    pending: Vec<TimingFrame>,
    pub(super) completed: Option<(u32, [f64; 6])>,
}

impl GpuTiming {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        let marker = device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("world-timestamp-marker"),
                    source: wgpu::ShaderSource::Wgsl(
                        "@compute @workgroup_size(1) fn main() {}".into(),
                    ),
                });
                device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                    label: Some("world-timestamp-marker"),
                    layout: None,
                    module: &shader,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                })
            });
        Self {
            marker,
            // Encoder-level Metal counters are an opt-in investigation path.
            // The portable pass markers retain the previously verified route.
            inside_encoder: device.features().contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS)
                && cfg!(not(target_arch = "wasm32"))
                && std::env::var_os("MOTIONLOOM_TRACE_ENCODER_TIMESTAMPS").is_some(),
            pending: Vec::new(),
            completed: None,
        }
    }

    pub(super) fn begin(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        frame: u32,
    ) -> Option<TimingFrame> {
        if self.marker.is_none() {
            return None;
        }
        // Dispatch map callbacks without waiting for unfinished rendering.
        device.poll(wgpu::PollType::Poll).ok();
        let mut index = 0;
        while index < self.pending.len() {
            let result = self.pending[index]
                .result
                .lock()
                .expect("world timestamp map lock")
                .take();
            let Some(result) = result else {
                index += 1;
                continue;
            };
            let timing = self.pending.remove(index);
            if result.is_err() {
                continue;
            }
            let mapped = timing.readback.slice(..).get_mapped_range();
            let values: Vec<u64> = mapped
                .chunks_exact(8)
                .map(|bytes| u64::from_ne_bytes(bytes.try_into().expect("timestamp bytes")))
                .collect();
            let scale = f64::from(queue.get_timestamp_period()) / 1_000_000.0;
            let stages =
                std::array::from_fn(|i| values[i + 1].saturating_sub(values[i]) as f64 * scale);
            drop(mapped);
            timing.readback.unmap();
            // Some Metal drivers expose encoder counters but return zero values.
            // Do not publish a fictitious zero-cost GPU frame; fall back to pass
            // timestamps without adding a synchronous frame wait.
            if values.iter().all(|value| *value == 0) {
                self.inside_encoder = false;
                continue;
            }
            self.completed = Some((timing.frame, stages));
            #[cfg(not(target_arch = "wasm32"))]
            if std::env::var_os("MOTIONLOOM_TRACE_BATCHES").is_some() {
                eprintln!(
                    "motionloom 3D GPU span: frame={} total_ms={:.3} shadows_planar_ms={:.3} evidence_ms={:.3} route_ms={:.3} opaque_ms={:.3} glass_ms={:.3} post_ms={:.3}",
                    timing.frame,
                    stages.iter().sum::<f64>(),
                    stages[0],
                    stages[1],
                    stages[2],
                    stages[3],
                    stages[4],
                    stages[5]
                );
            }
        }
        // At most three tiny readbacks remain live. Slow frames skip profiling
        // rather than changing queue scheduling to make a measurement available.
        if self.pending.len() >= 3 {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("world-stage-timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: MARKERS,
        });
        let buffer = |label, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: u64::from(MARKERS) * 8,
                usage,
                mapped_at_creation: false,
            })
        };
        let timing = TimingFrame {
            frame,
            queries,
            resolve: buffer(
                "world-timestamps-resolve",
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            ),
            readback: buffer(
                "world-timestamps-readback",
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            ),
            result: Arc::new(Mutex::new(None)),
        };
        self.mark(encoder, Some(&timing), 0);
        Some(timing)
    }

    pub(super) fn mark(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        timing: Option<&TimingFrame>,
        index: u32,
    ) {
        let (Some(timing), Some(pipeline)) = (timing, self.marker.as_ref()) else {
            return;
        };
        if self.inside_encoder {
            // Native encoder timestamps delimit the actual graphics queue.
            // Independent empty compute markers can overlap raster work on
            // backends with concurrent graphics/compute scheduling.
            encoder.write_timestamp(&timing.queries, index);
            return;
        }
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("world-stage-timestamp"),
            timestamp_writes: Some(wgpu::ComputePassTimestampWrites {
                query_set: &timing.queries,
                beginning_of_pass_write_index: Some(index),
                end_of_pass_write_index: None,
            }),
        });
        pass.set_pipeline(pipeline);
        pass.dispatch_workgroups(1, 1, 1);
    }

    pub(super) fn end(&self, encoder: &mut wgpu::CommandEncoder, timing: Option<&TimingFrame>) {
        let Some(timing) = timing else {
            return;
        };
        self.mark(encoder, Some(timing), MARKERS - 1);
        encoder.resolve_query_set(&timing.queries, 0..MARKERS, &timing.resolve, 0);
        encoder.copy_buffer_to_buffer(
            &timing.resolve,
            0,
            &timing.readback,
            0,
            u64::from(MARKERS) * 8,
        );
    }

    pub(super) fn submitted(&mut self, timing: Option<TimingFrame>) {
        let Some(timing) = timing else {
            return;
        };
        let result = timing.result.clone();
        timing
            .readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |value| {
                *result.lock().expect("world timestamp map lock") = Some(value);
            });
        self.pending.push(timing);
    }
}
