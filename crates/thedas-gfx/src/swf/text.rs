//! `DefineEditText` and `DefineFont2/3`.

use super::read::{Reader, Result};
use super::shape::{Record, read_glyph};
use super::types::{Color, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct EditText {
    pub bounds: Rect,
    pub word_wrap: bool,
    pub multiline: bool,
    pub password: bool,
    pub read_only: bool,
    pub auto_size: bool,
    pub no_select: bool,
    pub border: bool,
    pub html: bool,
    pub use_outlines: bool,
    pub font: Option<u16>,
    pub font_class: Option<String>,
    /// Font height in twips.
    pub height: u16,
    pub color: Option<Color>,
    pub max_length: Option<u16>,
    pub align: TextAlign,
    /// Margins, indent and leading in twips.
    pub left_margin: u16,
    pub right_margin: u16,
    pub indent: u16,
    pub leading: i16,
    pub variable: String,
    pub initial: Option<String>,
}

pub fn read_edit_text(r: &mut Reader) -> Result<EditText> {
    let bounds = Rect::read(r)?;
    let f1 = r.u8()?;
    let f2 = r.u8()?;
    let mut t = EditText {
        bounds,
        word_wrap: f1 & 0x40 != 0,
        multiline: f1 & 0x20 != 0,
        password: f1 & 0x10 != 0,
        read_only: f1 & 0x08 != 0,
        auto_size: f2 & 0x40 != 0,
        no_select: f2 & 0x10 != 0,
        border: f2 & 0x08 != 0,
        html: f2 & 0x02 != 0,
        use_outlines: f2 & 0x01 != 0,
        ..Default::default()
    };
    let has_text = f1 & 0x80 != 0;
    let has_color = f1 & 0x04 != 0;
    let has_max = f1 & 0x02 != 0;
    let has_font = f1 & 0x01 != 0;
    let has_font_class = f2 & 0x80 != 0;
    let has_layout = f2 & 0x20 != 0;
    if has_font {
        t.font = Some(r.u16()?);
    }
    if has_font_class {
        t.font_class = Some(r.cstr()?);
    }
    if has_font || has_font_class {
        t.height = r.u16()?;
    }
    if has_color {
        t.color = Some(Color::rgba(r)?);
    }
    if has_max {
        t.max_length = Some(r.u16()?);
    }
    if has_layout {
        t.align = match r.u8()? {
            1 => TextAlign::Right,
            2 => TextAlign::Center,
            3 => TextAlign::Justify,
            _ => TextAlign::Left,
        };
        t.left_margin = r.u16()?;
        t.right_margin = r.u16()?;
        t.indent = r.u16()?;
        t.leading = r.i16()?;
    }
    t.variable = r.cstr()?;
    if has_text {
        t.initial = Some(r.cstr()?);
    }
    Ok(t)
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Glyph {
    pub code: char,
    pub outline: Vec<Record>,
    /// Advance in EM units (see [`Font::em`]).
    pub advance: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Font {
    pub name: String,
    pub bold: bool,
    pub italic: bool,
    pub glyphs: Vec<Glyph>,
    /// Ascent, descent and leading in EM units.
    pub ascent: f32,
    pub descent: f32,
    pub leading: f32,
    /// Kerning pairs: (left, right) -> adjustment in EM units.
    pub kerning: Vec<((char, char), f32)>,
    /// The size of the EM square in glyph units: 1024 for DefineFont2, 20480 for DefineFont3.
    pub em: f32,
    /// Code point to glyph index.
    pub index: std::collections::HashMap<char, usize>,
}

impl Font {
    pub fn glyph(&self, c: char) -> Option<&Glyph> {
        self.index.get(&c).and_then(|&i| self.glyphs.get(i))
    }

    pub fn kern(&self, a: char, b: char) -> f32 {
        if self.kerning.is_empty() {
            return 0.0;
        }
        self.kerning
            .iter()
            .find(|(p, _)| *p == (a, b))
            .map(|(_, k)| *k)
            .unwrap_or(0.0)
    }
}

/// `DefineFont2` (`v3` false) or `DefineFont3` after the font id.
pub fn read_font(r: &mut Reader, v3: bool) -> Result<Font> {
    let flags = r.u8()?;
    let has_layout = flags & 0x80 != 0;
    let wide_offsets = flags & 0x08 != 0;
    let wide_codes = flags & 0x04 != 0;
    let _lang = r.u8()?;
    let name = r.pstr()?;
    let n = r.u16()? as usize;
    let table = r.rest();
    let mut tr = Reader::new(table);
    let mut offsets = Vec::with_capacity(n + 1);
    for _ in 0..n {
        offsets.push(if wide_offsets {
            tr.u32()? as usize
        } else {
            tr.u16()? as usize
        });
    }
    let code_off = if n > 0 || tr.remaining() >= 2 {
        if wide_offsets {
            tr.u32()? as usize
        } else {
            tr.u16()? as usize
        }
    } else {
        tr.pos()
    };
    let mut glyphs = Vec::with_capacity(n);
    for i in 0..n {
        let start = offsets[i];
        let end = offsets.get(i + 1).copied().unwrap_or(code_off);
        let data = table
            .get(start..end.max(start))
            .ok_or(super::SwfError::Truncated)?;
        glyphs.push(Glyph {
            outline: read_glyph(data).unwrap_or_default(),
            ..Default::default()
        });
    }
    let mut cr = Reader::new(table.get(code_off..).ok_or(super::SwfError::Truncated)?);
    for g in glyphs.iter_mut() {
        let c = if wide_codes {
            cr.u16()? as u32
        } else {
            cr.u8()? as u32
        };
        g.code = char::from_u32(c).unwrap_or('\u{FFFD}');
    }
    let em = if v3 { 20480.0 } else { 1024.0 };
    let mut font = Font {
        name,
        bold: flags & 0x01 != 0,
        italic: flags & 0x02 != 0,
        em,
        ..Default::default()
    };
    if has_layout {
        font.ascent = cr.u16()? as f32;
        font.descent = cr.u16()? as f32;
        font.leading = cr.i16()? as f32;
        for g in glyphs.iter_mut() {
            g.advance = cr.i16()? as f32;
        }
        for _ in 0..n {
            Rect::read(&mut cr)?;
        }
        let k = cr.u16()? as usize;
        for _ in 0..k {
            let (a, b) = if wide_codes {
                (cr.u16()? as u32, cr.u16()? as u32)
            } else {
                (cr.u8()? as u32, cr.u8()? as u32)
            };
            let adj = cr.i16()? as f32;
            if let (Some(a), Some(b)) = (char::from_u32(a), char::from_u32(b)) {
                font.kerning.push(((a, b), adj));
            }
        }
    }
    for (i, g) in glyphs.iter().enumerate() {
        font.index.entry(g.code).or_insert(i);
    }
    font.glyphs = glyphs;
    Ok(font)
}
