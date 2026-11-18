//! Bindings for the EclipseRay Python scripting interface.

mod aergia;
mod gff;
mod postprocess;

pub use aergia::install_aergia;
pub use gff::install_gff_reader;
pub use postprocess::{install_postprocessors, run_postprocessor};
