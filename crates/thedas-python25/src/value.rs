use std::{any::Any, cell::RefCell, collections::BTreeMap, fmt, rc::Rc};

use num_bigint::BigInt;
use num_traits::{ToPrimitive, Zero};

use crate::{Runtime, runtime::Function};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub class: String,
    pub message: String,
    pub line: Option<usize>,
    pub(crate) fatal: bool,
}

impl Error {
    pub fn new(class: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            class: class.into(),
            message: message.into(),
            line: None,
            fatal: false,
        }
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self {
            fatal: true,
            ..Self::new("Unsupported", message)
        }
    }

    pub(crate) fn type_error(message: impl Into<String>) -> Self {
        Self::new("TypeError", message)
    }

    pub(crate) fn at(mut self, line: usize) -> Self {
        self.line.get_or_insert(line);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.message)?;
        if let Some(line) = self.line {
            write!(f, " (line {line})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Error {}

#[derive(Default)]
pub struct Args {
    pub positional: Vec<Value>,
    pub named: BTreeMap<String, Value>,
}

impl Args {
    pub fn take<const N: usize>(self) -> Result<[Value; N]> {
        if self.positional.len() != N || !self.named.is_empty() {
            return Err(Error::type_error(format!(
                "expected {N} positional arguments"
            )));
        }
        self.positional
            .try_into()
            .map_err(|_| Error::type_error("incorrect argument count"))
    }
}

/// A host object can expose properties and bound methods to Python code.
pub trait NativeObject {
    fn buffer(&self) -> Option<Vec<u8>> {
        None
    }

    fn get(&self, name: &str) -> Result<Value> {
        Err(Error::new("AttributeError", name))
    }

    fn set(&self, name: &str, _value: Value) -> Result<()> {
        Err(Error::new("AttributeError", name))
    }

    fn binary(&self, _op: &str, _other: &Value) -> Option<Result<Value>> {
        None
    }

    fn compare(&self, _other: &Value) -> Option<std::cmp::Ordering> {
        None
    }

    fn truth(&self) -> bool {
        true
    }
}

#[derive(Clone)]
pub(crate) struct HostObject {
    pub native: Rc<dyn NativeObject>,
    typed: Rc<dyn Any>,
}

pub(crate) type Names = Rc<RefCell<BTreeMap<String, Value>>>;
pub(crate) type NativeFn = dyn Fn(&mut Runtime, Args) -> Result<Value>;
pub(crate) type Items = Rc<RefCell<Vec<(Value, Value)>>>;

#[derive(Clone)]
pub struct Value(pub(crate) Kind);

#[derive(Clone)]
pub(crate) enum Kind {
    None,
    Bool(bool),
    Int(BigInt),
    Float(f64),
    Bytes(Rc<[u8]>),
    Buffer(Rc<[u8]>),
    Unicode(Rc<str>),
    Tuple(Rc<[Value]>),
    List(Rc<RefCell<Vec<Value>>>),
    XRange {
        start: i64,
        step: i64,
        len: usize,
    },
    Dict(Items),
    Module(Names),
    Class {
        attrs: Names,
        base: Option<Rc<Value>>,
    },
    Instance {
        class: Rc<Value>,
        attrs: Names,
    },
    Function(Rc<Function>),
    Native(Rc<NativeFn>),
    Bound {
        function: Rc<Value>,
        receiver: Rc<Value>,
    },
    Host(HostObject),
    ExceptionType(Rc<str>),
    Exception(Rc<Error>),
}

impl Value {
    pub const NONE: Self = Self(Kind::None);

    pub fn int(value: impl Into<BigInt>) -> Self {
        Self(Kind::Int(value.into()))
    }
    pub fn float(value: f64) -> Self {
        Self(Kind::Float(value))
    }
    pub fn bool(value: bool) -> Self {
        Self(Kind::Bool(value))
    }
    pub fn bytes(value: impl AsRef<[u8]>) -> Self {
        Self(Kind::Bytes(value.as_ref().into()))
    }
    /// A read-only Python 2 buffer, such as a GFF blob returned by pywin32.
    pub fn buffer(value: impl AsRef<[u8]>) -> Self {
        Self(Kind::Buffer(value.as_ref().into()))
    }
    pub fn unicode(value: impl AsRef<str>) -> Self {
        Self(Kind::Unicode(value.as_ref().into()))
    }
    pub fn tuple(values: Vec<Self>) -> Self {
        Self(Kind::Tuple(values.into()))
    }
    pub fn list(values: Vec<Self>) -> Self {
        Self(Kind::List(Rc::new(RefCell::new(values))))
    }
    pub fn module(values: impl IntoIterator<Item = (impl Into<String>, Self)>) -> Self {
        Self(Kind::Module(Rc::new(RefCell::new(
            values.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        ))))
    }
    pub fn function(callback: impl Fn(&mut Runtime, Args) -> Result<Self> + 'static) -> Self {
        Self(Kind::Native(Rc::new(callback)))
    }
    pub fn object(object: impl NativeObject + 'static) -> Self {
        let object = Rc::new(object);
        Self(Kind::Host(HostObject {
            native: object.clone(),
            typed: object,
        }))
    }
    /// Recover a value owned by a native module that accepts typed object arguments.
    pub fn downcast_object<T: NativeObject + 'static>(&self) -> Option<&T> {
        match &self.0 {
            Kind::Host(object) => object.typed.downcast_ref(),
            _ => None,
        }
    }
    pub fn exception_type(name: impl AsRef<str>) -> Self {
        Self(Kind::ExceptionType(name.as_ref().into()))
    }

    pub fn as_int(&self) -> Result<BigInt> {
        match &self.0 {
            Kind::Int(value) => Ok(value.clone()),
            Kind::Bool(value) => Ok(BigInt::from(u8::from(*value))),
            _ => Err(Error::type_error("expected an integer")),
        }
    }

    pub fn as_i64(&self) -> Result<i64> {
        self.as_int()?
            .to_i64()
            .ok_or_else(|| Error::new("OverflowError", "integer out of range"))
    }

    pub fn as_f64(&self) -> Result<f64> {
        match &self.0 {
            Kind::Float(value) => Ok(*value),
            _ => self
                .as_int()?
                .to_f64()
                .filter(|value| value.is_finite())
                .ok_or_else(|| Error::new("OverflowError", "integer too large for float")),
        }
    }

    pub fn as_text(&self) -> Result<&str> {
        match &self.0 {
            Kind::Bytes(value) => std::str::from_utf8(value)
                .map_err(|_| Error::new("UnicodeDecodeError", "expected UTF-8")),
            Kind::Unicode(value) => Ok(value),
            _ => Err(Error::type_error("expected a string")),
        }
    }

    pub fn as_bytes(&self) -> Result<&[u8]> {
        match &self.0 {
            Kind::Bytes(value) | Kind::Buffer(value) => Ok(value),
            _ => Err(Error::type_error("expected a byte string")),
        }
    }

    pub fn truth(&self) -> bool {
        match &self.0 {
            Kind::None => false,
            Kind::Bool(value) => *value,
            Kind::Int(value) => !value.is_zero(),
            Kind::Float(value) => *value != 0.0,
            Kind::Bytes(value) | Kind::Buffer(value) => !value.is_empty(),
            Kind::Unicode(value) => !value.is_empty(),
            Kind::Tuple(value) => !value.is_empty(),
            Kind::List(value) => !value.borrow().is_empty(),
            Kind::XRange { len, .. } => *len != 0,
            Kind::Dict(value) => !value.borrow().is_empty(),
            Kind::Host(object) => object.native.truth(),
            _ => true,
        }
    }

    pub fn items(&self) -> Result<Vec<Self>> {
        match &self.0 {
            Kind::Tuple(values) => Ok(values.to_vec()),
            Kind::List(values) => Ok(values.borrow().clone()),
            Kind::XRange { start, step, len } => {
                if *len > 16_000_000 {
                    return Err(Error::unsupported(
                        "materialized range exceeds interpreter limit",
                    ));
                }
                let mut items = Vec::new();
                items
                    .try_reserve_exact(*len)
                    .map_err(|_| Error::new("MemoryError", "range allocation failed"))?;
                items.extend((0..*len).map(|index| {
                    Self::int(i128::from(*start) + i128::from(*step) * index as i128)
                }));
                Ok(items)
            }
            Kind::Dict(values) => Ok(values.borrow().iter().map(|(key, _)| key.clone()).collect()),
            Kind::Bytes(bytes) | Kind::Buffer(bytes) => {
                Ok(bytes.iter().map(|byte| Self::bytes([*byte])).collect())
            }
            Kind::Unicode(text) => Ok(text
                .chars()
                .map(|ch| Self::unicode(ch.to_string()))
                .collect()),
            _ => Err(Error::type_error("expected an iterable")),
        }
    }

    pub(crate) fn type_name(&self) -> &'static str {
        match self.0 {
            Kind::None => "NoneType",
            Kind::Bool(_) => "bool",
            Kind::Int(_) => "int",
            Kind::Float(_) => "float",
            Kind::Bytes(_) => "str",
            Kind::Buffer(_) => "buffer",
            Kind::Unicode(_) => "unicode",
            Kind::Tuple(_) => "tuple",
            Kind::List(_) => "list",
            Kind::Dict(_) => "dict",
            Kind::XRange { .. } => "xrange",
            Kind::Module(_) => "module",
            Kind::Class { .. } => "classobj",
            Kind::Instance { .. } | Kind::Host(_) => "instance",
            Kind::ExceptionType(_) | Kind::Exception(_) => "exception",
            Kind::Function(_) | Kind::Native(_) | Kind::Bound { .. } => "function",
        }
    }

    pub(crate) fn string(&self) -> String {
        match &self.0 {
            Kind::Bytes(value) | Kind::Buffer(value) => String::from_utf8_lossy(value).into_owned(),
            Kind::Unicode(value) => value.to_string(),
            Kind::Exception(error) => error.message.clone(),
            _ => format!("{self:?}"),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        repr(self, f, &mut Vec::new(), 0)
    }
}

fn repr(
    value: &Value,
    f: &mut fmt::Formatter<'_>,
    parents: &mut Vec<*const ()>,
    depth: usize,
) -> fmt::Result {
    if depth > 128 {
        return f.write_str("...");
    }
    match &value.0 {
        Kind::None => f.write_str("None"),
        Kind::Bool(true) => f.write_str("True"),
        Kind::Bool(false) => f.write_str("False"),
        Kind::Int(value) => write!(f, "{value}"),
        Kind::Float(value) => write!(f, "{value:?}"),
        Kind::Bytes(value) => {
            let quote = if value.contains(&b'\'') && !value.contains(&b'"') {
                b'"'
            } else {
                b'\''
            };
            write!(f, "{}", quote as char)?;
            for &byte in value.as_ref() {
                match byte {
                    b'\\' => f.write_str("\\\\")?,
                    b'\n' => f.write_str("\\n")?,
                    b'\r' => f.write_str("\\r")?,
                    b'\t' => f.write_str("\\t")?,
                    byte if byte == quote => write!(f, "\\{}", byte as char)?,
                    32..=126 => write!(f, "{}", byte as char)?,
                    _ => write!(f, "\\x{byte:02x}")?,
                }
            }
            write!(f, "{}", quote as char)
        }
        Kind::Unicode(value) => write!(f, "u{value:?}"),
        Kind::Tuple(values) => sequence_repr(values, f, parents, depth, true),
        Kind::List(values) => {
            let ptr = Rc::as_ptr(values).cast();
            if parents.contains(&ptr) {
                return f.write_str("[...]");
            }
            parents.push(ptr);
            let result = sequence_repr(&values.borrow(), f, parents, depth, false);
            parents.pop();
            result
        }
        Kind::Dict(values) => {
            let ptr = Rc::as_ptr(values).cast();
            if parents.contains(&ptr) {
                return f.write_str("{...}");
            }
            parents.push(ptr);
            f.write_str("{")?;
            for (index, (key, value)) in values.borrow().iter().enumerate() {
                if index != 0 {
                    f.write_str(", ")?;
                }
                repr(key, f, parents, depth + 1)?;
                f.write_str(": ")?;
                repr(value, f, parents, depth + 1)?;
            }
            parents.pop();
            f.write_str("}")
        }
        Kind::Exception(error) => write!(f, "{error}"),
        Kind::ExceptionType(name) => write!(f, "<exception {name}>"),
        _ => write!(f, "<{}>", value.type_name()),
    }
}

fn sequence_repr(
    values: &[Value],
    f: &mut fmt::Formatter<'_>,
    parents: &mut Vec<*const ()>,
    depth: usize,
    tuple: bool,
) -> fmt::Result {
    f.write_str(if tuple { "(" } else { "[" })?;
    for (i, item) in values.iter().enumerate() {
        if i != 0 {
            f.write_str(", ")?;
        }
        repr(item, f, parents, depth + 1)?;
    }
    if tuple && values.len() == 1 {
        f.write_str(",")?;
    }
    f.write_str(if tuple { ")" } else { "]" })
}
