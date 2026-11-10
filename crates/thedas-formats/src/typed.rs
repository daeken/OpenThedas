//! Shared helpers for the typed readers that sit on top of GFF4 documents (`.arl`, `.rml`,
//! `.mmh`, `.msh`). They read fields by label and turn "missing" and "wrong type" into errors
//! instead of panics.

use crate::gff4::{FourCC, Gff4, Gff4Error, List, StructId, Value};

/// An error from one of the typed GFF4 readers.
#[derive(Debug, thiserror::Error)]
pub enum TypedError {
    #[error(transparent)]
    Gff4(#[from] Gff4Error),
    #[error("expected file type {expected:?}, found {found}")]
    FileType {
        expected: &'static str,
        found: FourCC,
    },
    #[error("{context}: missing field {label}")]
    Missing { context: &'static str, label: u32 },
    #[error("{context}: field {label} has an unexpected type ({found})")]
    WrongType {
        context: &'static str,
        label: u32,
        found: &'static str,
    },
    #[error("{0}")]
    Invalid(String),
}

pub type TypedResult<T> = std::result::Result<T, TypedError>;

/// Maximum depth of a node tree we will follow. Real models nest a few levels deep; this only
/// guards against cycles and stack exhaustion on malformed input.
pub(crate) const MAX_DEPTH: usize = 256;

pub(crate) fn value_kind(v: &Value) -> &'static str {
    match v {
        Value::Absent => "absent",
        Value::U8(_) | Value::U16(_) | Value::U32(_) | Value::U64(_) => "unsigned",
        Value::I8(_) | Value::I16(_) | Value::I32(_) | Value::I64(_) => "signed",
        Value::F32(_) | Value::F64(_) => "float",
        Value::Vec3(_) => "vec3",
        Value::Vec4(_) => "vec4",
        Value::Quat(_) => "quat",
        Value::Color(_) => "color",
        Value::Mat4(_) => "mat4",
        Value::String(_) => "string",
        Value::Tlk(_) => "tlk",
        Value::Struct(_) => "struct",
        Value::Generic(_) => "generic",
        Value::List(_) => "list",
    }
}

/// Removes the trailing NULs that DA:O writes into many string fields.
pub(crate) fn clean(s: &str) -> String {
    s.trim_end_matches('\0').to_string()
}

/// Read access to one GFF4 document with errors tagged by context.
#[derive(Clone, Copy)]
pub(crate) struct Doc<'a> {
    pub doc: &'a Gff4,
    pub context: &'static str,
}

impl<'a> Doc<'a> {
    pub fn new(doc: &'a Gff4, context: &'static str) -> Self {
        Self { doc, context }
    }

    pub fn expect_type(&self, expected: &'static str) -> TypedResult<()> {
        if self.doc.file_type.0 != expected.as_bytes()[..4] {
            return Err(TypedError::FileType {
                expected,
                found: self.doc.file_type,
            });
        }
        Ok(())
    }

    pub fn label(&self, id: StructId) -> FourCC {
        self.doc.template_of(id).label
    }

    /// The raw value, unwrapping generic slots. `None` when the template has no such field,
    /// the field has no storage, or a generic slot is null.
    pub fn get(&self, id: StructId, label: u32) -> Option<&'a Value> {
        match self.doc.field(id, label)? {
            Value::Absent => None,
            Value::Generic(slot) => slot.value.as_ref(),
            v => Some(v),
        }
    }

    fn wrong(&self, label: u32, v: &Value) -> TypedError {
        TypedError::WrongType {
            context: self.context,
            label,
            found: value_kind(v),
        }
    }

    fn missing(&self, label: u32) -> TypedError {
        TypedError::Missing {
            context: self.context,
            label,
        }
    }

    /// A string field with trailing NULs removed. A null string pointer reads as `None`.
    pub fn opt_str(&self, id: StructId, label: u32) -> TypedResult<Option<String>> {
        match self.get(id, label) {
            None => Ok(None),
            Some(Value::String(s)) => Ok(s.as_deref().map(clean)),
            Some(v) => Err(self.wrong(label, v)),
        }
    }

    pub fn str(&self, id: StructId, label: u32) -> TypedResult<String> {
        self.opt_str(id, label)?.ok_or_else(|| self.missing(label))
    }

    pub fn opt_i64(&self, id: StructId, label: u32) -> TypedResult<Option<i64>> {
        match self.get(id, label) {
            None => Ok(None),
            Some(v) => v.as_i64().map(Some).ok_or_else(|| self.wrong(label, v)),
        }
    }

    pub fn i64(&self, id: StructId, label: u32) -> TypedResult<i64> {
        self.opt_i64(id, label)?.ok_or_else(|| self.missing(label))
    }

    /// An integer field that must fit in a `u32`.
    pub fn u32(&self, id: StructId, label: u32) -> TypedResult<u32> {
        let v = self.i64(id, label)?;
        u32::try_from(v).map_err(|_| {
            TypedError::Invalid(format!(
                "{}: field {label} value {v} is out of range",
                self.context
            ))
        })
    }

    pub fn opt_f32(&self, id: StructId, label: u32) -> TypedResult<Option<f32>> {
        match self.get(id, label) {
            None => Ok(None),
            Some(v) => v
                .as_f64()
                .map(|f| Some(f as f32))
                .ok_or_else(|| self.wrong(label, v)),
        }
    }

    /// Any 3- or 4-component vector, as four floats (`w` = `default_w` for a vec3).
    pub fn opt_vec4(
        &self,
        id: StructId,
        label: u32,
        default_w: f32,
    ) -> TypedResult<Option<[f32; 4]>> {
        match self.get(id, label) {
            None => Ok(None),
            Some(Value::Vec3([x, y, z])) => Ok(Some([*x, *y, *z, default_w])),
            Some(Value::Vec4(v) | Value::Quat(v) | Value::Color(v)) => Ok(Some(*v)),
            Some(v) => Err(self.wrong(label, v)),
        }
    }

    pub fn opt_vec3(&self, id: StructId, label: u32) -> TypedResult<Option<[f32; 3]>> {
        Ok(self.opt_vec4(id, label, 1.0)?.map(|[x, y, z, _]| [x, y, z]))
    }

    /// The structs in a list field: a struct list, or a generic list whose slots hold structs.
    /// A missing or null list is empty; null entries are skipped.
    pub fn structs(&self, id: StructId, label: u32) -> TypedResult<Vec<StructId>> {
        match self.get(id, label) {
            None | Some(Value::List(None)) => Ok(Vec::new()),
            Some(Value::List(Some(List::Struct(v)))) => Ok(v.iter().flatten().copied().collect()),
            Some(Value::List(Some(List::Generic(slots)))) => {
                let mut out = Vec::with_capacity(slots.len());
                for s in slots {
                    match &s.value {
                        None => {}
                        Some(Value::Struct(Some(c))) => out.push(*c),
                        Some(Value::Struct(None)) => {}
                        Some(v) => return Err(self.wrong(label, v)),
                    }
                }
                Ok(out)
            }
            Some(Value::Struct(Some(c))) => Ok(vec![*c]),
            Some(v) => Err(self.wrong(label, v)),
        }
    }

    /// A byte-list field (vertex and index blobs). Missing or null reads as empty.
    pub fn bytes(&self, id: StructId, label: u32) -> TypedResult<&'a [u8]> {
        match self.get(id, label) {
            None | Some(Value::List(None)) => Ok(&[]),
            Some(Value::List(Some(List::U8(v)))) => Ok(v),
            Some(v) => Err(self.wrong(label, v)),
        }
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds small GFF4 documents in memory for unit tests.

    use crate::gff4::{
        FieldDef, FourCC, GenericSlot, Gff4, List, StructData, StructId, StructTemplate, Value,
        Version,
    };

    pub struct Builder {
        pub doc: Gff4,
    }

    impl Builder {
        pub fn new(file_type: &[u8; 4]) -> Self {
            Self {
                doc: Gff4 {
                    version: Version::V40,
                    platform: FourCC::new(b"PC  "),
                    file_type: FourCC::new(file_type),
                    file_version: FourCC::new(b"V0.1"),
                    templates: Vec::new(),
                    structs: Vec::new(),
                    root: StructId(0),
                    shared_strings: None,
                },
            }
        }

        /// Adds a struct with the given label and (field label, value) pairs.
        pub fn add(&mut self, label: &[u8; 4], fields: Vec<(u32, Value)>) -> StructId {
            let template = self.doc.templates.len() as u32;
            self.doc.templates.push(StructTemplate {
                label: FourCC::new(label),
                fields: fields
                    .iter()
                    .map(|(l, _)| FieldDef {
                        label: *l,
                        type_code: 0,
                        flags: 0,
                        offset: 0,
                    })
                    .collect(),
                size: 0,
            });
            self.doc.structs.push(StructData {
                template,
                values: fields.into_iter().map(|(_, v)| v).collect(),
            });
            StructId(self.doc.structs.len() - 1)
        }

        pub fn finish(mut self, root: StructId) -> Gff4 {
            self.doc.root = root;
            self.doc
        }
    }

    pub fn s(text: &str) -> Value {
        Value::String(Some(format!("{text}\0")))
    }

    pub fn structs(ids: &[StructId]) -> Value {
        Value::List(Some(List::Struct(ids.iter().map(|&i| Some(i)).collect())))
    }

    pub fn generic(ids: &[StructId]) -> Value {
        Value::List(Some(List::Generic(
            ids.iter()
                .map(|&i| GenericSlot {
                    type_code: 0,
                    flags: 0,
                    value: Some(Value::Struct(Some(i))),
                })
                .collect(),
        )))
    }
}
