//! The bytecode loop.

use std::rc::Rc;

use super::object::{CodeFn, DONT_ENUM, Func, Kind};
use super::value::{ObjId, Str, Value, to_int32};
use super::{Scope, ScopeKind, Vm};
use crate::display::DisplayId;
use crate::swf::Actions;
use crate::swf::read::Reader;

struct Frame {
    code: Actions,
    pc: usize,
    end: usize,
    pool: Rc<Vec<Str>>,
    regs: Vec<Value>,
    scope: Rc<Scope>,
    target: DisplayId,
    stack: Vec<Value>,
}

impl Frame {
    fn pop(&mut self) -> Value {
        self.stack.pop().unwrap_or_default()
    }
}

/// Runs timeline code (a frame's actions, an init action, a clip event) on `target`.
pub fn run_actions(vm: &mut Vm, code: &Actions, target: DisplayId) -> Value {
    if vm.display.get(target).is_none() {
        return Value::Undefined;
    }
    let tobj = vm.display_object(target);
    let Some(tobj) = tobj.as_object() else {
        return Value::Undefined;
    };
    let global = Rc::new(Scope {
        object: vm.global,
        kind: ScopeKind::Global,
        parent: None,
    });
    let scope = Rc::new(Scope {
        object: tobj,
        kind: ScopeKind::Target,
        parent: Some(global),
    });
    let mut f = Frame {
        code: code.clone(),
        pc: 0,
        end: code.len(),
        pool: Rc::new(Vec::new()),
        regs: vec![Value::Undefined; 4],
        scope,
        target,
        stack: Vec::new(),
    };
    if !vm.enter_script() {
        return Value::Undefined;
    }
    let result = exec(vm, &mut f);
    vm.leave_script();
    result
}

/// Calls a bytecode function.
pub fn call_code(
    vm: &mut Vm,
    fobj: ObjId,
    code: &Rc<CodeFn>,
    this: Value,
    args: &[Value],
    base: Option<ObjId>,
) -> Value {
    if vm.trace_calls {
        let n = match args.first() {
            Some(Value::String(s)) => s.to_string(),
            Some(Value::Number(x)) => x.to_string(),
            Some(v) => v.type_name().to_string(),
            None => String::new(),
        };
        let name = vm
            .fn_names
            .get(&fobj)
            .cloned()
            .unwrap_or_else(|| code.name.clone());
        vm.log
            .push(format!("call {name} ({} args) {n}", args.len()));
    }
    // Called without a known holder (a plain call, `apply`, `call`): `super` starts from
    // `this`'s own prototype, as Flash does.
    let base = base.or_else(|| {
        this.as_object()
            .and_then(|o| vm.heap.get(o))
            .and_then(|o| o.proto)
    });
    let act = vm.alloc(None, Kind::Plain);
    let target = code
        .target
        .filter(|t| vm.display.get(*t).is_some())
        .unwrap_or(vm.root);
    let scope = Rc::new(Scope {
        object: act,
        kind: ScopeKind::Local,
        parent: Some(code.scope.clone()),
    });
    let nregs = code.v2.map(|(n, _)| n as usize).unwrap_or(4).max(4);
    let mut regs = vec![Value::Undefined; nregs];
    let arguments = {
        let a = vm.new_array(args.to_vec());
        vm.define(a, "callee", Value::Object(fobj), DONT_ENUM);
        Value::Object(a)
    };
    let super_obj = || -> Value { Value::Undefined };
    let _ = super_obj;
    let make_super = |vm: &mut Vm| -> Value {
        let proto = base.and_then(|b| vm.heap.get(b).and_then(|o| o.proto));
        let ctor = base
            .and_then(|b| {
                vm.heap
                    .get(b)
                    .and_then(|o| o.props.get("__constructor__"))
                    .map(|p| p.value.clone())
            })
            .unwrap_or_default();
        Value::Object(vm.alloc(
            None,
            Kind::Super {
                this: this.clone(),
                proto,
                ctor,
            },
        ))
    };
    match code.v2 {
        None => {
            vm.define(act, "this", this.clone(), DONT_ENUM);
            vm.define(act, "arguments", arguments, DONT_ENUM);
            let s = make_super(vm);
            vm.define(act, "super", s, DONT_ENUM);
        }
        Some((_, flags)) => {
            let mut r = 1usize;
            let mut put = |regs: &mut Vec<Value>, v: Value| {
                if r < regs.len() {
                    regs[r] = v;
                }
                r += 1;
            };
            // PreloadThis 0x01, SuppressThis 0x02, PreloadArguments 0x04,
            // SuppressArguments 0x08, PreloadSuper 0x10, SuppressSuper 0x20,
            // PreloadRoot 0x40, PreloadParent 0x80, PreloadGlobal 0x100.
            if flags & 0x01 != 0 {
                put(&mut regs, this.clone());
            } else if flags & 0x02 == 0 {
                vm.define(act, "this", this.clone(), DONT_ENUM);
            }
            if flags & 0x04 != 0 {
                put(&mut regs, arguments);
            } else if flags & 0x08 == 0 {
                vm.define(act, "arguments", arguments, DONT_ENUM);
            }
            if flags & 0x10 != 0 {
                let s = make_super(vm);
                put(&mut regs, s);
            } else if flags & 0x20 == 0 {
                let s = make_super(vm);
                vm.define(act, "super", s, DONT_ENUM);
            }
            if flags & 0x40 != 0 {
                let root = vm.display_object(vm.root);
                put(&mut regs, root);
            }
            if flags & 0x80 != 0 {
                let p = vm
                    .display
                    .get(target)
                    .and_then(|o| o.parent)
                    .map(|p| vm.display_object(p))
                    .unwrap_or_default();
                put(&mut regs, p);
            }
            if flags & 0x100 != 0 {
                put(&mut regs, Value::Object(vm.global));
            }
        }
    }
    for (i, (reg, name)) in code.params.iter().enumerate() {
        let v = args.get(i).cloned().unwrap_or_default();
        if *reg > 0 && (*reg as usize) < regs.len() {
            regs[*reg as usize] = v;
        } else {
            vm.define(act, name, v, 0);
        }
    }
    let mut f = Frame {
        code: code.code.clone(),
        pc: code.start,
        end: code.end.min(code.code.len()),
        pool: code.pool.clone(),
        regs,
        scope,
        target,
        stack: Vec::new(),
    };
    exec(vm, &mut f)
}

fn get_variable(vm: &mut Vm, f: &Frame, name: &str) -> Value {
    if name.contains('.') || name.contains('/') || name.contains(':') {
        // `path:var` names a variable of a clip.
        if let Some((path, var)) = name.rsplit_once(':') {
            let clip = vm.resolve_path(path, &f.scope, f.target);
            return vm.get(&clip, var);
        }
        return vm.resolve_path(name, &f.scope, f.target);
    }
    vm.get_variable_simple(name, &f.scope, f.target)
}

fn set_variable(vm: &mut Vm, f: &Frame, name: &str, v: Value) {
    if let Some((path, var)) = name.rsplit_once(':').or_else(|| name.rsplit_once('.')) {
        let obj = vm.resolve_path(path, &f.scope, f.target);
        vm.set(&obj, var, v);
        return;
    }
    let mut s = Some(f.scope.clone());
    while let Some(sc) = s {
        let ov = Value::Object(sc.object);
        if sc.kind == ScopeKind::Target
            || sc.kind == ScopeKind::Global
            || vm.has_property(&ov, name)
        {
            vm.set(&ov, name, v);
            return;
        }
        s = sc.parent.clone();
    }
}

fn abstract_eq(vm: &mut Vm, a: &Value, b: &Value) -> bool {
    use Value::*;
    match (a, b) {
        (Undefined | Null, Undefined | Null) => true,
        (Undefined | Null, _) | (_, Undefined | Null) => false,
        (Number(x), Number(y)) => x == y,
        (String(x), String(y)) => x == y,
        (Bool(x), Bool(y)) => x == y,
        (Object(x), Object(y)) => x == y,
        (Number(x), String(_)) => *x == vm.to_number(b),
        (String(_), Number(y)) => vm.to_number(a) == *y,
        (Bool(_), _) => {
            let n = Number(vm.to_number(a));
            abstract_eq(vm, &n, b)
        }
        (_, Bool(_)) => {
            let n = Number(vm.to_number(b));
            abstract_eq(vm, a, &n)
        }
        (Object(_), _) => {
            let p = vm.to_primitive(a, true);
            if matches!(p, Object(_)) {
                false
            } else {
                abstract_eq(vm, &p, b)
            }
        }
        (_, Object(_)) => {
            let p = vm.to_primitive(b, true);
            if matches!(p, Object(_)) {
                false
            } else {
                abstract_eq(vm, a, &p)
            }
        }
    }
}

/// `a < b`: Some(result), or None when a NaN is involved.
fn less(vm: &mut Vm, a: &Value, b: &Value) -> Option<bool> {
    let pa = vm.to_primitive(a, true);
    let pb = vm.to_primitive(b, true);
    if let (Value::String(x), Value::String(y)) = (&pa, &pb) {
        return Some(x.encode_utf16().lt(y.encode_utf16()));
    }
    let x = vm.to_number(&pa);
    let y = vm.to_number(&pb);
    if x.is_nan() || y.is_nan() {
        None
    } else {
        Some(x < y)
    }
}

fn instance_of(vm: &mut Vm, obj: &Value, ctor: &Value) -> bool {
    let Some(o) = obj.as_object() else {
        return false;
    };
    let Some(proto) = vm.get(ctor, "prototype").as_object() else {
        return false;
    };
    let mut cur = vm.heap.get(o).and_then(|x| x.proto);
    let mut guard = 0;
    while let Some(c) = cur {
        if c == proto {
            return true;
        }
        cur = vm.heap.get(c).and_then(|x| x.proto);
        guard += 1;
        if guard > 256 {
            break;
        }
    }
    false
}

fn type_of(vm: &Vm, v: &Value) -> &'static str {
    match v {
        Value::Object(o) => match vm.heap.get(*o).map(|x| &x.kind) {
            Some(Kind::Function(_)) => "function",
            Some(Kind::Display(d)) => match vm.display.get(*d).map(|x| &x.kind) {
                Some(crate::display::Kind::Clip(_)) => "movieclip",
                _ => "object",
            },
            _ => "object",
        },
        v => v.type_name(),
    }
}

/// Reads a function definition at `r`'s position (after the action header).
fn read_function(
    vm: &mut Vm,
    f: &Frame,
    body: &[u8],
    v2: bool,
    code_start: usize,
) -> Option<(Str, Rc<CodeFn>, usize)> {
    let mut r = Reader::new(body);
    let name = r.cstr().ok()?;
    let n = r.u16().ok()? as usize;
    let mut params = Vec::with_capacity(n);
    let mut v2info = None;
    if v2 {
        let regs = r.u8().ok()?;
        let flags = r.u16().ok()?;
        v2info = Some((regs, flags));
        for _ in 0..n {
            let reg = r.u8().ok()?;
            params.push((reg, Str::from(r.cstr().ok()?)));
        }
    } else {
        for _ in 0..n {
            params.push((0u8, Str::from(r.cstr().ok()?)));
        }
    }
    let size = r.u16().ok()? as usize;
    let _ = vm;
    Some((
        Str::from(name.as_str()),
        Rc::new(CodeFn {
            name: Str::from(name),
            code: f.code.clone(),
            start: code_start,
            end: code_start + size,
            params,
            v2: v2info,
            pool: f.pool.clone(),
            scope: f.scope.clone(),
            target: Some(f.target),
        }),
        size,
    ))
}

fn property_name(i: u32) -> &'static str {
    const NAMES: [&str; 22] = [
        "_x",
        "_y",
        "_xscale",
        "_yscale",
        "_currentframe",
        "_totalframes",
        "_alpha",
        "_visible",
        "_width",
        "_height",
        "_rotation",
        "_target",
        "_framesloaded",
        "_name",
        "_droptarget",
        "_url",
        "_highquality",
        "_focusrect",
        "_soundbuftime",
        "_quality",
        "_xmouse",
        "_ymouse",
    ];
    NAMES.get(i as usize).copied().unwrap_or("")
}

fn exec(vm: &mut Vm, f: &mut Frame) -> Value {
    let old = vm.native_target.replace(f.target);
    let result = exec_inner(vm, f);
    vm.native_target = old;
    result
}

fn exec_inner(vm: &mut Vm, f: &mut Frame) -> Value {
    let code = f.code.clone();
    while f.pc < f.end && !vm.script_stopped {
        vm.budget += 1;
        if vm.budget > super::OP_BUDGET {
            vm.stop_script("script ran too long; stopped");
            return Value::Undefined;
        }
        let op = code[f.pc];
        f.pc += 1;
        let mut body: &[u8] = &[];
        if op >= 0x80 {
            let Some(len) = code
                .get(f.pc..f.pc + 2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
            else {
                break;
            };
            f.pc += 2;
            body = code.get(f.pc..f.pc + len).unwrap_or(&[]);
            f.pc += len;
        }
        match op {
            0x00 => break,
            0x04 => crate::player::clip_step(vm, f.target, 1),
            0x05 => crate::player::clip_step(vm, f.target, -1),
            0x06 => crate::player::set_playing(vm, f.target, true),
            0x07 => crate::player::set_playing(vm, f.target, false),
            0x08 | 0x09 => {}
            0x0A..=0x0F => {
                let b = f.pop();
                let a = f.pop();
                let (x, y) = (vm.to_number(&a), vm.to_number(&b));
                f.stack.push(match op {
                    0x0A => Value::Number(x + y),
                    0x0B => Value::Number(x - y),
                    0x0C => Value::Number(x * y),
                    0x0D => Value::Number(x / y),
                    0x0E => Value::Bool(x == y),
                    _ => Value::Bool(x < y),
                });
            }
            0x10 | 0x11 => {
                let b = f.pop();
                let a = f.pop();
                let (x, y) = (vm.to_bool(&a), vm.to_bool(&b));
                f.stack
                    .push(Value::Bool(if op == 0x10 { x && y } else { x || y }));
            }
            0x12 => {
                let a = f.pop();
                f.stack.push(Value::Bool(!vm.to_bool(&a)));
            }
            0x13 | 0x29 | 0x68 => {
                let b = f.pop();
                let a = f.pop();
                let (x, y) = (vm.to_str(&a), vm.to_str(&b));
                f.stack.push(Value::Bool(match op {
                    0x13 => x == y,
                    0x29 => x < y,
                    _ => x > y,
                }));
            }
            0x14 | 0x31 => {
                let a = f.pop();
                let s = vm.to_str(&a);
                f.stack.push(Value::Number(s.chars().count() as f64));
            }
            0x15 | 0x35 => {
                let n = f.pop();
                let i = f.pop();
                let s = f.pop();
                let s = vm.to_str(&s);
                let i = (vm.to_number(&i) as i64 - 1).max(0) as usize;
                let n = vm.to_number(&n);
                let n = if n < 0.0 { usize::MAX } else { n as usize };
                f.stack
                    .push(Value::from(s.chars().skip(i).take(n).collect::<String>()));
            }
            0x17 => {
                f.pop();
            }
            0x18 => {
                let a = f.pop();
                let n = vm.to_number(&a);
                f.stack
                    .push(Value::Number(if n.is_nan() { 0.0 } else { n.trunc() }));
            }
            0x1C => {
                let n = f.pop();
                let name = vm.to_str(&n);
                let v = get_variable(vm, f, &name);
                f.stack.push(v);
            }
            0x1D => {
                let v = f.pop();
                let n = f.pop();
                let name = vm.to_str(&n);
                set_variable(vm, f, &name, v);
            }
            0x20 => {
                f.pop();
                vm.warn_once("SetTarget2 is not supported".into());
            }
            0x21 => {
                let b = f.pop();
                let a = f.pop();
                let mut s = vm.to_str(&a).to_string();
                s.push_str(&vm.to_str(&b));
                f.stack.push(Value::from(s));
            }
            0x22 => {
                let i = f.pop();
                let t = f.pop();
                let idx = vm.to_number(&i) as u32;
                let clip = if matches!(&t, Value::String(s) if s.is_empty()) {
                    Some(f.target)
                } else {
                    vm.as_display(&t, &f.scope, f.target)
                };
                let v = match clip {
                    Some(c) => {
                        let o = vm.display_object(c);
                        vm.get(&o, property_name(idx))
                    }
                    None => Value::Undefined,
                };
                f.stack.push(v);
            }
            0x23 => {
                let v = f.pop();
                let i = f.pop();
                let t = f.pop();
                let idx = vm.to_number(&i) as u32;
                let clip = if matches!(&t, Value::String(s) if s.is_empty()) {
                    Some(f.target)
                } else {
                    vm.as_display(&t, &f.scope, f.target)
                };
                if let Some(c) = clip {
                    let o = vm.display_object(c);
                    vm.set(&o, property_name(idx), v);
                }
            }
            0x24 => {
                let depth = f.pop();
                let target = f.pop();
                let source = f.pop();
                let _ = (depth, target, source);
                vm.warn_once("duplicateMovieClip is not supported".into());
            }
            0x25 => {
                let t = f.pop();
                if let Some(c) = vm.as_display(&t, &f.scope, f.target) {
                    crate::player::remove_clip(vm, c);
                }
            }
            0x26 => {
                let v = f.pop();
                let s = vm.to_str(&v);
                log::debug!("gfx {} trace: {s}", vm.name);
                vm.log.push(format!("trace: {s}"));
            }
            0x27 => {
                let t = f.pop();
                let lock = f.pop();
                let lock = vm.to_bool(&lock);
                let constrain = f.pop();
                if vm.to_bool(&constrain) {
                    for _ in 0..4 {
                        f.pop();
                    }
                }
                if let Some(c) = vm.as_display(&t, &f.scope, f.target) {
                    crate::player::start_drag(vm, c, lock);
                }
            }
            0x28 => vm.dragging = None,
            0x2A => {
                let v = f.pop();
                let s = vm.to_str(&v);
                vm.warn_once(format!("uncaught throw: {s}"));
                return Value::Undefined;
            }
            0x2B => {
                let o = f.pop();
                let c = f.pop();
                let r = if instance_of(vm, &o, &c) {
                    o
                } else {
                    Value::Null
                };
                f.stack.push(r);
            }
            0x2C => {
                let _ctor = f.pop();
                let n = f.pop();
                let n = vm.to_number(&n) as usize;
                for _ in 0..n.min(f.stack.len()) {
                    f.pop();
                }
            }
            0x30 => {
                let m = f.pop();
                let m = vm.to_number(&m).max(0.0) as u64;
                let r = if m == 0 {
                    0.0
                } else {
                    (builtins_random(vm) * m as f64).floor()
                };
                f.stack.push(Value::Number(r));
            }
            0x32 | 0x36 => {
                let a = f.pop();
                let s = vm.to_str(&a);
                f.stack.push(Value::Number(
                    s.chars().next().map(|c| c as u32 as f64).unwrap_or(0.0),
                ));
            }
            0x33 | 0x37 => {
                let a = f.pop();
                let n = vm.to_number(&a) as u32;
                f.stack.push(Value::from(
                    char::from_u32(n).map(String::from).unwrap_or_default(),
                ));
            }
            0x34 => f.stack.push(Value::Number(vm.time_ms.floor())),
            0x3A => {
                let n = f.pop();
                let o = f.pop();
                let name = vm.to_str(&n);
                let r = vm.delete(&o, &name);
                f.stack.push(Value::Bool(r));
            }
            0x3B => {
                let n = f.pop();
                let name = vm.to_str(&n);
                let mut s = Some(f.scope.clone());
                let mut r = false;
                while let Some(sc) = s {
                    let ov = Value::Object(sc.object);
                    if vm.has_property(&ov, &name) {
                        r = vm.delete(&ov, &name);
                        break;
                    }
                    s = sc.parent.clone();
                }
                f.stack.push(Value::Bool(r));
            }
            0x3C => {
                let v = f.pop();
                let n = f.pop();
                let name = vm.to_str(&n);
                let o = Value::Object(f.scope.object);
                if f.scope.kind == ScopeKind::Local {
                    vm.define(f.scope.object, &name, v, 0);
                } else {
                    vm.set(&o, &name, v);
                }
            }
            0x41 => {
                let n = f.pop();
                let name = vm.to_str(&n);
                let o = Value::Object(f.scope.object);
                if !vm.has_property(&o, &name) {
                    vm.define(f.scope.object, &name, Value::Undefined, 0);
                }
            }
            0x3D => {
                let n = f.pop();
                let argc = f.pop();
                let argc = vm.to_number(&argc).max(0.0) as usize;
                let args: Vec<Value> = (0..argc.min(f.stack.len())).map(|_| f.pop()).collect();
                let name = vm.to_str(&n);
                let func = get_variable(vm, f, &name);
                let r = if matches!(&func, Value::Object(o) if vm.is_function(*o)) {
                    let this = vm.display_object(f.target);
                    vm.call(&func, this, &args)
                } else {
                    vm.warn_once(format!("call to missing function {name}"));
                    Value::Undefined
                };
                f.stack.push(r);
            }
            0x3E => return f.pop(),
            0x3F => {
                let b = f.pop();
                let a = f.pop();
                let (x, y) = (vm.to_number(&a), vm.to_number(&b));
                f.stack.push(Value::Number(x % y));
            }
            0x40 => {
                let n = f.pop();
                let argc = f.pop();
                let argc = vm.to_number(&argc).max(0.0) as usize;
                let args: Vec<Value> = (0..argc.min(f.stack.len())).map(|_| f.pop()).collect();
                let name = vm.to_str(&n);
                let ctor = get_variable(vm, f, &name);
                let r = if matches!(&ctor, Value::Object(o) if vm.is_function(*o)) {
                    vm.construct(&ctor, &args)
                } else {
                    vm.warn_once(format!("new of missing class {name}"));
                    Value::Undefined
                };
                f.stack.push(r);
            }
            0x42 => {
                let n = f.pop();
                let n = vm.to_number(&n).max(0.0) as usize;
                let items: Vec<Value> = (0..n.min(f.stack.len())).map(|_| f.pop()).collect();
                let a = vm.new_array(items);
                f.stack.push(Value::Object(a));
            }
            0x43 => {
                let n = f.pop();
                let n = vm.to_number(&n).max(0.0) as usize;
                let o = vm.new_object();
                for _ in 0..n.min(f.stack.len() / 2) {
                    let v = f.pop();
                    let k = f.pop();
                    let k = vm.to_str(&k);
                    vm.set(&Value::Object(o), &k, v);
                }
                f.stack.push(Value::Object(o));
            }
            0x44 => {
                let v = f.pop();
                f.stack.push(Value::from(type_of(vm, &v)));
            }
            0x45 => {
                let v = f.pop();
                let r = match vm.display_of(&v) {
                    Some(d) => Value::from(vm.display.path(d, true)),
                    None => Value::Undefined,
                };
                f.stack.push(r);
            }
            0x46 | 0x55 => {
                let v = f.pop();
                let obj = if op == 0x46 {
                    let name = vm.to_str(&v);
                    get_variable(vm, f, &name)
                } else {
                    v
                };
                let keys = vm.keys(&obj);
                f.stack.push(Value::Null);
                for k in keys {
                    f.stack.push(Value::String(k));
                }
            }
            0x47 => {
                let b = f.pop();
                let a = f.pop();
                let pa = vm.to_primitive(&a, false);
                let pb = vm.to_primitive(&b, false);
                if matches!(pa, Value::String(_)) || matches!(pb, Value::String(_)) {
                    let mut s = vm.to_str(&pa).to_string();
                    s.push_str(&vm.to_str(&pb));
                    f.stack.push(Value::from(s));
                } else {
                    let (x, y) = (vm.to_number(&pa), vm.to_number(&pb));
                    f.stack.push(Value::Number(x + y));
                }
            }
            0x48 => {
                let b = f.pop();
                let a = f.pop();
                let r = less(vm, &a, &b);
                f.stack.push(r.map(Value::Bool).unwrap_or(Value::Undefined));
            }
            0x67 => {
                let b = f.pop();
                let a = f.pop();
                let r = less(vm, &b, &a);
                f.stack.push(r.map(Value::Bool).unwrap_or(Value::Undefined));
            }
            0x49 => {
                let b = f.pop();
                let a = f.pop();
                let r = abstract_eq(vm, &a, &b);
                f.stack.push(Value::Bool(r));
            }
            0x66 => {
                let b = f.pop();
                let a = f.pop();
                f.stack.push(Value::Bool(a == b));
            }
            0x4A => {
                let a = f.pop();
                let n = vm.to_number(&a);
                f.stack.push(Value::Number(n));
            }
            0x4B => {
                let a = f.pop();
                let s = vm.to_str(&a);
                f.stack.push(Value::String(s));
            }
            0x4C => {
                let v = f.stack.last().cloned().unwrap_or_default();
                f.stack.push(v);
            }
            0x4D => {
                let b = f.pop();
                let a = f.pop();
                f.stack.push(b);
                f.stack.push(a);
            }
            0x4E => {
                let n = f.pop();
                let o = f.pop();
                let name = vm.to_str(&n);
                let v = vm.get(&o, &name);
                f.stack.push(v);
            }
            0x4F => {
                let v = f.pop();
                let n = f.pop();
                let o = f.pop();
                let name = vm.to_str(&n);
                if vm.trace_calls {
                    if let Value::Object(fo) = &v {
                        if vm.is_function(*fo) {
                            vm.fn_names.entry(*fo).or_insert_with(|| name.clone());
                        }
                    }
                }
                vm.set(&o, &name, v);
            }
            0x50 | 0x51 => {
                let a = f.pop();
                let n = vm.to_number(&a);
                f.stack
                    .push(Value::Number(if op == 0x50 { n + 1.0 } else { n - 1.0 }));
            }
            0x52 => {
                let n = f.pop();
                let o = f.pop();
                let argc = f.pop();
                let argc = vm.to_number(&argc).max(0.0) as usize;
                let args: Vec<Value> = (0..argc.min(f.stack.len())).map(|_| f.pop()).collect();
                let r = match &n {
                    Value::Undefined => call_self(vm, &o, &args),
                    Value::String(s) if s.is_empty() => call_self(vm, &o, &args),
                    _ => {
                        let name = vm.to_str(&n);
                        if o.is_nullish() {
                            vm.warn_once(format!("method {name} called on undefined"));
                            Value::Undefined
                        } else {
                            vm.call_method(&o, &name, &args)
                        }
                    }
                };
                f.stack.push(r);
            }
            0x53 => {
                let n = f.pop();
                let o = f.pop();
                let argc = f.pop();
                let argc = vm.to_number(&argc).max(0.0) as usize;
                let args: Vec<Value> = (0..argc.min(f.stack.len())).map(|_| f.pop()).collect();
                let ctor = match &n {
                    Value::Undefined => o.clone(),
                    Value::String(s) if s.is_empty() => o.clone(),
                    _ => {
                        let name = vm.to_str(&n);
                        vm.get(&o, &name)
                    }
                };
                let r = if matches!(&ctor, Value::Object(c) if vm.is_function(*c)) {
                    vm.construct(&ctor, &args)
                } else {
                    let name = vm.to_str(&n);
                    vm.warn_once(format!("new of missing class {name}"));
                    Value::Undefined
                };
                f.stack.push(r);
            }
            0x54 => {
                let c = f.pop();
                let o = f.pop();
                let r = instance_of(vm, &o, &c);
                f.stack.push(Value::Bool(r));
            }
            0x60..=0x65 => {
                let b = f.pop();
                let a = f.pop();
                let (x, y) = (vm.to_number(&a), vm.to_number(&b));
                let (x, y) = (to_int32(x), to_int32(y));
                let r = match op {
                    0x60 => (x & y) as f64,
                    0x61 => (x | y) as f64,
                    0x62 => (x ^ y) as f64,
                    0x63 => x.wrapping_shl(y as u32 & 31) as f64,
                    0x64 => x.wrapping_shr(y as u32 & 31) as f64,
                    _ => (x as u32).wrapping_shr(y as u32 & 31) as f64,
                };
                f.stack.push(Value::Number(r));
            }
            0x69 => {
                let sup = f.pop();
                let sub = f.pop();
                let sp = vm.get(&sup, "prototype").as_object();
                let proto = vm.alloc(sp, Kind::Plain);
                vm.define(proto, "__constructor__", sup, DONT_ENUM);
                vm.set(&sub, "prototype", Value::Object(proto));
            }
            0x81 => {
                let fr = u16::from_le_bytes([
                    body.first().copied().unwrap_or(0),
                    body.get(1).copied().unwrap_or(0),
                ]);
                crate::player::goto(vm, f.target, fr, false);
            }
            0x83 => {
                let mut r = Reader::new(body);
                let url = r.cstr().unwrap_or_default();
                let target = r.cstr().unwrap_or_default();
                get_url(vm, f, &url, &Value::from(target), 0);
            }
            0x87 => {
                let r = body.first().copied().unwrap_or(0) as usize;
                let v = f.stack.last().cloned().unwrap_or_default();
                if r < f.regs.len() {
                    f.regs[r] = v;
                }
            }
            0x88 => {
                let mut r = Reader::new(body);
                let n = r.u16().unwrap_or(0);
                let mut pool = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    match r.cstr() {
                        Ok(s) => pool.push(Str::from(s)),
                        Err(_) => break,
                    }
                }
                f.pool = Rc::new(pool);
            }
            0x8A | 0x8D => {}
            0x8B => vm.warn_once("SetTarget is not supported".into()),
            0x8C => {
                let label = Reader::new(body).cstr().unwrap_or_default();
                crate::player::goto_label(vm, f.target, &label, false);
            }
            0x8E | 0x9B => {
                let start = f.pc;
                if let Some((name, def, size)) = read_function(vm, f, body, op == 0x8E, start) {
                    f.pc = (start + size).min(f.end);
                    let fobj = vm.make_function_object(Func::Code(def));
                    if name.is_empty() {
                        f.stack.push(Value::Object(fobj));
                    } else {
                        let o = Value::Object(f.scope.object);
                        if f.scope.kind == ScopeKind::Local {
                            vm.define(f.scope.object, &name, Value::Object(fobj), 0);
                        } else {
                            vm.set(&o, &name, Value::Object(fobj));
                        }
                    }
                }
            }
            0x8F | 0x94 => {
                vm.warn_once(format!("action {op:#x} is not supported"));
            }
            0x96 => {
                let mut r = Reader::new(body);
                while !r.is_empty() {
                    let Ok(t) = r.u8() else { break };
                    let v = match t {
                        0 => r.cstr().map(Value::from),
                        1 => r.f32().map(|x| Value::Number(x as f64)),
                        2 => Ok(Value::Null),
                        3 => Ok(Value::Undefined),
                        4 => r
                            .u8()
                            .map(|i| f.regs.get(i as usize).cloned().unwrap_or_default()),
                        5 => r.u8().map(|b| Value::Bool(b != 0)),
                        6 => r.f64_swapped().map(Value::Number),
                        7 => r.i32().map(|i| Value::Number(i as f64)),
                        8 => r.u8().map(|i| {
                            f.pool
                                .get(i as usize)
                                .cloned()
                                .map(Value::String)
                                .unwrap_or_default()
                        }),
                        9 => r.u16().map(|i| {
                            f.pool
                                .get(i as usize)
                                .cloned()
                                .map(Value::String)
                                .unwrap_or_default()
                        }),
                        _ => break,
                    };
                    match v {
                        Ok(v) => f.stack.push(v),
                        Err(_) => break,
                    }
                }
            }
            0x99 | 0x9D => {
                let off = i16::from_le_bytes([
                    body.first().copied().unwrap_or(0),
                    body.get(1).copied().unwrap_or(0),
                ]) as isize;
                let jump = if op == 0x99 {
                    true
                } else {
                    let c = f.pop();
                    vm.to_bool(&c)
                };
                if jump {
                    let to = f.pc as isize + off;
                    if to < 0 || to as usize > f.end {
                        break;
                    }
                    f.pc = to as usize;
                }
            }
            0x9A => {
                let method = body.first().copied().unwrap_or(0);
                let target = f.pop();
                let url = f.pop();
                let url = vm.to_str(&url);
                get_url(vm, f, &url, &target, method);
            }
            0x9E => vm.warn_once("Call is not supported".into()),
            0x9F => {
                let flags = body.first().copied().unwrap_or(0);
                let bias = if flags & 2 != 0 {
                    u16::from_le_bytes([
                        body.get(1).copied().unwrap_or(0),
                        body.get(2).copied().unwrap_or(0),
                    ])
                } else {
                    0
                };
                let fr = f.pop();
                let play = flags & 1 != 0;
                goto_value(vm, f, &fr, play, bias);
            }
            _ => vm.warn_once(format!("unknown action {op:#x}")),
        }
    }
    Value::Undefined
}

/// `obj()` through CallMethod with no name: `super(...)` or a function value.
fn call_self(vm: &mut Vm, o: &Value, args: &[Value]) -> Value {
    if let Some(oid) = o.as_object() {
        if let Some(Kind::Super { this, proto, ctor }) = vm.heap.get(oid).map(|x| x.kind.clone()) {
            return vm.call_with_base(&ctor, this, args, proto);
        }
    }
    vm.call(o, Value::Undefined, args)
}

fn goto_value(vm: &mut Vm, f: &Frame, fr: &Value, play: bool, bias: u16) {
    match fr {
        Value::String(s) => {
            let (clip, label) = match s.rsplit_once(':') {
                Some((path, l)) => (
                    vm.as_display(&Value::from(path), &f.scope, f.target),
                    l.to_string(),
                ),
                None => (Some(f.target), s.to_string()),
            };
            let Some(clip) = clip else { return };
            if let Ok(n) = label.parse::<f64>() {
                crate::player::goto(vm, clip, (n as i64 - 1).max(0) as u16 + bias, play);
            } else {
                crate::player::goto_label(vm, clip, &label, play);
            }
        }
        v => {
            let n = vm.to_number(v);
            if n.is_finite() {
                crate::player::goto(vm, f.target, ((n as i64) - 1).max(0) as u16 + bias, play);
            }
        }
    }
}

fn get_url(vm: &mut Vm, f: &Frame, url: &str, target: &Value, method: u8) {
    if let Some(cmd) = url
        .strip_prefix("FSCommand:")
        .or_else(|| url.strip_prefix("fscommand:"))
    {
        let args = vm.to_str(target);
        vm.host.fscommand(cmd, &args);
        return;
    }
    // Load into a clip (loadMovie through getURL2's target flag).
    if method & 0x40 != 0 || method & 0x80 == 0 {
        if let Some(clip) = vm.as_display(target, &f.scope, f.target) {
            crate::player::load_into(vm, clip, url);
            return;
        }
    }
    vm.warn_once(format!("getURL {url} ignored"));
}

fn builtins_random(vm: &mut Vm) -> f64 {
    super::builtins::random(vm)
}
