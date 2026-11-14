//! Objects and the heap they live in.
//!
//! Objects are stored in an arena and named by [`ObjId`]. Garbage is collected by mark and
//! sweep from roots the player supplies, only between script runs, so no Rust code holds an
//! id across a collection.

use std::collections::HashMap;
use std::rc::Rc;

use super::value::{ObjId, Str, Value};
use crate::display::DisplayId;

pub const DONT_ENUM: u8 = 1;
pub const DONT_DELETE: u8 = 2;
pub const READ_ONLY: u8 = 4;

#[derive(Clone, Debug)]
pub struct Prop {
    pub value: Value,
    pub flags: u8,
    /// `addProperty` getter and setter.
    pub accessor: Option<(Value, Value)>,
}

/// Properties in insertion order, indexed by name once there are more than a few.
#[derive(Clone, Debug, Default)]
pub struct Props {
    list: Vec<(Str, Prop)>,
    index: Option<HashMap<Str, usize>>,
}

const INDEX_AT: usize = 12;

impl Props {
    fn find(&self, k: &str) -> Option<usize> {
        match &self.index {
            Some(ix) => ix.get(k).copied(),
            None => self.list.iter().position(|(n, _)| &**n == k),
        }
    }

    pub fn get(&self, k: &str) -> Option<&Prop> {
        self.find(k).map(|i| &self.list[i].1)
    }

    pub fn get_mut(&mut self, k: &str) -> Option<&mut Prop> {
        self.find(k).map(|i| &mut self.list[i].1)
    }

    pub fn insert(&mut self, k: Str, p: Prop) {
        if let Some(i) = self.find(&k) {
            self.list[i].1 = p;
            return;
        }
        if let Some(ix) = &mut self.index {
            ix.insert(k.clone(), self.list.len());
        }
        self.list.push((k, p));
        if self.index.is_none() && self.list.len() > INDEX_AT {
            self.index = Some(
                self.list
                    .iter()
                    .enumerate()
                    .map(|(i, (n, _))| (n.clone(), i))
                    .collect(),
            );
        }
    }

    pub fn remove(&mut self, k: &str) -> Option<Prop> {
        let i = self.find(k)?;
        let (_, p) = self.list.remove(i);
        if self.index.is_some() {
            self.index = Some(
                self.list
                    .iter()
                    .enumerate()
                    .map(|(i, (n, _))| (n.clone(), i))
                    .collect(),
            );
        }
        Some(p)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &(Str, Prop)> {
        self.list.iter()
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }
}

/// A native function: `(vm, this, args) -> result`.
pub type NativeFn = fn(&mut super::Vm, Value, &[Value]) -> Value;

/// A function compiled from bytecode, with the scope it closed over.
#[derive(Debug)]
pub struct CodeFn {
    pub name: Str,
    pub code: crate::swf::Actions,
    pub start: usize,
    pub end: usize,
    pub params: Vec<(u8, Str)>,
    /// DefineFunction2's register count and flags; `None` for DefineFunction.
    pub v2: Option<(u8, u16)>,
    pub pool: Rc<Vec<Str>>,
    pub scope: Rc<super::Scope>,
    /// The clip whose timeline defined the function (its `_root`, `_parent` and target).
    pub target: Option<DisplayId>,
}

#[derive(Clone, Debug)]
pub enum Func {
    Native(NativeFn),
    Code(Rc<CodeFn>),
}

#[derive(Clone, Debug, Default)]
pub enum Kind {
    #[default]
    Plain,
    Array(Vec<Value>),
    Function(Func),
    /// A `String`, `Number` or `Boolean` wrapper.
    Boxed(Value),
    Date(f64),
    /// A display object's script object.
    Display(DisplayId),
    /// `super` inside a method: `this`, and the prototype lookups start from.
    Super {
        this: Value,
        proto: Option<ObjId>,
        /// The superclass constructor `super(...)` calls.
        ctor: Value,
    },
}

#[derive(Clone, Debug, Default)]
pub struct Object {
    pub props: Props,
    pub proto: Option<ObjId>,
    pub kind: Kind,
    /// `Object.watch` callbacks: (property, callback, user data).
    pub watchers: Vec<(Str, Value, Value)>,
}

#[derive(Default)]
pub struct Heap {
    objects: Vec<Option<Object>>,
    free: Vec<u32>,
    marks: Vec<bool>,
    pub allocated_since_gc: usize,
}

impl Heap {
    pub fn alloc(&mut self, o: Object) -> ObjId {
        self.allocated_since_gc += 1;
        if let Some(i) = self.free.pop() {
            self.objects[i as usize] = Some(o);
            ObjId(i)
        } else {
            self.objects.push(Some(o));
            ObjId(self.objects.len() as u32 - 1)
        }
    }

    pub fn get(&self, id: ObjId) -> Option<&Object> {
        self.objects.get(id.0 as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: ObjId) -> Option<&mut Object> {
        self.objects.get_mut(id.0 as usize).and_then(Option::as_mut)
    }

    pub fn live(&self) -> usize {
        self.objects.len() - self.free.len()
    }

    /// Frees every object not reachable from `roots`.
    pub fn collect(&mut self, roots: impl IntoIterator<Item = ObjId>, extra: &[Value]) {
        self.marks.clear();
        self.marks.resize(self.objects.len(), false);
        let mut stack: Vec<ObjId> = roots.into_iter().collect();
        stack.extend(extra.iter().filter_map(Value::as_object));
        let mut scopes: Vec<Rc<super::Scope>> = Vec::new();
        while let Some(id) = stack.pop() {
            let i = id.0 as usize;
            if i >= self.marks.len() || self.marks[i] {
                continue;
            }
            self.marks[i] = true;
            let Some(o) = self.objects[i].as_ref() else {
                continue;
            };
            let push = |v: &Value, s: &mut Vec<ObjId>| {
                if let Value::Object(o) = v {
                    s.push(*o);
                }
            };
            for (_, p) in o.props.iter() {
                push(&p.value, &mut stack);
                if let Some((g, s)) = &p.accessor {
                    push(g, &mut stack);
                    push(s, &mut stack);
                }
            }
            if let Some(p) = o.proto {
                stack.push(p);
            }
            for (_, a, b) in &o.watchers {
                push(a, &mut stack);
                push(b, &mut stack);
            }
            match &o.kind {
                Kind::Array(v) => v.iter().for_each(|x| push(x, &mut stack)),
                Kind::Boxed(v) => push(v, &mut stack),
                Kind::Super { this, proto, ctor } => {
                    push(this, &mut stack);
                    push(ctor, &mut stack);
                    stack.extend(proto.iter().copied());
                }
                Kind::Function(Func::Code(c)) => scopes.push(c.scope.clone()),
                _ => {}
            }
            while let Some(s) = scopes.pop() {
                stack.push(s.object);
                if let Some(p) = &s.parent {
                    scopes.push(p.clone());
                }
            }
        }
        for (i, slot) in self.objects.iter_mut().enumerate() {
            if slot.is_some() && !self.marks[i] {
                *slot = None;
                self.free.push(i as u32);
            }
        }
        self.allocated_since_gc = 0;
    }
}
