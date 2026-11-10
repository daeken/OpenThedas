//! Parsers and writers for Dragon Age data formats.
//!
//! Generic containers:
//!
//! - [`gff4`]: the generic binary tree that most DA:O data is stored in.
//! - [`gff3`]: the older Aurora GFF, still used for object templates (`.utc`, `.uti`, ...).
//! - [`gda`]: GDA tables (BioWare's 2DA successor), stored as GFF4.
//! - [`tlk`]: talk tables (localized strings), stored as GFF4.
//!
//! Typed readers for the area rendering chain (area -> layout -> rooms -> models -> meshes and
//! materials -> textures):
//!
//! - [`area`]: `.are` area files (GFF3); names the environment layout.
//! - [`level`]: `.arl` area layouts and `.rml` room layouts (GFF4).
//! - [`model`]: `.mmh` model hierarchies (GFF4).
//! - [`mesh`]: `.msh` mesh data with D3D9 vertex declarations (GFF4).
//! - [`material`]: `.mao` material objects and `.mat` material definitions (XML).
//! - [`state`]: `.sta` render states (XML).
//! - [`dds`]: DDS textures, with a CPU decoder for BC1/2/3.

pub mod area;
pub mod dds;
pub mod gda;
pub mod gff3;
pub mod gff4;
pub mod level;
pub mod material;
pub mod mesh;
pub mod model;
pub mod state;
pub mod tlk;
pub mod typed;

pub use area::Area;
pub use dds::Dds;
pub use gda::Gda;
pub use gff3::Gff3;
pub use gff4::{FourCC, Gff4, Gff4Error, List, StructId, Value};
pub use level::{Arl, Rml};
pub use material::{MaterialDef, MaterialObject};
pub use mesh::Msh;
pub use model::Mmh;
pub use state::MaterialState;
pub use tlk::Tlk;
pub use typed::TypedError;
