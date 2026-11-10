//! The minimap's picture of a level: `<layout>_mmap_ph.dds`, and `<layout>_mmi.gff` (GFF4
//! `MMI `) saying where it lies in the world.
//!
//! Provisional, from the files' contents and how minimap.gfx uses its values: field 3137 is
//! the share of the texture the map covers (width, height), 3138 the world's lower corner
//! and 3139 its upper corner (x, y; z is ±500). The movie maps a world point between the
//! corners (`Area.MinimapLeft`/`Bottom` and `Right`/`Top`) onto that part of the texture.

use thedas_formats::Gff4;
use thedas_formats::gff4::Value;
use thedas_vfs::Vfs;

const FIELD_USED: u32 = 3137;
const FIELD_MIN: u32 = 3138;
const FIELD_MAX: u32 = 3139;

#[derive(Clone, Debug, PartialEq)]
pub struct MinimapInfo {
    /// The texture's resource name.
    pub texture: String,
    /// The world's lower and upper corners (x, y).
    pub min: [f32; 2],
    pub max: [f32; 2],
    /// The share of the texture the map covers (width, height).
    pub used: [f32; 2],
    /// The texture's size in pixels.
    pub size: [f32; 2],
}

impl MinimapInfo {
    /// The minimap of level `layout`, from its resources; `None` when it has none.
    pub fn read(vfs: &Vfs, layout: &str) -> Option<Self> {
        let texture = format!("{layout}_mmap_ph.dds");
        let (image, _) = vfs.read(&texture).ok()?;
        let dds = thedas_formats::Dds::parse(&image).ok()?;
        let (data, _) = vfs.read(&format!("{layout}_mmi.gff")).ok()?;
        let mut m = Self::parse(&data, texture)?;
        m.size = [dds.width as f32, dds.height as f32];
        Some(m)
    }

    /// Where world point (`x`, `y`) lies on the map picture, in its pixels (minimap.gfx
    /// places its icons, and centres the map on the camera, in these). Provisional: the
    /// picture's top is the world's upper y.
    pub fn to_map(&self, x: f32, y: f32) -> [f32; 2] {
        let (w, h) = (self.size[0] * self.used[0], self.size[1] * self.used[1]);
        [
            (x - self.min[0]) / (self.max[0] - self.min[0]) * w,
            (self.max[1] - y) / (self.max[1] - self.min[1]) * h,
        ]
    }

    /// The info in an `MMI ` file's bytes, for `texture`.
    pub fn parse(data: &[u8], texture: String) -> Option<Self> {
        Self::from_gff4(&Gff4::parse(data).ok()?, texture)
    }

    pub fn from_gff4(g: &Gff4, texture: String) -> Option<Self> {
        let vec = |label| match g.field(g.root, label) {
            Some(Value::Vec3(v)) => Some(*v),
            Some(Value::Vec4(v)) => Some([v[0], v[1], v[2]]),
            _ => None,
        };
        let (used, min, max) = (vec(FIELD_USED)?, vec(FIELD_MIN)?, vec(FIELD_MAX)?);
        let finite = [used, min, max].iter().flatten().all(|v| v.is_finite());
        (finite && max[0] > min[0] && max[1] > min[1]).then(|| Self {
            texture,
            min: [min[0], min[1]],
            max: [max[0], max[1]],
            used: [used[0], used[1]],
            size: [0.0, 0.0],
        })
    }
}

#[cfg(test)]
mod tests {
    use thedas_formats::gff4::{FieldDef, FourCC, StructData, StructId, StructTemplate, Version};

    use super::*;

    fn mmi(used: [f32; 3], min: [f32; 3], max: [f32; 3]) -> Gff4 {
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"MMI "),
            file_version: FourCC::new(b"V4.0"),
            templates: vec![StructTemplate {
                label: FourCC::new(b"MMI "),
                fields: [FIELD_USED, FIELD_MIN, FIELD_MAX]
                    .iter()
                    .map(|&label| FieldDef {
                        label,
                        type_code: 0,
                        flags: 0,
                        offset: 0,
                    })
                    .collect(),
                size: 0,
            }],
            structs: vec![StructData {
                template: 0,
                values: vec![Value::Vec3(used), Value::Vec3(min), Value::Vec3(max)],
            }],
            root: StructId(0),
            shared_strings: None,
        }
    }

    #[test]
    fn reads_the_corners_and_the_share_of_the_texture() {
        let g = mmi(
            [0.55, 0.6, 1.0],
            [-70.7, -52.7, -500.0],
            [24.7, 50.7, 500.0],
        );
        let m = MinimapInfo::from_gff4(&g, "x_mmap_ph.dds".into()).unwrap();
        assert_eq!(m.min, [-70.7, -52.7]);
        assert_eq!(m.max, [24.7, 50.7]);
        assert_eq!(m.used, [0.55, 0.6]);
        let m = MinimapInfo {
            size: [512.0, 512.0],
            ..m
        };
        // The lower x and upper y corner is the picture's top left.
        assert_eq!(m.to_map(-70.7, 50.7), [0.0, 0.0]);
        let [x, y] = m.to_map(24.7, -52.7);
        assert!((x - 512.0 * 0.55).abs() < 1e-3 && (y - 512.0 * 0.6).abs() < 1e-3);
        // Corners the wrong way round are not a map.
        let bad = mmi([1.0, 1.0, 1.0], [5.0, 5.0, 0.0], [1.0, 9.0, 0.0]);
        assert!(MinimapInfo::from_gff4(&bad, String::new()).is_none());
        assert!(MinimapInfo::parse(b"junk", String::new()).is_none());
    }
}
