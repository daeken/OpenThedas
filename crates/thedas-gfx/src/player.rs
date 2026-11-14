//! The player: timelines, the action queue, events, input and the host interface.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::avm1::object::Heap;
use crate::avm1::value::{ObjId, Str, Value};
use crate::avm1::{Host, HostValue, Queued, Vm, builtins, display_api, interp};
use crate::display::{Clip, DEPTH_OFFSET, Display, DisplayId, Kind, Object};
use crate::swf::{Character, ClipAction, Command, Movie, Place, Record, clip_event};
use crate::text::{Fonts, TextField};

#[cfg(test)]
mod construction_tests;

/// Scaleform's mouse buttons as the movies number them.
pub const MOUSE_LEFT: u8 = 1;
pub const MOUSE_RIGHT: u8 = 2;
pub const MOUSE_MIDDLE: u8 = 3;

pub struct Player {
    pub vm: Vm,
    /// Milliseconds per frame.
    pub frame_ms: f64,
    accum: f64,
    /// The clip under the mouse that takes button events, and whether the button is down
    /// on it.
    hover: Option<DisplayId>,
    pressed: Option<(DisplayId, u8)>,
    text_drag: Option<DisplayId>,
    queue_init: VecDeque<Queued>,
    queue_construct: VecDeque<Queued>,
    queue_default: VecDeque<Queued>,
    /// Whether press handlers get the mouse button as Scaleform passed it (`onPress(1)`).
    pub pass_buttons: bool,
}

fn clip_for(timeline: Arc<crate::swf::Timeline>, character: u16) -> Clip {
    Clip {
        timeline,
        character,
        frame: None,
        playing: true,
        drawing: None,
        clip_actions: Vec::new(),
        loaded: false,
        enabled: true,
        hit_area: None,
        loaded_image: None,
    }
}

impl Player {
    /// Starts a movie. `extra_frame_one` is bytecode run after frame one's own actions (the
    /// host's glue); `stage_size` is the visible area in stage units.
    pub fn new(
        data: &[u8],
        name: &str,
        host: Box<dyn Host>,
        fonts: Arc<dyn Fonts>,
        extra_frame_one: Option<Vec<u8>>,
    ) -> Result<Self, crate::swf::SwfError> {
        let mut movie = Movie::parse(data)?;
        if let Some(extra) = extra_frame_one {
            let mut root = (*movie.root).clone();
            if let Some(f) = root.frames.first_mut() {
                f.actions.push(Arc::from(extra));
            }
            movie.root = Arc::new(root);
        }
        let movie = Arc::new(movie);
        let mut heap = Heap::default();
        let (global, protos) = builtins::create_protos(&mut heap);
        let alloc_plain = |heap: &mut Heap| {
            heap.alloc(crate::avm1::object::Object {
                proto: Some(protos.object),
                ..Default::default()
            })
        };
        let key = alloc_plain(&mut heap);
        let mouse = alloc_plain(&mut heap);
        let stage = alloc_plain(&mut heap);
        let selection = alloc_plain(&mut heap);
        let mut display = Display::default();
        let mut root_obj = Object::new(Kind::Clip(Box::new(clip_for(movie.root.clone(), 0))));
        root_obj.name = Str::from("_level0");
        let root = display.add(root_obj);
        let stage_size = movie.stage_size();
        let mut vm = Vm {
            next_instance_id: 1,
            heap,
            display,
            global,
            protos,
            root,
            native_target: None,
            movie: movie.clone(),
            name: name.to_string(),
            classes: HashMap::new(),
            inits_done: HashSet::new(),
            queue: VecDeque::new(),
            intervals: Vec::new(),
            next_interval: 0,
            time_ms: 0.0,
            timer_time_ms: 0.0,
            host,
            callbacks: HashMap::new(),
            key,
            mouse,
            stage,
            selection,
            stage_size: (stage_size.0 as f64, stage_size.1 as f64),
            mouse_pos: (-1e6, -1e6),
            keys_down: HashSet::new(),
            last_key: 0,
            focus: None,
            log: Vec::new(),
            dirty: true,
            pick_dirty: true,
            budget: 0,
            call_depth: 0,
            fonts,
            warned: HashSet::new(),
            dragging: None,
            rng: 0x9E37_79B9_7F4A_7C15,
            trace_calls: std::env::var_os("GFX_TRACE_CALLS").is_some(),
            fn_names: HashMap::new(),
        };
        builtins::install(&mut vm);
        let frame_ms = if movie.frame_rate > 0.0 {
            1000.0 / movie.frame_rate as f64
        } else {
            1000.0 / 30.0
        };
        let mut p = Self {
            vm,
            frame_ms,
            accum: 0.0,
            hover: None,
            pressed: None,
            text_drag: None,
            queue_init: VecDeque::new(),
            queue_construct: VecDeque::new(),
            queue_default: VecDeque::new(),
            pass_buttons: true,
        };
        // The root's first frame.
        display_api::display_value(&mut p.vm, root);
        enter_frame_of(&mut p.vm, root, 0);
        p.run_queue();
        Ok(p)
    }

    /// Advances time; runs intervals and as many frames as the frame rate says.
    pub fn tick(&mut self, dt_ms: f64) {
        let dt = dt_ms.clamp(0.0, 1000.0);
        self.vm.time_ms += dt;
        self.vm.timer_time_ms += dt;
        if let Some(t) = self
            .vm
            .focus
            .and_then(|d| self.vm.display.get_mut(d))
            .and_then(|o| o.text_mut())
        {
            self.vm.dirty |= t.advance_caret(dt);
        }
        self.run_intervals();
        self.accum += dt;
        let mut n = 0;
        while self.accum >= self.frame_ms && n < 4 {
            self.accum -= self.frame_ms;
            self.run_frame();
            n += 1;
        }
        if self.accum > self.frame_ms {
            self.accum = 0.0;
        }
        self.update_hover();
        self.run_queue();
    }

    fn run_intervals(&mut self) {
        let now = self.vm.timer_time_ms;
        // Catch up in deadline order, preserving each interval's phase. Bounding the
        // callbacks also bounds work when a frame stalls or callbacks create timers.
        for _ in 0..10 {
            let Some(id) = self
                .vm
                .intervals
                .iter()
                .filter(|i| i.next < now)
                .min_by(|a, b| a.next.total_cmp(&b.next).then(a.id.cmp(&b.id)))
                .map(|i| i.id)
            else {
                return;
            };
            let Some(i) = self.vm.intervals.iter_mut().find(|i| i.id == id) else {
                continue;
            };
            let once = i.every < 0.0;
            i.next += i.every.abs();
            let (this, func, method, args) = (
                i.this.clone(),
                i.func.clone(),
                i.method.clone(),
                i.args.clone(),
            );
            if once {
                self.vm.intervals.retain(|x| x.id != id);
            }
            if method.is_some()
                && this.as_object().and_then(|o| self.vm.heap.get(o)).is_some_and(|o| {
                    matches!(&o.kind, crate::avm1::object::Kind::Display(d) if self.vm.display.get(*d).is_none())
                })
            {
                self.vm.intervals.retain(|x| x.id != id);
                continue;
            }
            self.vm.budget = 0;
            match method {
                Some(m) => {
                    self.vm.call_method(&this, &m, &args);
                }
                None => {
                    self.vm.call(&func, Value::Undefined, &args);
                }
            }
            self.run_queue();
        }
        if let Some(next) = self
            .vm
            .intervals
            .iter()
            .map(|i| i.next)
            .min_by(f64::total_cmp)
            && next < now
        {
            // Resume just before the next deadline on the following update. Keep this
            // scheduler clock separate from the movie's actual elapsed time/getTimer.
            self.vm.timer_time_ms = next - 0.1;
        }
    }

    /// Plays one frame: enter-frame handlers, then every playing timeline advances.
    pub fn run_frame(&mut self) {
        let clips = all_clips(&self.vm);
        for c in &clips {
            if self.vm.display.get(*c).is_none() {
                continue;
            }
            let events = clip_events(&self.vm, *c, clip_event::ENTER_FRAME);
            self.vm
                .queue
                .push_back(Queued::Event(*c, "onEnterFrame", events));
        }
        self.run_queue();
        let clips = all_clips(&self.vm);
        for c in clips {
            advance(&mut self.vm, c);
        }
        self.run_queue();
        self.update_hover();
        self.run_queue();
        self.maybe_collect();
    }

    /// Runs queued actions until none are left.
    pub fn run_queue(&mut self) {
        let mut guard = 0;
        loop {
            // Sort newly queued work into the priority queues.
            while let Some(q) = self.vm.queue.pop_front() {
                match q {
                    Queued::Init(..) => self.queue_init.push_back(q),
                    Queued::Construct { .. } => self.queue_construct.push_back(q),
                    _ => self.queue_default.push_back(q),
                }
            }
            let Some(q) = self
                .queue_init
                .pop_front()
                .or_else(|| self.queue_construct.pop_front())
                .or_else(|| self.queue_default.pop_front())
            else {
                break;
            };
            guard += 1;
            if guard > 100_000 {
                self.vm.warn_once("action queue runaway".into());
                self.queue_init.clear();
                self.queue_construct.clear();
                self.queue_default.clear();
                break;
            }
            run_queued(&mut self.vm, q);
        }
    }

    fn maybe_collect(&mut self) {
        if self.vm.heap.allocated_since_gc < 50_000 {
            return;
        }
        let vm = &mut self.vm;
        let mut roots: Vec<ObjId> = vec![
            vm.global,
            vm.key,
            vm.mouse,
            vm.stage,
            vm.selection,
            vm.protos.object,
            vm.protos.function,
            vm.protos.array,
            vm.protos.string,
            vm.protos.number,
            vm.protos.boolean,
            vm.protos.date,
            vm.protos.movie_clip,
            vm.protos.text_field,
            vm.protos.text_format,
        ];
        for id in vm.display.ids().collect::<Vec<_>>() {
            if let Some(s) = vm.display.get(id).and_then(|o| o.script) {
                roots.push(s);
            }
        }
        let mut extra: Vec<Value> = Vec::new();
        for (t, f) in vm.callbacks.values() {
            extra.push(t.clone());
            extra.push(f.clone());
        }
        for i in &vm.intervals {
            extra.push(i.this.clone());
            extra.push(i.func.clone());
            extra.extend(i.args.iter().cloned());
        }
        extra.extend(vm.classes.values().cloned());
        for q in vm
            .queue
            .iter()
            .chain(self.queue_init.iter())
            .chain(self.queue_construct.iter())
            .chain(self.queue_default.iter())
        {
            if let Queued::Construct { ctor: Some(c), .. } = q {
                extra.push(c.clone());
            }
        }
        vm.heap.collect(roots, &extra);
    }

    // ---- host access --------------------------------------------------------------------

    fn global_scope(&self) -> std::rc::Rc<crate::avm1::Scope> {
        let g = std::rc::Rc::new(crate::avm1::Scope {
            object: self.vm.global,
            kind: crate::avm1::ScopeKind::Global,
            parent: None,
        });
        let r = self
            .vm
            .display
            .get(self.vm.root)
            .and_then(|o| o.script)
            .unwrap_or(self.vm.global);
        std::rc::Rc::new(crate::avm1::Scope {
            object: r,
            kind: crate::avm1::ScopeKind::Target,
            parent: Some(g),
        })
    }

    /// The value at a dotted path (`_global.X.y`, `_root.a.b`).
    pub fn get_variable(&mut self, path: &str) -> HostValue {
        let scope = self.global_scope();
        let root = self.vm.root;
        let v = self.vm.resolve_path(path, &scope, root);
        self.vm.to_host(&v, 0)
    }

    pub fn get_value(&mut self, path: &str) -> Value {
        let scope = self.global_scope();
        let root = self.vm.root;
        self.vm.resolve_path(path, &scope, root)
    }

    pub fn set_variable(&mut self, path: &str, v: &HostValue) {
        let scope = self.global_scope();
        let root = self.vm.root;
        let val = self.vm.from_host(v);
        match path.rsplit_once('.') {
            Some((obj, name)) => {
                let o = self.vm.resolve_path(obj, &scope, root);
                self.vm.set(&o, name, val);
            }
            None => {
                let r = self.vm.display_object(root);
                self.vm.set(&r, path, val);
            }
        }
        self.run_queue();
    }

    /// Calls the function at `path` with its object as `this` (Scaleform's `Invoke`).
    pub fn invoke(&mut self, path: &str, args: &[HostValue]) -> HostValue {
        let scope = self.global_scope();
        let root = self.vm.root;
        let args: Vec<Value> = args.iter().map(|a| self.vm.from_host(a)).collect();
        self.vm.budget = 0;
        let r = match path.rsplit_once('.') {
            Some((obj, name)) => {
                let o = self.vm.resolve_path(obj, &scope, root);
                if o.is_nullish() {
                    Value::Undefined
                } else {
                    self.vm.call_method(&o, name, &args)
                }
            }
            None => {
                let f = self.vm.resolve_path(path, &scope, root);
                let this = self.vm.display_object(root);
                self.vm.call(&f, this, &args)
            }
        };
        self.run_queue();
        self.vm.to_host(&r, 0)
    }

    /// Calls a function the movie registered with `ExternalInterface.addCallback`.
    pub fn call_callback(&mut self, name: &str, args: &[HostValue]) -> HostValue {
        let Some((this, f)) = self.vm.callbacks.get(name).cloned() else {
            return HostValue::Undefined;
        };
        let args: Vec<Value> = args.iter().map(|a| self.vm.from_host(a)).collect();
        self.vm.budget = 0;
        let f = match &f {
            Value::String(m) => self.vm.get(&this, m),
            f => f.clone(),
        };
        let r = self.vm.call(&f, this, &args);
        self.run_queue();
        self.vm.to_host(&r, 0)
    }

    pub fn callbacks(&self) -> Vec<String> {
        self.vm.callbacks.keys().cloned().collect()
    }

    pub fn set_stage_size(&mut self, w: f64, h: f64) {
        if self.vm.stage_size != (w, h) {
            self.vm.stage_size = (w, h);
            self.vm.dirty = true;
            let stage = self.vm.stage;
            display_api::broadcast(&mut self.vm, stage, "onResize", &[]);
            self.run_queue();
        }
    }

    pub fn drain_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.vm.log)
    }

    /// Whether anything drawn changed since the last call.
    pub fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.vm.dirty, false)
    }

    // ---- input --------------------------------------------------------------------------

    /// The mouse moved to `(x, y)` in stage pixels.
    pub fn mouse_move(&mut self, x: f64, y: f64) {
        if self.vm.mouse_pos == (x, y) {
            return;
        }
        self.vm.mouse_pos = (x, y);
        if let Some(d) = self.text_drag.filter(|d| self.vm.focus == Some(*d)) {
            self.place_text_caret(d, x, y, true);
        }
        if let Some((d, lock, off)) = self.vm.dragging {
            drag_to(&mut self.vm, d, lock, off);
        }
        let mouse = self.vm.mouse;
        display_api::broadcast(&mut self.vm, mouse, "onMouseMove", &[]);
        self.clip_mouse_event("onMouseMove", clip_event::MOUSE_MOVE, &[]);
        self.vm.pick_dirty = true;
        self.update_hover();
        self.run_queue();
    }

    fn clip_mouse_event(&mut self, method: &'static str, flag: u32, _args: &[Value]) {
        for c in all_clips(&self.vm) {
            let events = clip_events(&self.vm, c, flag);
            self.vm.queue.push_back(Queued::Event(c, method, events));
        }
    }

    fn update_hover(&mut self) {
        if !self.vm.pick_dirty {
            return;
        }
        self.vm.pick_dirty = false;
        let (x, y) = self.vm.mouse_pos;
        let hit = pick(&self.vm, (x * 20.0) as f32, (y * 20.0) as f32);
        if hit == self.hover {
            return;
        }
        let old = self.hover;
        self.hover = hit;
        let pressed = self.pressed.map(|p| p.0);
        if let Some(o) = old {
            let ev = if pressed == Some(o) {
                "onDragOut"
            } else {
                "onRollOut"
            };
            button_event(&mut self.vm, o, ev, &[]);
        }
        if let Some(n) = hit {
            let ev = if pressed == Some(n) {
                "onDragOver"
            } else {
                "onRollOver"
            };
            button_event(&mut self.vm, n, ev, &[]);
        }
    }

    /// The clip `Mouse.getTopMostEntity` reports (any clip, not only buttons).
    pub fn topmost_at(&self, x: f64, y: f64) -> Option<DisplayId> {
        pick_any(&self.vm, self.vm.root, (x * 20.0) as f32, (y * 20.0) as f32)
    }

    /// A mouse button changed. `button` is [`MOUSE_LEFT`], [`MOUSE_RIGHT`] or
    /// [`MOUSE_MIDDLE`]; `double` marks a double click's second press.
    pub fn mouse_button(&mut self, x: f64, y: f64, button: u8, down: bool, double: bool) {
        self.vm.mouse_pos = (x, y);
        self.vm.pick_dirty = true;
        self.update_hover();
        let b = Value::Number((button + if double { 4 } else { 0 }) as f64);
        let press_args: Vec<Value> = if self.pass_buttons {
            vec![b.clone()]
        } else {
            Vec::new()
        };
        let top = self
            .topmost_at(x, y)
            .map(|d| display_api::display_value(&mut self.vm, d))
            .unwrap_or_default();
        let mouse = self.vm.mouse;
        if down {
            display_api::broadcast(&mut self.vm, mouse, "onMouseDown", &[b.clone(), top]);
            self.clip_mouse_event("onMouseDown", clip_event::MOUSE_DOWN, &[]);
            if let Some(h) = self.hover {
                self.pressed = Some((h, button));
                button_event(&mut self.vm, h, "onPress", &press_args);
            }
            // A click outside the focused text field takes its focus away.
            let under = pick_in(
                &self.vm,
                self.vm.root,
                (x * 20.0) as f32,
                (y * 20.0) as f32,
                PickMode::Focus,
            );
            let text = under.filter(|d| {
                self.vm
                    .display
                    .get(*d)
                    .and_then(|o| o.text())
                    .is_some_and(|t| t.input || t.selectable)
            });
            if button == MOUSE_LEFT {
                let extend = self.vm.focus == text && self.vm.keys_down.contains(&16);
                if text.is_some() || self.vm.focus.is_some() {
                    set_focus(&mut self.vm, text);
                }
                self.text_drag = text;
                if let Some(d) = text {
                    self.place_text_caret(d, x, y, extend);
                }
            }
        } else {
            if button == MOUSE_LEFT {
                if let Some(d) = self.text_drag.take().filter(|d| self.vm.focus == Some(*d)) {
                    self.place_text_caret(d, x, y, true);
                }
            }
            // Scaleform releases the button under the mouse before the mouse-up handlers.
            if let Some((p, _)) = self.pressed.take() {
                if self.hover == Some(p) {
                    button_event(&mut self.vm, p, "onRelease", &press_args);
                } else {
                    button_event(&mut self.vm, p, "onReleaseOutside", &press_args);
                }
            }
            self.run_queue();
            display_api::broadcast(&mut self.vm, mouse, "onMouseUp", &[b, top]);
            self.clip_mouse_event("onMouseUp", clip_event::MOUSE_UP, &[]);
        }
        self.run_queue();
        self.vm.pick_dirty = true;
        self.update_hover();
        self.run_queue();
    }

    fn place_text_caret(&mut self, d: DisplayId, x: f64, y: f64, extend: bool) {
        let Some(inv) = self.vm.display.world_matrix(d).invert() else {
            return;
        };
        let (x, y) = inv.apply((x * 20.0) as f32, (y * 20.0) as f32);
        let fonts = self.vm.fonts.clone();
        if let Some(t) = self.vm.display.get_mut(d).and_then(|o| o.text_mut()) {
            let index = t.index_at(&*fonts, x / 20.0, y / 20.0);
            let anchor = if extend { t.selection_anchor } else { index };
            t.set_selection(anchor, index);
            self.vm.dirty = true;
        }
    }

    pub fn mouse_wheel(&mut self, x: f64, y: f64, delta: f64) {
        self.vm.mouse_pos = (x, y);
        let top = self
            .topmost_at(x, y)
            .map(|d| display_api::display_value(&mut self.vm, d))
            .unwrap_or_default();
        let mouse = self.vm.mouse;
        display_api::broadcast(
            &mut self.vm,
            mouse,
            "onMouseWheel",
            &[Value::Number(delta), top],
        );
        self.run_queue();
        self.update_hover();
        self.run_queue();
    }

    /// A key went down (`code` is Flash's key code; `ch` the character it types, if any).
    pub fn key_down(&mut self, code: u32, ch: Option<char>) {
        self.vm.keys_down.insert(code);
        self.vm.last_key = code;
        if let Some(f) = self.vm.focus {
            if text_key(&mut self.vm, f, code, ch) {
                self.run_queue();
                return;
            }
        }
        let key = self.vm.key;
        display_api::broadcast(&mut self.vm, key, "onKeyDown", &[]);
        self.clip_mouse_event("onKeyDown", clip_event::KEY_DOWN, &[]);
        self.run_queue();
    }

    pub fn key_up(&mut self, code: u32) {
        self.vm.keys_down.remove(&code);
        self.vm.last_key = code;
        let key = self.vm.key;
        display_api::broadcast(&mut self.vm, key, "onKeyUp", &[]);
        self.clip_mouse_event("onKeyUp", clip_event::KEY_UP, &[]);
        self.run_queue();
    }

    /// Selects all text in the focused editable or selectable field.
    pub fn select_all(&mut self) {
        if let Some(t) = self
            .vm
            .focus
            .and_then(|f| self.vm.display.get_mut(f))
            .and_then(|o| o.text_mut())
            && (t.input || t.selectable)
        {
            t.set_selection(0, t.len());
            self.vm.dirty = true;
        }
    }

    /// Types `text` into the focused text field.
    pub fn type_text(&mut self, text: &str) -> bool {
        let Some(f) = self.vm.focus else {
            return false;
        };
        let mut any = false;
        for c in text.chars() {
            any |= text_key(&mut self.vm, f, 0, Some(c));
        }
        self.run_queue();
        any
    }
}

fn run_queued(vm: &mut Vm, q: Queued) {
    vm.budget = 0;
    match q {
        Queued::Frame(c, code) | Queued::Init(c, code) => {
            interp::run_actions(vm, &code, c);
        }
        Queued::Construct { clip, ctor, events } => {
            if vm.display.get(clip).is_none() {
                return;
            }
            for e in events {
                interp::run_actions(vm, &e, clip);
            }
            if let Some(ctor) = ctor {
                let obj = display_api::display_value(vm, clip);
                let proto = vm.get(&ctor, "prototype").as_object();
                if let Some(o) = obj.as_object() {
                    vm.define(
                        o,
                        "__constructor__",
                        ctor.clone(),
                        crate::avm1::object::DONT_ENUM,
                    );
                }
                vm.call_with_base(&ctor, obj, &[], proto);
            }
        }
        Queued::Event(c, method, events) => {
            if vm.display.get(c).is_none() {
                return;
            }
            for e in events {
                interp::run_actions(vm, &e, c);
            }
            if method == "onLoad" {
                if let Some(cl) = vm.display.get_mut(c).and_then(|o| o.clip_mut()) {
                    cl.loaded = true;
                }
            }
            let obj = display_api::display_value(vm, c);
            let f = vm.get(&obj, method);
            if matches!(&f, Value::Object(o) if vm.is_function(*o)) {
                vm.call(&f, obj, &[]);
            }
        }
    }
}

/// Every clip, parents before children, lower depths first.
fn all_clips(vm: &Vm) -> Vec<DisplayId> {
    let mut out = Vec::new();
    let mut stack = vec![vm.root];
    while let Some(d) = stack.pop() {
        let Some(o) = vm.display.get(d) else { continue };
        if o.clip().is_some() {
            out.push(d);
        }
        for c in o.children.values().rev() {
            stack.push(*c);
        }
    }
    out
}

fn clip_events(vm: &Vm, d: DisplayId, flag: u32) -> Vec<crate::swf::Actions> {
    vm.display
        .get(d)
        .and_then(|o| o.clip())
        .map(|c| {
            c.clip_actions
                .iter()
                .filter(|a| a.events & flag != 0)
                .map(|a| a.actions.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Runs a button-style handler (`onPress`, ...) on clip `d`.
fn button_event(vm: &mut Vm, d: DisplayId, method: &'static str, args: &[Value]) {
    let flag = match method {
        "onPress" => clip_event::PRESS,
        "onRelease" => clip_event::RELEASE,
        "onReleaseOutside" => clip_event::RELEASE_OUTSIDE,
        "onRollOver" => clip_event::ROLL_OVER,
        "onRollOut" => clip_event::ROLL_OUT,
        "onDragOver" => clip_event::DRAG_OVER,
        "onDragOut" => clip_event::DRAG_OUT,
        _ => 0,
    };
    for e in clip_events(vm, d, flag) {
        interp::run_actions(vm, &e, d);
    }
    let obj = display_api::display_value(vm, d);
    let f = vm.get(&obj, method);
    if matches!(&f, Value::Object(o) if vm.is_function(*o)) {
        vm.budget = 0;
        vm.call(&f, obj, args);
    }
}

// ---- timelines ---------------------------------------------------------------------------

/// Runs frame `frame` of clip `d` for the first time after reaching it: init actions,
/// display commands, then its actions are queued.
fn enter_frame_of(vm: &mut Vm, d: DisplayId, frame: u16) {
    let Some(tl) = vm
        .display
        .get(d)
        .and_then(|o| o.clip())
        .map(|c| c.timeline.clone())
    else {
        return;
    };
    let Some(f) = tl.frames.get(frame as usize) else {
        return;
    };
    for (sprite, code) in &f.init_actions {
        // Init actions run at once, before the frame's clips are made: they register the
        // classes those clips are constructed with.
        if vm.inits_done.insert(*sprite) {
            interp::run_actions(vm, code, d);
        }
    }
    if let Some(c) = vm.display.get_mut(d).and_then(|o| o.clip_mut()) {
        c.frame = Some(frame);
    }
    for cmd in &f.commands {
        apply_command(vm, d, cmd);
    }
    for a in &f.actions {
        vm.queue.push_back(Queued::Frame(d, a.clone()));
    }
    vm.dirty = true;
    vm.pick_dirty = true;
}

fn advance(vm: &mut Vm, d: DisplayId) {
    let Some(c) = vm.display.get(d).and_then(|o| o.clip()) else {
        return;
    };
    if !c.playing || c.loaded_image.is_some() {
        return;
    }
    let total = c.timeline.frames.len() as u16;
    let cur = c.frame.unwrap_or(0);
    if total <= 1 {
        return;
    }
    let next = cur + 1;
    if next >= total {
        goto(vm, d, 0, true);
    } else {
        enter_frame_of(vm, d, next);
    }
}

fn apply_command(vm: &mut Vm, d: DisplayId, cmd: &Command) {
    match cmd {
        Command::Remove { depth } => {
            let dd = *depth as i32 + DEPTH_OFFSET;
            if let Some(c) = vm.display.get(d).and_then(|o| o.children.get(&dd).copied()) {
                remove_clip(vm, c);
            }
        }
        Command::Place(p) => place(vm, d, p),
    }
}

fn place(vm: &mut Vm, parent: DisplayId, p: &Place) {
    let depth = p.depth as i32 + DEPTH_OFFSET;
    let existing = vm
        .display
        .get(parent)
        .and_then(|o| o.children.get(&depth).copied());
    if p.is_move {
        let Some(e) = existing else { return };
        // A move with a new character swaps a shape's outline in place.
        if let Some(id) = p.character {
            let ch = vm.movie.characters.get(&id).cloned();
            if let (Some(o), Some(ch)) = (vm.display.get_mut(e), ch) {
                match (&mut o.kind, ch) {
                    (Kind::Shape(s, cid), Character::Shape(n)) => {
                        *s = n;
                        *cid = id;
                    }
                    (Kind::Morph(s, cid), Character::Morph(n)) => {
                        *s = n;
                        *cid = id;
                    }
                    _ => {}
                }
            }
        }
        apply_place_props(vm, e, p);
        vm.dirty = true;
        return;
    }
    let Some(id) = p.character else { return };
    if let Some(e) = existing {
        // Flash leaves an occupied depth alone unless the object there is a different one.
        let same = vm.display.get(e).is_some_and(|o| match &o.kind {
            Kind::Clip(c) => c.character == id,
            Kind::Shape(_, c) | Kind::Morph(_, c) => *c == id,
            _ => false,
        });
        if same {
            apply_place_props(vm, e, p);
            return;
        }
        remove_clip(vm, e);
    }
    let Some(mut obj) = instantiate(vm, id) else {
        return;
    };
    obj.name = match &p.name {
        Some(n) => Str::from(n.as_str()),
        None if matches!(obj.kind, Kind::Clip(_) | Kind::Text(_)) => {
            let name = Str::from(format!("instance{}", vm.next_instance_id));
            vm.next_instance_id = vm.next_instance_id.wrapping_add(1);
            name
        }
        None => Str::from(""),
    };
    let new = vm.display.add(obj);
    insert_child(vm, parent, new, depth);
    if let Some(o) = vm.display.get_mut(new) {
        o.from_timeline = true;
    }
    apply_place_props(vm, new, p);
    if let Some(c) = vm.display.get_mut(new).and_then(|o| o.clip_mut()) {
        c.clip_actions = p.clip_actions.clone();
    }
    constructed(vm, new, false, None);
}

fn apply_place_props(vm: &mut Vm, d: DisplayId, p: &Place) {
    let Some(o) = vm.display.get_mut(d) else {
        return;
    };
    if let Some(m) = p.matrix {
        o.matrix = m;
        o.scale_rotation = None;
    }
    if let Some(c) = p.color {
        o.color = c;
    }
    if let Some(r) = p.ratio {
        o.ratio = r;
    }
    if let Some(cd) = p.clip_depth {
        o.clip_depth = Some(cd as i32 + DEPTH_OFFSET);
    }
    if let Some(b) = p.blend {
        o.blend = b;
    }
    if let Some(f) = &p.filters {
        o.filters = f.clone();
    }
    if let Some(v) = p.visible {
        o.visible = v;
    }
    vm.dirty = true;
    vm.pick_dirty = true;
}

/// Makes a display object for character `id` (not yet in the list).
fn instantiate(vm: &mut Vm, id: u16) -> Option<Object> {
    let ch = vm.movie.characters.get(&id)?.clone();
    Some(Object::new(match ch {
        Character::Shape(s) => Kind::Shape(s, id),
        Character::Morph(m) => Kind::Morph(m, id),
        Character::Sprite(t) => Kind::Clip(Box::new(clip_for(t, id))),
        Character::EditText(e) => {
            let font = e.font.and_then(|f| font_name(vm, f));
            Kind::Text(Box::new(TextField::new(e, font)))
        }
        Character::Font(_) | Character::Image(_) => return None,
    }))
}

/// The name of the font character `id` (its own, or the one imported under that id).
fn font_name(vm: &Vm, id: u16) -> Option<String> {
    if let Some(Character::Font(f)) = vm.movie.characters.get(&id) {
        return Some(f.name.clone());
    }
    for imp in &vm.movie.imports {
        for (cid, name) in &imp.assets {
            if *cid == id {
                // `$BodyFont` / `$TitleFont`: the font library maps them to its fonts.
                return Some(name.clone());
            }
        }
    }
    None
}

fn insert_child(vm: &mut Vm, parent: DisplayId, child: DisplayId, depth: i32) {
    if let Some(o) = vm.display.get_mut(child) {
        o.parent = Some(parent);
        o.depth = depth;
    }
    if let Some(p) = vm.display.get_mut(parent) {
        p.children.insert(depth, child);
    }
    vm.dirty = true;
    vm.pick_dirty = true;
}

/// After a clip is in the list: its script object, its first frame, construction and load.
fn constructed(vm: &mut Vm, d: DisplayId, by_script: bool, init: Option<ObjId>) {
    let Some(o) = vm.display.get(d) else { return };
    match &o.kind {
        Kind::Text(t) => {
            // A field bound to a variable shows it.
            if !t.variable.is_empty() {
                let var = t.variable.clone();
                if let Some(p) = o.parent {
                    let pv = display_api::display_value(vm, p);
                    let v = vm.get(&pv, &var);
                    if !v.is_undefined() {
                        let s = vm.to_str(&v);
                        if let Some(t) = vm.display.get_mut(d).and_then(|o| o.text_mut()) {
                            if t.html {
                                t.set_html(&s);
                            } else {
                                t.set_text(&s);
                            }
                        }
                    }
                }
            }
            display_api::apply_autosize(vm, d);
            return;
        }
        Kind::Clip(_) => {}
        _ => return,
    }
    let character = o.clip().map(|c| c.character).unwrap_or(0);
    let ctor = vm.classes.get(&character).cloned();
    display_api::display_value(vm, d);
    if !by_script {
        // Reserve the parent's construction work before frame one recursively places
        // its children. Registered classes may keep the last instance they construct
        // to associate post-load callbacks with the final descendant.
        for e in clip_events(vm, d, clip_event::INITIALIZE) {
            vm.queue.push_back(Queued::Init(d, e));
        }
        vm.queue.push_back(Queued::Construct {
            clip: d,
            ctor: ctor.clone(),
            events: clip_events(vm, d, clip_event::CONSTRUCT),
        });
    }
    // The clip's load event is queued before its first frame makes its children, so a
    // parent hears `onLoad` before the clips inside it (as Ruffle and Scaleform order it).
    let load = clip_events(vm, d, clip_event::LOAD);
    vm.queue.push_back(Queued::Event(d, "onLoad", load));
    enter_frame_of(vm, d, 0);
    if let Some(init) = init {
        let keys = vm.keys(&Value::Object(init));
        let obj = display_api::display_value(vm, d);
        for k in keys.iter().rev() {
            let v = vm.get(&Value::Object(init), k);
            vm.set(&obj, k, v);
        }
    }
    if by_script {
        if let Some(ctor) = ctor {
            let obj = display_api::display_value(vm, d);
            let proto = vm.get(&ctor, "prototype").as_object();
            if let Some(o) = obj.as_object() {
                vm.define(
                    o,
                    "__constructor__",
                    ctor.clone(),
                    crate::avm1::object::DONT_ENUM,
                );
            }
            vm.call_with_base(&ctor, obj, &[], proto);
        }
    }
}

/// Shows frame `frame` (0-based) of clip `d`, rebuilding its timeline children as Flash does
/// for a jump, and plays or stops it.
pub fn goto(vm: &mut Vm, d: DisplayId, frame: u16, play: bool) {
    let Some(c) = vm.display.get(d).and_then(|o| o.clip()) else {
        return;
    };
    let total = c.timeline.frames.len() as u16;
    let frame = frame.min(total.saturating_sub(1));
    let cur = c.frame;
    let tl = c.timeline.clone();
    if let Some(c) = vm.display.get_mut(d).and_then(|o| o.clip_mut()) {
        c.playing = play;
    }
    if cur == Some(frame) {
        return;
    }
    let forward_from = match cur {
        Some(cur) if frame > cur => cur + 1,
        _ => 0,
    };
    // Replay the commands from `forward_from` to `frame` into one final state per depth.
    let mut state: HashMap<i32, Option<Place>> = HashMap::new();
    if forward_from == 0 {
        // A rewind: every timeline child not in the target state goes.
        if let Some(o) = vm.display.get(d) {
            for (depth, c) in &o.children {
                if *depth < 0 && vm.display.get(*c).is_some_and(|x| x.from_timeline) {
                    state.insert(*depth, None);
                }
            }
        }
    }
    let mut replaced: HashSet<i32> = HashSet::new();
    for fi in forward_from..=frame {
        let Some(f) = tl.frames.get(fi as usize) else {
            break;
        };
        for (sprite, code) in &f.init_actions {
            if vm.inits_done.insert(*sprite) {
                interp::run_actions(vm, code, d);
            }
        }
        for cmd in &f.commands {
            match cmd {
                Command::Remove { depth } => {
                    state.insert(*depth as i32 + DEPTH_OFFSET, None);
                }
                Command::Place(p) => {
                    let depth = p.depth as i32 + DEPTH_OFFSET;
                    let entry = state.entry(depth).or_insert(None);
                    match (p.is_move, entry.as_mut()) {
                        (true, Some(prev)) => merge_place(prev, p),
                        (true, None) => {
                            // A move of an object placed before the replay began.
                            let mut q = (**p).clone();
                            if q.character.is_none() {
                                q.is_move = true;
                            }
                            *entry = Some(q);
                        }
                        (false, _) => {
                            *entry = Some((**p).clone());
                            replaced.insert(depth);
                        }
                    }
                }
            }
        }
    }
    if let Some(c) = vm.display.get_mut(d).and_then(|o| o.clip_mut()) {
        c.frame = Some(frame);
    }
    let mut depths: Vec<i32> = state.keys().copied().collect();
    depths.sort();
    for depth in depths {
        let existing = vm
            .display
            .get(d)
            .and_then(|o| o.children.get(&depth).copied());
        match state.remove(&depth).flatten() {
            None => {
                if let Some(e) = existing {
                    if vm
                        .display
                        .get(e)
                        .is_some_and(|o| o.from_timeline || depth < 0)
                    {
                        remove_clip(vm, e);
                    }
                }
            }
            Some(p) => {
                let same_char = existing.is_some_and(|e| {
                    vm.display.get(e).is_some_and(|o| match &o.kind {
                        Kind::Clip(c) => Some(c.character) == p.character,
                        Kind::Shape(_, c) | Kind::Morph(_, c) => Some(*c) == p.character,
                        Kind::Text(t) => p.character.and_then(|id| vm.movie.characters.get(&id)).is_some_and(|c| matches!(c, Character::EditText(def) if Arc::ptr_eq(&t.def, def))),
                        _ => false,
                    })
                });
                if let Some(e) = existing {
                    if p.is_move || (same_char && !replaced.contains(&depth)) || same_char {
                        let mut q = p.clone();
                        q.is_move = true;
                        place(vm, d, &q);
                        continue;
                    }
                    remove_clip(vm, e);
                }
                if !p.is_move {
                    place(vm, d, &p);
                }
            }
        }
    }
    if let Some(f) = tl.frames.get(frame as usize) {
        for a in &f.actions {
            vm.queue.push_back(Queued::Frame(d, a.clone()));
        }
    }
    vm.dirty = true;
    vm.pick_dirty = true;
}

fn merge_place(prev: &mut Place, p: &Place) {
    if let Some(c) = p.character {
        prev.character = Some(c);
    }
    if p.matrix.is_some() {
        prev.matrix = p.matrix;
    }
    if p.color.is_some() {
        prev.color = p.color;
    }
    if p.ratio.is_some() {
        prev.ratio = p.ratio;
    }
    if p.blend.is_some() {
        prev.blend = p.blend;
    }
    if p.filters.is_some() {
        prev.filters = p.filters.clone();
    }
    if p.visible.is_some() {
        prev.visible = p.visible;
    }
    if p.clip_depth.is_some() {
        prev.clip_depth = p.clip_depth;
    }
}

pub fn goto_label(vm: &mut Vm, d: DisplayId, label: &str, play: bool) {
    let Some(f) = vm
        .display
        .get(d)
        .and_then(|o| o.clip())
        .and_then(|c| c.timeline.label(label))
    else {
        vm.warn_once(format!("no frame label {label}"));
        return;
    };
    goto(vm, d, f, play);
}

pub fn clip_step(vm: &mut Vm, d: DisplayId, by: i32) {
    let Some(c) = vm.display.get(d).and_then(|o| o.clip()) else {
        return;
    };
    let cur = c.frame.unwrap_or(0) as i32;
    let total = c.timeline.frames.len() as i32;
    let to = (cur + by).clamp(0, total - 1);
    goto(vm, d, to as u16, false);
}

pub fn set_playing(vm: &mut Vm, d: DisplayId, playing: bool) {
    if let Some(c) = vm.display.get_mut(d).and_then(|o| o.clip_mut()) {
        c.playing = playing;
    }
}

/// Takes `d` (and everything under it) out of the display list, running `onUnload`.
pub fn remove_clip(vm: &mut Vm, d: DisplayId) {
    let Some(o) = vm.display.get(d) else { return };
    let parent = o.parent;
    let depth = o.depth;
    // Unload handlers run while the clip is still reachable.
    let mut stack = vec![d];
    let mut clips = Vec::new();
    while let Some(x) = stack.pop() {
        if let Some(o) = vm.display.get(x) {
            if o.clip().is_some() {
                clips.push(x);
            }
            stack.extend(o.children.values().copied());
        }
    }
    for c in clips {
        let has = vm.display.get(c).and_then(|o| o.script).is_some();
        for e in clip_events(vm, c, clip_event::UNLOAD) {
            interp::run_actions(vm, &e, c);
        }
        if has {
            let obj = display_api::display_value(vm, c);
            let f = vm.get(&obj, "onUnload");
            if matches!(&f, Value::Object(x) if vm.is_function(*x)) {
                vm.call(&f, obj, &[]);
            }
        }
    }
    if let Some(p) = parent.and_then(|p| vm.display.get_mut(p)) {
        if p.children.get(&depth) == Some(&d) {
            p.children.remove(&depth);
        }
    }
    let gone = vm.display.free_tree(d);
    if vm.focus.is_some_and(|f| gone.contains(&f)) {
        vm.focus = None;
    }
    if vm.dragging.is_some_and(|(f, _, _)| gone.contains(&f)) {
        vm.dragging = None;
    }
    // Script objects of removed clips no longer resolve to them.
    vm.dirty = true;
    vm.pick_dirty = true;
}

/// `attachMovie`: a new instance of the exported symbol `export` in `parent` at `depth`.
pub fn attach(
    vm: &mut Vm,
    parent: DisplayId,
    export: &str,
    name: &str,
    depth: i32,
    init: Option<ObjId>,
) -> Option<DisplayId> {
    let id = *vm.movie.exports.get(&export.to_ascii_lowercase())?;
    let obj = instantiate(vm, id)?;
    replace_at(vm, parent, depth);
    let new = vm.display.add(obj);
    insert_child(vm, parent, new, depth);
    if let Some(o) = vm.display.get_mut(new) {
        o.name = Str::from(name);
    }
    constructed(vm, new, true, init);
    Some(new)
}

fn replace_at(vm: &mut Vm, parent: DisplayId, depth: i32) {
    if let Some(e) = vm
        .display
        .get(parent)
        .and_then(|o| o.children.get(&depth).copied())
    {
        remove_clip(vm, e);
    }
}

/// Duplicates a clip's authored symbol and drawing, then constructs a fresh script object.
/// Dynamic properties belong to the original and are not copied.
pub fn duplicate(
    vm: &mut Vm,
    source: DisplayId,
    name: &str,
    depth: i32,
    init: Option<ObjId>,
) -> Option<DisplayId> {
    // Flash's allowed depths, expressed in the same unshifted units as getDepth.
    if !(DEPTH_OFFSET..=2_130_690_044).contains(&depth) {
        return None;
    }
    let source = vm.display.get(source)?;
    let parent = source.parent?;
    let clip = source.clip()?;
    let (character, matrix, color, drawing, actions) = (
        clip.character,
        source.matrix,
        source.color,
        clip.drawing.clone(),
        clip.clip_actions.clone(),
    );
    let mut object = if character == 0 {
        Object::new(Kind::Clip(Box::new(clip_for(
            Arc::new(crate::swf::Timeline {
                frames: vec![Default::default()],
                labels: vec![],
            }),
            0,
        ))))
    } else {
        instantiate(vm, character)?
    };
    object.name = Str::from(name);
    object.matrix = matrix;
    object.color = color;
    let clip = object.clip_mut()?;
    clip.drawing = drawing;
    clip.clip_actions = actions;
    replace_at(vm, parent, depth);
    let new = vm.display.add(object);
    insert_child(vm, parent, new, depth);
    constructed(vm, new, true, init);
    Some(new)
}

pub fn create_empty(vm: &mut Vm, parent: DisplayId, name: &str, depth: i32) -> Option<DisplayId> {
    replace_at(vm, parent, depth);
    let tl = Arc::new(crate::swf::Timeline {
        frames: vec![Default::default()],
        labels: Vec::new(),
    });
    let mut obj = Object::new(Kind::Clip(Box::new(clip_for(tl, 0))));
    obj.name = Str::from(name);
    let new = vm.display.add(obj);
    insert_child(vm, parent, new, depth);
    constructed(vm, new, true, None);
    Some(new)
}

pub fn create_text(
    vm: &mut Vm,
    parent: DisplayId,
    name: &str,
    depth: i32,
    rect: [f64; 4],
) -> Option<DisplayId> {
    replace_at(vm, parent, depth);
    let def = crate::swf::EditText {
        bounds: crate::swf::Rect {
            x_min: 0,
            y_min: 0,
            x_max: (rect[2] * 20.0) as i32,
            y_max: (rect[3] * 20.0) as i32,
        },
        read_only: true,
        height: 240,
        ..Default::default()
    };
    let mut obj = Object::new(Kind::Text(Box::new(TextField::new(Arc::new(def), None))));
    obj.name = Str::from(name);
    obj.matrix = crate::swf::Matrix::translate((rect[0] * 20.0) as f32, (rect[1] * 20.0) as f32);
    let new = vm.display.add(obj);
    insert_child(vm, parent, new, depth);
    Some(new)
}

pub fn swap_depths(vm: &mut Vm, d: DisplayId, with: Result<DisplayId, i32>) {
    let Some(o) = vm.display.get(d) else { return };
    let Some(parent) = o.parent else { return };
    let my_depth = o.depth;
    let other_depth = match with {
        Ok(other) => {
            let Some(oo) = vm.display.get(other) else {
                return;
            };
            if oo.parent != Some(parent) {
                return;
            }
            oo.depth
        }
        Err(dep) => dep,
    };
    if other_depth == my_depth {
        return;
    }
    let other = vm
        .display
        .get(parent)
        .and_then(|p| p.children.get(&other_depth).copied());
    if let Some(p) = vm.display.get_mut(parent) {
        p.children.remove(&my_depth);
        if let Some(other) = other {
            p.children.insert(my_depth, other);
        }
        p.children.insert(other_depth, d);
    }
    if let Some(o) = vm.display.get_mut(d) {
        o.depth = other_depth;
        o.from_timeline = false;
    }
    if let Some(other) = other.and_then(|x| vm.display.get_mut(x)) {
        other.depth = my_depth;
    }
    vm.dirty = true;
    vm.pick_dirty = true;
}

/// Loads an image (or nothing) into clip `d`, replacing its content. Images only: the movies
/// load `img://` engine images and atlas images this way.
pub fn load_into(vm: &mut Vm, d: DisplayId, url: &str) -> bool {
    unload(vm, d);
    let size = vm.host.image_size(url);
    let Some((w, h)) = size else {
        vm.warn_once(format!("image {url} not found"));
        return false;
    };
    let bmp = Object::new(Kind::Bitmap {
        url: url.to_string(),
        width: w as f32,
        height: h as f32,
    });
    let b = vm.display.add(bmp);
    insert_child(vm, d, b, 0);
    if let Some(c) = vm.display.get_mut(d).and_then(|o| o.clip_mut()) {
        c.loaded_image = Some(url.to_string());
    }
    true
}

pub fn attach_bitmap(
    vm: &mut Vm,
    parent: DisplayId,
    depth: i32,
    url: String,
    width: u16,
    height: u16,
) {
    replace_at(vm, parent, depth);
    let bitmap = vm.display.add(Object::new(Kind::Bitmap {
        url,
        width: width as f32,
        height: height as f32,
    }));
    insert_child(vm, parent, bitmap, depth);
}

pub fn unload(vm: &mut Vm, d: DisplayId) {
    let kids: Vec<DisplayId> = vm
        .display
        .get(d)
        .map(|o| o.children.values().copied().collect())
        .unwrap_or_default();
    for k in kids {
        remove_clip(vm, k);
    }
    if let Some(c) = vm.display.get_mut(d).and_then(|o| o.clip_mut()) {
        c.loaded_image = None;
        c.drawing = None;
    }
}

pub fn start_drag(vm: &mut Vm, d: DisplayId, lock: bool) {
    let Some(o) = vm.display.get(d) else { return };
    let parent_m = o
        .parent
        .map(|p| vm.display.world_matrix(p))
        .unwrap_or_default();
    let (mx, my) = parent_m
        .invert()
        .map(|i| {
            i.apply(
                (vm.mouse_pos.0 * 20.0) as f32,
                (vm.mouse_pos.1 * 20.0) as f32,
            )
        })
        .unwrap_or((0.0, 0.0));
    let off = ((o.matrix.tx - mx) as f64, (o.matrix.ty - my) as f64);
    vm.dragging = Some((d, lock, off));
}

fn drag_to(vm: &mut Vm, d: DisplayId, lock: bool, off: (f64, f64)) {
    let Some(o) = vm.display.get(d) else { return };
    let parent_m = o
        .parent
        .map(|p| vm.display.world_matrix(p))
        .unwrap_or_default();
    let Some(inv) = parent_m.invert() else { return };
    let (mx, my) = inv.apply(
        (vm.mouse_pos.0 * 20.0) as f32,
        (vm.mouse_pos.1 * 20.0) as f32,
    );
    if let Some(o) = vm.display.get_mut(d) {
        if lock {
            o.matrix.tx = mx;
            o.matrix.ty = my;
        } else {
            o.matrix.tx = mx + off.0 as f32;
            o.matrix.ty = my + off.1 as f32;
        }
    }
    vm.dirty = true;
}

pub fn set_focus(vm: &mut Vm, d: Option<DisplayId>) {
    let old = vm.focus;
    if old == d {
        return;
    }
    vm.focus = d;
    if let Some(t) = old
        .and_then(|o| vm.display.get_mut(o))
        .and_then(|o| o.text_mut())
    {
        t.clear_selection();
    }
    if let Some(t) = d
        .and_then(|o| vm.display.get_mut(o))
        .and_then(|o| o.text_mut())
    {
        t.set_selection(t.len(), t.len());
    }
    let ov = old
        .map(|o| display_api::display_value(vm, o))
        .unwrap_or(Value::Null);
    let nv = d
        .map(|o| display_api::display_value(vm, o))
        .unwrap_or(Value::Null);
    if let Some(o) = old {
        let obj = display_api::display_value(vm, o);
        vm.call_method_if(&obj, "onKillFocus", std::slice::from_ref(&nv));
    }
    if let Some(n) = d {
        let obj = display_api::display_value(vm, n);
        vm.call_method_if(&obj, "onSetFocus", std::slice::from_ref(&ov));
    }
    let sel = vm.selection;
    display_api::broadcast(vm, sel, "onSetFocus", &[ov, nv]);
    vm.dirty = true;
}

impl Vm {
    /// Calls `obj.name(args)` when it is a function.
    pub fn call_method_if(&mut self, obj: &Value, name: &str, args: &[Value]) -> Option<Value> {
        let f = self.get(obj, name);
        if matches!(&f, Value::Object(o) if self.is_function(*o)) {
            Some(self.call(&f, obj.clone(), args))
        } else {
            None
        }
    }
}

/// Edits the focused input field; true when the key was the field's.
fn text_key(vm: &mut Vm, f: DisplayId, code: u32, ch: Option<char>) -> bool {
    let Some(t) = vm.display.get_mut(f).and_then(|o| o.text_mut()) else {
        return false;
    };
    if !t.input {
        return false;
    }
    let mut text: Vec<char> = t.text().chars().collect();
    let mut caret = t.caret.unwrap_or(text.len()).min(text.len());
    let (begin, end) = t.selection();
    let shift = vm.keys_down.contains(&16);
    let selecting_all = code == 65 && vm.keys_down.contains(&17);
    let mut anchor = t.selection_anchor.min(text.len());
    let mut changed = false;
    match (code, ch) {
        (65, _) if selecting_all => {
            anchor = 0;
            caret = text.len();
        }
        (8, _) => {
            if begin != end {
                text.drain(begin..end);
                caret = begin;
                changed = true;
            } else if caret > 0 {
                text.remove(caret - 1);
                caret -= 1;
                changed = true;
            }
        }
        (46, _) => {
            if begin != end {
                text.drain(begin..end);
                caret = begin;
                changed = true;
            } else if caret < text.len() {
                text.remove(caret);
                changed = true;
            }
        }
        (37, _) => {
            caret = if !shift && begin != end {
                begin
            } else {
                caret.saturating_sub(1)
            }
        }
        (39, _) => {
            caret = if !shift && begin != end {
                end
            } else {
                (caret + 1).min(text.len())
            }
        }
        (36, _) => caret = 0,
        (35, _) => caret = text.len(),
        (13, _) if t.multiline => {
            text.drain(begin..end);
            caret = begin;
            text.insert(caret, '\r');
            caret += 1;
            changed = true;
        }
        (_, Some(c)) if !c.is_control() => {
            let allowed = t.restrict.as_deref().is_none_or(|r| restrict_allows(r, c));
            if allowed && (t.max_chars == 0 || ((text.len() - (end - begin)) as u32) < t.max_chars)
            {
                text.drain(begin..end);
                caret = begin;
                text.insert(caret, c);
                caret += 1;
                changed = true;
            } else {
                return true;
            }
        }
        _ => return false,
    }
    let s: String = text.iter().collect();
    if changed {
        t.set_text(&s);
    }
    if changed || (!shift && !selecting_all) {
        anchor = caret;
    }
    t.set_selection(anchor, caret);
    let var = t.variable.clone();
    vm.dirty = true;
    if changed {
        let obj = display_api::display_value(vm, f);
        vm.call_method_if(&obj, "onChanged", std::slice::from_ref(&obj));
        if !var.is_empty() {
            if let Some(p) = vm.display.get(f).and_then(|o| o.parent) {
                let pv = display_api::display_value(vm, p);
                if let Some(po) = pv.as_object() {
                    vm.define(po, &var, Value::from(s), 0);
                }
            }
        }
    }
    true
}

/// Flash's `restrict` syntax: ranges, `^` to exclude, `\` to escape.
pub fn restrict_allows(r: &str, c: char) -> bool {
    let chars: Vec<char> = r.chars().collect();
    let mut allowed = false;
    let mut any_include = false;
    let mut include = true;
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        let mut a = chars[i];
        if a == '^' {
            include = !include;
            if first {
                allowed = true;
            }
            i += 1;
            first = false;
            continue;
        }
        first = false;
        if a == '\\' && i + 1 < chars.len() {
            i += 1;
            a = chars[i];
        }
        let mut b = a;
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            b = chars[i + 2];
            i += 2;
        }
        if include {
            any_include = true;
        }
        if c >= a && c <= b {
            allowed = include;
        }
        i += 1;
    }
    if !any_include && !allowed {
        return true;
    }
    allowed
}

// ---- picking -----------------------------------------------------------------------------

fn takes_mouse(vm: &Vm, d: DisplayId) -> bool {
    let Some(o) = vm.display.get(d) else {
        return false;
    };
    let Some(c) = o.clip() else { return false };
    let flags = clip_event::PRESS
        | clip_event::RELEASE
        | clip_event::RELEASE_OUTSIDE
        | clip_event::ROLL_OVER
        | clip_event::ROLL_OUT
        | clip_event::DRAG_OVER
        | clip_event::DRAG_OUT;
    if c.clip_actions.iter().any(|a| a.events & flags != 0) {
        return true;
    }
    let Some(s) = o.script else { return false };
    let v = Value::Object(s);
    for h in [
        "onPress",
        "onRelease",
        "onReleaseOutside",
        "onRollOver",
        "onRollOut",
        "onDragOver",
        "onDragOut",
    ] {
        if let Some((_, p)) = lookup_prop(vm, s, h) {
            if matches!(&p, Value::Object(f) if vm.is_function(*f)) {
                let _ = &v;
                return true;
            }
        }
    }
    false
}

fn lookup_prop(vm: &Vm, o: ObjId, name: &str) -> Option<(ObjId, Value)> {
    let mut cur = Some(o);
    let mut guard = 0;
    while let Some(c) = cur {
        let obj = vm.heap.get(c)?;
        if let Some(p) = obj.props.get(name) {
            return Some((c, p.value.clone()));
        }
        cur = obj.proto;
        guard += 1;
        if guard > 64 {
            break;
        }
    }
    None
}

fn enabled(vm: &Vm, d: DisplayId) -> bool {
    let Some(s) = vm.display.get(d).and_then(|o| o.script) else {
        return true;
    };
    !matches!(lookup_prop(vm, s, "enabled"), Some((_, Value::Bool(false))))
}

/// The topmost clip taking button events under the point (twips, stage space).
fn pick(vm: &Vm, x: f32, y: f32) -> Option<DisplayId> {
    pick_in(vm, vm.root, x, y, PickMode::Button)
}

fn pick_any(vm: &Vm, d: DisplayId, x: f32, y: f32) -> Option<DisplayId> {
    pick_in(vm, d, x, y, PickMode::Any)
}

#[derive(Clone, Copy)]
enum PickMode {
    Button,
    Any,
    Focus,
}

fn pick_in(vm: &Vm, d: DisplayId, x: f32, y: f32, mode: PickMode) -> Option<DisplayId> {
    let o = vm.display.get(d)?;
    if !o.visible
        || o.hit_test_disable
        || o.mask_for.is_some()
        || o.mask.is_some_and(|m| !hit_shape_impl(vm, m, x, y, true))
    {
        return None;
    }
    // Children, topmost first; masks hide what is outside them.
    let kids: Vec<(i32, DisplayId)> = o.children.iter().map(|(k, v)| (*k, *v)).collect();
    let masks: Vec<(i32, i32, DisplayId)> = kids
        .iter()
        .filter_map(|(depth, c)| {
            vm.display
                .get(*c)
                .and_then(|co| co.clip_depth)
                .map(|cd| (*depth, cd, *c))
        })
        .collect();
    for (depth, c) in kids.iter().rev() {
        if vm.display.get(*c).is_some_and(|co| co.clip_depth.is_some()) {
            continue;
        }
        if masks
            .iter()
            .any(|(md, cd, m)| depth > md && depth <= cd && !hit_shape_impl(vm, *m, x, y, true))
        {
            continue;
        }
        let is_clip = vm.display.get(*c).is_some_and(|co| co.clip().is_some());
        if matches!(mode, PickMode::Focus) {
            // Decorative geometry (including a widget's transparent hit area) must
            // not intercept a field. Another interactive widget still blocks it.
            if is_clip {
                if let Some(h) = pick_in(vm, *c, x, y, mode) {
                    return Some(h);
                }
                if takes_mouse(vm, *c) && enabled(vm, *c) && hit_shape(vm, *c, x, y) {
                    return Some(*c);
                }
            } else if vm
                .display
                .get(*c)
                .and_then(|o| o.text())
                .is_some_and(|t| t.input || t.selectable)
                && hit_shape(vm, *c, x, y)
            {
                return Some(*c);
            }
        } else if matches!(mode, PickMode::Button) {
            if is_clip && takes_mouse(vm, *c) && enabled(vm, *c) {
                if hit_shape(vm, *c, x, y) {
                    return Some(*c);
                }
                continue;
            }
            if is_clip {
                if let Some(h) = pick_in(vm, *c, x, y, mode) {
                    return Some(h);
                }
            }
        } else {
            if is_clip {
                if let Some(h) = pick_in(vm, *c, x, y, mode) {
                    return Some(h);
                }
            } else if hit_shape(vm, *c, x, y) {
                // A shape belongs to its clip; a text field is its own entity.
                if vm.display.get(*c).is_some_and(|co| co.text().is_some()) {
                    return Some(*c);
                }
                return Some(d);
            }
        }
    }
    None
}

/// Whether the point (twips, stage space) is on `d`'s drawn content.
pub fn hit_shape(vm: &Vm, d: DisplayId, x: f32, y: f32) -> bool {
    hit_shape_impl(vm, d, x, y, false)
}

fn hit_shape_impl(vm: &Vm, d: DisplayId, x: f32, y: f32, masking: bool) -> bool {
    let Some(o) = vm.display.get(d) else {
        return false;
    };
    if !masking
        && (!o.visible
            || o.hit_test_disable
            || o.mask_for.is_some()
            || o.mask.is_some_and(|m| !hit_shape_impl(vm, m, x, y, true)))
    {
        return false;
    }
    let Some(inv) = vm.display.world_matrix(d).invert() else {
        return false;
    };
    let (lx, ly) = inv.apply(x, y);
    match &o.kind {
        Kind::Shape(s, _) => {
            let b = s.bounds;
            if lx < b.x_min as f32
                || lx > b.x_max as f32
                || ly < b.y_min as f32
                || ly > b.y_max as f32
            {
                return false;
            }
            shape_contains(&s.records, s.lines.len(), lx, ly)
        }
        Kind::Morph(m, _) => {
            let b = m.start_bounds;
            lx >= b.x_min as f32
                && lx <= b.x_max as f32
                && ly >= b.y_min as f32
                && ly <= b.y_max as f32
        }
        Kind::Text(t) => {
            let (bx, by, w, h) = t.bounds;
            let (px, py) = (lx / 20.0, ly / 20.0);
            px >= bx && px <= bx + w && py >= by && py <= by + h
        }
        Kind::Bitmap { width, height, .. } => {
            lx >= 0.0 && ly >= 0.0 && lx <= width * 20.0 && ly <= height * 20.0
        }
        Kind::Clip(c) => {
            if let Some(dr) = &c.drawing {
                for (_, pts) in &dr.fills {
                    if polygon_contains(pts, lx, ly) {
                        return true;
                    }
                }
            }
            let kids: Vec<DisplayId> = o.children.values().copied().collect();
            kids.iter().any(|k| {
                vm.display.get(*k).is_some_and(|ko| ko.clip_depth.is_none())
                    && hit_shape_impl(vm, *k, x, y, masking)
            })
        }
    }
}

fn polygon_contains(pts: &[(f32, f32)], x: f32, y: f32) -> bool {
    let mut inside = false;
    let n = pts.len();
    for i in 0..n {
        let (x0, y0) = pts[i];
        let (x1, y1) = pts[(i + 1) % n];
        if (y0 > y) != (y1 > y) && x < x0 + (y - y0) / (y1 - y0) * (x1 - x0) {
            inside = !inside;
        }
    }
    inside
}

/// Point-in-shape for SWF edge records: a point is inside when, for some fill style, the
/// edges bounding it cross a ray from it an odd number of times.
fn shape_contains(records: &[Record], _lines: usize, x: f32, y: f32) -> bool {
    let mut parity: HashMap<(usize, u32), bool> = HashMap::new();
    let (mut px, mut py) = (0.0f32, 0.0f32);
    let (mut f0, mut f1) = (0u32, 0u32);
    let mut styles = 0usize;
    let mut cross = |a: (f32, f32), b: (f32, f32), f0: u32, f1: u32, styles: usize| {
        if (a.1 > y) != (b.1 > y) && x < a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0) {
            for f in [f0, f1] {
                if f != 0 {
                    let e = parity.entry((styles, f)).or_insert(false);
                    *e = !*e;
                }
            }
        }
    };
    for r in records {
        match r {
            Record::Style {
                move_to,
                fill0,
                fill1,
                new_styles,
                ..
            } => {
                if new_styles.is_some() {
                    styles += 1;
                    f0 = 0;
                    f1 = 0;
                }
                if let Some((mx, my)) = move_to {
                    px = *mx as f32;
                    py = *my as f32;
                }
                if let Some(f) = fill0 {
                    f0 = *f;
                }
                if let Some(f) = fill1 {
                    f1 = *f;
                }
            }
            Record::Line { to } => {
                let b = (to.0 as f32, to.1 as f32);
                cross((px, py), b, f0, f1, styles);
                (px, py) = b;
            }
            Record::Curve { control, to } => {
                let c = (control.0 as f32, control.1 as f32);
                let e = (to.0 as f32, to.1 as f32);
                let mut prev = (px, py);
                for i in 1..=8 {
                    let t = i as f32 / 8.0;
                    let u = 1.0 - t;
                    let q = (
                        u * u * px + 2.0 * u * t * c.0 + t * t * e.0,
                        u * u * py + 2.0 * u * t * c.1 + t * t * e.1,
                    );
                    cross(prev, q, f0, f1, styles);
                    prev = q;
                }
                (px, py) = e;
            }
        }
    }
    parity.values().any(|v| *v)
}

/// The clip actions of a placed clip (for tests and tools).
pub fn clip_actions(vm: &Vm, d: DisplayId) -> Vec<ClipAction> {
    vm.display
        .get(d)
        .and_then(|o| o.clip())
        .map(|c| c.clip_actions.clone())
        .unwrap_or_default()
}

/// `Mouse.getTopMostEntity`: the clip (or text field) under the mouse.
pub fn topmost(vm: &Vm) -> Option<DisplayId> {
    pick_any(
        vm,
        vm.root,
        (vm.mouse_pos.0 * 20.0) as f32,
        (vm.mouse_pos.1 * 20.0) as f32,
    )
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    use crate::avm1::{Host, HostValue};
    use crate::text::Fonts;

    struct Empty;
    impl Host for Empty {
        fn fscommand(&mut self, _: &str, _: &str) {}
        fn external_call(&mut self, _: &str, _: &[HostValue]) -> HostValue {
            HostValue::Undefined
        }
    }
    impl Fonts for Empty {
        fn font(&self, _: &str, _: bool, _: bool) -> Option<Arc<crate::swf::Font>> {
            None
        }
    }

    fn trace(text: &str) -> crate::swf::Actions {
        let mut code = vec![0x96];
        code.extend_from_slice(&((text.len() + 2) as u16).to_le_bytes());
        code.push(0);
        code.extend_from_slice(text.as_bytes());
        code.extend_from_slice(&[0, 0x26, 0]);
        Arc::from(code)
    }

    fn input_player() -> (Player, DisplayId) {
        let movie = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p =
            Player::new(movie, "selection", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let text = crate::text::TextField::new(Arc::new(crate::swf::EditText::default()), None);
        let mut object = Object::new(Kind::Text(Box::new(text)));
        object.parent = Some(p.vm.root);
        let field = p.vm.display.add(object);
        p.vm.display
            .get_mut(p.vm.root)
            .unwrap()
            .children
            .insert(1, field);
        (p, field)
    }

    #[test]
    fn interval_callbacks_catch_up_in_deadline_order_without_drift() {
        let (mut p, _) = input_player();
        let trace = p.vm.new_native(|vm, _, args| {
            let text = vm.to_str(args.first().unwrap());
            vm.log.push(text.to_string());
            Value::Undefined
        });
        let global = Value::Object(p.vm.global);
        for (delay, name) in [(20.0, "a"), (35.0, "b")] {
            p.vm.call_method(
                &global,
                "setInterval",
                &[
                    Value::Object(trace),
                    Value::Number(delay),
                    Value::from(name),
                ],
            );
        }
        p.tick(81.0);
        assert_eq!(p.drain_log(), ["a", "b", "a", "a", "b", "a"]);
        // Keep the original deadline rather than postponing it after each update.
        p.tick(19.0);
        assert!(p.drain_log().is_empty());
        p.tick(1.0);
        assert_eq!(p.drain_log(), ["a"]);
    }

    #[test]
    fn interval_catchup_is_bounded_without_rewinding_get_timer() {
        let (mut p, _) = input_player();
        let trace = p.vm.new_native(|vm, _, _| {
            vm.log.push("tick".into());
            Value::Undefined
        });
        let global = Value::Object(p.vm.global);
        p.vm.call_method(
            &global,
            "setInterval",
            &[Value::Object(trace), Value::Number(1.0)],
        );
        assert_eq!(p.vm.intervals[0].every, 10.0);
        p.tick(10.0);
        assert!(p.drain_log().is_empty());
        p.tick(990.0);
        assert_eq!(p.drain_log().len(), 10);
        assert_eq!(
            p.vm.call_method(&global, "getTimer", &[]),
            Value::Number(1000.0)
        );
        p.tick(20.0);
        assert_eq!(p.drain_log().len(), 2);
        assert_eq!(
            p.vm.call_method(&global, "getTimer", &[]),
            Value::Number(1020.0)
        );
    }

    #[test]
    fn interval_callbacks_can_cancel_and_removed_clip_methods_stop() {
        let (mut p, _) = input_player();
        let global = Value::Object(p.vm.global);
        let trace = p.vm.new_native(|vm, _, _| {
            vm.log.push("repeat".into());
            Value::Undefined
        });
        let id = p.vm.call_method(
            &global,
            "setInterval",
            &[Value::Object(trace), Value::Number(20.0)],
        );
        p.vm.set(&global, "cancelId", id);
        let cancel = p.vm.new_native(|vm, _, _| {
            vm.log.push("timeout".into());
            let global = Value::Object(vm.global);
            let id = vm.get(&global, "cancelId");
            vm.call_method(&global, "clearInterval", &[id]);
            Value::Undefined
        });
        p.vm.call_method(
            &global,
            "setTimeout",
            &[Value::Object(cancel), Value::Number(55.0)],
        );
        p.tick(81.0);
        assert_eq!(p.drain_log(), ["repeat", "repeat", "timeout"]);
        assert!(p.vm.intervals.is_empty());
        let root = p.vm.root;
        let clip = create_empty(&mut p.vm, root, "timer", 2).unwrap();
        let receiver = p.vm.display_object(clip);
        p.vm.set(&receiver, "onTick", Value::Object(trace));
        p.vm.call_method(
            &global,
            "setInterval",
            &[receiver, Value::from("onTick"), Value::Number(16.9)],
        );
        assert_eq!(p.vm.intervals[0].every, 16.0);
        remove_clip(&mut p.vm, clip);
        p.tick(100.0);
        assert!(p.drain_log().is_empty());
        assert!(p.vm.intervals.is_empty());
    }

    #[test]
    fn mouse_caret_and_drag_selection_use_transformed_text_coordinates() {
        let (mut p, field) = input_player();
        let o = p.vm.display.get_mut(field).unwrap();
        o.matrix = crate::swf::Matrix {
            a: 0.0,
            b: 2.0,
            c: -2.0,
            d: 0.0,
            tx: 2000.0,
            ty: 400.0,
        };
        let t = o.text_mut().unwrap();
        t.bounds = (3.0, 4.0, 40.0, 30.0);
        t.default_format.size = 10.0;
        t.set_text("ab Ωd");
        let matrix = p.vm.display.get(field).unwrap().matrix;
        let root = p.vm.root;
        let overlay = create_empty(&mut p.vm, root, "hitArea", 2).unwrap();
        let o = p.vm.display.get_mut(overlay).unwrap();
        o.matrix = matrix;
        o.color.mul[3] = 0.0;
        let mut area = Object::new(Kind::Bitmap {
            url: "hit-area".into(),
            width: 40.0,
            height: 30.0,
        });
        area.matrix = crate::swf::Matrix::translate(60.0, 80.0);
        let area = p.vm.display.add(area);
        insert_child(&mut p.vm, overlay, area, 0);
        let point = |x: f64, y: f64| (100.0 - y * 2.0, 20.0 + x * 2.0);
        let (x, y) = point(8.0, 8.0);
        assert_eq!(p.topmost_at(x, y), Some(overlay));
        p.mouse_button(x, y, MOUSE_LEFT, true, false);
        assert_eq!(p.vm.focus, Some(field));
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (1, 1)
        );
        let (x, y) = point(23.0, 8.0);
        p.mouse_move(x, y);
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (1, 4)
        );
        // Capture continues outside the field and ends on release.
        let (x, y) = point(100.0, 8.0);
        p.mouse_button(x, y, MOUSE_LEFT, false, false);
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (1, 5)
        );
        let (x, y) = point(6.0, 8.0);
        p.mouse_move(x, y);
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (1, 5)
        );
        p.type_text("!");
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().text(),
            "a!"
        );
        p.vm.keys_down.insert(16);
        p.mouse_button(x, y, MOUSE_LEFT, true, false);
        p.mouse_button(x, y, MOUSE_LEFT, false, false);
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (0, 2)
        );
        p.mouse_button(x, y, MOUSE_RIGHT, true, false);
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (0, 2)
        );
        let button = display_api::display_value(&mut p.vm, overlay);
        let press = p.vm.new_native(|_, _, _| Value::Undefined);
        p.vm.set(&button, "onPress", Value::Object(press));
        p.mouse_button(x, y, MOUSE_LEFT, true, false);
        assert_eq!(
            p.vm.focus, None,
            "an interactive button must block text behind it"
        );
    }

    #[test]
    fn select_all_replaces_text_and_keeps_restrictions_limits_and_notifications() {
        let (mut p, field) = input_player();
        let selection = Value::Object(p.vm.selection);
        assert_eq!(
            p.vm.call_method(&selection, "getBeginIndex", &[]),
            Value::Number(-1.0)
        );
        set_focus(&mut p.vm, Some(field));
        let text_value = p.vm.display_object(field);
        let changed = p.vm.new_native(|vm, this, _| {
            let text = vm.get(&this, "text");
            let s = vm.to_str(&text);
            vm.log.push(format!("changed: {s}"));
            Value::Undefined
        });
        p.vm.set(&text_value, "onChanged", Value::Object(changed));
        let t = p.vm.display.get_mut(field).unwrap().text_mut().unwrap();
        t.variable = "name".into();
        t.max_chars = 5;
        t.restrict = Some("A-Za-z".into());
        p.type_text("Rowan");
        p.select_all();
        assert_eq!(
            p.vm.call_method(&selection, "getBeginIndex", &[]),
            Value::Number(0.0)
        );
        assert_eq!(
            p.vm.call_method(&selection, "getEndIndex", &[]),
            Value::Number(5.0)
        );
        p.type_text("!");
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (0, 5)
        );
        p.type_text("Sera");
        let t = p.vm.display.get(field).unwrap().text().unwrap();
        assert_eq!(t.text(), "Sera");
        assert_eq!(t.selection(), (4, 4));
        let root = p.vm.display_object(p.vm.root);
        assert_eq!(p.vm.get(&root, "name"), Value::from("Sera"));
        assert_eq!(p.drain_log().last().unwrap(), "changed: Sera");
        p.key_down(17, None);
        p.key_down(65, Some('a'));
        p.key_up(65);
        p.key_up(17);
        p.key_down(8, None);
        p.key_up(8);
        assert_eq!(p.vm.display.get(field).unwrap().text().unwrap().text(), "");
        assert_eq!(
            p.vm.call_method(&selection, "getCaretIndex", &[]),
            Value::Number(0.0)
        );
        set_focus(&mut p.vm, None);
        assert_eq!(
            p.vm.call_method(&selection, "getCaretIndex", &[]),
            Value::Number(-1.0)
        );
    }

    #[test]
    fn stationary_hover_refreshes_after_geometry_visibility_and_wheel_changes() {
        fn over(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
            let name = vm.get(&this, "_name");
            let name = vm.to_str(&name);
            vm.log.push(format!("{name} over"));
            Value::Undefined
        }
        fn out(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
            let name = vm.get(&this, "_name");
            let name = vm.to_str(&name);
            vm.log.push(format!("{name} out"));
            Value::Undefined
        }
        let (mut p, _) = input_player();
        let root = p.vm.root;
        let a = create_empty(&mut p.vm, root, "a", 2).unwrap();
        let b = create_empty(&mut p.vm, root, "b", 3).unwrap();
        for (d, x) in [(a, 0.0), (b, 1200.0)] {
            let o = p.vm.display.get_mut(d).unwrap();
            o.matrix.tx = x;
            o.clip_mut().unwrap().drawing = Some(crate::display::Drawing {
                fills: vec![(
                    [255, 0, 0, 255],
                    vec![(0.0, 0.0), (1000.0, 0.0), (1000.0, 400.0), (0.0, 400.0)],
                )],
                ..Default::default()
            });
            let v = display_api::display_value(&mut p.vm, d);
            let f = p.vm.new_native(over);
            p.vm.set(&v, "onRollOver", Value::Object(f));
            let f = p.vm.new_native(out);
            p.vm.set(&v, "onRollOut", Value::Object(f));
        }
        let av = display_api::display_value(&mut p.vm, a);
        let bv = display_api::display_value(&mut p.vm, b);
        p.mouse_move(10.0, 10.0);
        assert_eq!(p.drain_log(), ["a over"]);
        p.vm.set(&av, "_x", Value::Number(60.0));
        p.vm.set(&bv, "_x", Value::Number(0.0));
        p.tick(0.0);
        assert_eq!(p.drain_log(), ["a out", "b over"]);
        assert!(!p.vm.pick_dirty);
        p.tick(0.0);
        assert!(p.drain_log().is_empty());
        p.vm.set(&bv, "_visible", Value::Bool(false));
        p.run_frame();
        assert_eq!(p.drain_log(), ["b out"]);
        p.vm.set(&av, "_x", Value::Number(0.0));
        p.run_frame();
        assert_eq!(p.drain_log(), ["a over"]);
        let wheel = p.vm.new_native(|vm, this, _| {
            vm.set(&this, "_x", Value::Number(60.0));
            Value::Undefined
        });
        p.vm.set(&av, "onMouseWheel", Value::Object(wheel));
        let mouse = Value::Object(p.vm.mouse);
        p.vm.call_method(&mouse, "addListener", std::slice::from_ref(&av));
        p.vm.set(&bv, "_visible", Value::Bool(true));
        p.mouse_wheel(10.0, 10.0, 1.0);
        assert_eq!(p.drain_log(), ["a out", "b over"]);
    }

    #[test]
    fn selection_ranges_shift_navigation_and_utf16_indices() {
        let (mut p, field) = input_player();
        set_focus(&mut p.vm, Some(field));
        p.type_text("Rowan");
        p.key_down(36, None);
        p.key_up(36);
        p.key_down(16, None);
        p.key_down(39, None);
        p.key_down(39, None);
        p.key_up(39);
        p.key_up(16);
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().selection(),
            (0, 2)
        );
        p.type_text("Br");
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().text(),
            "Brwan"
        );
        let selection = Value::Object(p.vm.selection);
        p.vm.call_method(
            &selection,
            "setSelection",
            &[Value::Number(4.0), Value::Number(1.0)],
        );
        assert_eq!(
            p.vm.call_method(&selection, "getBeginIndex", &[]),
            Value::Number(1.0)
        );
        assert_eq!(
            p.vm.call_method(&selection, "getCaretIndex", &[]),
            Value::Number(1.0)
        );
        p.key_down(39, None);
        p.key_up(39);
        assert_eq!(
            p.vm.call_method(&selection, "getCaretIndex", &[]),
            Value::Number(4.0)
        );
        p.vm.call_method(
            &selection,
            "setSelection",
            &[Value::Number(1.0), Value::Number(4.0)],
        );
        p.key_down(8, None);
        p.key_up(8);
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().text(),
            "Bn"
        );
        p.vm.display
            .get_mut(field)
            .unwrap()
            .text_mut()
            .unwrap()
            .set_text("A😀B");
        p.vm.call_method(
            &selection,
            "setSelection",
            &[Value::Number(1.0), Value::Number(3.0)],
        );
        assert_eq!(
            p.vm.call_method(&selection, "getCaretIndex", &[]),
            Value::Number(3.0)
        );
        p.type_text("X");
        assert_eq!(
            p.vm.display.get(field).unwrap().text().unwrap().text(),
            "AXB"
        );
        p.vm.call_method(
            &selection,
            "setSelection",
            &[Value::Number(-2.0), Value::Number(999.0)],
        );
        p.vm.call_method(&selection, "setSelection", &[]);
        p.key_down(46, None);
        p.key_up(46);
        assert_eq!(p.vm.display.get(field).unwrap().text().unwrap().text(), "");
    }

    #[test]
    fn script_masks_reassign_clear_and_detach_on_removal() {
        let movie = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p = Player::new(movie, "masks", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let root = p.vm.root;
        let a = create_empty(&mut p.vm, root, "a", 1).unwrap();
        let b = create_empty(&mut p.vm, root, "b", 2).unwrap();
        let mask = create_empty(&mut p.vm, root, "mask", 3).unwrap();
        let av = p.vm.display_object(a);
        let bv = p.vm.display_object(b);
        let mv = p.vm.display_object(mask);
        assert_eq!(
            p.vm.call_method(&av, "setMask", std::slice::from_ref(&mv)),
            Value::Bool(true)
        );
        assert_eq!(p.vm.display.get(a).unwrap().mask, Some(mask));
        assert_eq!(p.vm.display.get(mask).unwrap().mask_for, Some(a));
        assert_eq!(
            p.vm.call_method(&bv, "setMask", std::slice::from_ref(&mv)),
            Value::Bool(true)
        );
        assert_eq!(p.vm.display.get(a).unwrap().mask, None);
        assert_eq!(p.vm.display.get(mask).unwrap().mask_for, Some(b));
        assert_eq!(
            p.vm.call_method(&bv, "setMask", &[Value::from("missing")]),
            Value::Bool(false)
        );
        assert_eq!(p.vm.call_method(&bv, "setMask", &[]), Value::Undefined);
        assert_eq!(p.vm.display.get(b).unwrap().mask, Some(mask));
        assert_eq!(
            p.vm.call_method(&bv, "setMask", std::slice::from_ref(&bv)),
            Value::Bool(false)
        );
        assert_eq!(
            p.vm.call_method(&bv, "setMask", &[Value::Null]),
            Value::Bool(true)
        );
        assert_eq!(p.vm.display.get(mask).unwrap().mask_for, None);
        p.vm.call_method(&av, "setMask", std::slice::from_ref(&mv));
        remove_clip(&mut p.vm, mask);
        assert_eq!(p.vm.display.get(a).unwrap().mask, None);
        let reused = create_empty(&mut p.vm, root, "newMask", 3).unwrap();
        assert_ne!(reused, mask);
        assert!(p.vm.display.set_mask(a, Some(reused)));
        remove_clip(&mut p.vm, a);
        assert_eq!(p.vm.display.get(reused).unwrap().mask_for, None);
    }

    #[test]
    fn script_mask_paths_use_the_activation_and_masks_clip_mouse_picking() {
        let movie = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p = Player::new(movie, "masks", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let root = p.vm.root;
        let container = create_empty(&mut p.vm, root, "container", 1).unwrap();
        let target = create_empty(&mut p.vm, container, "target", 0).unwrap();
        let mask = create_empty(&mut p.vm, container, "mask", 1).unwrap();
        let rect = |width| crate::display::Drawing {
            fills: vec![(
                [255; 4],
                vec![(0.0, 0.0), (width, 0.0), (width, 400.0), (0.0, 400.0)],
            )],
            ..Default::default()
        };
        p.vm.display
            .get_mut(target)
            .unwrap()
            .clip_mut()
            .unwrap()
            .drawing = Some(rect(400.0));
        let o = p.vm.display.get_mut(mask).unwrap();
        o.clip_mut().unwrap().drawing = Some(rect(100.0));
        o.matrix.tx = 100.0;
        o.visible = false;
        o.color.mul[3] = 0.0;
        let tv = p.vm.display_object(target);
        let press = p.vm.new_native(|_, _, _| Value::Undefined);
        p.vm.set(&tv, "onPress", Value::Object(press));
        // target.setMask("mask"), evaluated on container's timeline.
        let mut code = Vec::new();
        let string = |code: &mut Vec<u8>, value: &str| {
            code.push(0x96);
            code.extend_from_slice(&((value.len() + 2) as u16).to_le_bytes());
            code.push(0);
            code.extend_from_slice(value.as_bytes());
            code.push(0);
        };
        string(&mut code, "mask");
        code.extend_from_slice(&[0x96, 5, 0, 7, 1, 0, 0, 0]);
        string(&mut code, "target");
        code.push(0x1c);
        string(&mut code, "setMask");
        code.extend_from_slice(&[0x52, 0x17, 0]);
        interp::run_actions(&mut p.vm, &Arc::from(code), container);
        assert_eq!(p.vm.native_target, None);
        assert_eq!(p.vm.display.get(target).unwrap().mask, Some(mask));
        assert_eq!(pick(&p.vm, 150.0, 200.0), Some(target));
        assert_eq!(pick(&p.vm, 50.0, 200.0), None);
        assert_eq!(pick(&p.vm, 250.0, 200.0), None);
        p.vm.display.get_mut(mask).unwrap().matrix.tx = 200.0;
        assert_eq!(pick(&p.vm, 150.0, 200.0), None);
        assert_eq!(pick(&p.vm, 250.0, 200.0), Some(target));
        assert!(!hit_shape(&p.vm, mask, 250.0, 200.0));
    }

    #[test]
    fn initialize_then_construct_then_fifo_frame_and_load_actions() {
        // Empty one-frame SWF; only the queued actions below matter.
        let movie = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut player =
            Player::new(movie, "queue-test", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let root = player.vm.root;
        player.vm.queue.extend([
            Queued::Frame(root, trace("frame")),
            Queued::Event(root, "onLoad", vec![trace("load")]),
            Queued::Construct {
                clip: root,
                ctor: None,
                events: vec![trace("construct")],
            },
            Queued::Init(root, trace("initialize")),
        ]);
        player.run_queue();
        assert_eq!(
            player.drain_log(),
            [
                "trace: initialize",
                "trace: construct",
                "trace: frame",
                "trace: load"
            ]
        );
    }

    #[test]
    fn unnamed_clips_get_unique_paths_for_host_and_post_load_callbacks() {
        let movie = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p = Player::new(movie, "paths", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let root = p.vm.root;
        Arc::get_mut(&mut p.vm.movie).unwrap().characters.insert(
            1,
            Character::Sprite(Arc::new(crate::swf::Timeline {
                frames: vec![Default::default()],
                labels: vec![],
            })),
        );
        for depth in 1..=2 {
            place(
                &mut p.vm,
                root,
                &Place {
                    depth,
                    character: Some(1),
                    ..Default::default()
                },
            );
        }
        p.run_queue();
        let first = p.vm.display.child_named(root, "instance1").unwrap();
        let second = p.vm.display.child_named(root, "instance2").unwrap();
        let value = display_api::display_value(&mut p.vm, first);
        assert_eq!(&*p.vm.to_str(&value), "_level0.instance1");
        let primitive = p.vm.to_primitive(&value, false);
        assert_eq!(primitive, value);
        assert_eq!(&*p.vm.to_str(&primitive), "_level0.instance1");
        assert!(p.vm.to_number(&value).is_nan());
        let root_value = p.vm.display_object(root);
        let keys = p.vm.keys(&root_value);
        assert!(keys.iter().any(|s| s.as_ref() == "instance1"));
        assert!(keys.iter().any(|s| s.as_ref() == "instance2"));
        assert_ne!(first, second);

        // Moving an existing placement preserves its name; a fresh placement gets a
        // fresh name even when the arena slot is reused.
        place(
            &mut p.vm,
            root,
            &Place {
                depth: 1,
                is_move: true,
                ..Default::default()
            },
        );
        assert_eq!(p.vm.display.child_named(root, "instance1"), Some(first));
        remove_clip(&mut p.vm, first);
        place(
            &mut p.vm,
            root,
            &Place {
                depth: 1,
                character: Some(1),
                ..Default::default()
            },
        );
        assert!(p.vm.display.child_named(root, "instance3").is_some());
    }

    #[test]
    fn exported_bitmaps_attach_with_intrinsic_size_and_replace_their_depth() {
        let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p = Player::new(bytes, "bitmaps", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let mut movie = Movie::default();
        movie.characters.insert(
            1,
            Character::Image(crate::swf::ExternalImage {
                format: 0,
                width: 32,
                height: 24,
                export_name: "test_coin".into(),
                file: "test-coin.dds".into(),
            }),
        );
        movie.exports.insert("test_coin".into(), 1);
        p.vm.movie = Arc::new(movie);
        let global = Value::Object(p.vm.global);
        let flash = p.vm.get(&global, "flash");
        let display = p.vm.get(&flash, "display");
        let class = p.vm.get(&display, "BitmapData");
        let bitmap =
            p.vm.call_method(&class, "loadBitmap", &[Value::from("test_coin")]);
        assert_eq!(p.vm.get(&bitmap, "width"), Value::Number(32.0));
        p.vm.set(&bitmap, "width", Value::Number(1.0));
        assert_eq!(p.vm.get(&bitmap, "width"), Value::Number(32.0));
        assert_eq!(
            p.vm.call_method(&class, "loadBitmap", &[Value::from("missing")]),
            Value::Null
        );
        let root = p.vm.root;
        let target = p.vm.display_object(root);
        p.vm.call_method(
            &target,
            "attachBitmap",
            &[bitmap.clone(), Value::Number(1.0)],
        );
        let old = p.vm.display.get(root).unwrap().children[&1];
        let bounds = display_api::local_bounds(&p.vm, root).unwrap();
        assert_eq!((bounds.width(), bounds.height()), (640, 480));
        assert!(
            matches!(&p.vm.display.get(old).unwrap().kind, Kind::Bitmap {url, ..} if url == "test-coin.dds")
        );
        p.vm.call_method(&target, "attachBitmap", &[Value::Null, Value::Number(1.0)]);
        assert!(p.vm.display.get(old).is_some());
        p.vm.call_method(&target, "attachBitmap", &[bitmap, Value::Number(1.0)]);
        assert!(p.vm.display.get(old).is_none());
        assert_eq!(p.vm.display.get(root).unwrap().children.len(), 1);
    }

    #[test]
    fn goto_replaces_a_different_text_character_at_the_same_depth() {
        let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p =
            Player::new(bytes, "text-state", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let mut movie = Movie::default();
        for (id, r, g) in [(1, 200, 180), (2, 153, 0)] {
            movie.characters.insert(
                id,
                Character::EditText(Arc::new(crate::swf::EditText {
                    color: Some(crate::swf::Color { r, g, b: 0, a: 255 }),
                    initial: Some("price".into()),
                    ..Default::default()
                })),
            );
        }
        let placement = |id| {
            Command::Place(Box::new(Place {
                depth: 1,
                character: Some(id),
                name: Some("TextField".into()),
                ..Default::default()
            }))
        };
        let timeline = Arc::new(crate::swf::Timeline {
            frames: vec![
                crate::swf::Frame {
                    commands: vec![placement(1)],
                    ..Default::default()
                },
                crate::swf::Frame {
                    commands: vec![Command::Remove { depth: 1 }, placement(2)],
                    ..Default::default()
                },
            ],
            labels: vec![],
        });
        p.vm.movie = Arc::new(movie);
        let root = p.vm.root;
        p.vm.display
            .get_mut(root)
            .unwrap()
            .clip_mut()
            .unwrap()
            .timeline = timeline;
        enter_frame_of(&mut p.vm, root, 0);
        let old = p.vm.display.child_named(root, "TextField").unwrap();
        goto(&mut p.vm, root, 1, false);
        let red = p.vm.display.child_named(root, "TextField").unwrap();
        assert_ne!(old, red);
        assert_eq!(
            display_api::display_get(&mut p.vm, red, "textColor"),
            Some(Value::Number(0x990000 as f64))
        );
        goto(&mut p.vm, root, 0, false);
        let gold = p.vm.display.child_named(root, "TextField").unwrap();
        assert_eq!(
            display_api::display_get(&mut p.vm, gold, "textColor"),
            Some(Value::Number(0xc8b400 as f64))
        );
    }

    #[test]
    fn duplicate_clip_constructs_the_class_without_copying_dynamic_properties() {
        let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p =
            Player::new(bytes, "duplicate", Box::new(Empty), Arc::new(Empty), None).unwrap();
        let mut movie = Movie::default();
        movie.characters.insert(
            1,
            Character::Sprite(Arc::new(crate::swf::Timeline {
                frames: vec![crate::swf::Frame {
                    actions: vec![trace("first frame")],
                    ..Default::default()
                }],
                labels: vec![],
            })),
        );
        movie.exports.insert("button".into(), 1);
        p.vm.movie = Arc::new(movie);
        let constructor = p.vm.new_native(|vm, this, _| {
            let seed = vm.get(&this, "seed");
            vm.set(&this, "constructed_with", seed);
            Value::Undefined
        });
        let prototype = p.vm.new_object();
        p.vm.define(prototype, "class_member", Value::Bool(true), 0);
        p.vm.define(constructor, "prototype", Value::Object(prototype), 0);
        p.vm.classes.insert(1, Value::Object(constructor));
        let root = p.vm.root;
        let source = attach(&mut p.vm, root, "button", "original", 1, None).unwrap();
        p.run_queue();
        p.drain_log();
        let source_value = p.vm.display_object(source);
        p.vm.set(&source_value, "private", Value::Number(99.0));
        let object = p.vm.display.get_mut(source).unwrap();
        object.matrix.tx = 400.0;
        object.color.mul[3] = 0.5;
        let init = p.vm.new_object();
        p.vm.define(init, "seed", Value::Number(42.0), 0);
        let copy = duplicate(&mut p.vm, source, "copy", 2, Some(init)).unwrap();
        p.run_queue();
        assert_eq!(p.drain_log(), ["trace: first frame"]);
        let copy_value = p.vm.display_object(copy);
        assert_eq!(
            p.vm.get(&copy_value, "constructed_with"),
            Value::Number(42.0)
        );
        assert_eq!(p.vm.get(&copy_value, "class_member"), Value::Bool(true));
        assert_eq!(p.vm.get(&copy_value, "private"), Value::Undefined);
        assert_eq!(p.vm.display.get(copy).unwrap().matrix.tx, 400.0);
        assert_eq!(p.vm.display.get(copy).unwrap().color.mul[3], 0.5);
        assert_eq!(p.vm.display.child_named(root, "original"), Some(source));
        assert!(duplicate(&mut p.vm, root, "root-copy", 3, None).is_none());
        let replaced = duplicate(&mut p.vm, source, "copy2", 2, None).unwrap();
        assert!(p.vm.display.get(copy).is_none());
        assert_eq!(p.vm.display.child_named(root, "copy2"), Some(replaced));
    }
}
