//! Text fields: the formatted text model, the HTML subset the movies use, and line layout.

use std::sync::Arc;

use crate::swf::{EditText, Font, TextAlign};

/// A character format (`TextFormat`). Sizes and margins are in pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Format {
    pub font: String,
    pub size: f32,
    pub color: [u8; 3],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub align: TextAlign,
    pub left_margin: f32,
    pub right_margin: f32,
    pub indent: f32,
    pub leading: f32,
    pub letter_spacing: f32,
    pub kerning: bool,
    pub url: String,
}

impl Default for Format {
    fn default() -> Self {
        Self {
            font: "Times New Roman".into(),
            size: 12.0,
            color: [0, 0, 0],
            bold: false,
            italic: false,
            underline: false,
            align: TextAlign::Left,
            left_margin: 0.0,
            right_margin: 0.0,
            indent: 0.0,
            leading: 0.0,
            letter_spacing: 0.0,
            kerning: false,
            url: String::new(),
        }
    }
}

/// An inline image (`<img>`): a linkage name or URL and its box.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub src: String,
    pub width: f32,
    pub height: f32,
    pub hspace: f32,
    pub vspace: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Piece {
    Text(String),
    Image(Image),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub piece: Piece,
    pub format: Format,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AutoSize {
    #[default]
    None,
    Left,
    Center,
    Right,
}

/// A laid out glyph: font, glyph index, pen position (pixels, baseline), size and colour.
#[derive(Clone, Debug)]
pub struct PlacedGlyph {
    pub font: Arc<Font>,
    pub glyph: usize,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: [u8; 3],
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub top: f32,
    pub height: f32,
    pub ascent: f32,
    pub width: f32,
    pub glyphs: Vec<PlacedGlyph>,
    pub images: Vec<(Image, f32, f32)>,
    pub underlines: Vec<(f32, f32, f32, [u8; 3])>,
}

#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub lines: Vec<Line>,
    /// `textWidth`/`textHeight` in pixels.
    pub width: f32,
    pub height: f32,
}

/// Finds a font by name and style.
pub trait Fonts {
    fn font(&self, name: &str, bold: bool, italic: bool) -> Option<Arc<Font>>;
}

#[derive(Clone, Debug)]
pub struct TextField {
    pub def: Arc<EditText>,
    pub spans: Vec<Span>,
    pub html: bool,
    pub default_format: Format,
    pub word_wrap: bool,
    pub multiline: bool,
    pub auto_size: AutoSize,
    /// Scaleform's `textAutoSize`: "none", "shrink" or "fit".
    pub text_auto_size: String,
    pub selectable: bool,
    pub input: bool,
    pub password: bool,
    pub max_chars: u32,
    pub restrict: Option<String>,
    pub variable: String,
    /// First visible line (1-based, as `scroll`).
    pub scroll: u32,
    /// Field box in pixels relative to the object (x, y, width, height).
    pub bounds: (f32, f32, f32, f32),
    pub border: bool,
    pub background: bool,
    pub border_color: [u8; 3],
    pub background_color: [u8; 3],
    /// Scaleform's `shadowStyle` and `shadowColor`.
    pub shadow_style: String,
    pub shadow_color: [u8; 3],
    /// Caret position in characters, when the field has focus.
    pub caret: Option<usize>,
    pub layout: Option<Layout>,
    /// Bumped whenever the text, its format or its box change (renderers cache by it).
    pub revision: u64,
}

/// Flash's gutter around a field's text (pixels).
pub const GUTTER: f32 = 2.0;

impl TextField {
    pub fn new(def: Arc<EditText>, font_name: Option<String>) -> Self {
        let b = def.bounds;
        let mut f = Format {
            size: def.height as f32 / 20.0,
            align: def.align,
            left_margin: def.left_margin as f32 / 20.0,
            right_margin: def.right_margin as f32 / 20.0,
            indent: def.indent as f32 / 20.0,
            leading: def.leading as f32 / 20.0,
            ..Default::default()
        };
        if let Some(n) = font_name {
            f.font = n;
        }
        if let Some(c) = def.color {
            f.color = [c.r, c.g, c.b];
        }
        let mut t = Self {
            spans: Vec::new(),
            html: def.html,
            default_format: f,
            word_wrap: def.word_wrap,
            multiline: def.multiline,
            auto_size: if def.auto_size {
                AutoSize::Left
            } else {
                AutoSize::None
            },
            text_auto_size: "none".into(),
            selectable: !def.no_select,
            input: !def.read_only,
            password: def.password,
            max_chars: def.max_length.unwrap_or(0) as u32,
            restrict: None,
            variable: def.variable.clone(),
            scroll: 1,
            bounds: (
                b.x_min as f32 / 20.0,
                b.y_min as f32 / 20.0,
                b.width() as f32 / 20.0,
                b.height() as f32 / 20.0,
            ),
            border: def.border,
            background: def.border,
            border_color: [0, 0, 0],
            background_color: [255, 255, 255],
            shadow_style: String::new(),
            shadow_color: [0, 0, 0],
            caret: None,
            layout: None,
            revision: 0,
            def,
        };
        if let Some(init) = t.def.initial.clone() {
            if t.html {
                t.set_html(&init);
            } else {
                t.set_text(&init);
            }
        }
        t
    }

    /// Drops the cached layout; the field has changed.
    pub fn invalidate(&mut self) {
        self.layout = None;
        self.revision = self.revision.wrapping_add(1);
    }

    /// The plain text (`text`), with paragraphs ended by `\r` as Flash reports them.
    pub fn text(&self) -> String {
        let mut s = String::new();
        for sp in &self.spans {
            if let Piece::Text(t) = &sp.piece {
                s.push_str(t);
            }
        }
        s
    }

    pub fn set_text(&mut self, s: &str) {
        let s = s.replace("\r\n", "\r").replace('\n', "\r");
        self.spans = vec![Span {
            piece: Piece::Text(s),
            format: self.default_format.clone(),
        }];
        self.invalidate();
    }

    pub fn set_html(&mut self, html: &str) {
        self.spans = parse_html(html, &self.default_format, self.multiline || true);
        self.invalidate();
    }

    /// `htmlText` as Flash writes it back.
    pub fn html_text(&self) -> String {
        if !self.html {
            return self.text();
        }
        let mut out = String::new();
        let mut para: Vec<Span> = Vec::new();
        let flush = |out: &mut String, para: &[Span], align: TextAlign| {
            let a = match align {
                TextAlign::Left => "LEFT",
                TextAlign::Right => "RIGHT",
                TextAlign::Center => "CENTER",
                TextAlign::Justify => "JUSTIFY",
            };
            out.push_str(&format!("<P ALIGN=\"{a}\">"));
            for sp in para {
                let f = &sp.format;
                out.push_str(&format!(
                    "<FONT FACE=\"{}\" SIZE=\"{}\" COLOR=\"#{:02X}{:02X}{:02X}\" LETTERSPACING=\"{}\" KERNING=\"{}\">",
                    f.font,
                    crate::avm1::value::number_to_string(f.size as f64),
                    f.color[0],
                    f.color[1],
                    f.color[2],
                    crate::avm1::value::number_to_string(f.letter_spacing as f64),
                    u8::from(f.kerning)
                ));
                let (bo, bc) = if f.bold { ("<B>", "</B>") } else { ("", "") };
                let (io, ic) = if f.italic { ("<I>", "</I>") } else { ("", "") };
                let (uo, uc) = if f.underline {
                    ("<U>", "</U>")
                } else {
                    ("", "")
                };
                out.push_str(bo);
                out.push_str(io);
                out.push_str(uo);
                match &sp.piece {
                    Piece::Text(t) => out.push_str(&escape(t)),
                    Piece::Image(i) => out.push_str(&format!("<IMG SRC=\"{}\">", i.src)),
                }
                out.push_str(uc);
                out.push_str(ic);
                out.push_str(bc);
                out.push_str("</FONT>");
            }
            out.push_str("</P>");
        };
        let align_of = |para: &[Span]| {
            para.first()
                .map(|s| s.format.align)
                .unwrap_or(self.default_format.align)
        };
        for sp in &self.spans {
            match &sp.piece {
                Piece::Text(t) => {
                    let mut rest = t.as_str();
                    while let Some(i) = rest.find('\r') {
                        para.push(Span {
                            piece: Piece::Text(rest[..i].to_string()),
                            format: sp.format.clone(),
                        });
                        flush(&mut out, &para, align_of(&para));
                        para.clear();
                        rest = &rest[i + 1..];
                    }
                    if !rest.is_empty() {
                        para.push(Span {
                            piece: Piece::Text(rest.to_string()),
                            format: sp.format.clone(),
                        });
                    }
                }
                Piece::Image(_) => para.push(sp.clone()),
            }
        }
        if !para.is_empty() || out.is_empty() {
            flush(&mut out, &para, align_of(&para));
        }
        out
    }

    /// Applies `f` (only its set fields, see [`FormatPatch`]) to characters `begin..end`.
    pub fn apply_format(&mut self, begin: usize, end: usize, f: &FormatPatch) {
        let mut out = Vec::new();
        let mut pos = 0usize;
        for sp in self.spans.drain(..) {
            let len = match &sp.piece {
                Piece::Text(t) => t.chars().count(),
                Piece::Image(_) => 1,
            };
            let (s, e) = (pos, pos + len);
            pos = e;
            if e <= begin || s >= end || len == 0 {
                out.push(sp);
                continue;
            }
            match sp.piece {
                Piece::Image(_) => {
                    let mut sp = sp;
                    f.apply(&mut sp.format);
                    out.push(sp);
                }
                Piece::Text(t) => {
                    let chars: Vec<char> = t.chars().collect();
                    let a = begin.saturating_sub(s).min(len);
                    let b = (end - s).min(len);
                    for (range, fmt) in [(0..a, false), (a..b, true), (b..len, false)] {
                        if range.is_empty() {
                            continue;
                        }
                        let mut format = sp.format.clone();
                        if fmt {
                            f.apply(&mut format);
                        }
                        out.push(Span {
                            piece: Piece::Text(chars[range].iter().collect()),
                            format,
                        });
                    }
                }
            }
        }
        self.spans = out;
        self.invalidate();
    }

    /// The format of the character at `i` (or of the first one).
    pub fn format_at(&self, i: usize) -> Format {
        let mut pos = 0;
        for sp in &self.spans {
            let len = match &sp.piece {
                Piece::Text(t) => t.chars().count(),
                Piece::Image(_) => 1,
            };
            if i < pos + len {
                return sp.format.clone();
            }
            pos += len;
        }
        self.spans
            .last()
            .map(|s| s.format.clone())
            .unwrap_or_else(|| self.default_format.clone())
    }

    pub fn len(&self) -> usize {
        self.spans
            .iter()
            .map(|s| match &s.piece {
                Piece::Text(t) => t.chars().count(),
                Piece::Image(_) => 1,
            })
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lays the text out (cached until the text, format or box changes).
    pub fn layout(&mut self, fonts: &dyn Fonts) -> &Layout {
        if self.layout.is_none() {
            self.layout = Some(layout(self, fonts));
        }
        self.layout.as_ref().unwrap_or_else(|| unreachable!())
    }

    /// `maxscroll`: the last line that can be the first visible one.
    pub fn max_scroll(&mut self, fonts: &dyn Fonts) -> u32 {
        let h = self.bounds.3 - 2.0 * GUTTER;
        let lines = &self.layout(fonts).lines;
        if lines.is_empty() {
            return 1;
        }
        let bottom = lines.last().map(|l| l.top + l.height).unwrap_or(0.0);
        let mut first = 0;
        while first < lines.len() && bottom - lines[first].top > h + 0.01 {
            first += 1;
        }
        first as u32 + 1
    }
}

/// A `TextFormat` with unset fields left alone (`null` in ActionScript).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormatPatch {
    pub font: Option<String>,
    pub size: Option<f32>,
    pub color: Option<[u8; 3]>,
    pub bold: Option<bool>,
    pub italic: Option<bool>,
    pub underline: Option<bool>,
    pub align: Option<TextAlign>,
    pub left_margin: Option<f32>,
    pub right_margin: Option<f32>,
    pub indent: Option<f32>,
    pub leading: Option<f32>,
    pub letter_spacing: Option<f32>,
    pub kerning: Option<bool>,
    pub url: Option<String>,
}

impl FormatPatch {
    pub fn apply(&self, f: &mut Format) {
        macro_rules! set {
            ($($n:ident),*) => {$(if let Some(v) = &self.$n { f.$n = v.clone(); })*};
        }
        set!(
            font,
            size,
            color,
            bold,
            italic,
            underline,
            align,
            left_margin,
            right_margin,
            indent,
            leading,
            letter_spacing,
            kerning,
            url
        );
    }

    pub fn from_format(f: &Format) -> Self {
        Self {
            font: Some(f.font.clone()),
            size: Some(f.size),
            color: Some(f.color),
            bold: Some(f.bold),
            italic: Some(f.italic),
            underline: Some(f.underline),
            align: Some(f.align),
            left_margin: Some(f.left_margin),
            right_margin: Some(f.right_margin),
            indent: Some(f.indent),
            leading: Some(f.leading),
            letter_spacing: Some(f.letter_spacing),
            kerning: Some(f.kerning),
            url: Some(f.url.clone()),
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest.find(';').filter(|&j| j < 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let c = match ent {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "amp" => Some('&'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{a0}'),
            _ => ent
                .strip_prefix("#x")
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn attrs(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = tag.as_bytes();
    let mut i = 0;
    // Skip the tag name.
    while i < b.len() && !b[i].is_ascii_whitespace() {
        i += 1;
    }
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b'/') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let ns = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = tag[ns..i].to_ascii_lowercase();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < b.len() && b[i] == b'=' {
            i += 1;
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            let v = if i < b.len() && (b[i] == b'"' || b[i] == b'\'') {
                let q = b[i];
                i += 1;
                let vs = i;
                while i < b.len() && b[i] != q {
                    i += 1;
                }
                let v = &tag[vs..i.min(tag.len())];
                i += 1;
                v
            } else {
                let vs = i;
                while i < b.len() && !b[i].is_ascii_whitespace() {
                    i += 1;
                }
                &tag[vs..i]
            };
            out.push((name, unescape(v)));
        } else {
            out.push((name, String::new()));
        }
    }
    out
}

fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    let h = h.strip_prefix("0x").unwrap_or(h);
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

/// Parses the HTML subset Flash's text fields read into spans.
pub fn parse_html(html: &str, base: &Format, _multiline: bool) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    let mut stack: Vec<(String, Format)> = Vec::new();
    let mut fmt = base.clone();
    let mut rest = html;
    let mut para_open = false;
    let push_text = |out: &mut Vec<Span>, t: &str, f: &Format| {
        if t.is_empty() {
            return;
        }
        if let Some(Span {
            piece: Piece::Text(p),
            format,
        }) = out.last_mut()
        {
            if format == f {
                p.push_str(t);
                return;
            }
        }
        out.push(Span {
            piece: Piece::Text(t.to_string()),
            format: f.clone(),
        });
    };
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push_text(&mut out, &unescape(rest), &fmt);
            break;
        };
        if lt > 0 {
            push_text(&mut out, &unescape(&rest[..lt]), &fmt);
        }
        rest = &rest[lt..];
        let Some(gt) = rest.find('>') else {
            push_text(&mut out, &unescape(rest), &fmt);
            break;
        };
        let tag = &rest[1..gt];
        rest = &rest[gt + 1..];
        let closing = tag.starts_with('/');
        let body = tag.trim_start_matches('/').trim_end_matches('/');
        let name = body
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        if closing {
            if name == "p" || name == "li" {
                push_text(&mut out, "\r", &fmt);
                para_open = false;
            }
            if let Some(i) = stack.iter().rposition(|(n, _)| *n == name) {
                fmt = stack[i].1.clone();
                stack.truncate(i);
            }
            continue;
        }
        let a = attrs(body);
        let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        match name.as_str() {
            "br" | "sbr" => push_text(&mut out, "\r", &fmt),
            "img" => {
                let n = |k| get(k).and_then(|v| v.parse::<f32>().ok());
                out.push(Span {
                    piece: Piece::Image(Image {
                        src: get("src").unwrap_or("").to_string(),
                        width: n("width").unwrap_or(0.0),
                        height: n("height").unwrap_or(0.0),
                        hspace: n("hspace").unwrap_or(8.0),
                        vspace: n("vspace").unwrap_or(8.0),
                    }),
                    format: fmt.clone(),
                });
            }
            _ => {
                let self_closing = tag.ends_with('/');
                if !self_closing {
                    stack.push((name.clone(), fmt.clone()));
                }
                match name.as_str() {
                    "p" | "li" => {
                        if para_open {
                            push_text(&mut out, "\r", &fmt);
                        }
                        para_open = true;
                        if let Some(al) = get("align") {
                            fmt.align = match al.to_ascii_lowercase().as_str() {
                                "right" => TextAlign::Right,
                                "center" => TextAlign::Center,
                                "justify" => TextAlign::Justify,
                                _ => TextAlign::Left,
                            };
                        }
                    }
                    "b" => fmt.bold = true,
                    "i" => fmt.italic = true,
                    "u" => fmt.underline = true,
                    "a" => fmt.url = get("href").unwrap_or("").to_string(),
                    "font" => {
                        if let Some(f) = get("face") {
                            fmt.font = f.to_string();
                        }
                        if let Some(s) = get("size") {
                            let s = s.trim();
                            if let Some(d) = s.strip_prefix('+') {
                                fmt.size += d.parse::<f32>().unwrap_or(0.0);
                            } else if let Some(d) = s.strip_prefix('-') {
                                fmt.size -= d.parse::<f32>().unwrap_or(0.0);
                            } else if let Ok(v) = s.parse::<f32>() {
                                fmt.size = v;
                            }
                        }
                        if let Some(c) = get("color").and_then(parse_color) {
                            fmt.color = c;
                        }
                        if let Some(ls) = get("letterspacing").and_then(|v| v.parse().ok()) {
                            fmt.letter_spacing = ls;
                        }
                        if let Some(k) = get("kerning") {
                            fmt.kerning = k == "1" || k.eq_ignore_ascii_case("true");
                        }
                    }
                    "textformat" => {
                        let n = |k| get(k).and_then(|v| v.parse::<f32>().ok());
                        if let Some(v) = n("leftmargin") {
                            fmt.left_margin = v;
                        }
                        if let Some(v) = n("rightmargin") {
                            fmt.right_margin = v;
                        }
                        if let Some(v) = n("indent") {
                            fmt.indent = v;
                        }
                        if let Some(v) = n("leading") {
                            fmt.leading = v;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // Flash drops the paragraph break a final `</p>` leaves.
    if let Some(Span {
        piece: Piece::Text(t),
        ..
    }) = out.last_mut()
    {
        if t.ends_with('\r') {
            t.pop();
        }
    }
    out.retain(|s| !matches!(&s.piece, Piece::Text(t) if t.is_empty()));
    out
}

struct Word {
    /// (char, format index)
    chars: Vec<(char, usize)>,
    image: Option<(Image, usize)>,
    /// Ends the paragraph.
    newline: bool,
    space_after: bool,
}

fn font_for(fonts: &dyn Fonts, f: &Format) -> Option<Arc<Font>> {
    fonts
        .font(&f.font, f.bold, f.italic)
        .or_else(|| fonts.font(&f.font, false, false))
}

/// The advance of `c` in `f` (pixels).
fn advance(font: Option<&Arc<Font>>, f: &Format, c: char) -> f32 {
    let Some(font) = font else {
        return f.size * 0.5;
    };
    let scale = f.size / font.em;
    let adv = font
        .glyph(c)
        .map(|g| g.advance)
        .or_else(|| font.glyph(' ').map(|g| g.advance))
        .unwrap_or(font.em * 0.5);
    adv * scale + f.letter_spacing
}

/// Lays out `t`'s spans into lines. Provisional: follows Flash's rules as Ruffle documents
/// them (2 px gutter, ascent + descent + leading per line); Scaleform's own layout is not
/// traced.
pub fn layout(t: &TextField, fonts: &dyn Fonts) -> Layout {
    let formats: Vec<Format> = t.spans.iter().map(|s| s.format.clone()).collect();
    let resolved: Vec<Option<Arc<Font>>> = formats.iter().map(|f| font_for(fonts, f)).collect();
    // Split into words.
    let mut words: Vec<Word> = Vec::new();
    let mut cur = Word {
        chars: Vec::new(),
        image: None,
        newline: false,
        space_after: false,
    };
    let flush = |cur: &mut Word, words: &mut Vec<Word>, newline: bool, space: bool| {
        let w = std::mem::replace(
            cur,
            Word {
                chars: Vec::new(),
                image: None,
                newline: false,
                space_after: false,
            },
        );
        words.push(Word {
            newline,
            space_after: space,
            ..w
        });
    };
    for (fi, sp) in t.spans.iter().enumerate() {
        match &sp.piece {
            Piece::Image(img) => {
                if !cur.chars.is_empty() {
                    flush(&mut cur, &mut words, false, false);
                }
                cur.image = Some((img.clone(), fi));
                flush(&mut cur, &mut words, false, false);
            }
            Piece::Text(s) => {
                for c in s.chars() {
                    let c = if t.password && c != '\r' { '*' } else { c };
                    match c {
                        '\r' | '\n' if t.multiline || t.html => {
                            flush(&mut cur, &mut words, true, false)
                        }
                        '\r' | '\n' => {}
                        ' ' | '\t' => {
                            cur.chars.push((' ', fi));
                            flush(&mut cur, &mut words, false, true);
                        }
                        _ => cur.chars.push((c, fi)),
                    }
                }
            }
        }
    }
    if !cur.chars.is_empty() || cur.image.is_some() || words.is_empty() {
        flush(&mut cur, &mut words, false, false);
    }

    let box_w = t.bounds.2 - 2.0 * GUTTER;
    let wrap = t.word_wrap && t.auto_size == AutoSize::None || t.word_wrap;
    let mut lines: Vec<Line> = Vec::new();
    let mut line_words: Vec<&Word> = Vec::new();
    let mut y = GUTTER;
    let mut para_start = true;
    let word_width = |w: &Word| -> f32 {
        if let Some((img, _)) = &w.image {
            return img.width + 2.0 * img.hspace;
        }
        w.chars
            .iter()
            .map(|&(c, fi)| advance(resolved[fi].as_ref(), &formats[fi], c))
            .sum()
    };
    let emit = |words: &[&Word], first_in_para: bool, y: &mut f32, lines: &mut Vec<Line>| {
        let fmt0 = words
            .iter()
            .find_map(|w| {
                w.chars
                    .first()
                    .map(|c| c.1)
                    .or(w.image.as_ref().map(|i| i.1))
            })
            .map(|i| formats[i].clone())
            .unwrap_or_else(|| {
                formats
                    .last()
                    .cloned()
                    .unwrap_or_else(|| t.default_format.clone())
            });
        let mut ascent: f32 = 0.0;
        let mut descent: f32 = 0.0;
        let mut leading: f32 = fmt0.leading;
        let mut any = false;
        for w in words {
            for &(_, fi) in &w.chars {
                any = true;
                let f = &formats[fi];
                if let Some(font) = &resolved[fi] {
                    let s = f.size / font.em;
                    ascent = ascent.max(font.ascent * s);
                    descent = descent.max(font.descent * s);
                } else {
                    ascent = ascent.max(f.size * 0.8);
                    descent = descent.max(f.size * 0.2);
                }
                leading = leading.max(f.leading);
            }
            if let Some((img, _)) = &w.image {
                any = true;
                ascent = ascent.max(img.height + img.vspace);
            }
        }
        if !any {
            let f = &fmt0;
            let fi = resolved
                .iter()
                .zip(formats.iter())
                .find(|(_, ff)| *ff == f)
                .and_then(|(r, _)| r.clone());
            if let Some(font) = fi {
                let s = f.size / font.em;
                ascent = font.ascent * s;
                descent = font.descent * s;
            } else {
                ascent = f.size * 0.8;
                descent = f.size * 0.2;
            }
        }
        let indent = if first_in_para { fmt0.indent } else { 0.0 };
        let left = GUTTER + fmt0.left_margin + indent;
        let avail = box_w - fmt0.left_margin - fmt0.right_margin - indent;
        // Trailing spaces do not count toward alignment.
        let mut width = 0.0;
        let mut trailing = 0.0;
        for w in words {
            let ww = word_width(w);
            width += ww;
            trailing = if w.space_after {
                w.chars
                    .last()
                    .map(|&(c, fi)| advance(resolved[fi].as_ref(), &formats[fi], c))
                    .unwrap_or(0.0)
            } else {
                0.0
            };
        }
        let content = width - trailing;
        let x0 = match fmt0.align {
            TextAlign::Left | TextAlign::Justify => left,
            TextAlign::Right => left + (avail - content).max(0.0),
            TextAlign::Center => left + ((avail - content) / 2.0).max(0.0),
        };
        let mut line = Line {
            top: *y,
            height: ascent + descent + leading,
            ascent,
            width: content,
            ..Default::default()
        };
        let base = *y + ascent;
        let mut x = x0;
        let mut prev: Option<char> = None;
        for w in words {
            if let Some((img, fi)) = &w.image {
                let _ = fi;
                line.images
                    .push((img.clone(), x + img.hspace, base - img.height));
                x += img.width + 2.0 * img.hspace;
                prev = None;
                continue;
            }
            for &(c, fi) in &w.chars {
                let f = &formats[fi];
                let font = resolved[fi].as_ref();
                if let (Some(font), Some(p)) = (font, prev) {
                    if f.kerning {
                        x += font.kern(p, c) * f.size / font.em;
                    }
                }
                let adv = advance(font, f, c);
                if let Some(font) = font {
                    if let Some(&gi) = font.index.get(&c) {
                        if c != ' ' {
                            line.glyphs.push(PlacedGlyph {
                                font: font.clone(),
                                glyph: gi,
                                x,
                                y: base,
                                size: f.size,
                                color: f.color,
                            });
                        }
                    }
                }
                if f.underline {
                    line.underlines.push((x, x + adv, base + 1.0, f.color));
                }
                x += adv;
                prev = Some(c);
            }
        }
        *y += line.height;
        lines.push(line);
    };
    let mut line_w = 0.0;
    for w in &words {
        let ww = word_width(w);
        let fi = w
            .chars
            .first()
            .map(|c| c.1)
            .or(w.image.as_ref().map(|i| i.1));
        let f = fi.map(|i| &formats[i]).unwrap_or(&t.default_format);
        let indent = if para_start { f.indent } else { 0.0 };
        let avail = box_w - f.left_margin - f.right_margin - indent;
        let space_w = if w.space_after {
            w.chars
                .last()
                .map(|&(c, fi)| advance(resolved[fi].as_ref(), &formats[fi], c))
                .unwrap_or(0.0)
        } else {
            0.0
        };
        if wrap && !line_words.is_empty() && line_w + ww - space_w > avail {
            emit(&line_words, para_start, &mut y, &mut lines);
            para_start = false;
            line_words.clear();
            line_w = 0.0;
        }
        line_words.push(w);
        line_w += ww;
        if w.newline {
            emit(&line_words, para_start, &mut y, &mut lines);
            para_start = true;
            line_words.clear();
            line_w = 0.0;
        }
    }
    if !line_words.is_empty() || lines.is_empty() {
        emit(&line_words, para_start, &mut y, &mut lines);
    }
    // Flash does not count the last line's leading in textHeight.
    let width = lines.iter().map(|l| l.width).fold(0.0, f32::max);
    let height = lines
        .last()
        .map(|l| l.top + l.ascent + (l.height - l.ascent) - GUTTER)
        .unwrap_or(0.0);
    Layout {
        lines,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_subset() {
        let base = Format::default();
        let s = parse_html(
            "<p align='center'><font color='#ff0000' size='20'>Hi &amp; <b>bye</b></font></p><p>x</p>",
            &base,
            true,
        );
        let text: String = s
            .iter()
            .map(|x| match &x.piece {
                Piece::Text(t) => t.as_str(),
                Piece::Image(_) => "",
            })
            .collect();
        assert_eq!(text, "Hi & bye\rx");
        assert_eq!(s[0].piece, Piece::Text("Hi & ".into()));
        assert_eq!(s[0].format.color, [255, 0, 0]);
        assert_eq!(s[0].format.size, 20.0);
        assert_eq!(s[0].format.align, TextAlign::Center);
        assert!(s[1].format.bold);
        assert_eq!(s[1].piece, Piece::Text("bye".into()));
        let last = s.last().unwrap_or_else(|| unreachable!());
        assert_eq!(last.piece, Piece::Text("x".into()));
        assert!(!last.format.bold);
    }

    struct NoFonts;
    impl Fonts for NoFonts {
        fn font(&self, _: &str, _: bool, _: bool) -> Option<Arc<Font>> {
            None
        }
    }

    #[test]
    fn wraps_words() {
        let def = EditText {
            bounds: crate::swf::Rect {
                x_min: 0,
                x_max: 20 * 64,
                y_min: 0,
                y_max: 20 * 100,
            },
            word_wrap: true,
            multiline: true,
            height: 20 * 10,
            ..Default::default()
        };
        let mut t = TextField::new(Arc::new(def), None);
        t.set_text("aaaa bbbb cccc");
        // Each char 5 px wide without fonts: "aaaa " is 25 px; a 60 px line fits two words.
        let l = layout(&t, &NoFonts);
        assert_eq!(l.lines.len(), 2);
    }
}
