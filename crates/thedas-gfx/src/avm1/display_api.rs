//! Display-side built-ins: MovieClip and TextField properties and methods, TextFormat,
//! Color, Key, Mouse, Stage, Selection, MovieClipLoader, ExternalInterface and flash.geom.

use super::builtins::class;
use super::object::{DONT_ENUM, Kind as OKind, NativeFn};
use super::value::{ObjId, Str, Value, number_to_string};
use super::{HostValue, Vm};
use crate::display::{DisplayId, Kind};
use crate::swf::{BlendMode, Rect, TextAlign};
use crate::text::{AutoSize, FormatPatch};

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or_default()
}

/// The script object of a display object (made on first use).
pub fn display_value(vm: &mut Vm, d: DisplayId) -> Value {
    let Some(o) = vm.display.get(d) else {
        return Value::Undefined;
    };
    if let Some(s) = o.script {
        return Value::Object(s);
    }
    let proto = match &o.kind {
        Kind::Clip(c) => vm
            .classes
            .get(&c.character)
            .cloned()
            .and_then(|ctor| vm.get(&ctor, "prototype").as_object())
            .unwrap_or(vm.protos.movie_clip),
        Kind::Text(_) => vm.protos.text_field,
        _ => vm.protos.movie_clip,
    };
    let s = vm.alloc(Some(proto), OKind::Display(d));
    if let Some(o) = vm.display.get_mut(d) {
        o.script = Some(s);
    }
    Value::Object(s)
}

/// `d`'s bounds in its own space (twips), `None` when it has no content.
pub fn local_bounds(vm: &Vm, d: DisplayId) -> Option<Rect> {
    let o = vm.display.get(d)?;
    let mut acc: Option<Rect> = None;
    let mut add = |r: Rect| {
        acc = Some(match acc {
            None => r,
            Some(a) => Rect {
                x_min: a.x_min.min(r.x_min),
                x_max: a.x_max.max(r.x_max),
                y_min: a.y_min.min(r.y_min),
                y_max: a.y_max.max(r.y_max),
            },
        });
    };
    match &o.kind {
        Kind::Shape(s, _) => add(s.bounds),
        Kind::Morph(m, _) => {
            let t = o.ratio as f32 / 65535.0;
            let l = |a: i32, b: i32| (a as f32 + (b - a) as f32 * t) as i32;
            let (s, e) = (m.start_bounds, m.end_bounds);
            add(Rect {
                x_min: l(s.x_min, e.x_min),
                x_max: l(s.x_max, e.x_max),
                y_min: l(s.y_min, e.y_min),
                y_max: l(s.y_max, e.y_max),
            });
        }
        Kind::Text(t) => {
            let (x, y, w, h) = t.bounds;
            add(Rect {
                x_min: (x * 20.0) as i32,
                y_min: (y * 20.0) as i32,
                x_max: ((x + w) * 20.0) as i32,
                y_max: ((y + h) * 20.0) as i32,
            });
        }
        Kind::Bitmap { width, height, .. } => add(Rect {
            x_min: 0,
            y_min: 0,
            x_max: (*width * 20.0) as i32,
            y_max: (*height * 20.0) as i32,
        }),
        Kind::Clip(c) => {
            if let Some(dr) = &c.drawing {
                for (_, pts) in dr.fills.iter().chain(dr.current.iter()) {
                    for &(x, y) in pts {
                        add(Rect {
                            x_min: x as i32,
                            x_max: x as i32,
                            y_min: y as i32,
                            y_max: y as i32,
                        });
                    }
                }
            }
        }
    }
    let children: Vec<DisplayId> = o.children.values().copied().collect();
    for c in children {
        if let Some(r) = local_bounds(vm, c) {
            if let Some(co) = vm.display.get(c) {
                add(co.matrix.transform_rect(&r));
            }
        }
    }
    acc
}

fn bounds_in(vm: &Vm, d: DisplayId, space: Option<DisplayId>) -> Rect {
    let Some(b) = local_bounds(vm, d) else {
        let m = vm.display.world_matrix(d);
        let (x, y) = m.apply(0.0, 0.0);
        return Rect {
            x_min: x as i32,
            x_max: x as i32,
            y_min: y as i32,
            y_max: y as i32,
        };
    };
    let world = vm.display.world_matrix(d);
    let m = match space.map(|s| vm.display.world_matrix(s).invert()) {
        Some(Some(inv)) => inv.mul(&world),
        _ => world,
    };
    m.transform_rect(&b)
}

fn clip_frames(vm: &Vm, d: DisplayId) -> Option<(u16, u16)> {
    let c = vm.display.get(d)?.clip()?;
    Some((c.frame.unwrap_or(0) + 1, c.timeline.frames.len() as u16))
}

fn rgb(c: [u8; 3]) -> f64 {
    ((c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32) as f64
}

fn to_rgb(n: f64) -> [u8; 3] {
    let v = super::value::to_int32(n) as u32;
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// Reads a display property; `None` when `name` is not one.
pub fn display_get(vm: &mut Vm, d: DisplayId, name: &str) -> Option<Value> {
    let o = vm.display.get(d)?;
    let is_text = matches!(o.kind, Kind::Text(_));
    let num = |n: f64| Some(Value::Number(n));
    match name {
        "_x" => return num(o.matrix.tx as f64 / 20.0),
        "_y" => return num(o.matrix.ty as f64 / 20.0),
        "_xscale" => return num(o.scale_rotation().0 * 100.0),
        "_yscale" => return num(o.scale_rotation().1 * 100.0),
        "_rotation" => return num(o.scale_rotation().2),
        "_alpha" => return num(o.color.mul[3] as f64 * 100.0),
        "_visible" => return Some(Value::Bool(o.visible)),
        "_name" => return Some(Value::String(o.name.clone())),
        "_parent" => {
            let p = o.parent;
            return Some(p.map(|p| display_value(vm, p)).unwrap_or_default());
        }
        "_root" | "_level0" => {
            let r = vm.root;
            return Some(display_value(vm, r));
        }
        "_target" => return Some(Value::from(vm.display.path(d, false))),
        "_width" | "_height" => {
            let parent = o.parent;
            let b = bounds_in(vm, d, parent);
            return num(if name == "_width" {
                b.width() as f64 / 20.0
            } else {
                b.height() as f64 / 20.0
            });
        }
        "_xmouse" | "_ymouse" => {
            let inv = vm.display.world_matrix(d).invert()?;
            let (x, y) = inv.apply(
                (vm.mouse_pos.0 * 20.0) as f32,
                (vm.mouse_pos.1 * 20.0) as f32,
            );
            return num(if name == "_xmouse" {
                x as f64
            } else {
                y as f64
            } / 20.0);
        }
        "_url" => return Some(Value::from(vm.name.as_str())),
        "_quality" => return Some(Value::from("HIGH")),
        "_highquality" => return num(1.0),
        "_focusrect" => return Some(Value::Bool(false)),
        "_droptarget" => return Some(Value::from("")),
        "blendMode" => return Some(Value::from(o.blend.name())),
        "hitTestDisable" => return Some(Value::Bool(o.hit_test_disable)),
        "topmostLevel" => return Some(Value::Bool(o.topmost)),
        _ => {}
    }
    if !is_text {
        return match name {
            "_currentframe" => num(clip_frames(vm, d)?.0 as f64),
            "_totalframes" | "_framesloaded" => num(clip_frames(vm, d)?.1 as f64),
            _ => None,
        };
    }
    let fonts = vm.fonts.clone();
    let t = vm.display.get_mut(d)?.text_mut()?;
    Some(match name {
        "text" => Value::from(t.text()),
        "htmlText" => Value::from(t.html_text()),
        "html" => Value::Bool(t.html),
        "length" => Value::Number(t.len() as f64),
        "textWidth" => Value::Number(t.layout(&*fonts).width.round() as f64),
        "textHeight" => Value::Number(t.layout(&*fonts).height.round() as f64),
        "autoSize" => match t.auto_size {
            AutoSize::None => Value::from("none"),
            AutoSize::Left => Value::from("left"),
            AutoSize::Center => Value::from("center"),
            AutoSize::Right => Value::from("right"),
        },
        "wordWrap" => Value::Bool(t.word_wrap),
        "multiline" => Value::Bool(t.multiline),
        "textColor" => Value::Number(rgb(t.default_format.color)),
        "selectable" => Value::Bool(t.selectable),
        "type" => Value::from(if t.input { "input" } else { "dynamic" }),
        "variable" => {
            if t.variable.is_empty() {
                Value::Null
            } else {
                Value::from(t.variable.as_str())
            }
        }
        "maxChars" => {
            if t.max_chars == 0 {
                Value::Null
            } else {
                Value::Number(t.max_chars as f64)
            }
        }
        "restrict" => t.restrict.clone().map(Value::from).unwrap_or(Value::Null),
        "scroll" => Value::Number(t.scroll as f64),
        "maxscroll" => Value::Number(t.max_scroll(&*fonts) as f64),
        "bottomScroll" => {
            let lines = t.layout(&*fonts).lines.len().max(1);
            Value::Number(lines as f64)
        }
        "password" => Value::Bool(t.password),
        "border" => Value::Bool(t.border),
        "background" => Value::Bool(t.background),
        "borderColor" => Value::Number(rgb(t.border_color)),
        "backgroundColor" => Value::Number(rgb(t.background_color)),
        "embedFonts" => Value::Bool(true),
        "textAutoSize" => Value::from(t.text_auto_size.as_str()),
        "shadowStyle" => Value::from(t.shadow_style.as_str()),
        "shadowColor" => Value::Number(rgb(t.shadow_color)),
        _ => return None,
    })
}

/// Writes a display property; false when `name` is not one.
pub fn display_set(vm: &mut Vm, d: DisplayId, name: &str, v: &Value) -> bool {
    let Some(o) = vm.display.get(d) else {
        return false;
    };
    let is_text = matches!(o.kind, Kind::Text(_));
    let n = |vm: &mut Vm| vm.to_number(v);
    vm.pick_dirty = true;
    match name {
        "_x" | "_y" => {
            let x = n(vm);
            if let (true, Some(o)) = (x.is_finite(), vm.display.get_mut(d)) {
                let t = (x * 20.0).round() as f32;
                if name == "_x" {
                    o.matrix.tx = t;
                } else {
                    o.matrix.ty = t;
                }
            }
            return true;
        }
        "_xscale" | "_yscale" | "_rotation" => {
            let x = n(vm);
            if let (true, Some(o)) = (x.is_finite(), vm.display.get_mut(d)) {
                let (mut sx, mut sy, mut r) = o.scale_rotation();
                match name {
                    "_xscale" => sx = x / 100.0,
                    "_yscale" => sy = x / 100.0,
                    _ => {
                        r = x % 360.0;
                        if r > 180.0 {
                            r -= 360.0;
                        } else if r < -180.0 {
                            r += 360.0;
                        }
                    }
                }
                o.set_scale_rotation(sx, sy, r);
            }
            return true;
        }
        "_alpha" => {
            let x = n(vm);
            if let (true, Some(o)) = (x.is_finite(), vm.display.get_mut(d)) {
                o.color.mul[3] = (x / 100.0) as f32;
            }
            return true;
        }
        "_visible" => {
            let b = vm.to_bool(v);
            if let Some(o) = vm.display.get_mut(d) {
                o.visible = b;
            }
            return true;
        }
        "_name" => {
            let s = vm.to_str(v);
            if let Some(o) = vm.display.get_mut(d) {
                o.name = s;
            }
            return true;
        }
        "_width" | "_height" => {
            let x = n(vm);
            if !x.is_finite() || x < 0.0 {
                return true;
            }
            if is_text {
                if let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) {
                    if name == "_width" {
                        t.bounds.2 = x as f32;
                    } else {
                        t.bounds.3 = x as f32;
                    }
                    t.invalidate();
                }
                return true;
            }
            let Some(b) = local_bounds(vm, d) else {
                return true;
            };
            let size = if name == "_width" {
                b.width()
            } else {
                b.height()
            } as f64
                / 20.0;
            if size <= 0.0 {
                return true;
            }
            if let Some(o) = vm.display.get_mut(d) {
                let (mut sx, mut sy, r) = o.scale_rotation();
                if name == "_width" {
                    sx = x / size;
                } else {
                    sy = x / size;
                }
                o.set_scale_rotation(sx, sy, r);
            }
            return true;
        }
        "_currentframe" | "_totalframes" | "_framesloaded" | "_target" | "_url" | "_xmouse"
        | "_ymouse" | "_parent" | "_droptarget" => return true,
        "_quality" | "_highquality" | "_focusrect" => return true,
        "blendMode" => {
            let m = match v {
                Value::Number(x) => BlendMode::from_u8(*x as u8),
                _ => {
                    let s = vm.to_str(v);
                    BlendMode::from_name(&s).unwrap_or_default()
                }
            };
            if let Some(o) = vm.display.get_mut(d) {
                o.blend = m;
            }
            return true;
        }
        "hitTestDisable" | "topmostLevel" => {
            let b = vm.to_bool(v);
            if let Some(o) = vm.display.get_mut(d) {
                if name == "hitTestDisable" {
                    o.hit_test_disable = b;
                } else {
                    o.topmost = b;
                }
            }
            return true;
        }
        _ => {}
    }
    if !is_text {
        return false;
    }
    let s = match v {
        Value::Undefined | Value::Null => None,
        v => Some(vm.to_str(v)),
    };
    let num = vm.to_number(v);
    let b = vm.to_bool(v);
    let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) else {
        return false;
    };
    match name {
        "text" => {
            t.set_text(s.as_deref().unwrap_or(""));
        }
        "htmlText" => {
            let s = s.as_deref().unwrap_or("");
            if t.html {
                t.set_html(s);
            } else {
                t.set_text(s);
            }
        }
        "html" => t.html = b,
        "autoSize" => {
            t.auto_size = match (v, s.as_deref()) {
                (Value::Bool(true), _) => AutoSize::Left,
                (_, Some("left")) => AutoSize::Left,
                (_, Some("center")) => AutoSize::Center,
                (_, Some("right")) => AutoSize::Right,
                _ => AutoSize::None,
            };
        }
        "wordWrap" => t.word_wrap = b,
        "multiline" => t.multiline = b,
        "textColor" => {
            let c = to_rgb(num);
            t.default_format.color = c;
            for sp in &mut t.spans {
                sp.format.color = c;
            }
        }
        "selectable" => t.selectable = b,
        "type" => t.input = s.as_deref() == Some("input"),
        "variable" => t.variable = s.map(|x| x.to_string()).unwrap_or_default(),
        "maxChars" => t.max_chars = if num.is_finite() { num as u32 } else { 0 },
        "restrict" => t.restrict = s.map(|x| x.to_string()),
        "scroll" => {
            t.scroll = if num.is_finite() {
                num.max(1.0) as u32
            } else {
                1
            }
        }
        "password" => t.password = b,
        "border" => t.border = b,
        "background" => t.background = b,
        "borderColor" => t.border_color = to_rgb(num),
        "backgroundColor" => t.background_color = to_rgb(num),
        "textAutoSize" => t.text_auto_size = s.map(|x| x.to_string()).unwrap_or_default(),
        "shadowStyle" => t.shadow_style = s.map(|x| x.to_string()).unwrap_or_default(),
        "shadowColor" => t.shadow_color = to_rgb(num),
        "embedFonts" | "condenseWhite" | "antiAliasType" | "gridFitType" | "sharpness"
        | "thickness" | "mouseWheelEnabled" | "length" | "textWidth" | "textHeight"
        | "maxscroll" | "bottomScroll" => {}
        _ => return false,
    }
    t.invalidate();
    apply_autosize(vm, d);
    true
}

/// Resizes an auto-sized field's box to its text.
pub fn apply_autosize(vm: &mut Vm, d: DisplayId) {
    let fonts = vm.fonts.clone();
    let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) else {
        return;
    };
    if t.auto_size == AutoSize::None {
        return;
    }
    let l = t.layout(&*fonts).clone();
    let g = crate::text::GUTTER;
    let w = l.width + 2.0 * g;
    let h = l.height + 2.0 * g;
    if !t.word_wrap {
        let old_w = t.bounds.2;
        match t.auto_size {
            AutoSize::Left => {}
            AutoSize::Center => t.bounds.0 += (old_w - w) / 2.0,
            AutoSize::Right => t.bounds.0 += old_w - w,
            AutoSize::None => {}
        }
        t.bounds.2 = w;
    }
    t.bounds.3 = h;
    t.invalidate();
}

/// After `name` was set on clip `d`, text fields bound to it show the new value.
pub fn sync_bound_text(vm: &mut Vm, d: DisplayId, name: &str) {
    let Some(o) = vm.display.get(d) else { return };
    let kids: Vec<DisplayId> = o.children.values().copied().collect();
    for c in kids {
        let bound = vm
            .display
            .get(c)
            .and_then(|o| o.text())
            .is_some_and(|t| t.variable == name);
        if bound {
            let ov = display_value(vm, d);
            let v = vm.get(&ov, name);
            let s = if v.is_undefined() {
                Str::from("")
            } else {
                vm.to_str(&v)
            };
            if let Some(t) = vm.display.get_mut(c).and_then(|o| o.text_mut()) {
                if t.html {
                    t.set_html(&s);
                } else {
                    t.set_text(&s);
                }
            }
            apply_autosize(vm, c);
            vm.dirty = true;
        }
    }
}

fn this_display(vm: &Vm, this: &Value) -> Option<DisplayId> {
    vm.display_of(this)
}

fn rect_object(vm: &mut Vm, r: Rect) -> Value {
    let o = vm.new_object();
    for (k, v) in [
        ("xMin", r.x_min),
        ("xMax", r.x_max),
        ("yMin", r.y_min),
        ("yMax", r.y_max),
    ] {
        vm.define(o, k, Value::Number(v as f64 / 20.0), 0);
    }
    Value::Object(o)
}

// ---- MovieClip -------------------------------------------------------------------------

fn mc_attach_movie(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(parent) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let id = vm.to_str(&arg(args, 0));
    let name = vm.to_str(&arg(args, 1));
    let depth = vm.to_number(&arg(args, 2));
    let init = arg(args, 3).as_object();
    match crate::player::attach(vm, parent, &id, &name, depth as i32, init) {
        Some(c) => display_value(vm, c),
        None => {
            vm.warn_once(format!("attachMovie: no symbol {id}"));
            Value::Undefined
        }
    }
}

fn mc_create_empty(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(parent) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let name = vm.to_str(&arg(args, 0));
    let depth = vm.to_number(&arg(args, 1));
    match crate::player::create_empty(vm, parent, &name, depth as i32) {
        Some(c) => display_value(vm, c),
        None => Value::Undefined,
    }
}

fn mc_duplicate(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(source) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let Some(name) = args.first() else {
        return Value::Undefined;
    };
    let name = vm.to_str(name);
    let depth = args.get(1).map(|v| vm.to_number(v)).unwrap_or(0.0);
    let init = args.get(2).and_then(Value::as_object);
    match crate::player::duplicate(vm, source, &name, super::value::to_int32(depth), init) {
        Some(d) => display_value(vm, d),
        None => Value::Undefined,
    }
}

fn mc_create_text_field(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(parent) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let name = vm.to_str(&arg(args, 0));
    let n: Vec<f64> = (1..6).map(|i| vm.to_number(&arg(args, i))).collect();
    match crate::player::create_text(vm, parent, &name, n[0] as i32, [n[1], n[2], n[3], n[4]]) {
        Some(c) => display_value(vm, c),
        None => Value::Undefined,
    }
}

fn mc_remove(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    if let Some(d) = this_display(vm, &this) {
        let depth = vm.display.get(d).map(|o| o.depth).unwrap_or(-1);
        if depth >= 0 && d != vm.root {
            crate::player::remove_clip(vm, d);
        }
    }
    Value::Undefined
}

fn mc_swap_depths(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let a = arg(args, 0);
    let other = match &a {
        Value::Object(_) => vm.display_of(&a).map(Ok),
        _ => {
            let n = vm.to_number(&a);
            n.is_finite().then_some(Err(n as i32))
        }
    };
    if let Some(t) = other {
        crate::player::swap_depths(vm, d, t);
    }
    Value::Undefined
}

fn mc_get_depth(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    this_display(vm, &this)
        .and_then(|d| vm.display.get(d))
        .map(|o| Value::Number(o.depth as f64))
        .unwrap_or_default()
}

fn mc_next_depth(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    let Some(o) = this_display(vm, &this).and_then(|d| vm.display.get(d)) else {
        return Value::Undefined;
    };
    let max = o.children.keys().next_back().copied().unwrap_or(-1);
    Value::Number((max + 1).max(0) as f64)
}

fn mc_instance_at_depth(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let depth = vm.to_number(&arg(args, 0)) as i32;
    let Some(c) = this_display(vm, &this)
        .and_then(|d| vm.display.get(d))
        .and_then(|o| o.children.get(&depth).copied())
    else {
        return Value::Undefined;
    };
    display_value(vm, c)
}

fn mc_get_bounds(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let space = match args.first() {
        Some(v) => vm.display_of(v),
        None => Some(d),
    };
    let r = if space == Some(d) {
        local_bounds(vm, d).unwrap_or_default()
    } else {
        bounds_in(vm, d, space)
    };
    rect_object(vm, r)
}

fn point_convert(vm: &mut Vm, this: Value, args: &[Value], to_global: bool) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let p = arg(args, 0);
    let x = vm.get(&p, "x");
    let y = vm.get(&p, "y");
    let (x, y) = (vm.to_number(&x), vm.to_number(&y));
    let m = vm.display.world_matrix(d);
    let m = if to_global { Some(m) } else { m.invert() };
    if let Some(m) = m {
        let (nx, ny) = m.apply((x * 20.0) as f32, (y * 20.0) as f32);
        vm.set(&p, "x", Value::Number(nx as f64 / 20.0));
        vm.set(&p, "y", Value::Number(ny as f64 / 20.0));
    }
    Value::Undefined
}

fn mc_hit_test(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Bool(false);
    };
    if args.len() >= 2 {
        let x = vm.to_number(&args[0]);
        let y = vm.to_number(&args[1]);
        let shape = args.get(2).is_some_and(|v| vm.to_bool(v));
        let (x, y) = ((x * 20.0) as f32, (y * 20.0) as f32);
        if shape {
            return Value::Bool(crate::player::hit_shape(vm, d, x, y));
        }
        let b = bounds_in(vm, d, None);
        return Value::Bool(
            x >= b.x_min as f32
                && x <= b.x_max as f32
                && y >= b.y_min as f32
                && y <= b.y_max as f32,
        );
    }
    let Some(other) = vm.display_of(&arg(args, 0)) else {
        return Value::Bool(false);
    };
    let a = bounds_in(vm, d, None);
    let b = bounds_in(vm, other, None);
    Value::Bool(
        a.x_min <= b.x_max && b.x_min <= a.x_max && a.y_min <= b.y_max && b.y_min <= a.y_max,
    )
}

fn mc_goto(vm: &mut Vm, this: Value, args: &[Value], play: bool) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    match arg(args, 0) {
        Value::String(s) => match s.parse::<f64>() {
            Ok(n) => crate::player::goto(vm, d, (n as i64 - 1).max(0) as u16, play),
            Err(_) => crate::player::goto_label(vm, d, &s, play),
        },
        v => {
            let n = vm.to_number(&v);
            if n.is_finite() {
                crate::player::goto(vm, d, (n as i64 - 1).max(0) as u16, play);
            }
        }
    }
    Value::Undefined
}

fn mc_load_movie(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    if let Some(d) = this_display(vm, &this) {
        let url = vm.to_str(&arg(args, 0));
        crate::player::load_into(vm, d, &url);
    }
    Value::Undefined
}

fn mc_unload_movie(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    if let Some(d) = this_display(vm, &this) {
        crate::player::unload(vm, d);
    }
    Value::Undefined
}

fn bitmap_load(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let image = vm.movie.exports.get(&name.to_ascii_lowercase())
        .and_then(|id| vm.movie.characters.get(id))
        .or_else(|| vm.movie.characters.values().find(|c| matches!(c, crate::swf::Character::Image(i) if i.export_name.eq_ignore_ascii_case(&name))));
    let Some(crate::swf::Character::Image(image)) = image else {
        return Value::Null;
    };
    let (url, width, height) = (image.file.clone(), image.width, image.height);
    let proto = vm.get(&this, "prototype").as_object();
    let object = vm.alloc(proto, OKind::BitmapData { url, width, height });
    for (key, size) in [("width", width), ("height", height)] {
        vm.define(
            object,
            key,
            Value::Number(size as f64),
            DONT_ENUM | super::object::READ_ONLY | super::object::DONT_DELETE,
        );
    }
    Value::Object(object)
}

fn mc_attach_bitmap(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let bitmap = arg(args, 0)
        .as_object()
        .and_then(|id| vm.heap.get(id))
        .map(|o| o.kind.clone());
    if let (Some(parent), Some(OKind::BitmapData { url, width, height })) =
        (vm.display_of(&this), bitmap)
    {
        let depth = super::value::to_int32(vm.to_number(&arg(args, 1)));
        crate::player::attach_bitmap(vm, parent, depth, url, width, height);
    }
    Value::Undefined
}

fn drawing<'a>(vm: &'a mut Vm, this: &Value) -> Option<&'a mut crate::display::Drawing> {
    let d = vm.display_of(this)?;
    vm.dirty = true;
    let c = vm.display.get_mut(d)?.clip_mut()?;
    Some(c.drawing.get_or_insert_with(Default::default))
}

fn mc_begin_fill(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let c = vm.to_number(&arg(args, 0));
    let a = if args.len() > 1 {
        vm.to_number(&args[1])
    } else {
        100.0
    };
    let c = to_rgb(c);
    let a = (a.clamp(0.0, 100.0) * 2.55).round() as u8;
    if let Some(dr) = drawing(vm, &this) {
        let pen = dr.pen;
        dr.current = Some(([c[0], c[1], c[2], a], vec![pen]));
    }
    Value::Undefined
}

fn mc_end_fill(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    if let Some(dr) = drawing(vm, &this) {
        if let Some(f) = dr.current.take() {
            if f.1.len() >= 3 {
                dr.fills.push(f);
            }
        }
    }
    Value::Undefined
}

fn mc_move_to(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let x = vm.to_number(&arg(args, 0)) as f32 * 20.0;
    let y = vm.to_number(&arg(args, 1)) as f32 * 20.0;
    if let Some(dr) = drawing(vm, &this) {
        dr.pen = (x, y);
        if let Some((_, pts)) = &mut dr.current {
            pts.push((x, y));
        }
    }
    Value::Undefined
}

fn mc_line_to(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let x = vm.to_number(&arg(args, 0)) as f32 * 20.0;
    let y = vm.to_number(&arg(args, 1)) as f32 * 20.0;
    if let Some(dr) = drawing(vm, &this) {
        dr.pen = (x, y);
        if let Some((_, pts)) = &mut dr.current {
            pts.push((x, y));
        }
    }
    Value::Undefined
}

fn mc_curve_to(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let cx = vm.to_number(&arg(args, 0)) as f32 * 20.0;
    let cy = vm.to_number(&arg(args, 1)) as f32 * 20.0;
    let x = vm.to_number(&arg(args, 2)) as f32 * 20.0;
    let y = vm.to_number(&arg(args, 3)) as f32 * 20.0;
    if let Some(dr) = drawing(vm, &this) {
        let (px, py) = dr.pen;
        if let Some((_, pts)) = &mut dr.current {
            for i in 1..=8 {
                let t = i as f32 / 8.0;
                let u = 1.0 - t;
                pts.push((
                    u * u * px + 2.0 * u * t * cx + t * t * x,
                    u * u * py + 2.0 * u * t * cy + t * t * y,
                ));
            }
        }
        dr.pen = (x, y);
    }
    Value::Undefined
}

fn mc_clear(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    if let Some(dr) = drawing(vm, &this) {
        *dr = Default::default();
    }
    Value::Undefined
}

fn mc_start_drag(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    if let Some(d) = this_display(vm, &this) {
        let lock = args.first().is_none_or(|v| vm.to_bool(v));
        crate::player::start_drag(vm, d, lock);
    }
    Value::Undefined
}

// ---- TextField -------------------------------------------------------------------------

/// A `TextFormat` object's set fields.
fn read_format(vm: &mut Vm, f: &Value) -> FormatPatch {
    let mut p = FormatPatch::default();
    if f.as_object().is_none() {
        return p;
    }
    let get = |vm: &mut Vm, k: &str| {
        let v = vm.get(f, k);
        if v.is_nullish() { None } else { Some(v) }
    };
    if let Some(v) = get(vm, "font") {
        p.font = Some(vm.to_str(&v).to_string());
    }
    if let Some(v) = get(vm, "size") {
        p.size = Some(vm.to_number(&v) as f32);
    }
    if let Some(v) = get(vm, "color") {
        p.color = Some(to_rgb(vm.to_number(&v)));
    }
    if let Some(v) = get(vm, "bold") {
        p.bold = Some(vm.to_bool(&v));
    }
    if let Some(v) = get(vm, "italic") {
        p.italic = Some(vm.to_bool(&v));
    }
    if let Some(v) = get(vm, "underline") {
        p.underline = Some(vm.to_bool(&v));
    }
    if let Some(v) = get(vm, "align") {
        p.align = Some(match &*vm.to_str(&v) {
            "right" => TextAlign::Right,
            "center" => TextAlign::Center,
            "justify" => TextAlign::Justify,
            _ => TextAlign::Left,
        });
    }
    for (k, slot) in [
        ("leftMargin", &mut p.left_margin),
        ("rightMargin", &mut p.right_margin),
        ("indent", &mut p.indent),
        ("leading", &mut p.leading),
        ("letterSpacing", &mut p.letter_spacing),
    ] {
        if let Some(v) = get(vm, k) {
            *slot = Some(vm.to_number(&v) as f32);
        }
    }
    if let Some(v) = get(vm, "kerning") {
        p.kerning = Some(vm.to_bool(&v));
    }
    if let Some(v) = get(vm, "url") {
        p.url = Some(vm.to_str(&v).to_string());
    }
    p
}

fn format_object(vm: &mut Vm, f: &crate::text::Format) -> Value {
    let proto = vm.protos.text_format;
    let o = vm.alloc(Some(proto), OKind::Plain);
    let align = match f.align {
        TextAlign::Left => "left",
        TextAlign::Right => "right",
        TextAlign::Center => "center",
        TextAlign::Justify => "justify",
    };
    let vals: Vec<(&str, Value)> = vec![
        ("font", Value::from(f.font.as_str())),
        ("size", Value::Number(f.size as f64)),
        ("color", Value::Number(rgb(f.color))),
        ("bold", Value::Bool(f.bold)),
        ("italic", Value::Bool(f.italic)),
        ("underline", Value::Bool(f.underline)),
        ("url", Value::from(f.url.as_str())),
        ("target", Value::from("")),
        ("align", Value::from(align)),
        ("leftMargin", Value::Number(f.left_margin as f64)),
        ("rightMargin", Value::Number(f.right_margin as f64)),
        ("indent", Value::Number(f.indent as f64)),
        ("leading", Value::Number(f.leading as f64)),
        ("letterSpacing", Value::Number(f.letter_spacing as f64)),
        ("kerning", Value::Bool(f.kerning)),
        ("blockIndent", Value::Number(0.0)),
        ("bullet", Value::Bool(false)),
        ("tabStops", Value::Null),
    ];
    for (k, v) in vals {
        vm.define(o, k, v, 0);
    }
    Value::Object(o)
}

fn tf_set_text_format(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let (b, e, f) = match args.len() {
        0 => return Value::Undefined,
        1 => (None, None, args[0].clone()),
        2 => (Some(vm.to_number(&args[0])), None, args[1].clone()),
        _ => (
            Some(vm.to_number(&args[0])),
            Some(vm.to_number(&args[1])),
            args[2].clone(),
        ),
    };
    let patch = read_format(vm, &f);
    if let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) {
        let len = t.len();
        let begin = b
            .filter(|x| x.is_finite() && *x >= 0.0)
            .map(|x| x as usize)
            .unwrap_or(0);
        let end = match (b, e) {
            (None, _) => len,
            (Some(_), None) => begin + 1,
            (Some(_), Some(e)) => {
                if e.is_finite() {
                    (e.max(0.0) as usize).min(len)
                } else {
                    len
                }
            }
        };
        if b.is_none() {
            // The whole field: Flash also makes it the format new text gets.
            patch.apply(&mut t.default_format);
        }
        t.apply_format(begin, end.max(begin), &patch);
    }
    apply_autosize(vm, d);
    vm.dirty = true;
    Value::Undefined
}

fn tf_set_new_text_format(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let patch = read_format(vm, &arg(args, 0));
    if let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) {
        patch.apply(&mut t.default_format);
    }
    Value::Undefined
}

fn tf_get_text_format(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let i = if args.is_empty() {
        0
    } else {
        vm.to_number(&args[0]).max(0.0) as usize
    };
    let f = vm
        .display
        .get(d)
        .and_then(|o| o.text())
        .map(|t| t.format_at(i));
    match f {
        Some(f) => format_object(vm, &f),
        None => Value::Undefined,
    }
}

fn tf_get_new_text_format(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    let Some(f) = this_display(vm, &this)
        .and_then(|d| vm.display.get(d))
        .and_then(|o| o.text())
        .map(|t| t.default_format.clone())
    else {
        return Value::Undefined;
    };
    format_object(vm, &f)
}

fn tf_replace_text(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = this_display(vm, &this) else {
        return Value::Undefined;
    };
    let b = vm.to_number(&arg(args, 0)).max(0.0) as usize;
    let e = vm.to_number(&arg(args, 1)).max(0.0) as usize;
    let s = vm.to_str(&arg(args, 2));
    if let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) {
        let text: Vec<char> = t.text().chars().collect();
        let (b, e) = (b.min(text.len()), e.min(text.len()).max(b.min(text.len())));
        let mut out: String = text[..b].iter().collect();
        out.push_str(&s);
        out.extend(text[e..].iter());
        t.set_text(&out);
    }
    apply_autosize(vm, d);
    vm.dirty = true;
    Value::Undefined
}

fn text_format_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let names = [
        "font",
        "size",
        "color",
        "bold",
        "italic",
        "underline",
        "url",
        "target",
        "align",
        "leftMargin",
        "rightMargin",
        "indent",
        "leading",
    ];
    for (i, n) in names.iter().enumerate() {
        let v = args.get(i).cloned().unwrap_or(Value::Null);
        vm.set(&this, n, v);
    }
    for n in [
        "letterSpacing",
        "kerning",
        "blockIndent",
        "bullet",
        "tabStops",
    ] {
        vm.set(&this, n, Value::Null);
    }
    this
}

// ---- Color -----------------------------------------------------------------------------

fn color_target(vm: &mut Vm, this: &Value) -> Option<DisplayId> {
    let t = vm.get(this, "__target");
    vm.display_of(&t)
}

fn color_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    if let Some(o) = this.as_object() {
        vm.define(o, "__target", arg(args, 0), DONT_ENUM);
    }
    this
}

fn color_set_rgb(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let c = to_rgb(vm.to_number(&arg(args, 0)));
    if let Some(d) = color_target(vm, &this) {
        if let Some(o) = vm.display.get_mut(d) {
            for (i, v) in c.iter().enumerate() {
                o.color.mul[i] = 0.0;
                o.color.add[i] = *v as f32;
            }
        }
        vm.dirty = true;
    }
    Value::Undefined
}

fn color_get_rgb(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    let Some(d) = color_target(vm, &this) else {
        return Value::Undefined;
    };
    let c = vm.display.get(d).map(|o| o.color).unwrap_or_default();
    Value::Number(rgb([c.add[0] as u8, c.add[1] as u8, c.add[2] as u8]))
}

fn color_set_transform(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(d) = color_target(vm, &this) else {
        return Value::Undefined;
    };
    let t = arg(args, 0);
    let mut cx = vm.display.get(d).map(|o| o.color).unwrap_or_default();
    for (i, c) in ["r", "g", "b", "a"].iter().enumerate() {
        let m = vm.get(&t, &format!("{c}a"));
        if !m.is_nullish() {
            cx.mul[i] = (vm.to_number(&m) / 100.0) as f32;
        }
        let a = vm.get(&t, &format!("{c}b"));
        if !a.is_nullish() {
            cx.add[i] = vm.to_number(&a) as f32;
        }
    }
    if let Some(o) = vm.display.get_mut(d) {
        o.color = cx;
    }
    vm.dirty = true;
    Value::Undefined
}

fn color_get_transform(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    let Some(d) = color_target(vm, &this) else {
        return Value::Undefined;
    };
    let cx = vm.display.get(d).map(|o| o.color).unwrap_or_default();
    let o = vm.new_object();
    for (i, c) in ["r", "g", "b", "a"].iter().enumerate() {
        vm.define(
            o,
            &format!("{c}a"),
            Value::Number(cx.mul[i] as f64 * 100.0),
            0,
        );
        vm.define(o, &format!("{c}b"), Value::Number(cx.add[i] as f64), 0);
    }
    Value::Object(o)
}

// ---- Key, Mouse, Stage, Selection --------------------------------------------------------

fn add_listener(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let list = listeners(vm, &this);
    let l = arg(args, 0);
    if let Some(OKind::Array(items)) = list.and_then(|o| vm.heap.get_mut(o)).map(|o| &mut o.kind) {
        items.retain(|x| *x != l);
        items.push(l);
    }
    Value::Bool(true)
}

fn remove_listener(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let list = listeners(vm, &this);
    let l = arg(args, 0);
    if let Some(OKind::Array(items)) = list.and_then(|o| vm.heap.get_mut(o)).map(|o| &mut o.kind) {
        let before = items.len();
        items.retain(|x| *x != l);
        return Value::Bool(before != items.len());
    }
    Value::Bool(false)
}

fn listeners(vm: &mut Vm, holder: &Value) -> Option<ObjId> {
    let v = vm.get(holder, "_listeners");
    if let Some(o) = v.as_object() {
        return Some(o);
    }
    let h = holder.as_object()?;
    let a = vm.new_array(Vec::new());
    vm.define(h, "_listeners", Value::Object(a), DONT_ENUM);
    Some(a)
}

/// Calls `method` on every listener of `holder` (Key, Mouse, Stage, a loader).
pub fn broadcast(vm: &mut Vm, holder: ObjId, method: &str, args: &[Value]) {
    let Some(list) = listeners(vm, &Value::Object(holder)) else {
        return;
    };
    let items = match vm.heap.get(list).map(|o| &o.kind) {
        Some(OKind::Array(v)) => v.clone(),
        _ => return,
    };
    for l in items {
        let f = vm.get(&l, method);
        if matches!(&f, Value::Object(o) if vm.is_function(*o)) {
            vm.call(&f, l.clone(), args);
        }
    }
}

fn selection_get_focus(vm: &mut Vm, _: Value, _: &[Value]) -> Value {
    match vm.focus.filter(|f| vm.display.get(*f).is_some()) {
        Some(f) => Value::from(vm.display.path(f, true)),
        None => Value::Null,
    }
}

fn selection_index(vm: &Vm, which: u8) -> Value {
    let Some(t) = vm
        .focus
        .and_then(|f| vm.display.get(f))
        .and_then(|o| o.text())
    else {
        return Value::Number(-1.0);
    };
    let (begin, end) = t.selection();
    let index = match which {
        0 => begin,
        1 => end,
        _ => t.caret.unwrap_or(end).min(t.len()),
    };
    Value::Number(
        t.text()
            .chars()
            .take(index)
            .map(|c| c.len_utf16())
            .sum::<usize>() as f64,
    )
}

fn selection_set_range(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    if args.is_empty() {
        return Value::Undefined;
    }
    let begin = super::value::to_int32(vm.to_number(&arg(args, 0))).max(0) as usize;
    let end = args
        .get(1)
        .map(|v| super::value::to_int32(vm.to_number(v)).max(0) as usize)
        .unwrap_or(usize::MAX);
    if let Some(t) = vm
        .focus
        .and_then(|f| vm.display.get_mut(f))
        .and_then(|o| o.text_mut())
    {
        let text = t.text();
        let char_index = |index| {
            let mut units = 0;
            text.chars()
                .take_while(|c| {
                    units += c.len_utf16();
                    units <= index
                })
                .count()
        };
        t.set_selection(char_index(begin), char_index(end));
        vm.dirty = true;
    }
    Value::Undefined
}

fn selection_set_focus(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let Some(a) = args.first() else {
        return Value::Bool(false);
    };
    if a.is_nullish() {
        crate::player::set_focus(vm, None);
        return Value::Bool(true);
    }
    let d = match a {
        Value::String(s) => {
            let scope = std::rc::Rc::new(super::Scope {
                object: vm.global,
                kind: super::ScopeKind::Global,
                parent: None,
            });
            let s = s.clone();
            let root = vm.root;
            vm.as_display(&Value::String(s), &scope, root)
        }
        v => vm.display_of(v),
    };
    let Some(d) = d else {
        return Value::Bool(false);
    };
    crate::player::set_focus(vm, Some(d));
    // AVM1 programmatic focus selects the field, even when it already had focus.
    // Mouse focus continues to place its caret from the pointer in the player.
    if let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) {
        t.set_selection(0, t.len());
        vm.dirty = true;
    }
    Value::Bool(true)
}

// ---- MovieClipLoader ---------------------------------------------------------------------

fn loader_new(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    if let Some(o) = this.as_object() {
        let a = vm.new_array(vec![this.clone()]);
        vm.define(o, "_listeners", Value::Object(a), DONT_ENUM);
    }
    this
}

fn loader_load_clip(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let url = vm.to_str(&arg(args, 0));
    let t = arg(args, 1);
    let target = match &t {
        Value::String(s) => {
            let scope = std::rc::Rc::new(super::Scope {
                object: vm.global,
                kind: super::ScopeKind::Global,
                parent: None,
            });
            let root = vm.root;
            vm.as_display(&Value::String(s.clone()), &scope, root)
        }
        v => vm.display_of(v),
    };
    let Some(target) = target else {
        return Value::Bool(false);
    };
    let ok = crate::player::load_into(vm, target, &url);
    let Some(holder) = this.as_object() else {
        return Value::Bool(ok);
    };
    let tv = display_value(vm, target);
    broadcast(vm, holder, "onLoadStart", std::slice::from_ref(&tv));
    if ok {
        broadcast(vm, holder, "onLoadComplete", std::slice::from_ref(&tv));
        broadcast(vm, holder, "onLoadInit", std::slice::from_ref(&tv));
    } else {
        broadcast(vm, holder, "onLoadError", &[tv, Value::from("URLNotFound")]);
    }
    Value::Bool(true)
}

fn loader_unload_clip(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    if let Some(d) = vm.display_of(&arg(args, 0)) {
        crate::player::unload(vm, d);
    }
    Value::Bool(true)
}

// ---- ExternalInterface -----------------------------------------------------------------

fn ei_call(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let hv: Vec<HostValue> = args
        .get(1..)
        .unwrap_or(&[])
        .iter()
        .map(|a| vm.to_host(a, 0))
        .collect();
    let r = vm.host.external_call(&name, &hv);
    vm.from_host(&r)
}

fn ei_add_callback(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0)).to_string();
    let this = arg(args, 1);
    let f = arg(args, 2);
    vm.callbacks.insert(name, (this, f));
    Value::Bool(true)
}

// ---- flash.geom --------------------------------------------------------------------------

fn point_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let x = args.first().map(|v| vm.to_number(v)).unwrap_or(0.0);
    let y = args.get(1).map(|v| vm.to_number(v)).unwrap_or(0.0);
    vm.set(&this, "x", Value::Number(x));
    vm.set(&this, "y", Value::Number(y));
    this
}

fn rect_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    for (i, k) in ["x", "y", "width", "height"].iter().enumerate() {
        let v = args.get(i).map(|v| vm.to_number(v)).unwrap_or(0.0);
        vm.set(&this, k, Value::Number(v));
    }
    this
}

fn nums(vm: &mut Vm, o: &Value, keys: &[&str]) -> Vec<f64> {
    keys.iter()
        .map(|k| {
            let v = vm.get(o, k);
            vm.to_number(&v)
        })
        .collect()
}

fn mc_set_mask(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(target) = vm.display_of(&this) else {
        return Value::Undefined;
    };
    let Some(value) = args.first() else {
        return Value::Undefined;
    };
    let mask = if value.is_nullish() {
        None
    } else {
        let activation = vm.native_target.unwrap_or(vm.root);
        let object = display_value(vm, activation)
            .as_object()
            .unwrap_or(vm.global);
        let scope = std::rc::Rc::new(super::Scope {
            object,
            kind: super::ScopeKind::Target,
            parent: Some(std::rc::Rc::new(super::Scope {
                object: vm.global,
                kind: super::ScopeKind::Global,
                parent: None,
            })),
        });
        let Some(mask) = vm.as_display(value, &scope, activation) else {
            return Value::Bool(false);
        };
        Some(mask)
    };
    let ok = vm.display.set_mask(target, mask);
    if ok {
        vm.dirty = true;
        vm.pick_dirty = true;
    }
    Value::Bool(ok)
}

pub fn install(vm: &mut Vm) {
    let g = vm.global;
    let p = vm.protos;
    let mc = class(vm, g, "MovieClip", |_, this, _| this, p.movie_clip);
    let _ = mc;
    let mc_methods: &[(&str, NativeFn)] = &[
        ("attachMovie", mc_attach_movie),
        ("createEmptyMovieClip", mc_create_empty),
        ("duplicateMovieClip", mc_duplicate),
        ("createTextField", mc_create_text_field),
        ("removeMovieClip", mc_remove),
        ("swapDepths", mc_swap_depths),
        ("getDepth", mc_get_depth),
        ("getNextHighestDepth", mc_next_depth),
        ("getInstanceAtDepth", mc_instance_at_depth),
        ("getBounds", mc_get_bounds),
        ("getRect", mc_get_bounds),
        ("localToGlobal", |vm, t, a| point_convert(vm, t, a, true)),
        ("globalToLocal", |vm, t, a| point_convert(vm, t, a, false)),
        ("hitTest", mc_hit_test),
        ("gotoAndStop", |vm, t, a| mc_goto(vm, t, a, false)),
        ("gotoAndPlay", |vm, t, a| mc_goto(vm, t, a, true)),
        ("play", |vm, t, _| {
            if let Some(d) = vm.display_of(&t) {
                crate::player::set_playing(vm, d, true);
            }
            Value::Undefined
        }),
        ("stop", |vm, t, _| {
            if let Some(d) = vm.display_of(&t) {
                crate::player::set_playing(vm, d, false);
            }
            Value::Undefined
        }),
        ("nextFrame", |vm, t, _| {
            if let Some(d) = vm.display_of(&t) {
                crate::player::clip_step(vm, d, 1);
            }
            Value::Undefined
        }),
        ("prevFrame", |vm, t, _| {
            if let Some(d) = vm.display_of(&t) {
                crate::player::clip_step(vm, d, -1);
            }
            Value::Undefined
        }),
        ("startDrag", mc_start_drag),
        ("stopDrag", |vm, _, _| {
            vm.dragging = None;
            Value::Undefined
        }),
        ("loadMovie", mc_load_movie),
        ("unloadMovie", mc_unload_movie),
        ("attachBitmap", mc_attach_bitmap),
        ("beginFill", mc_begin_fill),
        ("endFill", mc_end_fill),
        ("moveTo", mc_move_to),
        ("lineTo", mc_line_to),
        ("curveTo", mc_curve_to),
        ("clear", mc_clear),
        ("lineStyle", |_, _, _| Value::Undefined),
        ("getSWFVersion", |_, _, _| Value::Number(8.0)),
        ("getBytesLoaded", |_, _, _| Value::Number(1.0)),
        ("getBytesTotal", |_, _, _| Value::Number(1.0)),
        ("setMask", mc_set_mask),
    ];
    for (n, f) in mc_methods {
        vm.define_native(p.movie_clip, n, *f);
    }
    vm.define(p.movie_clip, "enabled", Value::Bool(true), DONT_ENUM);
    vm.define(p.movie_clip, "useHandCursor", Value::Bool(true), DONT_ENUM);

    class(vm, g, "TextField", |_, this, _| this, p.text_field);
    let tf_methods: &[(&str, NativeFn)] = &[
        ("setTextFormat", tf_set_text_format),
        ("setNewTextFormat", tf_set_new_text_format),
        ("getTextFormat", tf_get_text_format),
        ("getNewTextFormat", tf_get_new_text_format),
        ("replaceText", tf_replace_text),
        ("removeTextField", mc_remove),
        ("getDepth", mc_get_depth),
        ("swapDepths", mc_swap_depths),
    ];
    for (n, f) in tf_methods {
        vm.define_native(p.text_field, n, *f);
    }
    class(vm, g, "TextFormat", text_format_new, p.text_format);

    let color_proto = vm.new_object();
    class(vm, g, "Color", color_new, color_proto);
    vm.define_native(color_proto, "setRGB", color_set_rgb);
    vm.define_native(color_proto, "getRGB", color_get_rgb);
    vm.define_native(color_proto, "setTransform", color_set_transform);
    vm.define_native(color_proto, "getTransform", color_get_transform);

    for (name, holder) in [
        ("Key", vm.key),
        ("Mouse", vm.mouse),
        ("Stage", vm.stage),
        ("Selection", vm.selection),
    ] {
        vm.define(g, name, Value::Object(holder), DONT_ENUM);
        vm.define_native(holder, "addListener", add_listener);
        vm.define_native(holder, "removeListener", remove_listener);
        listeners(vm, &Value::Object(holder));
    }
    let key = vm.key;
    for (k, v) in [
        ("BACKSPACE", 8),
        ("CAPSLOCK", 20),
        ("CONTROL", 17),
        ("DELETEKEY", 46),
        ("DOWN", 40),
        ("END", 35),
        ("ENTER", 13),
        ("ESCAPE", 27),
        ("HOME", 36),
        ("INSERT", 45),
        ("LEFT", 37),
        ("PGDN", 34),
        ("PGUP", 33),
        ("RIGHT", 39),
        ("SHIFT", 16),
        ("SPACE", 32),
        ("TAB", 9),
        ("UP", 38),
        ("ALT", 18),
    ] {
        vm.define(key, k, Value::Number(v as f64), DONT_ENUM);
    }
    vm.define_native(key, "isDown", |vm, _, a| {
        let k = vm.to_number(&arg(a, 0)) as u32;
        Value::Bool(vm.keys_down.contains(&k))
    });
    vm.define_native(key, "getCode", |vm, _, _| Value::Number(vm.last_key as f64));
    vm.define_native(key, "getAscii", |vm, _, _| {
        Value::Number(vm.last_key as f64)
    });
    vm.define_native(key, "isToggled", |_, _, _| Value::Bool(false));
    let mouse = vm.mouse;
    vm.define_native(mouse, "show", |_, _, _| Value::Number(1.0));
    vm.define_native(mouse, "hide", |_, _, _| Value::Number(1.0));
    vm.define_native(
        mouse,
        "getTopMostEntity",
        |vm, _, args| match crate::player::topmost_entity(
            vm,
            args.first().is_none_or(|v| vm.to_bool(v)),
        ) {
            Some(d) => display_value(vm, d),
            None => Value::Undefined,
        },
    );
    let stage = vm.stage;
    let getter = super::builtins::native(vm, |vm, _, _| Value::Number(vm.stage_size.0));
    let getter_h = super::builtins::native(vm, |vm, _, _| Value::Number(vm.stage_size.1));
    for (k, gtr) in [("width", getter), ("height", getter_h)] {
        if let Some(o) = vm.heap.get_mut(stage) {
            o.props.insert(
                Str::from(k),
                super::object::Prop {
                    value: Value::Undefined,
                    flags: DONT_ENUM,
                    accessor: Some((gtr, Value::Undefined)),
                },
            );
        }
    }
    vm.define(stage, "scaleMode", Value::from("showAll"), DONT_ENUM);
    vm.define(stage, "align", Value::from(""), DONT_ENUM);
    vm.define(stage, "showMenu", Value::Bool(false), DONT_ENUM);
    let sel = vm.selection;
    vm.define_native(sel, "getFocus", selection_get_focus);
    vm.define_native(sel, "setFocus", selection_set_focus);
    vm.define_native(sel, "getBeginIndex", |vm, _, _| selection_index(vm, 0));
    vm.define_native(sel, "getEndIndex", |vm, _, _| selection_index(vm, 1));
    vm.define_native(sel, "getCaretIndex", |vm, _, _| selection_index(vm, 2));
    vm.define_native(sel, "setSelection", selection_set_range);

    let loader_proto = vm.new_object();
    class(vm, g, "MovieClipLoader", loader_new, loader_proto);
    vm.define_native(loader_proto, "loadClip", loader_load_clip);
    vm.define_native(loader_proto, "unloadClip", loader_unload_clip);
    vm.define_native(loader_proto, "addListener", add_listener);
    vm.define_native(loader_proto, "removeListener", remove_listener);
    vm.define_native(loader_proto, "getProgress", |vm, _, _| {
        let o = vm.new_object();
        vm.define(o, "bytesLoaded", Value::Number(1.0), 0);
        vm.define(o, "bytesTotal", Value::Number(1.0), 0);
        Value::Object(o)
    });

    // The `flash` package.
    let flash = vm.new_object();
    vm.define(g, "flash", Value::Object(flash), DONT_ENUM);
    let display = vm.new_object();
    vm.define(flash, "display", Value::Object(display), 0);
    let bitmap_proto = vm.new_object();
    let bitmap_class = class(vm, display, "BitmapData", |_, this, _| this, bitmap_proto);
    vm.define_native(bitmap_class, "loadBitmap", bitmap_load);
    let external = vm.new_object();
    vm.define(flash, "external", Value::Object(external), 0);
    let ei = vm.new_object();
    vm.define(external, "ExternalInterface", Value::Object(ei), 0);
    vm.define_native(ei, "call", ei_call);
    vm.define_native(ei, "addCallback", ei_add_callback);
    vm.define(ei, "available", Value::Bool(true), 0);
    let geom = vm.new_object();
    vm.define(flash, "geom", Value::Object(geom), 0);
    let point_proto = vm.new_object();
    class(vm, geom, "Point", point_new, point_proto);
    vm.define_native(point_proto, "clone", |vm, t, _| {
        let v = nums(vm, &t, &["x", "y"]);
        let o = vm.new_object();
        vm.define(o, "x", Value::Number(v[0]), 0);
        vm.define(o, "y", Value::Number(v[1]), 0);
        if let Some(pp) = t
            .as_object()
            .and_then(|x| vm.heap.get(x))
            .and_then(|x| x.proto)
        {
            if let Some(obj) = vm.heap.get_mut(o) {
                obj.proto = Some(pp);
            }
        }
        Value::Object(o)
    });
    vm.define_native(point_proto, "toString", |vm, t, _| {
        let v = nums(vm, &t, &["x", "y"]);
        Value::from(format!(
            "(x={}, y={})",
            number_to_string(v[0]),
            number_to_string(v[1])
        ))
    });
    let rect_proto = vm.new_object();
    class(vm, geom, "Rectangle", rect_new, rect_proto);
    vm.define_native(rect_proto, "contains", |vm, t, a| {
        let r = nums(vm, &t, &["x", "y", "width", "height"]);
        let x = vm.to_number(&arg(a, 0));
        let y = vm.to_number(&arg(a, 1));
        Value::Bool(x >= r[0] && x < r[0] + r[2] && y >= r[1] && y < r[1] + r[3])
    });
    vm.define_native(rect_proto, "containsPoint", |vm, t, a| {
        let r = nums(vm, &t, &["x", "y", "width", "height"]);
        let pt = nums(vm, &arg(a, 0), &["x", "y"]);
        Value::Bool(pt[0] >= r[0] && pt[0] < r[0] + r[2] && pt[1] >= r[1] && pt[1] < r[1] + r[3])
    });
    vm.define_native(rect_proto, "isEmpty", |vm, t, _| {
        let r = nums(vm, &t, &["width", "height"]);
        Value::Bool(r[0] <= 0.0 || r[1] <= 0.0)
    });
    vm.define_native(rect_proto, "intersects", |vm, t, a| {
        let r = nums(vm, &t, &["x", "y", "width", "height"]);
        let s = nums(vm, &arg(a, 0), &["x", "y", "width", "height"]);
        Value::Bool(
            r[0] < s[0] + s[2] && s[0] < r[0] + r[2] && r[1] < s[1] + s[3] && s[1] < r[1] + r[3],
        )
    });
}
