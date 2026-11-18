//! The GFF reader interface used by LightMapper.py to load loose Toolset MMH/MSH files.
//!
//! The original 32-bit reader reports GFF FourCCs as little-endian integers. Its
//! automation bridge returns tuples for arrays and a Python buffer for the binary
//! vertex and index data. Generic references wrap their values until dereferenced.

use std::{cell::RefCell, fs, path::Path, rc::Rc};

use thedas_formats::gff4::{
    FieldDef, GenericSlot, Gff4, List, StructId, Value as GffValue, Version,
};
use thedas_python25::{Error, NativeObject, Result, Runtime, Value};

const GFF_MAGIC: u32 = u32::from_le_bytes(*b"GFF ");
const GFF40: u32 = u32::from_le_bytes(*b"V4.0");
const GFF41: u32 = u32::from_le_bytes(*b"V4.1");

/// Expose the subset of win32com and pywintypes used by the Toolset GFF reader.
pub fn install_gff_reader(runtime: &mut Runtime) -> Result<()> {
    runtime.enable_case_insensitive_reads();
    let dispatch = Value::function(|_, args| {
        let [class] = args.take()?;
        let class = class.as_text()?;
        if !class.eq_ignore_ascii_case("Bioware.Eclipse.EngGFFReader") {
            return Err(Error::unsupported(format!("COM class {class}")));
        }
        Ok(Value::object(Reader::default()))
    });
    runtime.register_module(
        "win32com.client",
        Value::module([
            ("Dispatch", dispatch.clone()),
            (
                "constants",
                Value::module([
                    ("EngGFFMagicNumber", Value::int(GFF_MAGIC)),
                    ("EngGFFVersion40", Value::int(GFF40)),
                    ("EngGFFVersion41", Value::int(GFF41)),
                ]),
            ),
        ]),
    )?;
    runtime.register_module(
        "win32com.client.gencache",
        Value::module([("EnsureDispatch", dispatch)]),
    )?;
    runtime.register_module(
        "pywintypes",
        Value::module([("com_error", Value::exception_type("com_error"))]),
    )
}

fn com_error(message: impl Into<String>) -> Error {
    Error::new("com_error", message)
}

#[derive(Default)]
struct Reader(Rc<RefCell<Option<Rc<Gff4>>>>);

impl NativeObject for Reader {
    fn get(&self, name: &str) -> Result<Value> {
        if name == "LoadGFFFromFile" {
            let state = self.0.clone();
            return Ok(Value::function(move |runtime, args| {
                let [path] = args.take()?;
                let path = path.as_text()?;
                let bytes = fs::read(runtime.resolve_read_path(Path::new(path)))
                    .map_err(|error| com_error(format!("{path}: {error}")))?;
                let parsed =
                    Gff4::parse(&bytes).map_err(|error| com_error(format!("{path}: {error}")))?;
                *state.borrow_mut() = Some(Rc::new(parsed));
                Ok(Value::NONE)
            }));
        }

        let state = self.0.borrow();
        let file = state
            .as_ref()
            .ok_or_else(|| com_error("No GFF file loaded"))?;
        match name {
            "MagicNumber" => Ok(Value::int(GFF_MAGIC)),
            "GFFVersion" => Ok(Value::int(match file.version {
                Version::V40 => GFF40,
                Version::V41 => GFF41,
            })),
            "TopLevelStruct" => Ok(node(file, file.root)),
            _ => Err(Error::new("AttributeError", name)),
        }
    }
}

fn node(file: &Rc<Gff4>, id: StructId) -> Value {
    Value::object(Struct {
        file: file.clone(),
        id,
    })
}

struct Struct {
    file: Rc<Gff4>,
    id: StructId,
}

impl NativeObject for Struct {
    fn get(&self, name: &str) -> Result<Value> {
        match name {
            "StructType" => Ok(Value::int(u32::from_le_bytes(
                self.file.template_of(self.id).label.0,
            ))),
            "GetValueByLabel" => {
                let file = self.file.clone();
                let id = self.id;
                Ok(Value::function(move |_, args| {
                    let [label] = args.take()?;
                    let label = u32::try_from(label.as_i64()?)
                        .map_err(|_| com_error("Field label outside u32"))?;
                    let template = file.template_of(id);
                    let index = template
                        .fields
                        .iter()
                        .position(|field| field.label == label)
                        .ok_or_else(|| com_error("Field not found with given label"))?;
                    let value = &file.get_struct(id).values[index];
                    if matches!(value, GffValue::Absent) {
                        return Err(com_error("Field not found with given label"));
                    }
                    convert(&file, value, Some(&template.fields[index]))
                }))
            }
            _ => Err(Error::new("AttributeError", name)),
        }
    }
}

struct Reference {
    file: Rc<Gff4>,
    target: Option<GffValue>,
}

impl NativeObject for Reference {
    fn get(&self, name: &str) -> Result<Value> {
        if name != "GetTargetValue" {
            return Err(Error::new("AttributeError", name));
        }
        let file = self.file.clone();
        let target = self.target.clone();
        Ok(Value::function(move |_, args| {
            let [] = args.take()?;
            target
                .as_ref()
                .map_or(Ok(Value::NONE), |target| convert(&file, target, None))
        }))
    }
}

fn reference(file: &Rc<Gff4>, target: Option<GffValue>) -> Value {
    Value::object(Reference {
        file: file.clone(),
        target,
    })
}

fn generic(file: &Rc<Gff4>, slot: &GenericSlot) -> Value {
    reference(file, slot.value.clone())
}

fn components(values: impl IntoIterator<Item = f32>) -> Value {
    Value::tuple(
        values
            .into_iter()
            .map(|value| Value::float(f64::from(value)))
            .collect(),
    )
}

fn com_string(value: &str) -> Value {
    // GFF4 retains the on-disk terminator. The COM string observed by
    // LightMapper.py excludes it, including in the referenced MSH filename.
    Value::unicode(value.trim_end_matches('\0'))
}

fn sequence<T: Copy>(values: &[T], convert: impl Fn(T) -> Value) -> Value {
    Value::tuple(values.iter().copied().map(convert).collect())
}

fn convert(file: &Rc<Gff4>, value: &GffValue, field: Option<&FieldDef>) -> Result<Value> {
    Ok(match value {
        GffValue::U8(value) => Value::int(*value),
        GffValue::I8(value) => Value::int(*value),
        GffValue::U16(value) => Value::int(*value),
        GffValue::I16(value) => Value::int(*value),
        GffValue::U32(value) => Value::int(*value),
        GffValue::I32(value) => Value::int(*value),
        GffValue::U64(value) => Value::int(*value),
        GffValue::I64(value) => Value::int(*value),
        GffValue::F32(value) => Value::float(f64::from(*value)),
        GffValue::F64(value) => Value::float(*value),
        GffValue::Vec3(values) => components(*values),
        GffValue::Vec4(values) | GffValue::Quat(values) | GffValue::Color(values) => {
            components(*values)
        }
        GffValue::Mat4(values) => components(*values),
        GffValue::String(value) => value.as_deref().map_or(Value::NONE, com_string),
        GffValue::Struct(id) => id.map_or(Value::NONE, |id| node(file, id)),
        GffValue::Generic(slot) => generic(file, slot),
        GffValue::List(None) => Value::NONE,
        GffValue::List(Some(values)) => match values {
            List::U8(bytes) if field.is_some_and(|field| matches!(field.label, 8022 | 8023)) => {
                Value::buffer(bytes)
            }
            List::U8(values) => sequence(values, Value::int),
            List::I8(values) => sequence(values, Value::int),
            List::U16(values) => sequence(values, Value::int),
            List::I16(values) => sequence(values, Value::int),
            List::U32(values) => sequence(values, Value::int),
            List::I32(values) => sequence(values, Value::int),
            List::U64(values) => sequence(values, Value::int),
            List::I64(values) => sequence(values, Value::int),
            List::F32(values) => sequence(values, |value| Value::float(f64::from(value))),
            List::F64(values) => sequence(values, Value::float),
            List::Vec3(values) => sequence(values, components),
            List::Vec4(values) | List::Quat(values) | List::Color(values) => {
                sequence(values, components)
            }
            List::Mat4(values) => sequence(values, components),
            List::String(values) => Value::tuple(
                values
                    .iter()
                    .map(|value| value.as_deref().map_or(Value::NONE, com_string))
                    .collect(),
            ),
            List::Struct(values) => {
                sequence(values, |id| id.map_or(Value::NONE, |id| node(file, id)))
            }
            List::Generic(values) => {
                Value::tuple(values.iter().map(|slot| generic(file, slot)).collect())
            }
        },
        GffValue::Absent => return Err(com_error("Field not found with given label")),
        GffValue::Tlk(_) => return Err(Error::unsupported("GFF reader talk-table field")),
    })
}
