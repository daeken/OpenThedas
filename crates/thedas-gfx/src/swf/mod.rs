//! The movie file: the GFx/SWF container, the character dictionary and the timelines.
//!
//! Only the tags the game's movies use are read (see `docs/scaleform-player.md`); any other
//! tag is counted in [`Movie::skipped`] and otherwise ignored. Every read is bounds-checked:
//! a damaged tag is skipped and counted, never a panic.

pub mod read;
pub mod shape;
pub mod text;
pub mod types;

use std::collections::{BTreeMap, HashMap};
use std::io::Read as _;
use std::sync::Arc;

use read::{Reader, Result};
pub use shape::{Fill, Gradient, Line, MorphShape, Record, Shape, Spread};
pub use text::{EditText, Font, TextAlign};
pub use types::{Color, ColorTransform, Matrix, Rect};

#[derive(Debug, thiserror::Error)]
pub enum SwfError {
    #[error("not an SWF/GFx movie")]
    BadSignature,
    #[error("truncated data")]
    Truncated,
    #[error("decompressing: {0}")]
    Zlib(#[from] std::io::Error),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

/// A block of AVM1 bytecode.
pub type Actions = Arc<[u8]>;

/// Scaleform blend modes (SWF's numbering; 0 and 1 are both normal).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum BlendMode {
    #[default]
    Normal,
    Layer,
    Multiply,
    Screen,
    Lighten,
    Darken,
    Difference,
    Add,
    Subtract,
    Invert,
    Alpha,
    Erase,
    Overlay,
    HardLight,
}

impl BlendMode {
    pub fn from_u8(v: u8) -> Self {
        use BlendMode::*;
        match v {
            2 => Layer,
            3 => Multiply,
            4 => Screen,
            5 => Lighten,
            6 => Darken,
            7 => Difference,
            8 => Add,
            9 => Subtract,
            10 => Invert,
            11 => Alpha,
            12 => Erase,
            13 => Overlay,
            14 => HardLight,
            _ => Normal,
        }
    }

    /// The name ActionScript's `blendMode` property reports.
    pub fn name(self) -> &'static str {
        use BlendMode::*;
        match self {
            Normal => "normal",
            Layer => "layer",
            Multiply => "multiply",
            Screen => "screen",
            Lighten => "lighten",
            Darken => "darken",
            Difference => "difference",
            Add => "add",
            Subtract => "subtract",
            Invert => "invert",
            Alpha => "alpha",
            Erase => "erase",
            Overlay => "overlay",
            HardLight => "hardlight",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        (0..=14)
            .map(Self::from_u8)
            .find(|m| m.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Filter {
    DropShadow {
        color: Color,
        blur: (f32, f32),
        angle: f32,
        distance: f32,
        strength: f32,
        inner: bool,
        knockout: bool,
        hide_object: bool,
        passes: u8,
    },
    Blur {
        blur: (f32, f32),
        passes: u8,
    },
    Glow {
        color: Color,
        blur: (f32, f32),
        strength: f32,
        inner: bool,
        knockout: bool,
        passes: u8,
    },
    /// Read and skipped (no movie uses them).
    Other(u8),
}

/// A clip event handler from `PlaceObject2/3`.
#[derive(Clone, Debug)]
pub struct ClipAction {
    /// SWF 6+ clip event flags (bit 0 = KeyUp ... as stored, little-endian u32).
    pub events: u32,
    pub key_code: Option<u8>,
    pub actions: Actions,
}

/// Clip event bits as stored in the little-endian `u32`.
pub mod clip_event {
    pub const KEY_UP: u32 = 0x80;
    pub const KEY_DOWN: u32 = 0x40;
    pub const MOUSE_UP: u32 = 0x20;
    pub const MOUSE_DOWN: u32 = 0x10;
    pub const MOUSE_MOVE: u32 = 0x08;
    pub const UNLOAD: u32 = 0x04;
    pub const ENTER_FRAME: u32 = 0x02;
    pub const LOAD: u32 = 0x01;
    pub const DRAG_OVER: u32 = 0x8000;
    pub const ROLL_OUT: u32 = 0x4000;
    pub const ROLL_OVER: u32 = 0x2000;
    pub const RELEASE_OUTSIDE: u32 = 0x1000;
    pub const RELEASE: u32 = 0x0800;
    pub const PRESS: u32 = 0x0400;
    pub const INITIALIZE: u32 = 0x0200;
    pub const DATA: u32 = 0x0100;
    pub const CONSTRUCT: u32 = 0x04_0000;
    pub const KEY_PRESS: u32 = 0x02_0000;
    pub const DRAG_OUT: u32 = 0x01_0000;
}

#[derive(Clone, Debug, Default)]
pub struct Place {
    pub depth: u16,
    /// True to modify the object already at the depth.
    pub is_move: bool,
    pub character: Option<u16>,
    pub matrix: Option<Matrix>,
    pub color: Option<ColorTransform>,
    pub ratio: Option<u16>,
    pub name: Option<String>,
    pub clip_depth: Option<u16>,
    pub filters: Option<Vec<Filter>>,
    pub blend: Option<BlendMode>,
    pub cache_as_bitmap: Option<bool>,
    pub visible: Option<bool>,
    pub clip_actions: Vec<ClipAction>,
}

#[derive(Clone, Debug)]
pub enum Command {
    Place(Box<Place>),
    Remove { depth: u16 },
}

#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub commands: Vec<Command>,
    pub actions: Vec<Actions>,
    /// `DoInitAction`s in this frame: (sprite id, code), run once, before the frame's actions.
    pub init_actions: Vec<(u16, Actions)>,
}

#[derive(Clone, Debug, Default)]
pub struct Timeline {
    pub frames: Vec<Frame>,
    /// Frame labels (as written) to 0-based frame index.
    pub labels: Vec<(String, u16)>,
}

impl Timeline {
    /// The frame a label names (ActionScript matches labels case-insensitively).
    pub fn label(&self, name: &str) -> Option<u16> {
        self.labels
            .iter()
            .find(|(l, _)| l.eq_ignore_ascii_case(name))
            .map(|(_, f)| *f)
    }
}

/// An image the exporter left outside the movie (`DefineExternalImage`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalImage {
    pub format: u16,
    pub width: u16,
    pub height: u16,
    pub export_name: String,
    pub file: String,
}

#[derive(Clone, Debug)]
pub enum Character {
    Shape(Arc<Shape>),
    Morph(Arc<MorphShape>),
    Sprite(Arc<Timeline>),
    EditText(Arc<EditText>),
    Font(Arc<Font>),
    Image(ExternalImage),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Import {
    pub url: String,
    /// (local character id, exported name)
    pub assets: Vec<(u16, String)>,
}

#[derive(Debug, Default)]
pub struct Movie {
    pub version: u8,
    pub stage: Rect,
    pub frame_rate: f32,
    pub background: Option<Color>,
    pub characters: HashMap<u16, Character>,
    pub root: Arc<Timeline>,
    /// Export names (lower-cased, as `attachMovie` matches them) to character ids.
    pub exports: HashMap<String, u16>,
    pub imports: Vec<Import>,
    /// Scaling grids by character id.
    pub scale9: HashMap<u16, Rect>,
    /// `CSMTextSettings` by text character id: (use advanced anti-aliasing, grid fit).
    pub text_settings: HashMap<u16, (bool, u8)>,
    /// Tags read but not handled, by code; and tags that failed to parse.
    pub skipped: BTreeMap<u16, u32>,
    pub damaged: BTreeMap<u16, u32>,
}

impl Movie {
    /// Stage size in pixels.
    pub fn stage_size(&self) -> (f32, f32) {
        (
            self.stage.width() as f32 / 20.0,
            self.stage.height() as f32 / 20.0,
        )
    }

    /// Parses a GFx (`GFX`/`CFX`) or plain SWF (`FWS`/`CWS`) movie.
    pub fn parse(data: &[u8]) -> Result<Self> {
        if data.len() < 8 {
            return Err(SwfError::Truncated);
        }
        let body = match &data[..3] {
            b"FWS" | b"GFX" => data[8..].to_vec(),
            b"CWS" | b"CFX" => {
                let mut out = Vec::new();
                flate2::read::ZlibDecoder::new(&data[8..]).read_to_end(&mut out)?;
                out
            }
            _ => return Err(SwfError::BadSignature),
        };
        let mut movie = Movie {
            version: data[3],
            ..Default::default()
        };
        let mut r = Reader::new(&body);
        movie.stage = Rect::read(&mut r)?;
        movie.frame_rate = r.fixed8()?;
        let _frames = r.u16()?;
        let root = movie.read_timeline(r.rest(), true)?;
        movie.root = Arc::new(root);
        Ok(movie)
    }

    fn read_timeline(&mut self, data: &[u8], root: bool) -> Result<Timeline> {
        let mut tl = Timeline::default();
        let mut frame = Frame::default();
        let mut r = Reader::new(data);
        while !r.is_empty() {
            let cl = r.u16()?;
            let code = cl >> 6;
            let mut len = (cl & 0x3f) as usize;
            if len == 0x3f {
                len = r.u32()? as usize;
            }
            let body = r.bytes(len.min(r.remaining()))?;
            if code == 0 {
                break;
            }
            if code == 1 {
                tl.frames.push(std::mem::take(&mut frame));
                continue;
            }
            if let Err(e) = self.read_tag(code, body, &mut tl, &mut frame, root) {
                log::debug!("gfx: tag {code} damaged: {e}");
                *self.damaged.entry(code).or_default() += 1;
            }
        }
        if !frame.commands.is_empty() || !frame.actions.is_empty() {
            tl.frames.push(frame);
        }
        if tl.frames.is_empty() {
            tl.frames.push(Frame::default());
        }
        Ok(tl)
    }

    fn read_tag(
        &mut self,
        code: u16,
        body: &[u8],
        tl: &mut Timeline,
        frame: &mut Frame,
        root: bool,
    ) -> Result<()> {
        let mut r = Reader::new(body);
        match code {
            9 if root => self.background = Some(Color::rgb(&mut r)?),
            2 | 22 | 32 | 83 => {
                let id = r.u16()?;
                let ver = match code {
                    2 => 1,
                    22 => 2,
                    32 => 3,
                    _ => 4,
                };
                let s = shape::read_shape(&mut r, ver)?;
                self.characters.insert(id, Character::Shape(Arc::new(s)));
            }
            46 => {
                let id = r.u16()?;
                let m = shape::read_morph(&mut r)?;
                self.characters.insert(id, Character::Morph(Arc::new(m)));
            }
            39 => {
                let id = r.u16()?;
                let _count = r.u16()?;
                let sprite = self.read_timeline(r.rest(), false)?;
                self.characters
                    .insert(id, Character::Sprite(Arc::new(sprite)));
            }
            37 => {
                let id = r.u16()?;
                let t = text::read_edit_text(&mut r)?;
                self.characters.insert(id, Character::EditText(Arc::new(t)));
            }
            48 | 75 => {
                let id = r.u16()?;
                let f = text::read_font(&mut r, code == 75)?;
                self.characters.insert(id, Character::Font(Arc::new(f)));
            }
            74 => {
                let id = r.u16()?;
                let b = r.u8()?;
                self.text_settings.insert(id, ((b >> 6) == 1, (b >> 3) & 7));
            }
            78 => {
                let id = r.u16()?;
                self.scale9.insert(id, Rect::read(&mut r)?);
            }
            1001 | 1009 => {
                let id = r.u16()?;
                let format = r.u16()?;
                let width = r.u16()?;
                let height = r.u16()?;
                let export_name = r.pstr()?;
                let file = r.pstr()?;
                self.characters.insert(
                    id,
                    Character::Image(ExternalImage {
                        format,
                        width,
                        height,
                        export_name,
                        file,
                    }),
                );
            }
            56 => {
                let n = r.u16()?;
                for _ in 0..n {
                    let id = r.u16()?;
                    let name = r.cstr()?;
                    self.exports.insert(name.to_ascii_lowercase(), id);
                }
            }
            57 | 71 => {
                let url = r.cstr()?;
                if code == 71 {
                    r.u16()?;
                }
                let n = r.u16()?;
                let mut assets = Vec::new();
                for _ in 0..n {
                    let id = r.u16()?;
                    assets.push((id, r.cstr()?));
                }
                self.imports.push(Import { url, assets });
            }
            43 => {
                let name = r.cstr()?;
                tl.labels.push((name, tl.frames.len() as u16));
            }
            12 => frame.actions.push(Arc::from(body)),
            59 => {
                let id = r.u16()?;
                frame.init_actions.push((id, Arc::from(r.rest())));
            }
            26 | 70 => frame
                .commands
                .push(Command::Place(Box::new(read_place(&mut r, code == 70)?))),
            28 => frame.commands.push(Command::Remove { depth: r.u16()? }),
            5 => {
                r.u16()?;
                frame.commands.push(Command::Remove { depth: r.u16()? });
            }
            // FileAttributes, metadata, exporter info, protection, script limits: nothing to do.
            69 | 77 | 1000 | 24 | 65 | 58 | 64 | 9 => {}
            _ => *self.skipped.entry(code).or_default() += 1,
        }
        Ok(())
    }
}

fn read_filters(r: &mut Reader) -> Result<Vec<Filter>> {
    let n = r.u8()?;
    let mut out = Vec::new();
    for _ in 0..n {
        let id = r.u8()?;
        out.push(match id {
            0 => {
                let color = Color::rgba(r)?;
                let blur = (r.fixed()?, r.fixed()?);
                let angle = r.fixed()?;
                let distance = r.fixed()?;
                let strength = r.fixed8()?;
                let f = r.u8()?;
                Filter::DropShadow {
                    color,
                    blur,
                    angle,
                    distance,
                    strength,
                    inner: f & 0x80 != 0,
                    knockout: f & 0x40 != 0,
                    hide_object: f & 0x20 != 0,
                    passes: f & 0x1f,
                }
            }
            1 => {
                let blur = (r.fixed()?, r.fixed()?);
                let passes = r.u8()? >> 3;
                Filter::Blur { blur, passes }
            }
            2 => {
                let color = Color::rgba(r)?;
                let blur = (r.fixed()?, r.fixed()?);
                let strength = r.fixed8()?;
                let f = r.u8()?;
                Filter::Glow {
                    color,
                    blur,
                    strength,
                    inner: f & 0x80 != 0,
                    knockout: f & 0x40 != 0,
                    passes: f & 0x1f,
                }
            }
            3 => {
                r.skip(27)?;
                Filter::Other(id)
            }
            4 | 7 => {
                let c = r.u8()? as usize;
                r.skip(c * 5 + 19)?;
                Filter::Other(id)
            }
            5 => {
                let (x, y) = (r.u8()? as usize, r.u8()? as usize);
                r.skip(8 + 4 * x * y + 5)?;
                Filter::Other(id)
            }
            6 => {
                r.skip(80)?;
                Filter::Other(id)
            }
            _ => return Err(SwfError::Unsupported(format!("filter {id}"))),
        });
    }
    Ok(out)
}

fn read_clip_actions(r: &mut Reader) -> Result<Vec<ClipAction>> {
    r.u16()?;
    r.u32()?;
    let mut out = Vec::new();
    loop {
        let events = r.u32()?;
        if events == 0 {
            break;
        }
        let size = r.u32()? as usize;
        let body = r.bytes(size)?;
        let (key_code, code) = if events & clip_event::KEY_PRESS != 0 {
            (body.first().copied(), body.get(1..).unwrap_or(&[]))
        } else {
            (None, body)
        };
        out.push(ClipAction {
            events,
            key_code,
            actions: Arc::from(code),
        });
    }
    Ok(out)
}

fn read_place(r: &mut Reader, v3: bool) -> Result<Place> {
    let f1 = r.u8()?;
    let f2 = if v3 { r.u8()? } else { 0 };
    let mut p = Place {
        depth: r.u16()?,
        is_move: f1 & 1 != 0,
        ..Default::default()
    };
    if v3 && (f2 & 8 != 0 || (f2 & 16 != 0 && f1 & 2 != 0)) {
        r.cstr()?;
    }
    if f1 & 2 != 0 {
        p.character = Some(r.u16()?);
    }
    if f1 & 4 != 0 {
        p.matrix = Some(Matrix::read(r)?);
    }
    if f1 & 8 != 0 {
        p.color = Some(ColorTransform::read(r, true)?);
    }
    if f1 & 16 != 0 {
        p.ratio = Some(r.u16()?);
    }
    if f1 & 32 != 0 {
        p.name = Some(r.cstr()?);
    }
    if f1 & 64 != 0 {
        p.clip_depth = Some(r.u16()?);
    }
    if v3 {
        if f2 & 1 != 0 {
            p.filters = Some(read_filters(r)?);
        }
        if f2 & 2 != 0 {
            p.blend = Some(BlendMode::from_u8(r.u8()?));
        }
        if f2 & 4 != 0 {
            p.cache_as_bitmap = Some(r.u8()? != 0);
        }
        if f2 & 32 != 0 {
            p.visible = Some(r.u8()? != 0);
        }
        if f2 & 64 != 0 {
            r.u32()?;
        }
    }
    if f1 & 128 != 0 {
        p.clip_actions = read_clip_actions(r)?;
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(code: u16, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&((code << 6) | 0x3f).to_le_bytes());
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend_from_slice(body);
        v
    }

    /// A tiny uncompressed movie: 100x50 stage, a solid square shape placed at depth 1 named
    /// "box", a frame label, a DoAction and two frames.
    pub(crate) fn tiny_movie() -> Vec<u8> {
        let mut body = Vec::new();
        // RECT: nbits=12: 0, 2000, 0, 1000
        let mut bits: Vec<bool> = Vec::new();
        let mut put = |v: i32, n: u32| {
            for i in (0..n).rev() {
                bits.push((v >> i) & 1 == 1);
            }
        };
        put(12, 5);
        for v in [0, 2000, 0, 1000] {
            put(v, 12);
        }
        let mut rect = vec![0u8; bits.len().div_ceil(8)];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                rect[i / 8] |= 0x80 >> (i % 8);
            }
        }
        body.extend_from_slice(&rect);
        body.extend_from_slice(&[0, 30, 2, 0]);
        // DefineShape id 1: bounds (5-bit zero rect), 1 solid red fill, 0 lines, bits 0/0, end.
        let shape = [1, 0, 0x00, 1, 0x00, 255, 0, 0, 0, 0x00, 0x00];
        body.extend(tag(2, &shape));
        body.extend(tag(43, b"start\0"));
        // PlaceObject2: has name + character, depth 1, id 1, "box"
        let mut po = vec![0x22, 1, 0, 1, 0];
        po.extend_from_slice(b"box\0");
        body.extend(tag(26, &po));
        body.extend(tag(12, &[0x07, 0x00]));
        body.extend(tag(1, &[]));
        body.extend(tag(28, &[1, 0]));
        body.extend(tag(1, &[]));
        body.extend(tag(0, &[]));
        let mut out = b"FWS\x08".to_vec();
        out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
        out.extend(body);
        out
    }

    #[test]
    fn parses_tiny_movie() {
        let m = Movie::parse(&tiny_movie()).unwrap();
        assert_eq!(m.stage_size(), (100.0, 50.0));
        assert_eq!(m.frame_rate, 30.0);
        assert_eq!(m.root.frames.len(), 2);
        assert_eq!(m.root.label("START"), Some(0));
        let Some(Character::Shape(s)) = m.characters.get(&1) else {
            panic!("no shape")
        };
        assert_eq!(
            s.fills,
            vec![Fill::Solid(Color {
                r: 255,
                g: 0,
                b: 0,
                a: 255
            })]
        );
        let Command::Place(p) = &m.root.frames[0].commands[0] else {
            panic!()
        };
        assert_eq!(p.name.as_deref(), Some("box"));
        assert_eq!(p.character, Some(1));
        assert_eq!(m.root.frames[0].actions.len(), 1);
        assert!(matches!(
            m.root.frames[1].commands[0],
            Command::Remove { depth: 1 }
        ));
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(Movie::parse(b"XYZ\x08\0\0\0\0").is_err());
        let mut m = tiny_movie();
        m.truncate(30);
        let _ = Movie::parse(&m);
    }
}
