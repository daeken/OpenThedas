//! Opt-in CPU stage timings and timestamps on actual offscreen render passes.

use crate::gpu::{GpuContext, GpuError};
use std::time::Instant;

const CAPACITY: u32 = 128;

pub(crate) fn enabled() -> bool {
    crate::config::CONFIG.profile
}

struct Gpu {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    readback: wgpu::Buffer,
}

pub(crate) struct CaptureProfile {
    start: Instant,
    marks: Vec<(&'static str, f64)>,
    passes: Vec<&'static str>,
    gpu: Option<Gpu>,
    waits: Vec<(&'static str, f64)>,
    waited_ms: f64,
    error: Option<String>,
}

impl CaptureProfile {
    pub(crate) fn new(ctx: &GpuContext) -> Option<Self> {
        if !enabled() {
            return None;
        }
        let gpu = ctx
            .device
            .features()
            .contains(wgpu::Features::TIMESTAMP_QUERY)
            .then(|| Gpu {
                queries: ctx.device.create_query_set(&wgpu::QuerySetDescriptor {
                    label: Some("capture profile"),
                    ty: wgpu::QueryType::Timestamp,
                    count: CAPACITY,
                }),
                resolve: ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("capture profile resolve"),
                    size: CAPACITY as u64 * 8,
                    usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                }),
                readback: ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("capture profile readback"),
                    size: CAPACITY as u64 * 8,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
            });
        Some(Self {
            start: Instant::now(),
            marks: Vec::new(),
            passes: Vec::new(),
            gpu,
            waits: Vec::new(),
            waited_ms: 0.0,
            error: None,
        })
    }

    pub(crate) fn mark(&mut self, label: &'static str) {
        self.marks.push((
            label,
            self.start.elapsed().as_secs_f64() * 1000.0 - self.waited_ms,
        ));
    }

    /// Optional submission boundaries remove vertex/fragment overlap from pass timings.
    /// These waits perturb scheduling, so ordinary capture benchmarks keep this off.
    pub(crate) fn stage(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        label: &'static str,
    ) {
        if crate::config::CONFIG.profile_serial {
            let start = Instant::now();
            let next = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("capture profile boundary"),
                });
            let previous = std::mem::replace(encoder, next);
            ctx.queue.submit([previous.finish()]);
            if let Err(e) = ctx.device.poll(wgpu::PollType::wait_indefinitely()) {
                self.error = Some(e.to_string());
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            self.waited_ms += ms;
            self.waits.push((label, ms));
        }
        self.mark(label);
    }

    pub(crate) fn pass(
        &mut self,
        label: &'static str,
    ) -> Option<wgpu::RenderPassTimestampWrites<'_>> {
        let gpu = self.gpu.as_ref()?;
        let first = self.passes.len() as u32 * 2;
        if first + 1 >= CAPACITY {
            return None;
        }
        self.passes.push(label);
        Some(wgpu::RenderPassTimestampWrites {
            query_set: &gpu.queries,
            beginning_of_pass_write_index: Some(first),
            end_of_pass_write_index: Some(first + 1),
        })
    }

    pub(crate) fn resolve(&self, encoder: &mut wgpu::CommandEncoder) {
        if let Some(gpu) = &self.gpu {
            let count = self.passes.len() as u32 * 2;
            if count == 0 {
                return;
            }
            encoder.resolve_query_set(&gpu.queries, 0..count, &gpu.resolve, 0);
            encoder.copy_buffer_to_buffer(&gpu.resolve, 0, &gpu.readback, 0, count as u64 * 8);
        }
    }

    pub(crate) fn finish(self, ctx: &GpuContext, path: crate::RenderPath) -> Result<(), GpuError> {
        if let Some(error) = self.error {
            return Err(GpuError::Readback(error));
        }
        // Pixel readback already waited for this submission. Its cost is measured before
        // mapping the diagnostic buffer, whose overhead belongs to the profiling tool.
        let elapsed = self.start.elapsed().as_secs_f64() * 1000.0 - self.waited_ms;
        let stamps = if let Some(gpu) = &self.gpu {
            let (tx, rx) = std::sync::mpsc::channel();
            gpu.readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    let _ = tx.send(result);
                });
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .map_err(|e| GpuError::Readback(e.to_string()))?;
            rx.recv()
                .map_err(|e| GpuError::Readback(e.to_string()))?
                .map_err(|e| GpuError::Readback(e.to_string()))?;
            let mapped = gpu
                .readback
                .slice(..)
                .get_mapped_range()
                .map_err(|e| GpuError::Readback(format!("{e:?}")))?;
            let values: Vec<u64> = bytemuck::cast_slice(&mapped).to_vec();
            drop(mapped);
            gpu.readback.unmap();
            Some(values)
        } else {
            None
        };
        let period = ctx.queue.get_timestamp_period() as f64 / 1_000_000.0;
        let mut previous_cpu = 0.0;
        let mut used = vec![false; self.passes.len()];
        for &(label, cpu) in &self.marks {
            let gpu = stamps.as_ref().and_then(|s| {
                self.passes
                    .iter()
                    .enumerate()
                    .position(|(i, p)| !used[i] && *p == label)
                    .map(|i| {
                        used[i] = true;
                        s[i * 2 + 1].saturating_sub(s[i * 2]) as f64 * period
                    })
            });
            println!(
                "render-profile path={path:?} stage={label} cpu_ms={:.4} gpu_ms={}",
                cpu - previous_cpu,
                gpu.map_or_else(|| "unavailable".into(), |ms| format!("{ms:.4}"))
            );
            previous_cpu = cpu;
        }
        println!(
            "render-profile path={path:?} stage=submit_wait_pixels cpu_ms={:.4} gpu_ms=unavailable",
            elapsed - previous_cpu
        );
        for (label, ms) in self.waits {
            println!("render-profile-serial path={path:?} stage={label} submit_wait_ms={ms:.4}");
        }
        Ok(())
    }
}
