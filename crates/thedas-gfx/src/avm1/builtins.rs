//! The core built-ins: the global object, Object, Function, Array, String, Number, Boolean,
//! Math, Date and the global functions. Display classes are in [`super::display_api`].

use std::cmp::Ordering;

use super::object::{DONT_DELETE, DONT_ENUM, Func, Kind, READ_ONLY};
use super::value::{ObjId, Str, Value, number_to_string, string_to_number, to_int32};
use super::{Interval, Protos, Vm};

pub use super::display_api::{display_get, display_set, display_value, sync_bound_text};

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or_default()
}

pub fn random(vm: &mut Vm) -> f64 {
    // xorshift64*
    let mut x = vm.rng;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    vm.rng = x;
    (x.wrapping_mul(0x2545F4914F6CDD1D) >> 11) as f64 / (1u64 << 53) as f64
}

/// Makes the global object and every built-in. Called once per movie.
pub fn create_protos(heap: &mut super::object::Heap) -> (ObjId, Protos) {
    use super::object::Object;
    let object = heap.alloc(Object::default());
    let mk = |heap: &mut super::object::Heap, kind: Kind| {
        heap.alloc(Object {
            proto: Some(object),
            kind,
            ..Default::default()
        })
    };
    let function = mk(heap, Kind::Plain);
    let protos = Protos {
        object,
        function,
        array: mk(heap, Kind::Array(Vec::new())),
        string: mk(heap, Kind::Boxed(Value::from(""))),
        number: mk(heap, Kind::Boxed(Value::Number(0.0))),
        boolean: mk(heap, Kind::Boxed(Value::Bool(false))),
        date: mk(heap, Kind::Plain),
        movie_clip: mk(heap, Kind::Plain),
        text_field: mk(heap, Kind::Plain),
        text_format: mk(heap, Kind::Plain),
    };
    let global = mk(heap, Kind::Plain);
    (global, protos)
}

/// Defines a class: a native constructor with `prototype` set to `proto`, stored as
/// `holder.name`.
pub fn class(
    vm: &mut Vm,
    holder: ObjId,
    name: &str,
    ctor: super::object::NativeFn,
    proto: ObjId,
) -> ObjId {
    let f = vm.new_native(ctor);
    vm.define(
        f,
        "prototype",
        Value::Object(proto),
        DONT_ENUM | DONT_DELETE,
    );
    vm.define(proto, "constructor", Value::Object(f), DONT_ENUM);
    vm.define(holder, name, Value::Object(f), DONT_ENUM);
    f
}

pub fn install(vm: &mut Vm) {
    let g = vm.global;
    let p = vm.protos;
    // Object
    let object_ctor = class(vm, g, "Object", object_new, p.object);
    vm.define_native(object_ctor, "registerClass", object_register_class);
    vm.define_native(p.object, "toString", |_, _, _| {
        Value::from("[object Object]")
    });
    vm.define_native(p.object, "valueOf", |_, this, _| this);
    vm.define_native(p.object, "hasOwnProperty", object_has_own);
    vm.define_native(p.object, "addProperty", object_add_property);
    vm.define_native(p.object, "watch", object_watch);
    vm.define_native(p.object, "unwatch", object_unwatch);
    vm.define_native(p.object, "isPrototypeOf", object_is_prototype_of);
    vm.define_native(p.object, "isPropertyEnumerable", object_is_enumerable);
    // Function
    class(vm, g, "Function", |_, this, _| this, p.function);
    vm.define_native(p.function, "call", function_call);
    vm.define_native(p.function, "apply", function_apply);
    // Array
    let array_ctor = class(vm, g, "Array", array_new, p.array);
    for (k, v) in [
        ("CASEINSENSITIVE", 1.0),
        ("DESCENDING", 2.0),
        ("UNIQUESORT", 4.0),
        ("RETURNINDEXEDARRAY", 8.0),
        ("NUMERIC", 16.0),
    ] {
        vm.define(array_ctor, k, Value::Number(v), DONT_ENUM | READ_ONLY);
    }
    let natives: &[(&str, super::object::NativeFn)] = &[
        ("push", array_push),
        ("pop", array_pop),
        ("shift", array_shift),
        ("unshift", array_unshift),
        ("splice", array_splice),
        ("slice", array_slice),
        ("concat", array_concat),
        ("join", array_join),
        ("toString", |vm, this, _| array_join(vm, this, &[])),
        ("reverse", array_reverse),
        ("sort", array_sort),
        ("sortOn", array_sort_on),
    ];
    for (n, f) in natives {
        vm.define_native(p.array, n, *f);
    }
    // String
    let string_ctor = class(vm, g, "String", string_new, p.string);
    vm.define_native(string_ctor, "fromCharCode", |vm, _, args| {
        let mut s = String::new();
        for a in args {
            let n = vm.to_number(a) as u32;
            s.extend(char::from_u32(n));
        }
        Value::from(s)
    });
    let natives: &[(&str, super::object::NativeFn)] = &[
        ("toString", string_value_of),
        ("valueOf", string_value_of),
        ("charAt", string_char_at),
        ("charCodeAt", string_char_code_at),
        ("indexOf", string_index_of),
        ("lastIndexOf", string_last_index_of),
        ("substr", string_substr),
        ("substring", string_substring),
        ("slice", string_slice),
        ("split", string_split),
        ("toUpperCase", |vm, this, _| {
            let s = this_string(vm, &this);
            Value::from(s.to_uppercase())
        }),
        ("toLowerCase", |vm, this, _| {
            let s = this_string(vm, &this);
            Value::from(s.to_lowercase())
        }),
        ("concat", |vm, this, args| {
            let mut s = this_string(vm, &this).to_string();
            for a in args {
                s.push_str(&vm.to_str(a));
            }
            Value::from(s)
        }),
    ];
    for (n, f) in natives {
        vm.define_native(p.string, n, *f);
    }
    // Number, Boolean
    let number_ctor = class(vm, g, "Number", number_new, p.number);
    for (k, v) in [
        ("MAX_VALUE", f64::MAX),
        ("MIN_VALUE", 5e-324),
        ("NaN", f64::NAN),
        ("POSITIVE_INFINITY", f64::INFINITY),
        ("NEGATIVE_INFINITY", f64::NEG_INFINITY),
    ] {
        vm.define(number_ctor, k, Value::Number(v), DONT_ENUM | READ_ONLY);
    }
    vm.define_native(p.number, "toString", number_to_string_native);
    vm.define_native(p.number, "valueOf", |vm, this, _| this_value(vm, &this));
    class(vm, g, "Boolean", boolean_new, p.boolean);
    vm.define_native(p.boolean, "toString", |vm, this, _| {
        let v = this_value(vm, &this);
        Value::String(vm.to_str(&v))
    });
    vm.define_native(p.boolean, "valueOf", |vm, this, _| this_value(vm, &this));
    // Date
    class(vm, g, "Date", date_new, p.date);
    vm.define_native(p.date, "getTime", date_get_time);
    vm.define_native(p.date, "valueOf", date_get_time);
    // Math
    let math = vm.new_object();
    vm.define(g, "Math", Value::Object(math), DONT_ENUM);
    for (k, v) in [
        ("PI", std::f64::consts::PI),
        ("E", std::f64::consts::E),
        ("LN2", std::f64::consts::LN_2),
        ("LN10", std::f64::consts::LN_10),
        ("LOG2E", std::f64::consts::LOG2_E),
        ("LOG10E", std::f64::consts::LOG10_E),
        ("SQRT2", std::f64::consts::SQRT_2),
        ("SQRT1_2", std::f64::consts::FRAC_1_SQRT_2),
    ] {
        vm.define(math, k, Value::Number(v), DONT_ENUM | READ_ONLY);
    }
    let unary: &[(&str, super::object::NativeFn)] = &[
        ("abs", |vm, _, a| math1(vm, a, f64::abs)),
        ("floor", |vm, _, a| math1(vm, a, f64::floor)),
        ("ceil", |vm, _, a| math1(vm, a, f64::ceil)),
        ("round", |vm, _, a| math1(vm, a, |x| (x + 0.5).floor())),
        ("sqrt", |vm, _, a| math1(vm, a, f64::sqrt)),
        ("sin", |vm, _, a| math1(vm, a, f64::sin)),
        ("cos", |vm, _, a| math1(vm, a, f64::cos)),
        ("tan", |vm, _, a| math1(vm, a, f64::tan)),
        ("asin", |vm, _, a| math1(vm, a, f64::asin)),
        ("acos", |vm, _, a| math1(vm, a, f64::acos)),
        ("atan", |vm, _, a| math1(vm, a, f64::atan)),
        ("exp", |vm, _, a| math1(vm, a, f64::exp)),
        ("log", |vm, _, a| math1(vm, a, f64::ln)),
        ("atan2", |vm, _, a| {
            let y = vm.to_number(&arg(a, 0));
            let x = vm.to_number(&arg(a, 1));
            Value::Number(y.atan2(x))
        }),
        ("pow", |vm, _, a| {
            let x = vm.to_number(&arg(a, 0));
            let y = vm.to_number(&arg(a, 1));
            Value::Number(x.powf(y))
        }),
        ("min", |vm, _, a| math_minmax(vm, a, true)),
        ("max", |vm, _, a| math_minmax(vm, a, false)),
        ("random", |vm, _, _| Value::Number(random(vm))),
    ];
    for (n, f) in unary {
        vm.define_native(math, n, *f);
    }
    // Global functions.
    let globals: &[(&str, super::object::NativeFn)] = &[
        ("parseInt", global_parse_int),
        ("parseFloat", global_parse_float),
        ("isNaN", |vm, _, a| {
            let n = vm.to_number(&arg(a, 0));
            Value::Bool(n.is_nan())
        }),
        ("isFinite", |vm, _, a| {
            let n = vm.to_number(&arg(a, 0));
            Value::Bool(n.is_finite())
        }),
        ("setInterval", global_set_interval),
        ("setTimeout", global_set_timeout),
        ("clearInterval", global_clear_interval),
        ("clearTimeout", global_clear_interval),
        ("ASSetPropFlags", global_set_prop_flags),
        ("getTimer", |vm, _, _| Value::Number(vm.time_ms.floor())),
        ("escape", |vm, _, a| {
            let s = vm.to_str(&arg(a, 0));
            let mut o = String::new();
            for b in s.bytes() {
                if b.is_ascii_alphanumeric() {
                    o.push(b as char);
                } else {
                    o.push_str(&format!("%{b:02X}"));
                }
            }
            Value::from(o)
        }),
        ("trace", |vm, _, a| {
            let s = vm.to_str(&arg(a, 0));
            vm.log.push(format!("trace: {s}"));
            Value::Undefined
        }),
        ("updateAfterEvent", |vm, _, _| {
            vm.dirty = true;
            Value::Undefined
        }),
    ];
    for (n, f) in globals {
        vm.define_native(g, n, *f);
    }
    vm.define(g, "NaN", Value::Number(f64::NAN), DONT_ENUM);
    vm.define(g, "Infinity", Value::Number(f64::INFINITY), DONT_ENUM);
    super::display_api::install(vm);
}

fn math1(vm: &mut Vm, a: &[Value], f: fn(f64) -> f64) -> Value {
    let x = vm.to_number(&arg(a, 0));
    Value::Number(f(x))
}

fn math_minmax(vm: &mut Vm, a: &[Value], min: bool) -> Value {
    if a.is_empty() {
        return Value::Number(if min {
            f64::INFINITY
        } else {
            f64::NEG_INFINITY
        });
    }
    let x = vm.to_number(&arg(a, 0));
    let y = vm.to_number(&arg(a, 1));
    if x.is_nan() || y.is_nan() {
        return Value::Number(f64::NAN);
    }
    Value::Number(if min { x.min(y) } else { x.max(y) })
}

// ---- Object ----------------------------------------------------------------------------

fn object_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    match args.first() {
        Some(Value::Object(o)) => Value::Object(*o),
        Some(v @ (Value::String(_) | Value::Number(_) | Value::Bool(_))) => box_value(vm, v),
        _ => {
            if this.as_object().is_some() {
                this
            } else {
                Value::Object(vm.new_object())
            }
        }
    }
}

pub fn box_value(vm: &mut Vm, v: &Value) -> Value {
    let proto = match v {
        Value::String(_) => vm.protos.string,
        Value::Number(_) => vm.protos.number,
        Value::Bool(_) => vm.protos.boolean,
        _ => return v.clone(),
    };
    Value::Object(vm.alloc(Some(proto), Kind::Boxed(v.clone())))
}

fn object_register_class(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let ctor = arg(args, 1);
    match vm.movie.exports.get(&name.to_ascii_lowercase()) {
        Some(&id) => {
            if ctor.is_nullish() {
                vm.classes.remove(&id);
            } else {
                vm.classes.insert(id, ctor);
            }
            Value::Bool(true)
        }
        None => Value::Bool(false),
    }
}

fn object_has_own(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let Some(o) = this.as_object() else {
        return Value::Bool(false);
    };
    let own = vm.heap.get(o).is_some_and(|x| x.props.get(&name).is_some());
    Value::Bool(own)
}

fn object_add_property(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let getter = arg(args, 1);
    let setter = arg(args, 2);
    let Some(o) = this.as_object() else {
        return Value::Bool(false);
    };
    if name.is_empty() || !matches!(&getter, Value::Object(g) if vm.is_function(*g)) {
        return Value::Bool(false);
    }
    if let Some(obj) = vm.heap.get_mut(o) {
        obj.props.insert(
            name,
            super::object::Prop {
                value: Value::Undefined,
                flags: 0,
                accessor: Some((getter, setter)),
            },
        );
    }
    Value::Bool(true)
}

fn object_watch(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let f = arg(args, 1);
    let Some(o) = this.as_object() else {
        return Value::Bool(false);
    };
    if !matches!(&f, Value::Object(g) if vm.is_function(*g)) {
        return Value::Bool(false);
    }
    if let Some(obj) = vm.heap.get_mut(o) {
        obj.watchers.retain(|(n, _, _)| *n != name);
        obj.watchers.push((name, f, arg(args, 2)));
    }
    Value::Bool(true)
}

fn object_unwatch(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let Some(o) = this.as_object() else {
        return Value::Bool(false);
    };
    if let Some(obj) = vm.heap.get_mut(o) {
        let before = obj.watchers.len();
        obj.watchers.retain(|(n, _, _)| *n != name);
        return Value::Bool(before != obj.watchers.len());
    }
    Value::Bool(false)
}

fn object_is_prototype_of(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let (Some(p), Some(o)) = (this.as_object(), arg(args, 0).as_object()) else {
        return Value::Bool(false);
    };
    let mut cur = vm.heap.get(o).and_then(|x| x.proto);
    while let Some(c) = cur {
        if c == p {
            return Value::Bool(true);
        }
        cur = vm.heap.get(c).and_then(|x| x.proto);
    }
    Value::Bool(false)
}

fn object_is_enumerable(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let name = vm.to_str(&arg(args, 0));
    let Some(o) = this.as_object() else {
        return Value::Bool(false);
    };
    Value::Bool(
        vm.heap
            .get(o)
            .and_then(|x| x.props.get(&name))
            .is_some_and(|p| p.flags & DONT_ENUM == 0),
    )
}

// ---- Function --------------------------------------------------------------------------

fn function_call(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let t = arg(args, 0);
    let t = if t.is_nullish() { Value::Undefined } else { t };
    vm.call(&this, t, args.get(1..).unwrap_or(&[]))
}

fn function_apply(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let t = arg(args, 0);
    let t = if t.is_nullish() { Value::Undefined } else { t };
    let list = match arg(args, 1).as_object().and_then(|o| vm.heap.get(o)) {
        Some(o) => match &o.kind {
            Kind::Array(items) => items.clone(),
            _ => Vec::new(),
        },
        None => Vec::new(),
    };
    vm.call(&this, t, &list)
}

// ---- Array -----------------------------------------------------------------------------

fn array_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let items = if args.len() == 1 {
        match &args[0] {
            Value::Number(n) if *n >= 0.0 && n.fract() == 0.0 => {
                vec![Value::Undefined; (*n as usize).min(1 << 20)]
            }
            v => vec![v.clone()],
        }
    } else {
        args.to_vec()
    };
    if let Some(o) = this.as_object() {
        if let Some(obj) = vm.heap.get_mut(o) {
            if matches!(obj.kind, Kind::Plain) {
                obj.kind = Kind::Array(items);
                return this;
            }
        }
    }
    Value::Object(vm.new_array(items))
}

/// The items of `this` when it is an array.
fn items(vm: &Vm, this: &Value) -> Option<Vec<Value>> {
    match vm.heap.get(this.as_object()?).map(|o| &o.kind) {
        Some(Kind::Array(v)) => Some(v.clone()),
        _ => None,
    }
}

fn with_items<R>(vm: &mut Vm, this: &Value, f: impl FnOnce(&mut Vec<Value>) -> R) -> Option<R> {
    match vm.heap.get_mut(this.as_object()?).map(|o| &mut o.kind) {
        Some(Kind::Array(v)) => Some(f(v)),
        _ => None,
    }
}

fn array_push(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    with_items(vm, &this, |v| {
        v.extend_from_slice(args);
        Value::Number(v.len() as f64)
    })
    .unwrap_or_default()
}

fn array_pop(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    with_items(vm, &this, |v| v.pop().unwrap_or_default()).unwrap_or_default()
}

fn array_shift(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    with_items(vm, &this, |v| {
        if v.is_empty() {
            Value::Undefined
        } else {
            v.remove(0)
        }
    })
    .unwrap_or_default()
}

fn array_unshift(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    with_items(vm, &this, |v| {
        v.splice(0..0, args.iter().cloned());
        Value::Number(v.len() as f64)
    })
    .unwrap_or_default()
}

fn clamp_index(vm: &mut Vm, v: &Value, len: usize, default: usize) -> usize {
    if v.is_undefined() {
        return default;
    }
    let n = vm.to_number(v);
    if n.is_nan() {
        return 0;
    }
    let n = n.trunc() as i64;
    if n < 0 {
        (len as i64 + n).max(0) as usize
    } else {
        (n as usize).min(len)
    }
}

fn array_splice(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(v) = items(vm, &this) else {
        return Value::Undefined;
    };
    let len = v.len();
    let start = clamp_index(vm, &arg(args, 0), len, 0);
    let count = if args.len() < 2 {
        len - start
    } else {
        let c = vm.to_number(&args[1]);
        if c.is_nan() || c < 0.0 {
            0
        } else {
            (c as usize).min(len - start)
        }
    };
    let insert: Vec<Value> = args.get(2..).unwrap_or(&[]).to_vec();
    let removed = with_items(vm, &this, |v| {
        v.splice(start..start + count, insert).collect::<Vec<_>>()
    })
    .unwrap_or_default();
    Value::Object(vm.new_array(removed))
}

fn array_slice(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(v) = items(vm, &this) else {
        return Value::Undefined;
    };
    let len = v.len();
    let s = clamp_index(vm, &arg(args, 0), len, 0);
    let e = clamp_index(vm, &arg(args, 1), len, len);
    let out = if s < e { v[s..e].to_vec() } else { Vec::new() };
    Value::Object(vm.new_array(out))
}

fn array_concat(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let mut out = items(vm, &this).unwrap_or_default();
    for a in args {
        match items(vm, a) {
            Some(more) => out.extend(more),
            None => out.push(a.clone()),
        }
    }
    Value::Object(vm.new_array(out))
}

fn array_join(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(v) = items(vm, &this) else {
        return Value::from("");
    };
    let sep = match args.first() {
        None | Some(Value::Undefined) => Str::from(","),
        Some(s) => vm.to_str(s),
    };
    let mut out = String::new();
    for (i, x) in v.iter().enumerate() {
        if i > 0 {
            out.push_str(&sep);
        }
        if !x.is_nullish() {
            out.push_str(&vm.to_str(x));
        }
    }
    Value::from(out)
}

fn array_reverse(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    with_items(vm, &this, |v| v.reverse());
    this
}

fn compare_values(vm: &mut Vm, a: &Value, b: &Value, flags: u32) -> Ordering {
    let ord = if flags & 16 != 0 {
        let (x, y) = (vm.to_number(a), vm.to_number(b));
        x.partial_cmp(&y).unwrap_or(Ordering::Equal)
    } else {
        // Undefined sorts last.
        match (a.is_undefined(), b.is_undefined()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            _ => {
                let (x, y) = (vm.to_str(a), vm.to_str(b));
                if flags & 1 != 0 {
                    x.to_lowercase().cmp(&y.to_lowercase())
                } else {
                    x.encode_utf16().cmp(y.encode_utf16())
                }
            }
        }
    };
    if flags & 2 != 0 { ord.reverse() } else { ord }
}

/// A stable merge sort calling out to script comparators (std's sort may panic on an
/// inconsistent order; this one cannot).
fn merge_sort(
    vm: &mut Vm,
    v: Vec<(usize, Value)>,
    cmp: &mut dyn FnMut(&mut Vm, &Value, &Value) -> Ordering,
) -> Vec<(usize, Value)> {
    if v.len() <= 1 {
        return v;
    }
    let mut right = v;
    let left = right.drain(..right.len() / 2).collect::<Vec<_>>();
    let left = merge_sort(vm, left, cmp);
    let right = merge_sort(vm, right, cmp);
    let mut out = Vec::with_capacity(left.len() + right.len());
    let (mut i, mut j) = (0, 0);
    while i < left.len() && j < right.len() {
        if cmp(vm, &right[j].1, &left[i].1) == Ordering::Less {
            out.push(right[j].clone());
            j += 1;
        } else {
            out.push(left[i].clone());
            i += 1;
        }
    }
    out.extend_from_slice(&left[i..]);
    out.extend_from_slice(&right[j..]);
    out
}

fn finish_sort(vm: &mut Vm, this: &Value, sorted: Vec<(usize, Value)>, flags: u32) -> Value {
    if flags & 4 != 0 {
        for w in sorted.windows(2) {
            if compare_values(vm, &w[0].1, &w[1].1, flags & !2) == Ordering::Equal {
                return Value::Number(0.0);
            }
        }
    }
    if flags & 8 != 0 {
        let idx = sorted
            .iter()
            .map(|(i, _)| Value::Number(*i as f64))
            .collect();
        return Value::Object(vm.new_array(idx));
    }
    let vals: Vec<Value> = sorted.into_iter().map(|(_, v)| v).collect();
    with_items(vm, this, |v| *v = vals);
    this.clone()
}

fn array_sort(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(v) = items(vm, &this) else {
        return Value::Undefined;
    };
    let (func, flags) = match args.first() {
        Some(f @ Value::Object(o)) if vm.is_function(*o) => {
            let fl = vm.to_number(&arg(args, 1));
            (Some(f.clone()), if fl.is_nan() { 0 } else { fl as u32 })
        }
        Some(x) => {
            let fl = vm.to_number(x);
            (None, if fl.is_nan() { 0 } else { fl as u32 })
        }
        None => (None, 0),
    };
    let indexed: Vec<(usize, Value)> = v.into_iter().enumerate().collect();
    let sorted = match func {
        Some(f) => {
            let desc = flags & 2 != 0;
            merge_sort(vm, indexed, &mut |vm, a, b| {
                let r = vm.call(&f, Value::Undefined, &[a.clone(), b.clone()]);
                let n = vm.to_number(&r);
                let o = if n < 0.0 {
                    Ordering::Less
                } else if n > 0.0 {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                };
                if desc { o.reverse() } else { o }
            })
        }
        None => merge_sort(vm, indexed, &mut |vm, a, b| compare_values(vm, a, b, flags)),
    };
    finish_sort(vm, &this, sorted, flags)
}

fn array_sort_on(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let Some(v) = items(vm, &this) else {
        return Value::Undefined;
    };
    let fields: Vec<Str> = match items(vm, &arg(args, 0)) {
        Some(list) => list.iter().map(|x| vm.to_str(x)).collect(),
        None => vec![vm.to_str(&arg(args, 0))],
    };
    let flag_list: Vec<u32> = match items(vm, &arg(args, 1)) {
        Some(list) => list
            .iter()
            .map(|x| {
                let n = vm.to_number(x);
                if n.is_nan() { 0 } else { n as u32 }
            })
            .collect(),
        None => {
            let n = vm.to_number(&arg(args, 1));
            vec![if n.is_nan() { 0 } else { n as u32 }; fields.len()]
        }
    };
    let flags0 = flag_list.first().copied().unwrap_or(0);
    let indexed: Vec<(usize, Value)> = v.into_iter().enumerate().collect();
    let sorted = merge_sort(vm, indexed, &mut |vm, a, b| {
        for (i, f) in fields.iter().enumerate() {
            let fa = vm.get(a, f);
            let fb = vm.get(b, f);
            let o = compare_values(vm, &fa, &fb, flag_list.get(i).copied().unwrap_or(0));
            if o != Ordering::Equal {
                return o;
            }
        }
        Ordering::Equal
    });
    finish_sort(vm, &this, sorted, flags0 & !2)
}

// ---- String, Number, Boolean ------------------------------------------------------------

fn this_value(vm: &Vm, this: &Value) -> Value {
    match this {
        Value::Object(o) => match vm.heap.get(*o).map(|x| &x.kind) {
            Some(Kind::Boxed(v)) => v.clone(),
            _ => this.clone(),
        },
        v => v.clone(),
    }
}

fn this_string(vm: &mut Vm, this: &Value) -> Str {
    let v = this_value(vm, this);
    vm.to_str(&v)
}

fn string_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = match args.first() {
        Some(v) => Value::String(vm.to_str(v)),
        None => Value::from(""),
    };
    // Called as a function: a conversion.
    match this.as_object().and_then(|o| vm.heap.get_mut(o)) {
        Some(o) if matches!(o.kind, Kind::Plain) && o.proto == Some(vm.protos.string) => {
            o.kind = Kind::Boxed(s);
            this
        }
        _ => s,
    }
}

fn number_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let n = Value::Number(match args.first() {
        Some(v) => vm.to_number(v),
        None => 0.0,
    });
    let num_proto = vm.protos.number;
    match this.as_object().and_then(|o| vm.heap.get_mut(o)) {
        Some(o) if matches!(o.kind, Kind::Plain) && o.proto == Some(num_proto) => {
            o.kind = Kind::Boxed(n);
            this
        }
        _ => n,
    }
}

fn boolean_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let b = Value::Bool(args.first().is_some_and(|v| vm.to_bool(v)));
    let bool_proto = vm.protos.boolean;
    match this.as_object().and_then(|o| vm.heap.get_mut(o)) {
        Some(o) if matches!(o.kind, Kind::Plain) && o.proto == Some(bool_proto) => {
            o.kind = Kind::Boxed(b);
            this
        }
        _ => b,
    }
}

fn string_value_of(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    Value::String(this_string(vm, &this))
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn from16(v: &[u16]) -> Value {
    Value::from(String::from_utf16_lossy(v))
}

fn string_char_at(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = utf16(&this_string(vm, &this));
    let i = vm.to_number(&arg(args, 0));
    if i.is_nan() || i < 0.0 || i as usize >= s.len() {
        return Value::from("");
    }
    from16(&s[i as usize..i as usize + 1])
}

fn string_char_code_at(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = utf16(&this_string(vm, &this));
    let i = vm.to_number(&arg(args, 0));
    let i = if i.is_nan() { 0.0 } else { i };
    if i < 0.0 || i as usize >= s.len() {
        return Value::Number(f64::NAN);
    }
    Value::Number(s[i as usize] as f64)
}

fn find16(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from.min(hay.len()));
    }
    (from..hay.len().saturating_sub(needle.len() - 1)).find(|&i| hay[i..].starts_with(needle))
}

fn string_index_of(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = utf16(&this_string(vm, &this));
    let n = utf16(&vm.to_str(&arg(args, 0)));
    let from = if args.len() > 1 {
        let f = vm.to_number(&args[1]);
        if f.is_nan() || f < 0.0 { 0 } else { f as usize }
    } else {
        0
    };
    Value::Number(find16(&s, &n, from).map(|i| i as f64).unwrap_or(-1.0))
}

fn string_last_index_of(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = utf16(&this_string(vm, &this));
    let n = utf16(&vm.to_str(&arg(args, 0)));
    let max = if args.len() > 1 {
        let f = vm.to_number(&args[1]);
        if f.is_nan() {
            s.len()
        } else {
            f.max(0.0) as usize
        }
    } else {
        s.len()
    };
    if n.len() > s.len() {
        return Value::Number(-1.0);
    }
    let start = max.min(s.len() - n.len());
    for i in (0..=start).rev() {
        if s[i..].starts_with(&n) {
            return Value::Number(i as f64);
        }
    }
    Value::Number(-1.0)
}

fn string_substr(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = utf16(&this_string(vm, &this));
    let start = clamp_index(vm, &arg(args, 0), s.len(), 0);
    let len = if args.len() > 1 && !args[1].is_undefined() {
        let n = vm.to_number(&args[1]);
        if n.is_nan() || n < 0.0 { 0 } else { n as usize }
    } else {
        s.len()
    };
    let end = (start + len.min(s.len())).min(s.len());
    from16(&s[start..end])
}

fn string_substring(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = utf16(&this_string(vm, &this));
    let pos = |vm: &mut Vm, v: &Value, d: usize| -> usize {
        if v.is_undefined() {
            return d;
        }
        let n = vm.to_number(v);
        if n.is_nan() || n < 0.0 {
            0
        } else {
            (n as usize).min(s.len())
        }
    };
    let a = pos(vm, &arg(args, 0), 0);
    let b = pos(vm, &arg(args, 1), s.len());
    let (a, b) = if a > b { (b, a) } else { (a, b) };
    from16(&s[a..b])
}

fn string_slice(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = utf16(&this_string(vm, &this));
    let a = clamp_index(vm, &arg(args, 0), s.len(), 0);
    let b = clamp_index(vm, &arg(args, 1), s.len(), s.len());
    if a >= b {
        return Value::from("");
    }
    from16(&s[a..b])
}

fn string_split(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let s = this_string(vm, &this);
    let out: Vec<Value> = match args.first() {
        None | Some(Value::Undefined) => vec![Value::String(s)],
        Some(d) => {
            let d = vm.to_str(d);
            if d.is_empty() {
                s.chars().map(|c| Value::from(c.to_string())).collect()
            } else {
                s.split(&*d).map(Value::from).collect()
            }
        }
    };
    let limit = args.get(1).map(|l| vm.to_number(l));
    let out = match limit {
        Some(l) if l.is_finite() && l >= 0.0 => out.into_iter().take(l as usize).collect(),
        _ => out,
    };
    Value::Object(vm.new_array(out))
}

fn number_to_string_native(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let v = this_value(vm, &this);
    let n = vm.to_number(&v);
    let radix = args.first().map(|r| vm.to_number(r)).unwrap_or(10.0);
    if radix == 10.0 || !(2.0..=36.0).contains(&radix) || n.is_nan() || n.is_infinite() {
        return Value::from(number_to_string(n));
    }
    let r = radix as u32;
    let mut i = to_int32(n) as i64;
    let neg = i < 0;
    i = i.abs();
    let mut digits = Vec::new();
    loop {
        digits.push(std::char::from_digit((i % r as i64) as u32, r).unwrap_or('0'));
        i /= r as i64;
        if i == 0 {
            break;
        }
    }
    if neg {
        digits.push('-');
    }
    Value::from(digits.into_iter().rev().collect::<String>())
}

fn global_parse_int(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let s = vm.to_str(&arg(args, 0));
    let mut t = s.trim_start();
    let radix = args
        .get(1)
        .map(|r| vm.to_number(r))
        .filter(|r| r.is_finite());
    let neg = t.starts_with('-');
    if neg || t.starts_with('+') {
        t = &t[1..];
    }
    let mut radix = radix.map(|r| r as u32).unwrap_or(0);
    if (radix == 0 || radix == 16) && (t.starts_with("0x") || t.starts_with("0X")) {
        t = &t[2..];
        radix = 16;
    }
    if radix == 0 {
        // Flash reads a leading 0 as octal.
        radix = if t.len() > 1 && t.starts_with('0') {
            8
        } else {
            10
        };
    }
    if !(2..=36).contains(&radix) {
        return Value::Number(f64::NAN);
    }
    let digits: String = t.chars().take_while(|c| c.is_digit(radix)).collect();
    if digits.is_empty() {
        return Value::Number(f64::NAN);
    }
    let mut v = 0f64;
    for c in digits.chars() {
        v = v * radix as f64 + c.to_digit(radix).unwrap_or(0) as f64;
    }
    Value::Number(if neg { -v } else { v })
}

fn global_parse_float(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let s = vm.to_str(&arg(args, 0));
    let t = s.trim_start();
    let mut end = 0;
    let b = t.as_bytes();
    let mut seen_dot = false;
    let mut seen_e = false;
    while end < b.len() {
        let c = b[end];
        let ok = c.is_ascii_digit()
            || (end == 0 && (c == b'-' || c == b'+'))
            || (c == b'.' && !seen_dot && !seen_e)
            || ((c == b'e' || c == b'E') && !seen_e && end > 0)
            || ((c == b'-' || c == b'+') && end > 0 && matches!(b[end - 1], b'e' | b'E'));
        if !ok {
            break;
        }
        seen_dot |= c == b'.';
        seen_e |= c == b'e' || c == b'E';
        end += 1;
    }
    let mut part = &t[..end];
    while !part.is_empty() && string_to_number(part).is_nan() {
        part = &part[..part.len() - 1];
    }
    Value::Number(if part.is_empty() {
        f64::NAN
    } else {
        string_to_number(part)
    })
}

fn add_interval(vm: &mut Vm, args: &[Value], once: bool) -> Value {
    let a0 = arg(args, 0);
    let (this, func, method, rest) = if matches!(&a0, Value::Object(o) if vm.is_function(*o)) {
        (Value::Undefined, a0, None, args.get(1..).unwrap_or(&[]))
    } else {
        let m = vm.to_str(&arg(args, 1));
        (a0, Value::Undefined, Some(m), args.get(2..).unwrap_or(&[]))
    };
    let every = vm.to_number(&arg(rest, 0));
    let every = if every.is_finite() {
        every.trunc().max(10.0)
    } else {
        10.0
    };
    vm.next_interval += 1;
    let id = vm.next_interval;
    vm.intervals.push(Interval {
        id,
        this,
        func,
        method,
        every: if once { -every } else { every },
        next: vm.timer_time_ms + every,
        args: rest.get(1..).unwrap_or(&[]).to_vec(),
    });
    Value::Number(id as f64)
}

fn global_set_interval(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    add_interval(vm, args, false)
}

fn global_set_timeout(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    add_interval(vm, args, true)
}

fn global_clear_interval(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let id = vm.to_number(&arg(args, 0));
    vm.intervals.retain(|i| i.id as f64 != id);
    Value::Undefined
}

fn global_set_prop_flags(vm: &mut Vm, _: Value, args: &[Value]) -> Value {
    let Some(o) = arg(args, 0).as_object() else {
        return Value::Undefined;
    };
    let names: Option<Vec<Str>> = match arg(args, 1) {
        Value::Null | Value::Undefined => None,
        Value::String(s) => Some(s.split(',').map(Str::from).collect()),
        v => items(vm, &v).map(|l| l.iter().map(|x| vm.to_str(x)).collect()),
    };
    let set = vm.to_number(&arg(args, 2));
    let clear = vm.to_number(&arg(args, 3));
    let set = if set.is_nan() { 0 } else { set as u32 as u8 };
    let clear = if clear.is_nan() {
        0
    } else {
        clear as u32 as u8
    };
    if let Some(obj) = vm.heap.get_mut(o) {
        let keys: Vec<Str> = match &names {
            Some(n) => n.clone(),
            None => obj.props.iter().map(|(k, _)| k.clone()).collect(),
        };
        for k in keys {
            if let Some(p) = obj.props.get_mut(&k) {
                p.flags = (p.flags & !clear) | set;
            }
        }
    }
    Value::Undefined
}

// ---- Date ------------------------------------------------------------------------------

fn date_new(vm: &mut Vm, this: Value, args: &[Value]) -> Value {
    let t = match args.first() {
        Some(v) if args.len() == 1 => vm.to_number(v),
        _ => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as f64)
            .unwrap_or(0.0),
    };
    match this.as_object().and_then(|o| vm.heap.get_mut(o)) {
        Some(o) => {
            o.kind = Kind::Date(t);
            this
        }
        None => Value::from(number_to_string(t)),
    }
}

fn date_get_time(vm: &mut Vm, this: Value, _: &[Value]) -> Value {
    match this
        .as_object()
        .and_then(|o| vm.heap.get(o))
        .map(|o| &o.kind)
    {
        Some(Kind::Date(t)) => Value::Number(*t),
        _ => Value::Number(f64::NAN),
    }
}

/// A native function value (for display_api's tables).
pub fn native(vm: &mut Vm, f: super::object::NativeFn) -> Value {
    Value::Object(vm.alloc(Some(vm.protos.function), Kind::Function(Func::Native(f))))
}
