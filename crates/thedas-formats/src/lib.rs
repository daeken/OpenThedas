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
//! - [`bmat`]: Dragon Age II's compiled material definitions (`.mat`, binary).
//! - [`vlm`]: Dragon Age II's per-vertex baked lighting (`.vlm`).
//! - [`dds`]: DDS textures, with a CPU decoder for BC1/2/3.
//! - [`morph`]: head morphs (`.mor`): a character's face, hair and their materials.
//! - [`tint`]: item material tints (`.tnt`), the colours of tint-masked equipment.
//! - [`probe`]: irradiance probes (`.mtx`), the diffuse light characters and props receive.
//! - [`stage`]: conversation stages (`.stg`) and the shot each conversation line takes.
//! - [`anim`]: animation trees (`.anb`), animation lengths (`.gad`) and events (`.evt`).
//! - [`facefx`]: FaceFX lip sync: each voiced line's facial animation (`.fxe`) and the face
//!   rigs (`.fxa`) it drives.
//! - [`phy`]: collision models (`.phy`): the shapes rays and perception test against.
//! - [`speedtree`]: SpeedTree 4 tree descriptions (`.spt`) for level vegetation.
//! - [`speedtree_gen`]: growing SpeedTree trees from their descriptions (in progress).
//!
//! Audio:
//!
//! - [`fev`]: FMOD Designer event files: the events the game plays and what they play.
//! - [`fsb`]: FMOD sound banks (FSB4), the waveforms behind the game's sound events.
//!
//! Interface:
//!
//! - [`gfx`]: Scaleform GFx movies (SWF with Scaleform tags), with a rewriter to plain SWF.
//! - [`cur`]: Windows cursor files, the game's mouse cursors.

pub mod anim;
pub mod area;
pub mod bmat;
pub mod cur;
pub mod dds;
pub mod emitter;
pub mod facefx;
pub mod fev;
pub mod fsb;
pub mod gda;
pub mod gff3;
pub mod gff4;
pub mod gfx;
pub mod level;
pub mod material;
pub mod mesh;
pub mod model;
pub mod morph;
pub mod phy;
pub mod probe;
pub mod speedtree;
pub mod speedtree_gen;
pub mod stage;
pub mod state;
pub mod tint;
pub mod tlk;
pub mod typed;
pub mod vlm;

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

/// Dragon Age II's 32-bit name hash: over the name's UTF-16 code units, each with `A`-`Z`
/// lowered, `h = h * 1000003 ^ c` from 0. DragonAge2.exe hashes a model's controller names
/// with it (0x713160, called from the model loader at 0x74abfe), and DA2's animations
/// (`.ani` field 4000) and head morphs (`.mor` field 2) store their targets as these hashes
/// instead of names. Verified on the shipped files: it reproduces 434 of a skeleton's 444
/// controller names among the animations' targets, and every node of a dwarf head morph.
pub fn name_hash32(name: &str) -> u32 {
    name.encode_utf16().fold(0u32, |h, c| {
        let c = if (0x41..=0x5a).contains(&c) {
            c + 0x20
        } else {
            c
        };
        h.wrapping_mul(1_000_003) ^ c as u32
    })
}

/// How a name stored only as its [`name_hash32`] is kept where a name is expected:
/// `#` and eight lowercase hex digits. No real name starts with `#`.
pub fn hashed_name(hash: u32) -> String {
    format!("#{hash:08x}")
}

/// The hash a [`hashed_name`] holds; `None` for a real name.
pub fn hash_of_hashed_name(name: &str) -> Option<u32> {
    let hex = name.strip_prefix('#')?;
    (hex.len() == 8).then(|| u32::from_str_radix(hex, 16).ok())?
}

/// Whether `stored` (a name, or a [`hashed_name`]) is `name`, ignoring case.
pub fn name_matches(stored: &str, name: &str) -> bool {
    match hash_of_hashed_name(stored) {
        Some(h) => h == name_hash32(name),
        None => stored.eq_ignore_ascii_case(name),
    }
}

#[cfg(test)]
mod name_hash_tests {
    use super::*;

    #[test]
    fn hashes_names_as_dragon_age_2_does() {
        // Values as the shipped DA2 data stores them: an eye animation's track, and a
        // head morph's node.
        assert_eq!(name_hash32("lefteye_rotation"), 0x7363_0f0f);
        assert_eq!(name_hash32("FaceM1"), 0x6887_06f5);
        assert_eq!(name_hash32("FACEM1"), name_hash32("facem1"));
        assert_eq!(name_hash32(""), 0);
        let stored = hashed_name(name_hash32("FaceM1"));
        assert_eq!(stored, "#688706f5");
        assert_eq!(hash_of_hashed_name(&stored), Some(0x6887_06f5));
        assert!(name_matches(&stored, "facem1"));
        assert!(!name_matches(&stored, "EyesM1"));
        assert!(name_matches("FaceM1", "facem1"));
        assert_eq!(hash_of_hashed_name("#xyz"), None);
        assert_eq!(hash_of_hashed_name("head"), None);
    }
}
