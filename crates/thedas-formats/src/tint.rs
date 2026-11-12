//! Tints (`.tnt`, GFF4 `GFF `): the ten colour vectors an item material (iron, steel,
//! rough hide, ...) gives tint-masked shaders as `mml_vTintMaskColours`.
//! `materialtypes.gda` names each material's file in its `TintFile` column.
//!
//! The vectors are fields 14000 to 14009, a missing field reading as (1, 1, 1, 1).
//! [`Tint::colours`] holds them in the order the tint-masked shaders read
//! `mml_vTintMaskColours` (`Ch1ArmTnt.psh`; `character.mat.xml` describes the array): a
//! diffuse and a specular opacity vector (14008, 14009; x, y, z, w weigh the tint mask's red,
//! green, blue and alpha channels), then a diffuse/specular colour pair per channel: red
//! 14000/14003, green 14001/14004, blue 14002/14005, and alpha, the skin channel,
//! 14006/14007: `Ch1ArmTnt.psh` multiplies the diffuse texture by `lerp(1, 14006, a)`, `a`
//! the mask's alpha times 14008's `w`.
//!
//! DAOrigins.exe's loader (0x749600, from 0x7496b9) stores the fields at tint offsets 0x1c
//! to 0xac in the order 14008, 14009, 14006, 14000, 14001, 14002, 14007, 14003, 14004,
//! 14005. The function that sets a tint on a material (0xa604a0) builds the ten-entry
//! `mml_vTintMaskColours` array from offsets 0x1c, 0x2c, 0x4c, 0x8c, 0x5c, 0x9c, 0x6c, 0xac,
//! 0x3c and 0x7c (0xa604f3-0xa60579): exactly [`FIELD_ORDER`]. The data agrees: item tints
//! give zero skin opacity and white skin colours, and the mabari's body tint (`t3_dog_o00`)
//! and skin tints (`t1_skn_*`) give white channel colours, their colour in 14006 and opacity
//! on alpha only.

use crate::gff4::Gff4;
use crate::typed::{Doc, TypedError, TypedResult};

pub const COLOURS: usize = 10;

/// The field each slot of [`Tint::colours`] is read from, in the shaders' order.
pub const FIELD_ORDER: [u32; COLOURS] = [
    14008, 14009, 14000, 14003, 14001, 14004, 14002, 14005, 14006, 14007,
];

#[derive(Clone, Debug, PartialEq)]
pub struct Tint {
    /// In the shaders' order (see [`FIELD_ORDER`]).
    pub colours: [[f32; 4]; COLOURS],
}

impl Tint {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(gff: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(gff, "tnt");
        if gff.file_type.0 != *b"GFF " {
            return Err(TypedError::FileType {
                expected: "GFF ",
                found: gff.file_type,
            });
        }
        let mut colours = [[1.0; 4]; COLOURS];
        for (slot, &label) in colours.iter_mut().zip(FIELD_ORDER.iter()) {
            if let Some(v) = d.opt_vec4(gff.root, label, 1.0)? {
                *slot = v;
            }
        }
        Ok(Tint { colours })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::Builder;

    #[test]
    fn reads_the_colours_in_the_shaders_order() {
        let mut b = Builder::new(b"GFF ");
        let fields = (0..COLOURS as u32)
            .filter(|&i| i != 7)
            .map(|i| (14000 + i, Value::Vec4([i as f32, 0.5, 0.25, 1.0])))
            .collect();
        let root = b.add(b"GFF ", fields);
        let t = Tint::from_gff4(&b.finish(root)).unwrap();
        let firsts: Vec<f32> = t.colours.iter().map(|c| c[0]).collect();
        // 14007 is missing and reads as ones.
        assert_eq!(firsts, [8.0, 9.0, 0.0, 3.0, 1.0, 4.0, 2.0, 5.0, 6.0, 1.0]);
        assert_eq!(t.colours[9], [1.0; 4]);
    }

    #[test]
    fn rejects_other_files_without_panicking() {
        let mut b = Builder::new(b"GFF ");
        let root = b.add(b"GFF ", vec![(14000, Value::U32(3))]);
        assert!(
            Tint::from_gff4(&b.finish(root)).is_err(),
            "wrong value type"
        );
        let mut b = Builder::new(b"MMH ");
        let root = b.add(b"mdlh", vec![]);
        assert!(Tint::from_gff4(&b.finish(root)).is_err(), "wrong file type");
        assert!(Tint::parse(b"junk").is_err());
    }
}
