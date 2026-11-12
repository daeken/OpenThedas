//! Images the engine draws into a movie while it runs (Scaleform render targets).
//!
//! A movie names such an image as an external image the game substitutes: inventory.gfx and
//! characterrecord.gfx fill a 270 by 459 rectangle with `paperdoll.dds`, a 16 by 16
//! placeholder the exporter left, which DAOrigins.exe replaces with the render target of its
//! paperdoll scene (`GUIPaperdoll`, which names the target `paperdoll`, 0x7bc659). Ruffle
//! cannot have a bitmap's pixels changed by the host, so [`Slots`] turns each shape filled
//! only with such an image into an empty sprite of the same id (a slot) that registers itself
//! (`_global.thedas_slots`, the image's name as `thedas_image`); the host loads the engine's
//! drawing into it as an image (see [`Slot`]).

use std::collections::BTreeMap;

use super::scale9::{Member, Rect, read_matrix, read_rect, register_action_in, tag};
use super::{ExternalImage, TAG_END, TAG_SHOW_FRAME, Tag};

const TAG_DEFINE_SHAPE: u16 = 2;
const TAG_DEFINE_SHAPE2: u16 = 22;
const TAG_DEFINE_SHAPE3: u16 = 32;
const TAG_DEFINE_SPRITE: u16 = 39;
const TAG_EXTERNAL_IMAGE: u16 = 1001;

/// Where an engine image goes in its slot: the filled rectangle in the slot's pixels (x, y,
/// width, height).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub rect: [f64; 4],
}

/// A movie's engine images and the shapes they fill.
pub(super) struct Slots {
    /// Shape id to (image name, bounds).
    shapes: BTreeMap<u16, (String, Rect)>,
}

/// The image's name as the engine knows it: the file name without its extension, lower-case
/// (`paperdoll.dds` is `paperdoll`).
pub fn image_name(file: &str) -> String {
    let f = file.to_ascii_lowercase();
    f.rsplit_once('.').map(|(n, _)| n.to_string()).unwrap_or(f)
}

impl Slots {
    /// Finds the shapes filled only with one of the `engine` images (by name, see
    /// [`image_name`]).
    pub(super) fn plan(tags: &[Tag], engine: &[&str]) -> Self {
        let mut bitmaps = BTreeMap::new();
        for t in tags.iter().filter(|t| t.code == TAG_EXTERNAL_IMAGE) {
            if let Some(ext) = ExternalImage::parse(t.body) {
                let name = image_name(&ext.file);
                if engine.iter().any(|e| e.eq_ignore_ascii_case(&name)) {
                    bitmaps.insert(ext.id, name);
                }
            }
        }
        let mut shapes = BTreeMap::new();
        if !bitmaps.is_empty() {
            for t in tags {
                if let Some((id, bitmap, bounds)) = bitmap_shape(t)
                    && let Some(name) = bitmaps.get(&bitmap)
                {
                    shapes.insert(id, (name.clone(), bounds));
                }
            }
        }
        Self { shapes }
    }

    /// The slot standing in for a shape, if it is one: the sprite tag, the image's name and
    /// where it goes.
    pub(super) fn rewrite_shape(&self, t: &Tag) -> Option<(Vec<u8>, String, Slot)> {
        if !matches!(
            t.code,
            TAG_DEFINE_SHAPE | TAG_DEFINE_SHAPE2 | TAG_DEFINE_SHAPE3
        ) {
            return None;
        }
        let id = u16::from_le_bytes([*t.body.first()?, *t.body.get(1)?]);
        let (name, [x0, y0, x1, y1]) = self.shapes.get(&id)?;
        let mut sprite = id.to_le_bytes().to_vec();
        sprite.extend_from_slice(&1u16.to_le_bytes());
        sprite.extend(register_action_in(
            "thedas_slots",
            "thedas_image",
            Member::Str(name),
        ));
        sprite.extend(tag(TAG_SHOW_FRAME, &[]));
        sprite.extend(tag(TAG_END, &[]));
        let px = |v: i32| v as f64 / 20.0;
        Some((
            tag(TAG_DEFINE_SPRITE, &sprite),
            name.clone(),
            Slot {
                rect: [px(*x0), px(*y0), px(x1 - x0), px(y1 - y0)],
            },
        ))
    }
}

/// A shape filled only with bitmaps: its id, the first bitmap's id that is not the empty
/// one (0xffff) and the shape's bounds.
fn bitmap_shape(t: &Tag) -> Option<(u16, u16, Rect)> {
    if !matches!(
        t.code,
        TAG_DEFINE_SHAPE | TAG_DEFINE_SHAPE2 | TAG_DEFINE_SHAPE3
    ) {
        return None;
    }
    let b = t.body;
    let id = u16::from_le_bytes([*b.first()?, *b.get(1)?]);
    let (bounds, len) = read_rect(b.get(2..)?)?;
    let mut i = 2 + len;
    let mut count = *b.get(i)? as u16;
    i += 1;
    if count == 0xff && t.code != TAG_DEFINE_SHAPE {
        count = u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]);
        i += 2;
    }
    let mut bitmap = None;
    for _ in 0..count {
        let kind = *b.get(i)?;
        if !(0x40..=0x43).contains(&kind) {
            return None;
        }
        let bid = u16::from_le_bytes([*b.get(i + 1)?, *b.get(i + 2)?]);
        let (_, mlen) = read_matrix(b.get(i + 3..)?)?;
        i += 3 + mlen;
        if bid != 0xffff && bitmap.is_none() {
            bitmap = Some(bid);
        }
    }
    Some((id, bitmap?, bounds))
}

#[cfg(test)]
mod tests {
    use super::super::read_tags;
    use super::*;

    #[test]
    fn a_shape_filled_with_an_engine_image_becomes_a_slot() {
        let mut movie = Vec::new();
        // External image 4, paperdoll.dds (16 by 16).
        let mut ext = Vec::new();
        for v in [4u16, 14, 16, 16] {
            ext.extend_from_slice(&v.to_le_bytes());
        }
        ext.push(0);
        ext.push(13);
        ext.extend_from_slice(b"paperdoll.dds");
        movie.extend(tag(TAG_EXTERNAL_IMAGE, &ext));
        // Shape 9: bounds 0..5400 x 0..9180 twips (270 by 459), one clipped bitmap fill of
        // image 4 (the rest of the shape does not matter here).
        let mut shape = 9u16.to_le_bytes().to_vec();
        shape.extend_from_slice(&[0x78, 0x00, 0x02, 0xa3, 0x00, 0x00, 0x11, 0xee, 0x00]);
        // Two fills as the movies have them: the empty bitmap, then image 4 (identity
        // matrices).
        shape.extend_from_slice(&[2, 0x43, 0xff, 0xff, 0x00, 0x43, 4, 0, 0x00]);
        movie.extend(tag(TAG_DEFINE_SHAPE, &shape));
        let tags = read_tags(&movie).unwrap();
        let slots = Slots::plan(&tags, &["paperdoll"]);
        let (sprite, name, slot) = slots.rewrite_shape(&tags[1]).unwrap();
        assert_eq!(name, "paperdoll");
        assert_eq!(slot.rect, [0.0, 0.0, 270.0, 459.0]);
        let t = read_tags(&sprite).unwrap();
        assert_eq!(
            (t[0].code, &t[0].body[..2]),
            (TAG_DEFINE_SPRITE, &[9u8, 0][..])
        );
        // Not an engine image: left alone.
        assert!(
            Slots::plan(&tags, &["portrait"])
                .rewrite_shape(&tags[1])
                .is_none()
        );
    }
}
