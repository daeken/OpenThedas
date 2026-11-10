//! The NWScript virtual machine: a stack of 4-byte cells, a base pointer for globals, and a
//! call stack. Engine functions (`ACTION`) go to a [`Host`], so the VM itself is pure and
//! deterministic: the same script, arguments and host answers give the same run.
//!
//! Semantics follow the NWScript stack machine as BioWare's compilers target it. Choices the
//! public descriptions leave open, and the DA:O additions, are provisional:
//!
//! - `SAVEBP` pushes the old base pointer and sets it to the new stack top; `RESTOREBP` pops it.
//! - Integer and float division and modulo by zero abort the script.
//! - Arrays are values (copied by `CPTOPSP`); `WRITEARRAY` grows the array with default
//!   values; `READARRAY` past the end yields the element type's default. The array operand's
//!   offset is taken relative to the stack top before the index is popped.
//! - `CONST object 0` is `OBJECT_SELF` (the object the script runs on) and `1` is
//!   `OBJECT_INVALID`.

use std::sync::Arc;

use crate::ldf::{EngineTable, Function, Type};
use crate::ncs::{Arg, ConstArg, Ncs, Op, tc};

/// An object id as the engine hands them out.
pub type ObjectId = u32;

/// The engine's invalid object id (what `OBJECT_INVALID` evaluates to). Provisional.
pub const OBJECT_INVALID: ObjectId = 0x7F00_0000;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Int(i32),
    Float(f32),
    String(String),
    Object(ObjectId),
    Resource(String),
    /// An engine structure (event, location, command, effect, ...): a handle the host owns.
    Engine {
        ty: u8,
        handle: Option<u64>,
    },
    Array {
        elem: Type,
        items: Vec<Value>,
    },
    /// Only at the host boundary: vectors are three float cells on the stack.
    Vector([f32; 3]),
}

impl Value {
    pub fn default_for(t: &Type) -> Value {
        match t {
            Type::Int | Type::Void | Type::Any => Value::Int(0),
            Type::Float => Value::Float(0.0),
            Type::String => Value::String(String::new()),
            Type::Object => Value::Object(OBJECT_INVALID),
            Type::Resource => Value::Resource(String::new()),
            Type::Vector => Value::Vector([0.0; 3]),
            Type::Engine(n) => Value::Engine {
                ty: *n,
                handle: None,
            },
            Type::Array(e) => Value::Array {
                elem: (**e).clone(),
                items: Vec::new(),
            },
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::String(_) => "string",
            Value::Object(_) => "object",
            Value::Resource(_) => "resource",
            Value::Engine { .. } => "engine",
            Value::Array { .. } => "array",
            Value::Vector(_) => "vector",
        }
    }

    pub fn as_int(&self) -> Option<i32> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum VmError {
    #[error("stack underflow at {at:#x}")]
    StackUnderflow { at: usize },
    #[error("bad stack offset {offset} at {at:#x}")]
    BadOffset { offset: i64, at: usize },
    #[error("type mismatch at {at:#x}: expected {expected}, found {found}")]
    TypeMismatch {
        at: usize,
        expected: &'static str,
        found: &'static str,
    },
    #[error("division by zero at {at:#x}")]
    DivideByZero { at: usize },
    #[error("unknown engine function #{id} at {at:#x}")]
    UnknownAction { id: u16, at: usize },
    #[error("{name}: called with {argc} arguments, takes {min}..={max}")]
    BadArgc {
        name: String,
        argc: u8,
        min: usize,
        max: usize,
    },
    #[error("unsupported instruction {op} at {at:#x}")]
    Unsupported { op: &'static str, at: usize },
    #[error("jump to {target:#x} at {at:#x} is not an instruction")]
    BadJump { at: usize, target: usize },
    #[error("instruction budget of {0} exhausted")]
    Budget(u64),
    #[error("engine function {name}: {message}")]
    Host { name: String, message: String },
}

pub type VmResult<T> = Result<T, VmError>;

/// What a script can ask of the engine.
pub trait Host {
    /// Runs engine function `f` for the script running on `caller`. `args` are in declaration
    /// order (vectors as [`Value::Vector`]). Return `None` for void functions; a `None` for a
    /// non-void function pushes the return type's default.
    fn action(
        &mut self,
        caller: ObjectId,
        f: &Function,
        args: Vec<Value>,
    ) -> VmResult<Option<Value>>;
}

/// A host that implements nothing: every call returns the default of its return type.
/// Records what was called, for tracing.
#[derive(Default)]
pub struct NullHost {
    pub calls: Vec<(String, Vec<Value>)>,
}

impl Host for NullHost {
    fn action(&mut self, _: ObjectId, f: &Function, args: Vec<Value>) -> VmResult<Option<Value>> {
        self.calls.push((f.name.clone(), args));
        Ok(None)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RunResult {
    /// The value left at the bottom of the stack (a conditional script's `int` result).
    pub value: Option<Value>,
    pub steps: u64,
}

pub struct Vm {
    ncs: Arc<Ncs>,
    table: Arc<EngineTable>,
    pub budget: u64,
}

struct State {
    stack: Vec<Value>,
    bp: usize,
    calls: Vec<usize>,
    pc: usize,
}

impl Vm {
    pub fn new(ncs: Arc<Ncs>, table: Arc<EngineTable>) -> Self {
        Self {
            ncs,
            table,
            budget: 10_000_000,
        }
    }

    /// Runs the script from the start on `caller` (`OBJECT_SELF`).
    pub fn run(&self, host: &mut dyn Host, caller: ObjectId) -> VmResult<RunResult> {
        let mut st = State {
            stack: Vec::new(),
            bp: 0,
            calls: Vec::new(),
            pc: 0,
        };
        let mut steps = 0u64;
        let code = &self.ncs.instructions;
        // Falling off the end ends the script.
        while let Some(ins) = code.get(st.pc) {
            steps += 1;
            if steps > self.budget {
                return Err(VmError::Budget(self.budget));
            }
            let mut next = st.pc + 1;
            match self.step(&mut st, ins, host, caller, &mut next)? {
                Flow::Continue => st.pc = next,
                Flow::Halt => break,
            }
        }
        Ok(RunResult {
            value: st.stack.first().cloned(),
            steps,
        })
    }

    fn jump(&self, at: usize, target: Option<usize>) -> VmResult<usize> {
        let t = target.unwrap_or(usize::MAX);
        self.ncs.at(t).ok_or(VmError::BadJump { at, target: t })
    }

    fn step(
        &self,
        st: &mut State,
        ins: &crate::ncs::Instruction,
        host: &mut dyn Host,
        caller: ObjectId,
        next: &mut usize,
    ) -> VmResult<Flow> {
        let at = ins.offset;
        let pop = |st: &mut State| st.stack.pop().ok_or(VmError::StackUnderflow { at });
        let pop_int = |st: &mut State| match pop(st)? {
            Value::Int(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                at,
                expected: "int",
                found: other.kind(),
            }),
        };
        let pop_float = |st: &mut State| match pop(st)? {
            Value::Float(v) => Ok(v),
            other => Err(VmError::TypeMismatch {
                at,
                expected: "float",
                found: other.kind(),
            }),
        };
        let cell = |base: usize, offset: i32| -> VmResult<usize> {
            if offset % 4 != 0 {
                return Err(VmError::BadOffset {
                    offset: offset as i64,
                    at,
                });
            }
            let i = base as i64 + (offset / 4) as i64;
            if i < 0 {
                return Err(VmError::BadOffset {
                    offset: offset as i64,
                    at,
                });
            }
            Ok(i as usize)
        };
        let cells = |size: u16| -> VmResult<usize> {
            if size % 4 != 0 {
                return Err(VmError::BadOffset {
                    offset: size as i64,
                    at,
                });
            }
            Ok(size as usize / 4)
        };
        use Op::*;
        match (ins.op, &ins.arg) {
            (CpDownSp | CpDownBp, Arg::Copy { offset, size }) => {
                let n = cells(*size)?;
                let base = if ins.op == CpDownSp {
                    st.stack.len()
                } else {
                    st.bp
                };
                let dst = cell(base, *offset)?;
                let src = st
                    .stack
                    .len()
                    .checked_sub(n)
                    .ok_or(VmError::StackUnderflow { at })?;
                if dst + n > st.stack.len() {
                    return Err(VmError::BadOffset {
                        offset: *offset as i64,
                        at,
                    });
                }
                for k in 0..n {
                    st.stack[dst + k] = st.stack[src + k].clone();
                }
            }
            (CpTopSp | CpTopBp, Arg::Copy { offset, size }) => {
                let n = cells(*size)?;
                let base = if ins.op == CpTopSp {
                    st.stack.len()
                } else {
                    st.bp
                };
                let src = cell(base, *offset)?;
                if src + n > st.stack.len() {
                    return Err(VmError::BadOffset {
                        offset: *offset as i64,
                        at,
                    });
                }
                for k in 0..n {
                    let v = st.stack[src + k].clone();
                    st.stack.push(v);
                }
            }
            (RsAdd, _) => {
                let t = crate::ncs::cell_type(ins.ty).ok_or(VmError::Unsupported {
                    op: "RSADD of unknown type",
                    at,
                })?;
                st.stack.push(Value::default_for(&t));
            }
            (Const, Arg::Const(c)) => st.stack.push(match c {
                ConstArg::Int(v) => Value::Int(*v),
                ConstArg::Float(v) => Value::Float(*v),
                ConstArg::String(s) => Value::String(s.clone()),
                ConstArg::Resource(s) => Value::Resource(s.clone()),
                ConstArg::Object(0) => Value::Object(caller),
                ConstArg::Object(1) => Value::Object(OBJECT_INVALID),
                ConstArg::Object(o) => Value::Object(*o),
            }),
            (Action, Arg::Action { id, argc }) => {
                let f = self
                    .table
                    .function(*id)
                    .ok_or(VmError::UnknownAction { id: *id, at })?;
                let argc = *argc as usize;
                if argc > f.params.len() || argc < f.required() {
                    return Err(VmError::BadArgc {
                        name: f.name.clone(),
                        argc: argc as u8,
                        min: f.required(),
                        max: f.params.len(),
                    });
                }
                let mut args = Vec::with_capacity(argc);
                for p in &f.params[..argc] {
                    if p.ty == Type::Vector {
                        let z = pop_float(st)?;
                        let y = pop_float(st)?;
                        let x = pop_float(st)?;
                        args.push(Value::Vector([x, y, z]));
                    } else {
                        args.push(pop(st)?);
                    }
                }
                let ret = host.action(caller, f, args)?;
                if f.ret != Type::Void {
                    push_return(st, &f.ret, ret);
                }
            }
            (LogAnd | LogOr | IncOr | ExcOr | BoolAnd | ShLeft | ShRight | UShRight | Mod, _) => {
                let b = pop_int(st)?;
                let a = pop_int(st)?;
                st.stack.push(Value::Int(match ins.op {
                    LogAnd => (a != 0 && b != 0) as i32,
                    LogOr => (a != 0 || b != 0) as i32,
                    IncOr => a | b,
                    ExcOr => a ^ b,
                    BoolAnd => a & b,
                    ShLeft => a.wrapping_shl(b as u32),
                    ShRight => a.wrapping_shr(b as u32),
                    UShRight => ((a as u32).wrapping_shr(b as u32)) as i32,
                    _ => {
                        if b == 0 {
                            return Err(VmError::DivideByZero { at });
                        }
                        a.wrapping_rem(b)
                    }
                }));
            }
            (Eq | Neq, _) => {
                let equal = if ins.ty == tc::STRUCT_STRUCT {
                    let Arg::StructSize(size) = ins.arg else {
                        unreachable!()
                    };
                    let n = cells(size)?;
                    if st.stack.len() < 2 * n {
                        return Err(VmError::StackUnderflow { at });
                    }
                    let b = st.stack.split_off(st.stack.len() - n);
                    let a = st.stack.split_off(st.stack.len() - n);
                    a == b
                } else {
                    let b = pop(st)?;
                    let a = pop(st)?;
                    match (&a, &b) {
                        (Value::Float(x), Value::Float(y)) => x == y,
                        _ if std::mem::discriminant(&a) == std::mem::discriminant(&b) => a == b,
                        _ => {
                            return Err(VmError::TypeMismatch {
                                at,
                                expected: a.kind(),
                                found: b.kind(),
                            });
                        }
                    }
                };
                st.stack.push(Value::Int((equal == (ins.op == Eq)) as i32));
            }
            (Geq | Gt | Lt | Leq, _) => {
                let r = match ins.ty {
                    tc::INT_INT => {
                        let b = pop_int(st)?;
                        let a = pop_int(st)?;
                        cmp(ins.op, a.partial_cmp(&b))
                    }
                    tc::FLOAT_FLOAT => {
                        let b = pop_float(st)?;
                        let a = pop_float(st)?;
                        cmp(ins.op, a.partial_cmp(&b))
                    }
                    _ => {
                        return Err(VmError::Unsupported {
                            op: "comparison type",
                            at,
                        });
                    }
                };
                st.stack.push(Value::Int(r as i32));
            }
            (Add | Sub | Mul | Div, _) => self.arith(st, ins, at)?,
            (Neg, _) => match pop(st)? {
                Value::Int(v) => st.stack.push(Value::Int(v.wrapping_neg())),
                Value::Float(v) => st.stack.push(Value::Float(-v)),
                other => {
                    return Err(VmError::TypeMismatch {
                        at,
                        expected: "int or float",
                        found: other.kind(),
                    });
                }
            },
            (Comp, _) => {
                let v = pop_int(st)?;
                st.stack.push(Value::Int(!v));
            }
            (Not, _) => {
                let v = pop_int(st)?;
                st.stack.push(Value::Int((v == 0) as i32));
            }
            (MovSp, Arg::Offset(o)) => {
                let keep = cell(st.stack.len(), *o)?;
                if keep > st.stack.len() {
                    return Err(VmError::BadOffset {
                        offset: *o as i64,
                        at,
                    });
                }
                st.stack.truncate(keep);
            }
            (Jmp, _) => *next = self.jump(at, ins.target())?,
            (Jsr, _) => {
                st.calls.push(st.pc + 1);
                *next = self.jump(at, ins.target())?;
            }
            (Jz | Jnz, _) => {
                let v = pop_int(st)?;
                if (v == 0) == (ins.op == Jz) {
                    *next = self.jump(at, ins.target())?;
                }
            }
            (Retn, _) => match st.calls.pop() {
                Some(r) => *next = r,
                None => return Ok(Flow::Halt),
            },
            (Destruct, Arg::Destruct { size, offset, keep }) => {
                let n = cells(*size)?;
                let start = st
                    .stack
                    .len()
                    .checked_sub(n)
                    .ok_or(VmError::StackUnderflow { at })?;
                let k0 = start as i64 + (*offset as i64) / 4;
                let k1 = k0 + cells(*keep)? as i64;
                if k0 < start as i64 || k1 > st.stack.len() as i64 {
                    return Err(VmError::BadOffset {
                        offset: *offset as i64,
                        at,
                    });
                }
                let kept: Vec<Value> = st.stack[k0 as usize..k1 as usize].to_vec();
                st.stack.truncate(start);
                st.stack.extend(kept);
            }
            (DecISp | IncISp | DecIBp | IncIBp, Arg::Offset(o)) => {
                let base = if matches!(ins.op, DecISp | IncISp) {
                    st.stack.len()
                } else {
                    st.bp
                };
                let i = cell(base, *o)?;
                let delta = if matches!(ins.op, IncISp | IncIBp) {
                    1
                } else {
                    -1
                };
                match st.stack.get_mut(i) {
                    Some(Value::Int(v)) => *v = v.wrapping_add(delta),
                    Some(other) => {
                        return Err(VmError::TypeMismatch {
                            at,
                            expected: "int",
                            found: other.kind(),
                        });
                    }
                    None => {
                        return Err(VmError::BadOffset {
                            offset: *o as i64,
                            at,
                        });
                    }
                }
            }
            (SaveBp, _) => {
                st.stack.push(Value::Int(st.bp as i32));
                st.bp = st.stack.len();
            }
            (RestoreBp, _) => {
                let v = pop_int(st)?;
                st.bp = v.max(0) as usize;
            }
            (Nop, _) => {}
            (WriteArray | ReadArray, Arg::Copy { offset, size }) => {
                let arr = cell(st.stack.len(), *offset)?;
                let n = cells(*size)?;
                if n != 1 {
                    return Err(VmError::Unsupported {
                        op: "multi-cell array elements",
                        at,
                    });
                }
                let index = pop_int(st)?;
                if ins.op == WriteArray {
                    let value = st
                        .stack
                        .last()
                        .cloned()
                        .ok_or(VmError::StackUnderflow { at })?;
                    match st.stack.get_mut(arr) {
                        Some(Value::Array { elem, items }) => {
                            if index < 0 {
                                return Err(VmError::BadOffset {
                                    offset: index as i64,
                                    at,
                                });
                            }
                            let i = index as usize;
                            if i >= items.len() {
                                let d = Value::default_for(elem);
                                items.resize(i + 1, d);
                            }
                            items[i] = value;
                        }
                        other => {
                            return Err(VmError::TypeMismatch {
                                at,
                                expected: "array",
                                found: other.map_or("nothing", |v| v.kind()),
                            });
                        }
                    }
                } else {
                    let v = match st.stack.get(arr) {
                        Some(Value::Array { elem, items }) => usize::try_from(index)
                            .ok()
                            .and_then(|i| items.get(i).cloned())
                            .unwrap_or_else(|| Value::default_for(elem)),
                        other => {
                            return Err(VmError::TypeMismatch {
                                at,
                                expected: "array",
                                found: other.map_or("nothing", |v| v.kind()),
                            });
                        }
                    };
                    st.stack.push(v);
                }
            }
            (StoreState | StoreStateAll | Unknown2E, _) => {
                return Err(VmError::Unsupported {
                    op: ins.op.mnemonic(),
                    at,
                });
            }
            (op, _) => {
                return Err(VmError::Unsupported {
                    op: op.mnemonic(),
                    at,
                });
            }
        }
        Ok(Flow::Continue)
    }

    fn arith(&self, st: &mut State, ins: &crate::ncs::Instruction, at: usize) -> VmResult<()> {
        let op = ins.op;
        let take = |st: &mut State, n: usize| -> VmResult<Vec<Value>> {
            if st.stack.len() < n {
                return Err(VmError::StackUnderflow { at });
            }
            Ok(st.stack.split_off(st.stack.len() - n))
        };
        let f = |v: &Value| match v {
            Value::Float(x) => Ok(*x),
            Value::Int(x) => Ok(*x as f32),
            other => Err(VmError::TypeMismatch {
                at,
                expected: "number",
                found: other.kind(),
            }),
        };
        let float_op = |a: f32, b: f32| -> VmResult<f32> {
            Ok(match op {
                Op::Add => a + b,
                Op::Sub => a - b,
                Op::Mul => a * b,
                _ => {
                    if b == 0.0 {
                        return Err(VmError::DivideByZero { at });
                    }
                    a / b
                }
            })
        };
        match ins.ty {
            tc::INT_INT => {
                let v = take(st, 2)?;
                let (Value::Int(a), Value::Int(b)) = (&v[0], &v[1]) else {
                    return Err(VmError::TypeMismatch {
                        at,
                        expected: "int",
                        found: v[0].kind(),
                    });
                };
                let r = match op {
                    Op::Add => a.wrapping_add(*b),
                    Op::Sub => a.wrapping_sub(*b),
                    Op::Mul => a.wrapping_mul(*b),
                    _ => {
                        if *b == 0 {
                            return Err(VmError::DivideByZero { at });
                        }
                        a.wrapping_div(*b)
                    }
                };
                st.stack.push(Value::Int(r));
            }
            tc::FLOAT_FLOAT | tc::INT_FLOAT | tc::FLOAT_INT => {
                let v = take(st, 2)?;
                st.stack.push(Value::Float(float_op(f(&v[0])?, f(&v[1])?)?));
            }
            tc::STRING_STRING if op == Op::Add => {
                let v = take(st, 2)?;
                match (&v[0], &v[1]) {
                    (Value::String(a), Value::String(b)) => {
                        st.stack.push(Value::String(format!("{a}{b}")))
                    }
                    _ => {
                        return Err(VmError::TypeMismatch {
                            at,
                            expected: "string",
                            found: v[0].kind(),
                        });
                    }
                }
            }
            tc::VECTOR_VECTOR if matches!(op, Op::Add | Op::Sub) => {
                let v = take(st, 6)?;
                for k in 0..3 {
                    st.stack
                        .push(Value::Float(float_op(f(&v[k])?, f(&v[k + 3])?)?));
                }
            }
            tc::VECTOR_FLOAT if matches!(op, Op::Mul | Op::Div) => {
                let v = take(st, 4)?;
                let s = f(&v[3])?;
                for x in &v[..3] {
                    st.stack.push(Value::Float(float_op(f(x)?, s)?));
                }
            }
            tc::FLOAT_VECTOR if op == Op::Mul => {
                let v = take(st, 4)?;
                let s = f(&v[0])?;
                for x in &v[1..] {
                    st.stack.push(Value::Float(float_op(s, f(x)?)?));
                }
            }
            _ => {
                return Err(VmError::Unsupported {
                    op: "arithmetic type",
                    at,
                });
            }
        }
        Ok(())
    }
}

enum Flow {
    Continue,
    Halt,
}

fn cmp(op: Op, o: Option<std::cmp::Ordering>) -> bool {
    use std::cmp::Ordering::*;
    match (op, o) {
        (_, None) => false,
        (Op::Gt, Some(x)) => x == Greater,
        (Op::Geq, Some(x)) => x != Less,
        (Op::Lt, Some(x)) => x == Less,
        (Op::Leq, Some(x)) => x != Greater,
        _ => false,
    }
}

fn host_vector(v: Option<Value>) -> [f32; 3] {
    match v {
        Some(Value::Vector(v)) => v,
        _ => [0.0; 3],
    }
}

fn push_return(st: &mut State, t: &Type, v: Option<Value>) {
    match t {
        Type::Vector => {
            for x in host_vector(v) {
                st.stack.push(Value::Float(x));
            }
        }
        _ => st.stack.push(v.unwrap_or_else(|| Value::default_for(t))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ncs::tests::assemble;

    const LDF: &str = "#define ENGINE_STRUCTURE_0 event\n\
        void PrintInteger(int n) = 0;\n\
        int AddOne(int n) = 1;\n\
        vector Vec(float x, float y, float z) = 2;\n\
        float VecX(vector v) = 3;\n\
        string GetTag(object o) = 4;\n";

    struct TestHost {
        log: Vec<String>,
    }

    impl Host for TestHost {
        fn action(
            &mut self,
            caller: ObjectId,
            f: &Function,
            args: Vec<Value>,
        ) -> VmResult<Option<Value>> {
            self.log.push(format!("{}{:?}", f.name, args));
            Ok(match f.name.as_str() {
                "AddOne" => Some(Value::Int(args[0].as_int().unwrap() + 1)),
                "Vec" => {
                    let g = |v: &Value| match v {
                        Value::Float(x) => *x,
                        _ => 0.0,
                    };
                    Some(Value::Vector([g(&args[0]), g(&args[1]), g(&args[2])]))
                }
                "VecX" => match &args[0] {
                    Value::Vector(v) => Some(Value::Float(v[0])),
                    _ => None,
                },
                "GetTag" => Some(Value::String(
                    format!(
                        "obj{:x}",
                        match args[0] {
                            Value::Object(o) => o,
                            _ => 0,
                        }
                    ) + if caller == 7 { "" } else { "?" },
                )),
                _ => None,
            })
        }
    }

    fn run(body: &[u8]) -> (VmResult<RunResult>, Vec<String>) {
        let ncs = Arc::new(Ncs::parse(&assemble(body)).unwrap());
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut host = TestHost { log: Vec::new() };
        let r = Vm::new(ncs, table).run(&mut host, 7);
        (r, host.log)
    }

    fn i32b(v: i32) -> [u8; 4] {
        v.to_be_bytes()
    }

    #[test]
    fn runs_a_conditional_with_calls_and_branches() {
        // RSADD I; JSR main; RETN
        // main: CONST I 41; ACTION AddOne(1); CONST I 42; EQUALII; JZ else;
        //       CONST I 1; CPDOWNSP -8,4; MOVSP -4; RETN
        // else: CONST I 0; CPDOWNSP -8,4; MOVSP -4; RETN
        let mut b = vec![0x02, 0x03];
        b.extend([0x1E, 0x00]);
        b.extend(i32b(8));
        b.extend([0x20, 0x00]);
        // main at 13+10 = 23
        b.extend([0x04, 0x03]);
        b.extend(i32b(41));
        b.extend([0x05, 0x00, 0x00, 0x01, 0x01]);
        b.extend([0x04, 0x03]);
        b.extend(i32b(42));
        b.extend([0x0B, 0x20]);
        b.extend([0x1F, 0x00]);
        b.extend(i32b(6 + 6 + 8 + 6 + 2));
        b.extend([0x04, 0x03]);
        b.extend(i32b(1));
        b.extend([0x01, 0x01]);
        b.extend(i32b(-8));
        b.extend([0x00, 0x04]);
        b.extend([0x1B, 0x00]);
        b.extend(i32b(-4));
        b.extend([0x20, 0x00]);
        b.extend([0x04, 0x03]);
        b.extend(i32b(0));
        b.extend([0x01, 0x01]);
        b.extend(i32b(-8));
        b.extend([0x00, 0x04]);
        b.extend([0x1B, 0x00]);
        b.extend(i32b(-4));
        b.extend([0x20, 0x00]);
        let (r, log) = run(&b);
        let r = r.unwrap();
        assert_eq!(r.value, Some(Value::Int(1)));
        assert_eq!(log, vec!["AddOne[Int(41)]"]);
    }

    #[test]
    fn vectors_strings_objects_and_arrays() {
        let mut b = Vec::new();
        // Vec(1, 2, 3): args pushed last first.
        for x in [3.0f32, 2.0, 1.0] {
            b.extend([0x04, 0x04]);
            b.extend(x.to_bits().to_be_bytes());
        }
        b.extend([0x05, 0x00, 0x00, 0x02, 0x03]);
        // VecX(v) -> 1.0
        b.extend([0x05, 0x00, 0x00, 0x03, 0x01]);
        b.extend([0x1B, 0x00]);
        b.extend(i32b(-4));
        // GetTag(OBJECT_SELF) + "!"
        b.extend([0x04, 0x06, 0, 0, 0, 0]);
        b.extend([0x05, 0x00, 0x00, 0x04, 0x01]);
        b.extend([0x04, 0x05, 0x00, 0x01, b'!']);
        b.extend([0x14, 0x23]);
        b.extend([0x1B, 0x00]);
        b.extend(i32b(-4));
        // int[] a; a[2] = 9; push a[2]; PrintInteger
        b.extend([0x02, 0x40]);
        b.extend([0x04, 0x03]);
        b.extend(i32b(9));
        b.extend([0x04, 0x03]);
        b.extend(i32b(2));
        b.extend([0x30, 0x01]);
        b.extend(i32b(-12));
        b.extend([0x00, 0x04]);
        b.extend([0x1B, 0x00]);
        b.extend(i32b(-4));
        b.extend([0x04, 0x03]);
        b.extend(i32b(2));
        b.extend([0x32, 0x01]);
        b.extend(i32b(-8));
        b.extend([0x00, 0x04]);
        b.extend([0x05, 0x00, 0x00, 0x00, 0x01]);
        b.extend([0x20, 0x00]);
        let (r, log) = run(&b);
        r.unwrap();
        assert_eq!(
            log,
            vec![
                "Vec[Float(1.0), Float(2.0), Float(3.0)]",
                "VecX[Vector([1.0, 2.0, 3.0])]",
                "GetTag[Object(7)]",
                "PrintInteger[Int(9)]",
            ]
        );
    }

    #[test]
    fn errors_are_reported_not_panics() {
        // Underflow.
        assert!(matches!(
            run(&[0x14, 0x20]).0,
            Err(VmError::StackUnderflow { .. })
        ));
        // Divide by zero.
        let mut b = vec![0x04, 0x03];
        b.extend(i32b(1));
        b.extend([0x04, 0x03]);
        b.extend(i32b(0));
        b.extend([0x17, 0x20]);
        assert!(matches!(run(&b).0, Err(VmError::DivideByZero { .. })));
        // Unknown action.
        assert!(matches!(
            run(&[0x05, 0x00, 0x01, 0x00, 0x00]).0,
            Err(VmError::UnknownAction { .. })
        ));
        // Infinite loop hits the budget.
        let mut b = vec![0x1D, 0x00];
        b.extend(i32b(0));
        let ncs = Arc::new(Ncs::parse(&assemble(&b)).unwrap());
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut vm = Vm::new(ncs, table);
        vm.budget = 1000;
        assert_eq!(
            vm.run(&mut NullHost::default(), 0),
            Err(VmError::Budget(1000))
        );
        // Type confusion.
        let mut b = vec![0x04, 0x05, 0x00, 0x00, 0x04, 0x03];
        b.extend(i32b(1));
        b.extend([0x14, 0x20]);
        assert!(matches!(run(&b).0, Err(VmError::TypeMismatch { .. })));
    }
}
