//! Parsers and writers for Dragon Age data formats.
//!
//! - [`gff4`]: the generic binary tree that most DA:O data is stored in.
//! - [`gff3`]: the older Aurora GFF, still used for object templates (`.utc`, `.uti`, ...).
//! - [`gda`]: GDA tables (BioWare's 2DA successor), stored as GFF4.
//! - [`tlk`]: talk tables (localized strings), stored as GFF4.

pub mod gda;
pub mod gff3;
pub mod gff4;
pub mod tlk;

pub use gda::Gda;
pub use gff3::Gff3;
pub use gff4::{FourCC, Gff4, Gff4Error, List, StructId, Value};
pub use tlk::Tlk;
