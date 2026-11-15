//! GPU storage the renderer batches through: one vertex arena every mesh lives in, and a
//! table of textures sampled by slot (bindless where the adapter allows it).
//!
//! Draws read their vertices from the arena and their per-object record from a storage
//! buffer, so meshes with different transforms, colours and (bindless) textures draw in one
//! call. See `render.rs` for how ops are merged.

use std::sync::{Arc, Mutex};

use crate::tess::Vertex;

/// A vertex as the shaders read it from storage (24 bytes, std430 alignment).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GpuVertex {
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: u32,
    pub _pad: u32,
}

impl From<&Vertex> for GpuVertex {
    fn from(v: &Vertex) -> Self {
        Self {
            pos: v.pos,
            uv: v.uv,
            color: u32::from_le_bytes(v.color),
            _pad: 0,
        }
    }
}

/// Free ranges of the arena (start, length), shared with the ranges handed out.
type FreeList = Arc<Mutex<Vec<(u32, u32)>>>;

/// A mesh's place in the arena; dropping it gives the place back.
#[derive(Debug)]
pub struct VRange {
    pub start: u32,
    pub len: u32,
    free: FreeList,
}

impl Drop for VRange {
    fn drop(&mut self) {
        if let Ok(mut f) = self.free.lock() {
            f.push((self.start, self.len));
        }
    }
}

/// Every mesh's vertices, in one storage buffer that grows as needed.
pub struct Arena {
    pub buffer: wgpu::Buffer,
    capacity: u32,
    end: u32,
    free: FreeList,
    /// Bumped when the buffer is replaced (bind groups naming it must be made again).
    pub generation: u64,
}

const MIN_VERTICES: u32 = 1 << 16;

fn arena_buffer(device: &wgpu::Device, capacity: u32) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gfx vertex arena"),
        size: capacity as u64 * std::mem::size_of::<GpuVertex>() as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

impl Arena {
    pub fn new(device: &wgpu::Device) -> Self {
        Self {
            buffer: arena_buffer(device, MIN_VERTICES),
            capacity: MIN_VERTICES,
            end: 0,
            free: Default::default(),
            generation: 0,
        }
    }

    /// Stores `vertices` and returns where they went.
    pub fn alloc(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        vertices: &[Vertex],
    ) -> VRange {
        let n = vertices.len() as u32;
        let start = self.take_free(n).unwrap_or_else(|| {
            if self.end + n > self.capacity {
                self.grow(device, queue, self.end + n);
            }
            let s = self.end;
            self.end += n;
            s
        });
        let data: Vec<GpuVertex> = vertices.iter().map(GpuVertex::from).collect();
        queue.write_buffer(
            &self.buffer,
            start as u64 * std::mem::size_of::<GpuVertex>() as u64,
            bytemuck::cast_slice(&data),
        );
        VRange {
            start,
            len: n,
            free: self.free.clone(),
        }
    }

    /// First fit from the freed ranges (the remainder stays free).
    fn take_free(&mut self, n: u32) -> Option<u32> {
        let mut f = self.free.lock().ok()?;
        let i = f.iter().position(|&(_, len)| len >= n)?;
        let (start, len) = f[i];
        if len == n {
            f.swap_remove(i);
        } else {
            f[i] = (start + n, len - n);
        }
        Some(start)
    }

    fn grow(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, need: u32) {
        let capacity = need.next_power_of_two().max(self.capacity * 2);
        let buffer = arena_buffer(device, capacity);
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gfx arena grow"),
        });
        enc.copy_buffer_to_buffer(
            &self.buffer,
            0,
            &buffer,
            0,
            self.end as u64 * std::mem::size_of::<GpuVertex>() as u64,
        );
        queue.submit(Some(enc.finish()));
        self.buffer = buffer;
        self.capacity = capacity;
        self.generation += 1;
    }
}

/// Samplers by index, as the shaders choose them.
pub const SAMPLER_CLAMP: u8 = 0;
pub const SAMPLER_REPEAT: u8 = 1;
pub const SAMPLER_CLAMP_NEAREST: u8 = 2;
pub const SAMPLER_REPEAT_NEAREST: u8 = 3;

/// Slots in use, and the views bound at each.
pub struct Slots {
    views: Vec<Option<wgpu::TextureView>>,
    free: Vec<u32>,
    pub generation: u64,
}

impl Slots {
    pub fn views(&self) -> &[Option<wgpu::TextureView>] {
        &self.views
    }
}

/// A texture as a draw samples it: its slot, the sampler, and a bind group of its own for
/// adapters without texture arrays.
#[derive(Clone)]
pub struct Tex(Arc<TexInner>);

struct TexInner {
    slot: u32,
    sampler: u8,
    single: wgpu::BindGroup,
    slots: Arc<Mutex<Slots>>,
}

impl Drop for TexInner {
    fn drop(&mut self) {
        if let Ok(mut s) = self.slots.lock() {
            if let Some(v) = s.views.get_mut(self.slot as usize) {
                *v = None;
            }
            s.free.push(self.slot);
            s.generation += 1;
        }
    }
}

impl std::fmt::Debug for Tex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tex({}, {})", self.0.slot, self.0.sampler)
    }
}

impl Tex {
    pub fn slot(&self) -> u32 {
        self.0.slot
    }

    pub fn sampler(&self) -> u8 {
        self.0.sampler
    }

    pub fn single(&self) -> &wgpu::BindGroup {
        &self.0.single
    }
}

/// Hands out texture slots.
pub struct Textures {
    pub slots: Arc<Mutex<Slots>>,
}

impl Textures {
    pub fn new() -> Self {
        Self {
            slots: Arc::new(Mutex::new(Slots {
                views: Vec::new(),
                free: Vec::new(),
                generation: 0,
            })),
        }
    }

    /// Registers `view`, with `single` its own bind group (texture and sampler).
    pub fn add(&self, view: wgpu::TextureView, sampler: u8, single: wgpu::BindGroup) -> Tex {
        let mut s = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let slot = match s.free.pop() {
            Some(i) => {
                s.views[i as usize] = Some(view);
                i
            }
            None => {
                s.views.push(Some(view));
                s.views.len() as u32 - 1
            }
        };
        s.generation += 1;
        drop(s);
        Tex(Arc::new(TexInner {
            slot,
            sampler,
            single,
            slots: self.slots.clone(),
        }))
    }

    pub fn generation(&self) -> u64 {
        self.slots.lock().map(|s| s.generation).unwrap_or(0)
    }
}

impl Default for Textures {
    fn default() -> Self {
        Self::new()
    }
}

/// Features and limits the renderer uses when the adapter has them (texture arrays for
/// merging draws across textures). Hosts creating the device should ask for these.
pub fn wanted(adapter: &wgpu::Adapter) -> (wgpu::Features, wgpu::Limits) {
    let features = adapter.features()
        & (wgpu::Features::TEXTURE_BINDING_ARRAY
            | wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
            | wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY);
    let mut limits = wgpu::Limits::default();
    let a = adapter.limits();
    limits.max_binding_array_elements_per_shader_stage =
        a.max_binding_array_elements_per_shader_stage.min(1 << 16);
    limits.max_binding_array_sampler_elements_per_shader_stage = a
        .max_binding_array_sampler_elements_per_shader_stage
        .min(1 << 16);
    (features, limits)
}

/// A device for tests and tools, with the renderer's optional features where available.
pub fn test_device() -> (wgpu::Device, wgpu::Queue) {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster_block(instance.request_adapter(&Default::default())).expect("adapter");
    let (required_features, required_limits) = wanted(&adapter);
    pollster_block(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gfx test"),
        required_features,
        required_limits,
        ..Default::default()
    }))
    .expect("device")
}

/// A tiny block-on for the futures wgpu hands back (tools and tests only).
fn pollster_block<F: std::future::Future>(f: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn noop(_: *const ()) {}
    fn clone(p: *const ()) -> RawWaker {
        RawWaker::new(p, &VTABLE)
    }
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    // SAFETY: the vtable's functions ignore the data pointer.
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    let mut f = std::pin::pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}
