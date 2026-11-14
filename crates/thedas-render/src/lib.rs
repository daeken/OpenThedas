//! Rendering for OpenThedas: loads an area's static geometry from the resource stack and
//! draws it with wgpu.
//!
//! - [`scene`]: CPU-side loading (layout -> rooms -> models -> meshes, materials, lightmaps).
//! - [`material`]: which program draws each material. A hand-written stand-in for now; the
//!   translated original shaders plug in per material.
//! - [`d3d`]: materials drawn with their original D3D9 shaders: program building, constant
//!   and sampler binding.
//! - [`gpu`]: device setup, uploads, pipelines, the draw loop and offscreen capture.
//! - [`camera`]: a free-fly camera in DA:O's Z-up world.
//!
//! Presentation only: nothing here writes simulation state.

pub mod animate;
pub mod atmosphere;
#[cfg(test)]
mod backend_check;
pub mod camera;
pub mod collide;
pub mod cut_effects;
pub mod d3d;
mod d3d_gpu;
pub mod deferred;
mod deferred_shadow;
pub mod distortion;
pub mod fxaa;
pub mod gpu;
pub mod look;
pub mod material;
pub mod msaa;
pub mod particles;
pub mod post;
pub mod scene;
pub mod shadow;
pub mod skeleton;

pub use animate::{Animator, Clip};
pub use camera::Camera;
pub use deferred::RenderPath;
pub use gpu::{AreaRenderer, GpuContext, RenderCache};
pub use material::{ProgramRegistry, ProgramSource};
pub use scene::{AreaScene, Prop, PropDraws, PropEffect, SceneProbe};
