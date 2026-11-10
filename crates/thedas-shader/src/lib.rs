//! Direct3D 9 shaders as DA:O ships them: bytecode and constant-table parsing
//! ([`bytecode`], [`ctab`]) and translation to validated naga IR ([`translate`]).

pub mod bytecode;
pub mod ctab;
pub mod translate;

pub use bytecode::{Shader, Stage};
pub use translate::{Options, Translation, translate};
