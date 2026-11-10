//! Rendering for OpenThedas: loads an area's static geometry from the resource stack and
//! draws it with wgpu.
//!
//! - [`scene`]: CPU-side loading (layout -> rooms -> models -> meshes, materials, lightmaps).
//! - [`material`]: which program draws each material. A hand-written stand-in for now; the
//!   translated original shaders plug in per material.
//! - [`gpu`]: device setup, uploads, pipelines, the draw loop and offscreen capture.
//! - [`camera`]: a free-fly camera in DA:O's Z-up world.
//!
//! Presentation only: nothing here writes simulation state.

pub mod camera;
pub mod gpu;
pub mod material;
pub mod scene;

pub use camera::Camera;
pub use gpu::{AreaRenderer, GpuContext};
pub use material::{ProgramRegistry, ProgramSource};
pub use scene::AreaScene;
