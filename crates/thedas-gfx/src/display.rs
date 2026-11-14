//! The display list: clips, shapes and text fields in an arena, each with its place in its
//! parent, its transform and a link to its script object.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::avm1::value::{ObjId, Str};
use crate::swf::{
    BlendMode, ClipAction, ColorTransform, Filter, Matrix, MorphShape, Shape, Timeline,
};
use crate::text::TextField;

/// A display object's handle; stale handles (to removed objects) resolve to nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DisplayId {
    pub index: u32,
    pub generation: u32,
}

/// Timeline depths are stored shifted down by this (Flash reports them so from `getDepth`).
pub const DEPTH_OFFSET: i32 = -16384;

/// A filled polygon of the drawing API: RGBA and points (twips).
pub type DrawnFill = ([u8; 4], Vec<(f32, f32)>);

/// Vector drawing made with the drawing API (`beginFill`, `lineTo`, ...).
#[derive(Clone, Debug, Default)]
pub struct Drawing {
    /// Closed fills: colour (RGBA) and polygon points in twips.
    pub fills: Vec<DrawnFill>,
    pub current: Option<DrawnFill>,
    pub pen: (f32, f32),
}

#[derive(Debug)]
pub struct Clip {
    pub timeline: Arc<Timeline>,
    /// The character id the clip was made from (0 for the root).
    pub character: u16,
    /// The frame shown, 0-based; `None` before the first frame has run.
    pub frame: Option<u16>,
    pub playing: bool,
    pub drawing: Option<Drawing>,
    pub clip_actions: Vec<ClipAction>,
    /// Whether `onLoad` (and Load clip events) have run.
    pub loaded: bool,
    pub enabled: bool,
    /// Frames whose `DoInitAction` ran, by sprite id, are kept movie-wide in the player.
    pub hit_area: Option<DisplayId>,
    /// The URL of an image loaded into the clip (`loadMovie`/`MovieClipLoader`).
    pub loaded_image: Option<String>,
}

#[derive(Debug)]
pub enum Kind {
    Clip(Box<Clip>),
    Shape(Arc<Shape>, u16),
    Morph(Arc<MorphShape>, u16),
    Text(Box<TextField>),
    /// A bitmap shown by itself (an external image loaded into a clip).
    Bitmap {
        url: String,
        width: f32,
        height: f32,
    },
}

#[derive(Debug)]
pub struct Object {
    pub kind: Kind,
    pub parent: Option<DisplayId>,
    pub depth: i32,
    pub name: Str,
    pub matrix: Matrix,
    /// Scale (1.0 = 100%) and rotation (degrees) as last set by script, so reading them back
    /// does not drift; `None` when the matrix was set directly.
    pub scale_rotation: Option<(f64, f64, f64)>,
    pub color: ColorTransform,
    pub visible: bool,
    pub blend: BlendMode,
    pub filters: Vec<Filter>,
    pub clip_depth: Option<i32>,
    pub ratio: u16,
    pub children: BTreeMap<i32, DisplayId>,
    pub script: Option<ObjId>,
    /// Placed by a timeline (rather than by script).
    pub from_timeline: bool,
    /// Scaleform's `hitTestDisable`: the object and its children ignore the mouse.
    pub hit_test_disable: bool,
    /// Scaleform's `topmostLevel`: drawn above everything else.
    pub topmost: bool,
}

impl Object {
    pub fn new(kind: Kind) -> Self {
        Self {
            kind,
            parent: None,
            depth: 0,
            name: Str::from(""),
            matrix: Matrix::IDENTITY,
            scale_rotation: None,
            color: ColorTransform::IDENTITY,
            visible: true,
            blend: BlendMode::Normal,
            filters: Vec::new(),
            clip_depth: None,
            ratio: 0,
            children: BTreeMap::new(),
            script: None,
            from_timeline: false,
            hit_test_disable: false,
            topmost: false,
        }
    }

    pub fn clip(&self) -> Option<&Clip> {
        match &self.kind {
            Kind::Clip(c) => Some(c),
            _ => None,
        }
    }

    pub fn clip_mut(&mut self) -> Option<&mut Clip> {
        match &mut self.kind {
            Kind::Clip(c) => Some(c),
            _ => None,
        }
    }

    pub fn text(&self) -> Option<&TextField> {
        match &self.kind {
            Kind::Text(t) => Some(t),
            _ => None,
        }
    }

    pub fn text_mut(&mut self) -> Option<&mut TextField> {
        match &mut self.kind {
            Kind::Text(t) => Some(t),
            _ => None,
        }
    }

    /// `_xscale`, `_yscale` (1.0 = 100%) and `_rotation` (degrees).
    pub fn scale_rotation(&self) -> (f64, f64, f64) {
        if let Some(s) = self.scale_rotation {
            return s;
        }
        let m = &self.matrix;
        let (a, b, c, d) = (m.a as f64, m.b as f64, m.c as f64, m.d as f64);
        let sx = (a * a + b * b).sqrt();
        let det = a * d - b * c;
        let sy = (c * c + d * d).sqrt() * if det < 0.0 { -1.0 } else { 1.0 };
        let rot = b.atan2(a).to_degrees();
        (sx, sy, rot)
    }

    pub fn set_scale_rotation(&mut self, sx: f64, sy: f64, rot: f64) {
        let (s, c) = rot.to_radians().sin_cos();
        self.matrix.a = (c * sx) as f32;
        self.matrix.b = (s * sx) as f32;
        self.matrix.c = (-s * sy) as f32;
        self.matrix.d = (c * sy) as f32;
        self.scale_rotation = Some((sx, sy, rot));
    }
}

#[derive(Default)]
pub struct Display {
    slots: Vec<(u32, Option<Object>)>,
    free: Vec<u32>,
}

impl Display {
    pub fn add(&mut self, o: Object) -> DisplayId {
        if let Some(i) = self.free.pop() {
            let slot = &mut self.slots[i as usize];
            slot.0 = slot.0.wrapping_add(1);
            slot.1 = Some(o);
            DisplayId {
                index: i,
                generation: slot.0,
            }
        } else {
            self.slots.push((0, Some(o)));
            DisplayId {
                index: self.slots.len() as u32 - 1,
                generation: 0,
            }
        }
    }

    pub fn get(&self, id: DisplayId) -> Option<&Object> {
        let (g, o) = self.slots.get(id.index as usize)?;
        if *g == id.generation {
            o.as_ref()
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, id: DisplayId) -> Option<&mut Object> {
        let (g, o) = self.slots.get_mut(id.index as usize)?;
        if *g == id.generation {
            o.as_mut()
        } else {
            None
        }
    }

    /// Frees `id` and everything under it (it must already be out of its parent's list).
    pub fn free_tree(&mut self, id: DisplayId) -> Vec<DisplayId> {
        let mut gone = Vec::new();
        let mut stack = vec![id];
        while let Some(i) = stack.pop() {
            if let Some((g, slot)) = self.slots.get_mut(i.index as usize) {
                if *g != i.generation {
                    continue;
                }
                if let Some(o) = slot.take() {
                    stack.extend(o.children.values().copied());
                    self.free.push(i.index);
                    gone.push(i);
                }
            }
        }
        gone
    }

    pub fn ids(&self) -> impl Iterator<Item = DisplayId> + '_ {
        self.slots.iter().enumerate().filter_map(|(i, (g, o))| {
            o.as_ref().map(|_| DisplayId {
                index: i as u32,
                generation: *g,
            })
        })
    }

    /// The child of `parent` named `name` (case-sensitive), topmost depth first as Flash
    /// resolves duplicates by lowest depth.
    pub fn child_named(&self, parent: DisplayId, name: &str) -> Option<DisplayId> {
        let p = self.get(parent)?;
        p.children
            .values()
            .copied()
            .find(|c| self.get(*c).is_some_and(|o| &*o.name == name))
    }

    /// The matrix from `id`'s space to the root's (twips).
    pub fn world_matrix(&self, id: DisplayId) -> Matrix {
        let mut m = Matrix::IDENTITY;
        let mut cur = Some(id);
        while let Some(c) = cur {
            let Some(o) = self.get(c) else { break };
            m = o.matrix.mul(&m);
            cur = o.parent;
        }
        m
    }

    /// `_target`-style path: `/a/b`, or `_level0.a.b` style when `dots`.
    pub fn path(&self, id: DisplayId, dots: bool) -> String {
        let mut names = Vec::new();
        let mut cur = Some(id);
        while let Some(c) = cur {
            let Some(o) = self.get(c) else { break };
            if o.parent.is_some() {
                names.push(o.name.to_string());
            }
            cur = o.parent;
        }
        names.reverse();
        if dots {
            let mut s = String::from("_level0");
            for n in names {
                s.push('.');
                s.push_str(&n);
            }
            s
        } else if names.is_empty() {
            "/".into()
        } else {
            format!("/{}", names.join("/"))
        }
    }
}
