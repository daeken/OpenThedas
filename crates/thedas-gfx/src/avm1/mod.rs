//! The ActionScript 2 virtual machine: values, objects, property access and calls.
//!
//! Semantics follow SWF 8 (case-sensitive names). Where Scaleform differs from Flash the
//! difference is documented where it is implemented.

pub mod builtins;
pub mod display_api;
pub mod interp;
pub mod object;
pub mod value;

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;
use std::sync::Arc;

use object::{DONT_ENUM, Func, Heap, Kind, NativeFn, Object, Prop, READ_ONLY};
use value::{ObjId, Str, Value, number_to_string, string_to_number};

use crate::display::{Display, DisplayId};
use crate::swf::{Actions, Movie};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeKind {
    Global,
    Target,
    Local,
}

/// One link of a scope chain.
#[derive(Debug)]
pub struct Scope {
    pub object: ObjId,
    pub kind: ScopeKind,
    pub parent: Option<Rc<Scope>>,
}

/// The prototypes and constructors the built-ins hang off.
#[derive(Clone, Copy, Debug)]
pub struct Protos {
    pub object: ObjId,
    pub function: ObjId,
    pub array: ObjId,
    pub string: ObjId,
    pub number: ObjId,
    pub boolean: ObjId,
    pub date: ObjId,
    pub movie_clip: ObjId,
    pub text_field: ObjId,
    pub text_format: ObjId,
}

/// A value passed between a movie and its host.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum HostValue {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    List(Vec<HostValue>),
    Object(std::collections::BTreeMap<String, HostValue>),
}

/// What a movie asks of the engine.
pub trait Host {
    fn fscommand(&mut self, command: &str, args: &str);
    fn external_call(&mut self, name: &str, args: &[HostValue]) -> HostValue;
    /// The pixel size of an image a clip loads (`img://...` or a file name).
    fn image_size(&mut self, _url: &str) -> Option<(u32, u32)> {
        None
    }
}

/// Work queued to run after the current script.
#[derive(Debug)]
pub enum Queued {
    Frame(DisplayId, Actions),
    Init(DisplayId, Actions),
    Construct {
        clip: DisplayId,
        ctor: Option<Value>,
        events: Vec<Actions>,
    },
    /// A method on the clip's script object (`onLoad`, ...), and its clip events.
    Event(DisplayId, &'static str, Vec<Actions>),
}

#[derive(Debug)]
pub struct Interval {
    pub id: u32,
    pub this: Value,
    pub func: Value,
    pub method: Option<Str>,
    pub every: f64,
    pub next: f64,
    pub args: Vec<Value>,
}

/// The whole script world of one movie.
pub struct Vm {
    /// Default names for unnamed timeline MovieClips and text fields.
    pub(crate) next_instance_id: u32,
    pub heap: Heap,
    pub display: Display,
    pub global: ObjId,
    pub protos: Protos,
    pub root: DisplayId,
    /// Timeline target of the current bytecode activation, for native path arguments.
    pub(crate) native_target: Option<DisplayId>,
    pub movie: Arc<Movie>,
    pub name: String,
    /// Registered classes (`Object.registerClass`) by character id.
    pub classes: HashMap<u16, Value>,
    pub inits_done: HashSet<u16>,
    pub queue: VecDeque<Queued>,
    pub intervals: Vec<Interval>,
    pub next_interval: u32,
    pub time_ms: f64,
    /// Timer time can pause to bound catch-up work while getTimer keeps elapsed time.
    pub timer_time_ms: f64,
    pub host: Box<dyn Host>,
    /// `ExternalInterface.addCallback` registrations: name -> (this, function).
    pub callbacks: HashMap<String, (Value, Value)>,
    /// The objects listening to `Key`, `Mouse`, `Stage` (their `_listeners` arrays).
    pub key: ObjId,
    pub mouse: ObjId,
    pub stage: ObjId,
    pub selection: ObjId,
    /// The visible area in stage units (`Stage.width`/`height`).
    pub stage_size: (f64, f64),
    pub mouse_pos: (f64, f64),
    pub keys_down: HashSet<u32>,
    pub last_key: u32,
    pub focus: Option<DisplayId>,
    pub log: Vec<String>,
    /// Something drawn changed since the last render.
    pub dirty: bool,
    /// The display list (or something under the mouse) changed since the last pick.
    pub pick_dirty: bool,
    pub budget: u64,
    /// Depth of nested calls, to stop runaway recursion.
    pub call_depth: u32,
    /// Nested bytecode/native/timeline entries sharing one action list's limits.
    pub(crate) script_depth: u32,
    pub(crate) script_stopped: bool,
    pub fonts: Arc<dyn crate::text::Fonts>,
    /// Images loaded into clips, by clip.
    pub warned: HashSet<String>,
    pub dragging: Option<(DisplayId, bool, (f64, f64))>,
    pub rng: u64,
    /// Log every bytecode function call (`GFX_TRACE_CALLS`), for debugging.
    pub trace_calls: bool,
    pub fn_names: HashMap<ObjId, Str>,
}

/// A runaway script stops after this many actions in one run.
pub const OP_BUDGET: u64 = 20_000_000;

impl Vm {
    pub(crate) fn enter_script(&mut self) -> bool {
        if self.script_depth == 0 {
            self.budget = 0;
            self.script_stopped = false;
        } else if self.script_stopped {
            return false;
        }
        self.script_depth += 1;
        true
    }

    pub(crate) fn leave_script(&mut self) {
        self.script_depth -= 1;
    }

    pub(crate) fn stop_script(&mut self, reason: &str) {
        self.script_stopped = true;
        self.warn_once(reason.into());
    }

    pub fn warn_once(&mut self, msg: String) {
        if self.warned.insert(msg.clone()) {
            log::debug!("gfx {}: {msg}", self.name);
            self.log.push(format!("warning: {msg}"));
        }
    }

    pub fn alloc(&mut self, proto: Option<ObjId>, kind: Kind) -> ObjId {
        self.heap.alloc(Object {
            proto,
            kind,
            ..Default::default()
        })
    }

    pub fn new_object(&mut self) -> ObjId {
        self.alloc(Some(self.protos.object), Kind::Plain)
    }

    pub fn new_array(&mut self, items: Vec<Value>) -> ObjId {
        self.alloc(Some(self.protos.array), Kind::Array(items))
    }

    pub fn new_native(&mut self, f: NativeFn) -> ObjId {
        self.alloc(Some(self.protos.function), Kind::Function(Func::Native(f)))
    }

    /// Defines a property without running setters or watchers.
    pub fn define(&mut self, o: ObjId, name: &str, v: Value, flags: u8) {
        if let Some(obj) = self.heap.get_mut(o) {
            obj.props.insert(
                Str::from(name),
                Prop {
                    value: v,
                    flags,
                    accessor: None,
                },
            );
        }
    }

    pub fn define_native(&mut self, o: ObjId, name: &str, f: NativeFn) -> ObjId {
        let fid = self.new_native(f);
        self.define(o, name, Value::Object(fid), DONT_ENUM);
        fid
    }

    // ---- conversions --------------------------------------------------------------------

    pub fn to_number(&mut self, v: &Value) -> f64 {
        match v {
            Value::Undefined | Value::Null => f64::NAN,
            Value::Bool(b) => f64::from(u8::from(*b)),
            Value::Number(n) => *n,
            Value::String(s) => string_to_number(s),
            Value::Object(_) => {
                let p = self.to_primitive(v, true);
                if matches!(p, Value::Object(_)) {
                    f64::NAN
                } else {
                    self.to_number(&p)
                }
            }
        }
    }

    pub fn to_bool(&self, v: &Value) -> bool {
        match v {
            Value::Undefined | Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => !n.is_nan() && *n != 0.0,
            // SWF 7+: non-empty strings are true.
            Value::String(s) => !s.is_empty(),
            Value::Object(_) => true,
        }
    }

    pub fn to_str(&mut self, v: &Value) -> Str {
        match v {
            Value::Undefined => Str::from("undefined"),
            Value::Null => Str::from("null"),
            Value::Bool(b) => Str::from(if *b { "true" } else { "false" }),
            Value::Number(n) => Str::from(number_to_string(*n)),
            Value::String(s) => s.clone(),
            Value::Object(o) => {
                if let Some(Kind::Display(d)) = self.heap.get(*o).map(|o| &o.kind) {
                    return Str::from(self.display.path(*d, true));
                }
                let p = self.to_primitive(v, false);
                match p {
                    Value::Object(o) => {
                        if self.is_function(o) {
                            Str::from("[type Function]")
                        } else {
                            Str::from("[object Object]")
                        }
                    }
                    p => self.to_str(&p),
                }
            }
        }
    }

    pub fn to_primitive(&mut self, v: &Value, number_hint: bool) -> Value {
        let Value::Object(o) = v else {
            return v.clone();
        };
        let o = *o;
        if matches!(self.heap.get(o).map(|o| &o.kind), Some(Kind::Display(_))) {
            if number_hint {
                return v.clone();
            }
            // AVM1's Add2 asks a clip for valueOf, then converts the clip itself to
            // its path if the other operand is a string. Object.toString is unrelated.
            let f = self.get(v, "valueOf");
            return self.call(&f, v.clone(), &[]);
        }
        let is_date = matches!(self.heap.get(o).map(|x| &x.kind), Some(Kind::Date(_)));
        let order: [&str; 2] = if number_hint && !is_date {
            ["valueOf", "toString"]
        } else {
            ["toString", "valueOf"]
        };
        for m in order {
            let f = self.get(v, m);
            if let Value::Object(fo) = f {
                if self.is_function(fo) {
                    let r = self.call(&f, v.clone(), &[]);
                    if !matches!(r, Value::Object(_)) {
                        return r;
                    }
                }
            }
        }
        v.clone()
    }

    pub fn is_function(&self, o: ObjId) -> bool {
        matches!(self.heap.get(o).map(|x| &x.kind), Some(Kind::Function(_)))
    }

    /// The object a primitive's properties are found on (its prototype), or the object.
    fn object_for(&self, v: &Value) -> Option<ObjId> {
        match v {
            Value::Object(o) => Some(*o),
            Value::String(_) => Some(self.protos.string),
            Value::Number(_) => Some(self.protos.number),
            Value::Bool(_) => Some(self.protos.boolean),
            _ => None,
        }
    }

    // ---- property access ----------------------------------------------------------------

    /// Finds `name` on `o` or its prototypes: (holder, accessor or value).
    fn lookup(&self, o: ObjId, name: &str) -> Option<(ObjId, Prop)> {
        let mut cur = Some(o);
        let mut guard = 0;
        while let Some(c) = cur {
            let obj = self.heap.get(c)?;
            if let Some(p) = obj.props.get(name) {
                return Some((c, p.clone()));
            }
            cur = obj.proto;
            guard += 1;
            if guard > 256 {
                break;
            }
        }
        None
    }

    pub fn has_property(&mut self, v: &Value, name: &str) -> bool {
        let Some(o) = self.object_for(v) else {
            return false;
        };
        if self.lookup(o, name).is_some() {
            return true;
        }
        match self.heap.get(o).map(|x| x.kind.clone()) {
            Some(Kind::Display(d)) => {
                self.display.child_named(d, name).is_some()
                    || builtins::display_get(self, d, name).is_some()
            }
            Some(Kind::Array(_)) => name == "length",
            _ => false,
        }
    }

    pub fn get(&mut self, v: &Value, name: &str) -> Value {
        // Primitive strings answer `length` themselves.
        if let Value::String(s) = v {
            if name == "length" {
                return Value::Number(s.encode_utf16().count() as f64);
            }
        }
        let Some(o) = self.object_for(v) else {
            return Value::Undefined;
        };
        let kind = match self.heap.get(o) {
            Some(x) => x.kind.clone(),
            None => return Value::Undefined,
        };
        match &kind {
            Kind::Array(items) => {
                if name == "length" {
                    return Value::Number(items.len() as f64);
                }
                if let Some(i) = array_index(name) {
                    return items.get(i).cloned().unwrap_or_default();
                }
            }
            Kind::Boxed(Value::String(s)) if name == "length" => {
                return Value::Number(s.encode_utf16().count() as f64);
            }
            Kind::Super { this, proto, .. } => {
                let Some(p) = proto else {
                    return Value::Undefined;
                };
                let this = this.clone();
                return match self.lookup(*p, name) {
                    Some((_, prop)) => self.read_prop(prop, &this),
                    None => Value::Undefined,
                };
            }
            _ => {}
        }
        if name == "__proto__" {
            return self
                .heap
                .get(o)
                .and_then(|x| x.proto)
                .map(Value::Object)
                .unwrap_or_default();
        }
        if let Kind::Display(d) = kind {
            // Own properties, then children by name, then display properties, then the
            // prototype chain.
            if let Some(p) = self.heap.get(o).and_then(|x| x.props.get(name)).cloned() {
                return self.read_prop(p, v);
            }
            if self.display.get(d).is_none() {
                return Value::Undefined;
            }
            if let Some(c) = self.display.child_named(d, name) {
                return builtins::display_value(self, c);
            }
            if let Some(val) = builtins::display_get(self, d, name) {
                return val;
            }
        }
        match self.lookup(o, name) {
            Some((_, p)) => self.read_prop(p, v),
            None => {
                if name != "__resolve" {
                    if let Some((_, p)) = self.lookup(o, "__resolve") {
                        let f = p.value;
                        if matches!(&f, Value::Object(fo) if self.is_function(*fo)) {
                            return self.call(&f, v.clone(), &[Value::from(name)]);
                        }
                    }
                }
                Value::Undefined
            }
        }
    }

    fn read_prop(&mut self, p: Prop, this: &Value) -> Value {
        match p.accessor {
            Some((g, _)) => {
                if g.is_undefined() {
                    Value::Undefined
                } else {
                    self.call(&g, this.clone(), &[])
                }
            }
            None => p.value,
        }
    }

    pub fn set(&mut self, v: &Value, name: &str, mut val: Value) {
        let Value::Object(o) = v else {
            return;
        };
        let o = *o;
        let Some(obj) = self.heap.get(o) else {
            return;
        };
        let kind = obj.kind.clone();
        if name == "__proto__" {
            let p = val.as_object();
            if let Some(x) = self.heap.get_mut(o) {
                x.proto = p;
            }
            return;
        }
        match &kind {
            Kind::Array(_) => {
                if name == "length" {
                    let n = self.to_number(&val);
                    let n = if n.is_finite() && n >= 0.0 {
                        n as usize
                    } else {
                        0
                    };
                    if let Some(Kind::Array(items)) = self.heap.get_mut(o).map(|x| &mut x.kind) {
                        items.resize(n.min(1 << 24), Value::Undefined);
                    }
                    return;
                }
                if let Some(i) = array_index(name) {
                    if i < (1 << 24) {
                        if let Some(Kind::Array(items)) = self.heap.get_mut(o).map(|x| &mut x.kind)
                        {
                            if i >= items.len() {
                                items.resize(i + 1, Value::Undefined);
                            }
                            items[i] = val;
                        }
                        return;
                    }
                }
            }
            Kind::Display(d) => {
                let d = *d;
                if builtins::display_set(self, d, name, &val) {
                    self.dirty = true;
                    return;
                }
            }
            Kind::Super { .. } => return,
            _ => {}
        }
        // Watchers see the change first and may replace the value.
        let watch = self.heap.get(o).and_then(|x| {
            x.watchers
                .iter()
                .find(|(n, _, _)| &**n == name)
                .map(|(_, f, d)| (f.clone(), d.clone()))
        });
        if let Some((f, data)) = watch {
            let old = self.get(v, name);
            let result = self.call(&f, v.clone(), &[Value::from(name), old, val.clone(), data]);
            // Execution-limit errors leave the incoming assignment in place, as Flash
            // does when a watcher fails, rather than treating the error as its return.
            if !self.script_stopped {
                val = result;
            }
        }
        // An accessor anywhere up the chain takes the write.
        if let Some((_, p)) = self.lookup(o, name) {
            if let Some((_, setter)) = p.accessor {
                if !setter.is_undefined() {
                    self.call(&setter, v.clone(), &[val]);
                }
                return;
            }
        }
        let Some(obj) = self.heap.get_mut(o) else {
            return;
        };
        if let Some(p) = obj.props.get_mut(name) {
            if p.flags & READ_ONLY == 0 {
                p.value = val;
            }
        } else {
            obj.props.insert(
                Str::from(name),
                Prop {
                    value: val,
                    flags: 0,
                    accessor: None,
                },
            );
        }
        // A text field bound to this variable shows the new value.
        if let Kind::Display(d) = kind {
            builtins::sync_bound_text(self, d, name);
        }
    }

    pub fn delete(&mut self, v: &Value, name: &str) -> bool {
        let Some(o) = v.as_object() else {
            return false;
        };
        let Some(obj) = self.heap.get_mut(o) else {
            return false;
        };
        match obj.props.get(name) {
            Some(p) if p.flags & object::DONT_DELETE != 0 => false,
            Some(_) => {
                obj.props.remove(name);
                true
            }
            None => false,
        }
    }

    /// Enumerable keys of `v` and its prototypes, as `for..in` sees them (most recently
    /// added first).
    pub fn keys(&mut self, v: &Value) -> Vec<Str> {
        let Some(o) = v.as_object() else {
            return Vec::new();
        };
        let mut out: Vec<Str> = Vec::new();
        let mut seen = HashSet::new();
        let mut cur = Some(o);
        let mut first = true;
        while let Some(c) = cur {
            let Some(obj) = self.heap.get(c) else { break };
            if first {
                match &obj.kind {
                    Kind::Array(items) => {
                        for i in (0..items.len()).rev() {
                            let k = Str::from(i.to_string());
                            seen.insert(k.clone());
                            out.push(k);
                        }
                    }
                    Kind::Display(d) => {
                        if let Some(dobj) = self.display.get(*d) {
                            for c in dobj.children.values().rev() {
                                if let Some(co) = self.display.get(*c) {
                                    if !co.name.is_empty() && seen.insert(co.name.clone()) {
                                        out.push(co.name.clone());
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            for (k, p) in obj.props.iter().rev() {
                if seen.insert(k.clone()) && p.flags & DONT_ENUM == 0 {
                    out.push(k.clone());
                }
            }
            first = false;
            cur = obj.proto;
        }
        out
    }

    // ---- calls --------------------------------------------------------------------------

    pub fn call(&mut self, f: &Value, this: Value, args: &[Value]) -> Value {
        self.call_with_base(f, this, args, None)
    }

    /// Calls `f`; `base` is the prototype the method was found on (for `super`).
    pub fn call_with_base(
        &mut self,
        f: &Value,
        this: Value,
        args: &[Value],
        base: Option<ObjId>,
    ) -> Value {
        let Value::Object(fo) = f else {
            return Value::Undefined;
        };
        let func = match self.heap.get(*fo).map(|x| &x.kind) {
            Some(Kind::Function(func)) => func.clone(),
            _ => return Value::Undefined,
        };
        if !self.enter_script() {
            return Value::Undefined;
        }
        if self.call_depth > 250 {
            self.stop_script("call stack overflow");
            self.leave_script();
            return Value::Undefined;
        }
        self.call_depth += 1;
        let r = match func {
            Func::Native(n) => n(self, this, args),
            Func::Code(code) => interp::call_code(self, *fo, &code, this, args, base),
        };
        self.call_depth -= 1;
        self.leave_script();
        r
    }

    /// Calls the method `name` of `obj` (found on its prototype chain).
    pub fn call_method(&mut self, obj: &Value, name: &str, args: &[Value]) -> Value {
        let base = self.object_for(obj).and_then(|o| {
            if let Some(Kind::Super { proto, .. }) = self.heap.get(o).map(|x| &x.kind) {
                return proto.and_then(|p| self.lookup(p, name)).map(|(h, _)| h);
            }
            self.lookup(o, name).map(|(h, _)| h)
        });
        let f = self.get(obj, name);
        let this = match obj {
            Value::Object(o) => match self.heap.get(*o).map(|x| &x.kind) {
                Some(Kind::Super { this, .. }) => this.clone(),
                _ => obj.clone(),
            },
            _ => obj.clone(),
        };
        if !matches!(&f, Value::Object(fo) if self.is_function(*fo)) {
            let on = match self.display_of(obj) {
                Some(d) => self.display.path(d, true),
                None => obj.type_name().to_string(),
            };
            self.warn_once(format!("call to missing method {name} on {on}"));
            return Value::Undefined;
        }
        self.call_with_base(&f, this, args, base)
    }

    /// `new f(args)`.
    pub fn construct(&mut self, f: &Value, args: &[Value]) -> Value {
        let Value::Object(fo) = f else {
            return Value::Undefined;
        };
        let proto = self.get(f, "prototype").as_object();
        let obj = self.alloc(proto, Kind::Plain);
        self.define(obj, "__constructor__", f.clone(), DONT_ENUM);
        let r = self.call_with_base(f, Value::Object(obj), args, proto);
        let _ = fo;
        match r {
            // Native constructors may hand back the object they made.
            Value::Object(o) if o != obj => Value::Object(o),
            _ => Value::Object(obj),
        }
    }

    pub fn make_function_object(&mut self, func: Func) -> ObjId {
        let f = self.alloc(Some(self.protos.function), Kind::Function(func));
        let proto = self.new_object();
        self.define(proto, "constructor", Value::Object(f), DONT_ENUM);
        self.define(f, "prototype", Value::Object(proto), DONT_ENUM);
        f
    }

    // ---- paths --------------------------------------------------------------------------

    pub fn display_object(&mut self, d: DisplayId) -> Value {
        builtins::display_value(self, d)
    }

    /// Resolves a dotted or slash path from `start` (a clip) or the scope chain.
    pub fn resolve_path(&mut self, path: &str, scope: &Rc<Scope>, target: DisplayId) -> Value {
        let path = path.trim();
        if path.is_empty() {
            return self.display_object(target);
        }
        if path.starts_with('/') || path.contains('/') {
            let mut cur = if path.starts_with('/') {
                self.root
            } else {
                target
            };
            for part in path.split('/').filter(|s| !s.is_empty()) {
                if part == ".." {
                    cur = self.display.get(cur).and_then(|o| o.parent).unwrap_or(cur);
                    continue;
                }
                match self.display.child_named(cur, part) {
                    Some(c) => cur = c,
                    None => return Value::Undefined,
                }
            }
            return self.display_object(cur);
        }
        let mut parts = path.split('.');
        let first = parts.next().unwrap_or("");
        let mut v = self.get_variable_simple(first, scope, target);
        for p in parts {
            if v.is_nullish() {
                return Value::Undefined;
            }
            v = self.get(&v, p);
        }
        v
    }

    pub fn get_variable_simple(
        &mut self,
        name: &str,
        scope: &Rc<Scope>,
        target: DisplayId,
    ) -> Value {
        match name {
            "_global" => return Value::Object(self.global),
            "_root" | "_level0" => return self.display_object(self.root),
            "this" => {
                // Found in the activation when a function set it, else the target.
                let mut s = Some(scope.clone());
                while let Some(sc) = s {
                    if sc.kind == ScopeKind::Local {
                        if let Some(p) = self.heap.get(sc.object).and_then(|o| o.props.get("this"))
                        {
                            return p.value.clone();
                        }
                    }
                    s = sc.parent.clone();
                }
                return self.display_object(target);
            }
            _ => {}
        }
        let mut s = Some(scope.clone());
        while let Some(sc) = s {
            let ov = Value::Object(sc.object);
            if self.has_property(&ov, name) {
                return self.get(&ov, name);
            }
            s = sc.parent.clone();
        }
        Value::Undefined
    }

    /// The display object a value names (a clip object, or a path string).
    pub fn as_display(
        &mut self,
        v: &Value,
        scope: &Rc<Scope>,
        target: DisplayId,
    ) -> Option<DisplayId> {
        match v {
            Value::Object(o) => match self.heap.get(*o).map(|x| &x.kind) {
                Some(Kind::Display(d)) => Some(*d).filter(|d| self.display.get(*d).is_some()),
                _ => None,
            },
            Value::String(s) => {
                let r = self.resolve_path(&s.clone(), scope, target);
                if let Value::Object(_) = r {
                    self.as_display(&r, scope, target)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub fn display_of(&self, v: &Value) -> Option<DisplayId> {
        match v {
            Value::Object(o) => match self.heap.get(*o).map(|x| &x.kind) {
                Some(Kind::Display(d)) => Some(*d).filter(|d| self.display.get(*d).is_some()),
                _ => None,
            },
            _ => None,
        }
    }

    // ---- host values --------------------------------------------------------------------

    pub fn to_host(&mut self, v: &Value, depth: u32) -> HostValue {
        match v {
            Value::Undefined => HostValue::Undefined,
            Value::Null => HostValue::Null,
            Value::Bool(b) => HostValue::Bool(*b),
            Value::Number(n) => HostValue::Number(*n),
            Value::String(s) => HostValue::String(s.to_string()),
            Value::Object(o) => {
                if depth > 16 {
                    return HostValue::Undefined;
                }
                match self.heap.get(*o).map(|x| x.kind.clone()) {
                    Some(Kind::Array(items)) => {
                        HostValue::List(items.iter().map(|x| self.to_host(x, depth + 1)).collect())
                    }
                    Some(Kind::Boxed(b)) => self.to_host(&b, depth + 1),
                    Some(Kind::Function(_)) => HostValue::Undefined,
                    _ => {
                        let mut m = std::collections::BTreeMap::new();
                        for k in self.keys(v) {
                            let x = self.get(v, &k);
                            if matches!(&x, Value::Object(f) if self.is_function(*f)) {
                                continue;
                            }
                            let h = self.to_host(&x, depth + 1);
                            m.insert(k.to_string(), h);
                        }
                        HostValue::Object(m)
                    }
                }
            }
        }
    }

    pub fn from_host(&mut self, v: &HostValue) -> Value {
        match v {
            HostValue::Undefined => Value::Undefined,
            HostValue::Null => Value::Null,
            HostValue::Bool(b) => Value::Bool(*b),
            HostValue::Number(n) => Value::Number(*n),
            HostValue::String(s) => Value::from(s.as_str()),
            HostValue::List(l) => {
                let items = l.iter().map(|x| self.from_host(x)).collect();
                Value::Object(self.new_array(items))
            }
            HostValue::Object(m) => {
                let o = self.new_object();
                for (k, x) in m {
                    let val = self.from_host(x);
                    self.define(o, k, val, 0);
                }
                Value::Object(o)
            }
        }
    }
}

/// `name` as an array index, if it is one.
pub fn array_index(name: &str) -> Option<usize> {
    if name.is_empty() || name.len() > 9 || !name.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if name.len() > 1 && name.starts_with('0') {
        return None;
    }
    name.parse().ok()
}
