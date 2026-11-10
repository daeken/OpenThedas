//! GFF4, BioWare's generic binary tree format in Dragon Age (V4.0) and Dragon Age II (V4.1).
//!
//! Layout, as read here:
//!
//! - Header: `GFF ` + version, platform, file type, file type version, struct template count,
//!   (V4.1 only: shared string count and offset), and the data offset. Offsets stored inside the
//!   data section are relative to the data offset; template field tables use absolute offsets.
//! - Struct templates: a label, a field table and a byte size. Every struct instance uses a
//!   template, and template 0 is the root struct, which sits at the start of the data section.
//! - Fields: a numeric label, a type code and flags. Flag `0x8000` makes the field a list
//!   (inline value is a pointer to `count, elements...`), `0x4000` means the type code is a
//!   struct template index, and `0x2000` means the value is stored behind a pointer.
//! - Generic fields (type `0xFFFF`) carry their own type and flags per value.
//!
//! The reader builds an owned graph. Structs live in an arena so that shared and cyclic
//! references survive a round trip. The writer emits a canonical layout, which is not byte for
//! byte what BioWare's tools produced but reads back to the same graph.
//!
//! Only little-endian (PC) files are supported; console files are rejected.

use std::collections::{HashMap, VecDeque};
use std::fmt;

const NULL: u32 = 0xFFFF_FFFF;

const FLAG_LIST: u16 = 0x8000;
const FLAG_STRUCT: u16 = 0x4000;
const FLAG_REFERENCE: u16 = 0x2000;

/// The type code of a generic field.
pub const TYPE_GENERIC: u16 = 0xFFFF;

#[derive(Debug, thiserror::Error)]
pub enum Gff4Error {
    #[error("not a GFF4 file")]
    NotGff4,
    #[error("unsupported GFF4 version {0}")]
    UnsupportedVersion(FourCC),
    #[error("unsupported platform {0} (only PC files are supported)")]
    UnsupportedPlatform(FourCC),
    #[error("read past end of data at offset {offset:#x} (+{len})")]
    OutOfBounds { offset: u64, len: u64 },
    #[error("struct template index {0} out of range")]
    BadTemplate(u32),
    #[error("unknown field type {type_code} (flags {flags:#06x})")]
    UnknownType { type_code: u16, flags: u16 },
    #[error("unsupported field configuration: type {type_code}, flags {flags:#06x}")]
    UnsupportedConfig { type_code: u16, flags: u16 },
    #[error("string at {offset:#x} is not valid UTF-16")]
    BadString { offset: u64 },
    #[error("shared string index {0} out of range")]
    BadSharedString(u32),
    #[error("value does not match its field declaration: {0}")]
    Mismatch(String),
    #[error("file has no struct templates")]
    NoStructs,
}

pub type Result<T> = std::result::Result<T, Gff4Error>;

/// A four-character code, stored in file order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FourCC(pub [u8; 4]);

impl FourCC {
    pub const fn new(s: &[u8; 4]) -> Self {
        FourCC(*s)
    }
}

impl fmt::Display for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for &b in &self.0 {
            if b.is_ascii_graphic() || b == b' ' {
                write!(f, "{}", b as char)?;
            } else {
                write!(f, "\\x{b:02x}")?;
            }
        }
        Ok(())
    }
}

impl fmt::Debug for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "'{self}'")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    V40,
    V41,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDef {
    pub label: u32,
    pub type_code: u16,
    pub flags: u16,
    /// Offset of the field inside its struct, or `0xFFFFFFFF` if the field has no storage.
    pub offset: u32,
}

impl FieldDef {
    pub fn is_list(&self) -> bool {
        self.flags & FLAG_LIST != 0
    }
    pub fn is_struct(&self) -> bool {
        self.flags & FLAG_STRUCT != 0
    }
    pub fn is_reference(&self) -> bool {
        self.flags & FLAG_REFERENCE != 0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StructTemplate {
    pub label: FourCC,
    pub fields: Vec<FieldDef>,
    /// Byte size of one instance.
    pub size: u32,
}

/// Index of a struct in [`Gff4::structs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructId(pub usize);

#[derive(Clone, Debug)]
pub struct StructData {
    pub template: u32,
    /// One value per field of the template, in template order.
    pub values: Vec<Value>,
}

/// A talk-table reference with an optional inline override string.
#[derive(Clone, Debug, PartialEq)]
pub struct TlkString {
    pub id: u32,
    pub text: Option<String>,
}

/// One value in a generic field: it carries its own type code and flags.
#[derive(Clone, Debug)]
pub struct GenericSlot {
    pub type_code: u16,
    pub flags: u16,
    /// `None` when the slot's pointer is null.
    pub value: Option<Value>,
}

#[derive(Clone, Debug)]
pub enum List {
    U8(Vec<u8>),
    I8(Vec<i8>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    U64(Vec<u64>),
    I64(Vec<i64>),
    F32(Vec<f32>),
    F64(Vec<f64>),
    Vec3(Vec<[f32; 3]>),
    Vec4(Vec<[f32; 4]>),
    Quat(Vec<[f32; 4]>),
    Color(Vec<[f32; 4]>),
    Mat4(Vec<[f32; 16]>),
    String(Vec<Option<String>>),
    Struct(Vec<Option<StructId>>),
    Generic(Vec<GenericSlot>),
}

impl List {
    pub fn len(&self) -> usize {
        match self {
            List::U8(v) => v.len(),
            List::I8(v) => v.len(),
            List::U16(v) => v.len(),
            List::I16(v) => v.len(),
            List::U32(v) => v.len(),
            List::I32(v) => v.len(),
            List::U64(v) => v.len(),
            List::I64(v) => v.len(),
            List::F32(v) => v.len(),
            List::F64(v) => v.len(),
            List::Vec3(v) => v.len(),
            List::Vec4(v) | List::Quat(v) | List::Color(v) => v.len(),
            List::Mat4(v) => v.len(),
            List::String(v) => v.len(),
            List::Struct(v) => v.len(),
            List::Generic(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    /// The template declares the field but gives it no storage.
    Absent,
    U8(u8),
    I8(i8),
    U16(u16),
    I16(i16),
    U32(u32),
    I32(i32),
    U64(u64),
    I64(i64),
    F32(f32),
    F64(f64),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
    Quat([f32; 4]),
    Color([f32; 4]),
    Mat4([f32; 16]),
    /// `None` for a null string pointer.
    String(Option<String>),
    Tlk(TlkString),
    /// `None` for a null struct pointer. Inline structs are always `Some`.
    Struct(Option<StructId>),
    Generic(Box<GenericSlot>),
    /// `None` for a null list pointer, as distinct from an empty list.
    List(Option<List>),
}

impl Value {
    /// The value as an unsigned integer, if it is any integer type.
    pub fn as_u64(&self) -> Option<u64> {
        Some(match *self {
            Value::U8(v) => v as u64,
            Value::I8(v) => v as u64,
            Value::U16(v) => v as u64,
            Value::I16(v) => v as u64,
            Value::U32(v) => v as u64,
            Value::I32(v) => v as u64,
            Value::U64(v) => v,
            Value::I64(v) => v as u64,
            _ => return None,
        })
    }

    /// The value as a signed integer, if it is any integer type.
    pub fn as_i64(&self) -> Option<i64> {
        Some(match *self {
            Value::U8(v) => v as i64,
            Value::I8(v) => v as i64,
            Value::U16(v) => v as i64,
            Value::I16(v) => v as i64,
            Value::U32(v) => v as i64,
            Value::I32(v) => v as i64,
            Value::U64(v) => v as i64,
            Value::I64(v) => v,
            _ => return None,
        })
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Value::F32(v) => Some(v as f64),
            Value::F64(v) => Some(v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(Some(s)) => Some(s),
            _ => None,
        }
    }

    pub fn as_struct(&self) -> Option<StructId> {
        match self {
            Value::Struct(s) => *s,
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&List> {
        match self {
            Value::List(Some(l)) => Some(l),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Gff4 {
    pub version: Version,
    pub platform: FourCC,
    pub file_type: FourCC,
    pub file_version: FourCC,
    pub templates: Vec<StructTemplate>,
    pub structs: Vec<StructData>,
    pub root: StructId,
    /// V4.1 only: the shared string table that string fields index into.
    pub shared_strings: Option<Vec<String>>,
}

/// Whether `data` starts with a GFF4 header.
pub fn is_gff4(data: &[u8]) -> bool {
    data.len() >= 8 && &data[..4] == b"GFF " && &data[4..6] == b"V4"
}

fn scalar_size(type_code: u16) -> Option<u32> {
    Some(match type_code {
        0 | 1 => 1,
        2 | 3 => 2,
        4 | 5 | 8 => 4,
        6 | 7 | 9 => 8,
        10 => 12,
        12 | 13 | 15 => 16,
        16 => 64,
        _ => return None,
    })
}

const TYPE_STRING: u16 = 14;
const TYPE_TLK: u16 = 17;

impl Gff4 {
    pub fn parse(data: &[u8]) -> Result<Self> {
        Reader::parse(data)
    }

    pub fn write(&self) -> Result<Vec<u8>> {
        Writer::write(self)
    }

    pub fn root(&self) -> &StructData {
        &self.structs[self.root.0]
    }

    pub fn get_struct(&self, id: StructId) -> &StructData {
        &self.structs[id.0]
    }

    pub fn template_of(&self, id: StructId) -> &StructTemplate {
        &self.templates[self.structs[id.0].template as usize]
    }

    /// The value of the field with `label` in struct `id`, if the template has it.
    pub fn field(&self, id: StructId, label: u32) -> Option<&Value> {
        let s = &self.structs[id.0];
        let t = &self.templates[s.template as usize];
        t.fields
            .iter()
            .position(|f| f.label == label)
            .map(|i| &s.values[i])
    }
}

// ---------------------------------------------------------------------------------------------
// Reading

struct Reader<'a> {
    d: &'a [u8],
    data_offset: u64,
    templates: Vec<StructTemplate>,
    shared: Option<Vec<String>>,
    structs: Vec<StructData>,
    memo: HashMap<(u64, u32), StructId>,
    queue: VecDeque<(StructId, u64)>,
}

impl<'a> Reader<'a> {
    fn parse(d: &'a [u8]) -> Result<Gff4> {
        if !is_gff4(d) || d.len() < 28 {
            return Err(Gff4Error::NotGff4);
        }
        let fcc = |o: usize| FourCC(d[o..o + 4].try_into().unwrap());
        let version = match &d[4..8] {
            b"V4.0" => Version::V40,
            b"V4.1" => Version::V41,
            _ => return Err(Gff4Error::UnsupportedVersion(fcc(4))),
        };
        let platform = fcc(8);
        if &platform.0 != b"PC  " {
            return Err(Gff4Error::UnsupportedPlatform(platform));
        }
        let file_type = fcc(12);
        let file_version = fcc(16);
        let mut r = Reader {
            d,
            data_offset: 0,
            templates: Vec::new(),
            shared: None,
            structs: Vec::new(),
            memo: HashMap::new(),
            queue: VecDeque::new(),
        };
        let struct_count = r.u32(20)?;
        let (strings, data_offset, templates_at) = match version {
            Version::V40 => (None, r.u32(24)?, 28u64),
            Version::V41 => (Some((r.u32(24)?, r.u32(28)?)), r.u32(32)?, 36u64),
        };
        r.data_offset = data_offset as u64;
        if struct_count == 0 {
            return Err(Gff4Error::NoStructs);
        }
        r.check(templates_at, struct_count as u64 * 16)?;
        for i in 0..struct_count as u64 {
            let at = templates_at + i * 16;
            let label = FourCC(r.bytes(at, 4)?.try_into().unwrap());
            let field_count = r.u32(at + 4)?;
            let field_offset = r.u32(at + 8)?;
            let size = r.u32(at + 12)?;
            let mut fields = Vec::new();
            if field_offset != NULL {
                r.check(field_offset as u64, field_count as u64 * 12)?;
                fields.reserve(field_count as usize);
                for j in 0..field_count as u64 {
                    let fa = field_offset as u64 + j * 12;
                    let type_and_flags = r.u32(fa + 4)?;
                    fields.push(FieldDef {
                        label: r.u32(fa)?,
                        type_code: type_and_flags as u16,
                        flags: (type_and_flags >> 16) as u16,
                        offset: r.u32(fa + 8)?,
                    });
                }
            } else if field_count != 0 {
                return Err(Gff4Error::Mismatch(
                    "template has fields but no field table".into(),
                ));
            }
            r.templates.push(StructTemplate {
                label,
                fields,
                size,
            });
        }
        if let Some((count, offset)) = strings {
            if offset != NULL {
                let mut table = Vec::with_capacity(count.min(1 << 20) as usize);
                let mut at = offset as u64;
                for _ in 0..count {
                    let rest =
                        r.d.get(at as usize..)
                            .ok_or(Gff4Error::OutOfBounds { offset: at, len: 1 })?;
                    let end = rest
                        .iter()
                        .position(|&b| b == 0)
                        .ok_or(Gff4Error::OutOfBounds {
                            offset: at,
                            len: rest.len() as u64 + 1,
                        })?;
                    table.push(String::from_utf8_lossy(&rest[..end]).into_owned());
                    at += end as u64 + 1;
                }
                r.shared = Some(table);
            } else {
                r.shared = Some(Vec::new());
            }
        }

        let root = r.struct_at(r.data_offset, 0)?;
        while let Some((id, at)) = r.queue.pop_front() {
            r.fill_struct(id, at)?;
        }

        Ok(Gff4 {
            version,
            platform,
            file_type,
            file_version,
            templates: r.templates,
            structs: r.structs,
            root,
            shared_strings: r.shared,
        })
    }

    fn check(&self, offset: u64, len: u64) -> Result<()> {
        match offset.checked_add(len) {
            Some(end) if end <= self.d.len() as u64 => Ok(()),
            _ => Err(Gff4Error::OutOfBounds { offset, len }),
        }
    }

    fn bytes(&self, offset: u64, len: u64) -> Result<&'a [u8]> {
        self.check(offset, len)?;
        Ok(&self.d[offset as usize..(offset + len) as usize])
    }

    fn u32(&self, offset: u64) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(offset, 4)?.try_into().unwrap(),
        ))
    }

    fn rel(&self, ptr: u32) -> u64 {
        self.data_offset + ptr as u64
    }

    /// Returns the struct at `at` with template `t`, scheduling it for reading if new.
    fn struct_at(&mut self, at: u64, t: u32) -> Result<StructId> {
        if let Some(&id) = self.memo.get(&(at, t)) {
            return Ok(id);
        }
        let tmpl = self
            .templates
            .get(t as usize)
            .ok_or(Gff4Error::BadTemplate(t))?;
        self.check(at, tmpl.size as u64)?;
        let id = StructId(self.structs.len());
        self.structs.push(StructData {
            template: t,
            values: Vec::new(),
        });
        self.memo.insert((at, t), id);
        self.queue.push_back((id, at));
        Ok(id)
    }

    fn fill_struct(&mut self, id: StructId, at: u64) -> Result<()> {
        let t = self.structs[id.0].template as usize;
        let n = self.templates[t].fields.len();
        let mut values = Vec::with_capacity(n);
        for i in 0..n {
            let f = &self.templates[t].fields[i];
            let (offset, type_code, flags) = (f.offset, f.type_code, f.flags);
            if offset == NULL {
                values.push(Value::Absent);
                continue;
            }
            values.push(self.value(at + offset as u64, type_code, flags, false)?);
        }
        self.structs[id.0].values = values;
        Ok(())
    }

    /// Reads a list header behind the pointer at `at`: returns the element start and count.
    fn list_at(&self, at: u64, elem: u64) -> Result<Option<(u64, u32)>> {
        let ptr = self.u32(at)?;
        if ptr == NULL {
            return Ok(None);
        }
        let start = self.rel(ptr);
        let count = self.u32(start)?;
        self.check(start + 4, count as u64 * elem)?;
        Ok(Some((start + 4, count)))
    }

    fn string_body(&self, at: u64) -> Result<String> {
        let len = self.u32(at)? as u64;
        let raw = self.bytes(at + 4, len * 2)?;
        let units: Vec<u16> = raw
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16(&units).map_err(|_| Gff4Error::BadString { offset: at })
    }

    /// A string stored as a pointer (or, with shared strings, an index) at `at`.
    fn string_ref(&self, at: u64) -> Result<Option<String>> {
        let v = self.u32(at)?;
        if v == NULL {
            return Ok(None);
        }
        match &self.shared {
            Some(table) => table
                .get(v as usize)
                .cloned()
                .map(Some)
                .ok_or(Gff4Error::BadSharedString(v)),
            None => self.string_body(self.rel(v)).map(Some),
        }
    }

    fn value(&mut self, at: u64, type_code: u16, flags: u16, generic: bool) -> Result<Value> {
        let is_list = flags & FLAG_LIST != 0;
        let is_ref = flags & FLAG_REFERENCE != 0;

        if flags & FLAG_STRUCT != 0 {
            let t = type_code as u32;
            let size = self
                .templates
                .get(t as usize)
                .ok_or(Gff4Error::BadTemplate(t))?
                .size;
            if is_list {
                let elem = if is_ref { 4 } else { size as u64 };
                let Some((start, count)) = self.list_at(at, elem)? else {
                    return Ok(Value::List(None));
                };
                let mut v = Vec::with_capacity(count as usize);
                for i in 0..count as u64 {
                    let p = start + i * elem;
                    v.push(if is_ref {
                        match self.u32(p)? {
                            NULL => None,
                            ptr => Some(self.struct_at(self.rel(ptr), t)?),
                        }
                    } else {
                        Some(self.struct_at(p, t)?)
                    });
                }
                return Ok(Value::List(Some(List::Struct(v))));
            }
            if is_ref {
                return Ok(Value::Struct(match self.u32(at)? {
                    NULL => None,
                    ptr => Some(self.struct_at(self.rel(ptr), t)?),
                }));
            }
            return Ok(Value::Struct(Some(self.struct_at(at, t)?)));
        }

        if type_code == TYPE_GENERIC {
            if generic {
                return Err(Gff4Error::UnsupportedConfig { type_code, flags });
            }
            if is_list {
                let Some((start, count)) = self.list_at(at, 8)? else {
                    return Ok(Value::List(None));
                };
                let mut v = Vec::with_capacity(count as usize);
                for i in 0..count as u64 {
                    v.push(self.generic_slot(start + i * 8, is_ref)?);
                }
                return Ok(Value::List(Some(List::Generic(v))));
            }
            return Ok(Value::Generic(Box::new(self.generic_slot(at, is_ref)?)));
        }

        if type_code == TYPE_STRING {
            if is_list {
                let Some((start, count)) = self.list_at(at, 4)? else {
                    return Ok(Value::List(None));
                };
                let mut v = Vec::with_capacity(count as usize);
                for i in 0..count as u64 {
                    v.push(self.string_ref(start + i * 4)?);
                }
                return Ok(Value::List(Some(List::String(v))));
            }
            if generic && self.shared.is_none() {
                return Ok(Value::String(Some(self.string_body(at)?)));
            }
            return Ok(Value::String(self.string_ref(at)?));
        }

        if type_code == TYPE_TLK {
            if is_list {
                return Err(Gff4Error::UnsupportedConfig { type_code, flags });
            }
            let id = self.u32(at)?;
            let off = self.u32(at + 4)?;
            let text = match (&self.shared, off) {
                (_, NULL) => None,
                (Some(table), i) => Some(
                    table
                        .get(i as usize)
                        .cloned()
                        .ok_or(Gff4Error::BadSharedString(i))?,
                ),
                (None, 0) => None,
                (None, o) => Some(self.string_body(self.rel(o))?),
            };
            return Ok(Value::Tlk(TlkString { id, text }));
        }

        let size =
            scalar_size(type_code).ok_or(Gff4Error::UnknownType { type_code, flags })? as u64;
        if is_list {
            let Some((start, count)) = self.list_at(at, size)? else {
                return Ok(Value::List(None));
            };
            let raw = self.bytes(start, count as u64 * size)?;
            return Ok(Value::List(Some(decode_list(type_code, raw))));
        }
        if is_ref {
            return Err(Gff4Error::UnsupportedConfig { type_code, flags });
        }
        Ok(decode_scalar(type_code, self.bytes(at, size)?))
    }

    fn generic_slot(&mut self, at: u64, is_ref: bool) -> Result<GenericSlot> {
        let tf = self.u32(at)?;
        let (type_code, flags) = (tf as u16, (tf >> 16) as u16);
        let loc = if is_ref {
            match self.u32(at + 4)? {
                NULL => {
                    return Ok(GenericSlot {
                        type_code,
                        flags,
                        value: None,
                    });
                }
                ptr => self.rel(ptr),
            }
        } else {
            at + 4
        };
        let value = self.value(loc, type_code, flags, true)?;
        Ok(GenericSlot {
            type_code,
            flags,
            value: Some(value),
        })
    }
}

fn f32s<const N: usize>(b: &[u8]) -> [f32; N] {
    let mut out = [0f32; N];
    for (i, c) in b.chunks_exact(4).take(N).enumerate() {
        out[i] = f32::from_le_bytes(c.try_into().unwrap());
    }
    out
}

fn decode_scalar(type_code: u16, b: &[u8]) -> Value {
    match type_code {
        0 => Value::U8(b[0]),
        1 => Value::I8(b[0] as i8),
        2 => Value::U16(u16::from_le_bytes(b.try_into().unwrap())),
        3 => Value::I16(i16::from_le_bytes(b.try_into().unwrap())),
        4 => Value::U32(u32::from_le_bytes(b.try_into().unwrap())),
        5 => Value::I32(i32::from_le_bytes(b.try_into().unwrap())),
        6 => Value::U64(u64::from_le_bytes(b.try_into().unwrap())),
        7 => Value::I64(i64::from_le_bytes(b.try_into().unwrap())),
        8 => Value::F32(f32::from_le_bytes(b.try_into().unwrap())),
        9 => Value::F64(f64::from_le_bytes(b.try_into().unwrap())),
        10 => Value::Vec3(f32s(b)),
        12 => Value::Vec4(f32s(b)),
        13 => Value::Quat(f32s(b)),
        15 => Value::Color(f32s(b)),
        16 => Value::Mat4(f32s(b)),
        _ => unreachable!("scalar_size accepted the type"),
    }
}

fn decode_list(type_code: u16, b: &[u8]) -> List {
    fn each<T, const N: usize>(b: &[u8], f: impl Fn([u8; N]) -> T) -> Vec<T> {
        b.chunks_exact(N)
            .map(|c| f(c.try_into().unwrap()))
            .collect()
    }
    match type_code {
        0 => List::U8(b.to_vec()),
        1 => List::I8(b.iter().map(|&x| x as i8).collect()),
        2 => List::U16(each(b, u16::from_le_bytes)),
        3 => List::I16(each(b, i16::from_le_bytes)),
        4 => List::U32(each(b, u32::from_le_bytes)),
        5 => List::I32(each(b, i32::from_le_bytes)),
        6 => List::U64(each(b, u64::from_le_bytes)),
        7 => List::I64(each(b, i64::from_le_bytes)),
        8 => List::F32(each(b, f32::from_le_bytes)),
        9 => List::F64(each(b, f64::from_le_bytes)),
        10 => List::Vec3(b.chunks_exact(12).map(f32s).collect()),
        12 => List::Vec4(b.chunks_exact(16).map(f32s).collect()),
        13 => List::Quat(b.chunks_exact(16).map(f32s).collect()),
        15 => List::Color(b.chunks_exact(16).map(f32s).collect()),
        16 => List::Mat4(b.chunks_exact(64).map(f32s).collect()),
        _ => unreachable!("scalar_size accepted the type"),
    }
}

// ---------------------------------------------------------------------------------------------
// Writing

struct Writer<'a> {
    doc: &'a Gff4,
    /// The data section; offsets in here are relative to the data offset.
    buf: Vec<u8>,
    memo: HashMap<StructId, u32>,
    queue: VecDeque<(StructId, u32)>,
    /// (struct, location) pairs already written, so a struct queued twice is written once.
    written: std::collections::HashSet<(StructId, u32)>,
    /// Where each inline struct lives: inside its parent, or in its parent's inline list.
    /// References to such a struct must point at that home rather than at a copy.
    homes: HashMap<StructId, Home>,
    /// Inline struct list blocks, allocated once per (owner struct, field index).
    list_blocks: HashMap<(StructId, usize), u32>,
    /// The struct and field currently being written, for inline list placement.
    owner: Option<(StructId, usize)>,
    shared: Option<(Vec<String>, HashMap<String, u32>)>,
}

#[derive(Clone, Copy)]
enum Home {
    Field {
        parent: StructId,
        offset: u32,
    },
    ListElem {
        parent: StructId,
        field: usize,
        index: u32,
        size: u32,
    },
}

fn find_homes(doc: &Gff4) -> HashMap<StructId, Home> {
    let mut homes = HashMap::new();
    for (si, s) in doc.structs.iter().enumerate() {
        let parent = StructId(si);
        let t = &doc.templates[s.template as usize];
        for (fi, (f, v)) in t.fields.iter().zip(&s.values).enumerate() {
            if !f.is_struct() || f.is_reference() || f.offset == NULL {
                continue;
            }
            match v {
                Value::Struct(Some(c)) if !f.is_list() => {
                    homes.entry(*c).or_insert(Home::Field {
                        parent,
                        offset: f.offset,
                    });
                }
                Value::List(Some(List::Struct(items))) if f.is_list() => {
                    let size = doc
                        .templates
                        .get(f.type_code as usize)
                        .map_or(0, |t| t.size);
                    for (k, c) in items.iter().enumerate() {
                        if let Some(c) = c {
                            homes.entry(*c).or_insert(Home::ListElem {
                                parent,
                                field: fi,
                                index: k as u32,
                                size,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
    }
    homes.remove(&doc.root);
    homes
}

fn mismatch(what: impl Into<String>) -> Gff4Error {
    Gff4Error::Mismatch(what.into())
}

impl<'a> Writer<'a> {
    fn write(doc: &'a Gff4) -> Result<Vec<u8>> {
        let table = match (doc.version, &doc.shared_strings) {
            (Version::V41, None) => Some(Vec::new()),
            (_, t) => t.clone(),
        };
        let shared = table.map(|t| {
            let index = t
                .iter()
                .enumerate()
                .map(|(i, s)| (s.clone(), i as u32))
                .collect();
            (t, index)
        });
        let mut w = Writer {
            doc,
            buf: Vec::new(),
            memo: HashMap::new(),
            queue: VecDeque::new(),
            written: Default::default(),
            homes: find_homes(doc),
            list_blocks: HashMap::new(),
            owner: None,
            shared,
        };
        if doc.structs[doc.root.0].template != 0 {
            return Err(mismatch("the root struct must use template 0"));
        }
        let root_size = doc.templates[0].size;
        let root_at = w.alloc(root_size);
        w.memo.insert(doc.root, root_at);
        w.queue.push_back((doc.root, root_at));
        while let Some((id, at)) = w.queue.pop_front() {
            if !w.written.insert((id, at)) {
                continue;
            }
            w.write_struct(id, at)?;
        }

        let header_len: u32 = match doc.version {
            Version::V40 => 28,
            Version::V41 => 36,
        };
        let templates_len = doc.templates.len() as u32 * 16;
        let fields_len: u32 = doc
            .templates
            .iter()
            .map(|t| t.fields.len() as u32 * 12)
            .sum();
        let data_offset = header_len + templates_len + fields_len;

        let mut out = Vec::with_capacity(data_offset as usize + w.buf.len());
        out.extend_from_slice(b"GFF ");
        out.extend_from_slice(match doc.version {
            Version::V40 => b"V4.0",
            Version::V41 => b"V4.1",
        });
        out.extend_from_slice(&doc.platform.0);
        out.extend_from_slice(&doc.file_type.0);
        out.extend_from_slice(&doc.file_version.0);
        out.extend_from_slice(&(doc.templates.len() as u32).to_le_bytes());
        let mut string_table = Vec::new();
        if doc.version == Version::V41 {
            let (table, _) = w
                .shared
                .as_ref()
                .expect("V4.1 documents carry a string table");
            for s in table {
                string_table.extend_from_slice(s.as_bytes());
                string_table.push(0);
            }
            let string_offset = data_offset + w.buf.len() as u32;
            out.extend_from_slice(&(table.len() as u32).to_le_bytes());
            out.extend_from_slice(&string_offset.to_le_bytes());
        }
        out.extend_from_slice(&data_offset.to_le_bytes());

        let mut field_at = header_len + templates_len;
        for t in &doc.templates {
            out.extend_from_slice(&t.label.0);
            out.extend_from_slice(&(t.fields.len() as u32).to_le_bytes());
            let fo = if t.fields.is_empty() { NULL } else { field_at };
            out.extend_from_slice(&fo.to_le_bytes());
            out.extend_from_slice(&t.size.to_le_bytes());
            field_at += t.fields.len() as u32 * 12;
        }
        for t in &doc.templates {
            for f in &t.fields {
                out.extend_from_slice(&f.label.to_le_bytes());
                let tf = f.type_code as u32 | (f.flags as u32) << 16;
                out.extend_from_slice(&tf.to_le_bytes());
                out.extend_from_slice(&f.offset.to_le_bytes());
            }
        }
        out.extend_from_slice(&w.buf);
        out.extend_from_slice(&string_table);
        Ok(out)
    }

    fn alloc(&mut self, size: u32) -> u32 {
        while self.buf.len() % 4 != 0 {
            self.buf.push(0);
        }
        let at = self.buf.len() as u32;
        self.buf.resize(self.buf.len() + size as usize, 0);
        at
    }

    fn put(&mut self, at: u32, bytes: &[u8]) {
        let at = at as usize;
        self.buf[at..at + bytes.len()].copy_from_slice(bytes);
    }

    fn put_u32(&mut self, at: u32, v: u32) {
        self.put(at, &v.to_le_bytes());
    }

    fn template_size(&self, t: u32) -> Result<u32> {
        self.doc
            .templates
            .get(t as usize)
            .map(|t| t.size)
            .ok_or(Gff4Error::BadTemplate(t))
    }

    /// The location of a struct, allocating it on first use. A struct that lives inline
    /// somewhere is placed at that home, so every reference to it points at one copy.
    fn struct_ref(&mut self, id: StructId) -> Result<u32> {
        if let Some(&at) = self.memo.get(&id) {
            return Ok(at);
        }
        let at = match self.homes.get(&id).copied() {
            Some(Home::Field { parent, offset }) => self.struct_ref(parent)? + offset,
            Some(Home::ListElem {
                parent,
                field,
                index,
                size,
            }) => {
                self.struct_ref(parent)?;
                self.inline_list_block(parent, field)? + index * size
            }
            None => {
                let size = self.template_size(self.doc.structs[id.0].template)?;
                self.alloc(size)
            }
        };
        self.memo.insert(id, at);
        self.queue.push_back((id, at));
        Ok(at)
    }

    /// The element start of the inline struct list in field `field` of `owner`.
    fn inline_list_block(&mut self, owner: StructId, field: usize) -> Result<u32> {
        if let Some(&start) = self.list_blocks.get(&(owner, field)) {
            return Ok(start);
        }
        let s = &self.doc.structs[owner.0];
        let f = &self.doc.templates[s.template as usize].fields[field];
        let Value::List(Some(List::Struct(items))) = &s.values[field] else {
            return Err(mismatch("inline struct list home is not a struct list"));
        };
        let size = self.template_size(f.type_code as u32)?;
        let start = self.list_block(items.len(), size);
        self.list_blocks.insert((owner, field), start);
        Ok(start)
    }

    /// If every struct in `items` is homed, in order, at the start of one inline list of the
    /// same length, returns that list's (owner, field).
    fn shared_list_home(&self, items: &[Option<StructId>]) -> Option<(StructId, usize)> {
        let first = (*items.first()?)?;
        let Some(&Home::ListElem { parent, field, .. }) = self.homes.get(&first) else {
            return None;
        };
        let Value::List(Some(List::Struct(home_items))) = &self.doc.structs[parent.0].values[field]
        else {
            return None;
        };
        (home_items.as_slice() == items).then_some((parent, field))
    }

    /// Places a struct inline at `at`.
    fn struct_inline(&mut self, id: StructId, at: u32) {
        match self.memo.get(&id) {
            // Already placed here (it was reached through a reference first).
            Some(&home) if home == at => {}
            Some(_) => self.queue.push_back((id, at)),
            None => {
                self.memo.insert(id, at);
                self.queue.push_back((id, at));
            }
        }
    }

    fn write_struct(&mut self, id: StructId, at: u32) -> Result<()> {
        let s = &self.doc.structs[id.0];
        let t = &self.doc.templates[s.template as usize];
        if s.values.len() != t.fields.len() {
            return Err(mismatch(format!(
                "struct {} has {} values for {} fields",
                id.0,
                s.values.len(),
                t.fields.len()
            )));
        }
        for (fi, (f, v)) in t.fields.iter().zip(&s.values).enumerate() {
            if f.offset == NULL {
                if !matches!(v, Value::Absent) {
                    return Err(mismatch("value for a field without storage"));
                }
                continue;
            }
            self.owner = Some((id, fi));
            self.write_value(at + f.offset, v, f.type_code, f.flags, false)?;
        }
        Ok(())
    }

    fn alloc_string(&mut self, s: &str) -> u32 {
        let units: Vec<u16> = s.encode_utf16().collect();
        let at = self.alloc(4 + units.len() as u32 * 2);
        self.put_u32(at, units.len() as u32);
        for (i, u) in units.iter().enumerate() {
            self.put(at + 4 + i as u32 * 2, &u.to_le_bytes());
        }
        at
    }

    fn shared_index(&mut self, s: &str) -> u32 {
        let (table, index) = self
            .shared
            .as_mut()
            .expect("only called with shared strings");
        if let Some(&i) = index.get(s) {
            return i;
        }
        let i = table.len() as u32;
        table.push(s.to_string());
        index.insert(s.to_string(), i);
        i
    }

    /// A string pointer (or shared index) for the given string.
    fn string_ref(&mut self, s: &Option<String>) -> u32 {
        match s {
            None => NULL,
            Some(s) if self.shared.is_some() => self.shared_index(s),
            Some(s) => self.alloc_string(s),
        }
    }

    fn list_block(&mut self, count: usize, elem: u32) -> u32 {
        let at = self.alloc(4 + count as u32 * elem);
        self.put_u32(at, count as u32);
        at + 4
    }

    fn write_value(
        &mut self,
        at: u32,
        v: &Value,
        type_code: u16,
        flags: u16,
        generic: bool,
    ) -> Result<()> {
        let is_list = flags & FLAG_LIST != 0;
        let is_ref = flags & FLAG_REFERENCE != 0;

        if is_list {
            let list = match v {
                Value::List(None) => {
                    self.put_u32(at, NULL);
                    return Ok(());
                }
                Value::List(Some(l)) => l,
                _ => return Err(mismatch("list field holds a non-list value")),
            };
            let owner = if generic { None } else { self.owner.take() };
            return self.write_list(at, list, type_code, flags, owner);
        }

        if flags & FLAG_STRUCT != 0 {
            let Value::Struct(s) = v else {
                return Err(mismatch("struct field holds a non-struct value"));
            };
            if is_ref {
                let p = match s {
                    None => NULL,
                    Some(id) => self.struct_ref(*id)?,
                };
                self.put_u32(at, p);
            } else {
                let id = s.ok_or_else(|| mismatch("inline struct is null"))?;
                self.struct_inline(id, at);
            }
            return Ok(());
        }

        if type_code == TYPE_GENERIC {
            let Value::Generic(slot) = v else {
                return Err(mismatch("generic field holds a non-generic value"));
            };
            return self.write_generic(at, slot, is_ref);
        }

        if type_code == TYPE_STRING {
            let Value::String(s) = v else {
                return Err(mismatch("string field holds a non-string value"));
            };
            if generic && self.shared.is_none() {
                // Generic string bodies are placed by write_generic; an inline (non-reference)
                // generic string would need its body in four bytes.
                return Err(Gff4Error::UnsupportedConfig { type_code, flags });
            }
            let p = self.string_ref(s);
            self.put_u32(at, p);
            return Ok(());
        }

        if type_code == TYPE_TLK {
            let Value::Tlk(t) = v else {
                return Err(mismatch("talk string field holds another value"));
            };
            self.put_u32(at, t.id);
            let p = self.string_ref(&t.text);
            self.put_u32(at + 4, p);
            return Ok(());
        }

        let bytes = encode_scalar(type_code, v)?;
        self.put(at, &bytes);
        Ok(())
    }

    fn write_list(
        &mut self,
        at: u32,
        list: &List,
        type_code: u16,
        flags: u16,
        owner: Option<(StructId, usize)>,
    ) -> Result<()> {
        let is_ref = flags & FLAG_REFERENCE != 0;
        let start = if flags & FLAG_STRUCT != 0 {
            let List::Struct(items) = list else {
                return Err(mismatch("struct list holds non-structs"));
            };
            let size = self.template_size(type_code as u32)?;
            let elem = if is_ref { 4 } else { size };
            let start = match owner {
                Some((o, f)) if !is_ref => {
                    // Several fields can point at one list block (dialogs do this a lot).
                    // If this list's structs all live in another list, share that block.
                    let (o, f) = self.shared_list_home(items).unwrap_or((o, f));
                    self.inline_list_block(o, f)?
                }
                _ => self.list_block(items.len(), elem),
            };
            for (i, s) in items.iter().enumerate() {
                let p = start + i as u32 * elem;
                if is_ref {
                    let target = match s {
                        None => NULL,
                        Some(id) => self.struct_ref(*id)?,
                    };
                    self.put_u32(p, target);
                } else {
                    let id = s.ok_or_else(|| mismatch("inline struct in list is null"))?;
                    self.struct_inline(id, p);
                }
            }
            start
        } else if type_code == TYPE_GENERIC {
            let List::Generic(items) = list else {
                return Err(mismatch("generic list holds non-generics"));
            };
            let start = self.list_block(items.len(), 8);
            for (i, slot) in items.iter().enumerate() {
                self.write_generic(start + i as u32 * 8, slot, is_ref)?;
            }
            start
        } else if type_code == TYPE_STRING {
            let List::String(items) = list else {
                return Err(mismatch("string list holds non-strings"));
            };
            let start = self.list_block(items.len(), 4);
            for (i, s) in items.iter().enumerate() {
                let p = self.string_ref(s);
                self.put_u32(start + i as u32 * 4, p);
            }
            start
        } else {
            let bytes = encode_list(type_code, list)?;
            let size = scalar_size(type_code).ok_or(Gff4Error::UnknownType { type_code, flags })?;
            let start = self.list_block(list.len(), size);
            self.put(start, &bytes);
            start
        };
        self.put_u32(at, start - 4);
        Ok(())
    }

    fn write_generic(&mut self, at: u32, slot: &GenericSlot, is_ref: bool) -> Result<()> {
        self.put_u32(at, slot.type_code as u32 | (slot.flags as u32) << 16);
        let Some(v) = &slot.value else {
            self.put_u32(at + 4, NULL);
            return Ok(());
        };
        let (ty, fl) = (slot.type_code, slot.flags);
        if !is_ref {
            return self.write_value(at + 4, v, ty, fl, true);
        }
        let loc =
            if ty == TYPE_STRING && fl & (FLAG_LIST | FLAG_STRUCT) == 0 && self.shared.is_none() {
                let Value::String(Some(s)) = v else {
                    return Err(mismatch("generic string slot holds a non-string"));
                };
                self.alloc_string(s)
            } else {
                let size = if fl & FLAG_LIST != 0 {
                    4
                } else if fl & FLAG_STRUCT != 0 {
                    if fl & FLAG_REFERENCE != 0 {
                        4
                    } else {
                        self.template_size(ty as u32)?
                    }
                } else if ty == TYPE_STRING {
                    4
                } else if ty == TYPE_TLK {
                    8
                } else {
                    scalar_size(ty).ok_or(Gff4Error::UnknownType {
                        type_code: ty,
                        flags: fl,
                    })?
                };
                let loc = self.alloc(size);
                self.write_value(loc, v, ty, fl, true)?;
                loc
            };
        self.put_u32(at + 4, loc);
        Ok(())
    }
}

fn encode_scalar(type_code: u16, v: &Value) -> Result<Vec<u8>> {
    fn floats(f: &[f32]) -> Vec<u8> {
        f.iter().flat_map(|x| x.to_le_bytes()).collect()
    }
    Ok(match (type_code, v) {
        (0, Value::U8(x)) => vec![*x],
        (1, Value::I8(x)) => vec![*x as u8],
        (2, Value::U16(x)) => x.to_le_bytes().to_vec(),
        (3, Value::I16(x)) => x.to_le_bytes().to_vec(),
        (4, Value::U32(x)) => x.to_le_bytes().to_vec(),
        (5, Value::I32(x)) => x.to_le_bytes().to_vec(),
        (6, Value::U64(x)) => x.to_le_bytes().to_vec(),
        (7, Value::I64(x)) => x.to_le_bytes().to_vec(),
        (8, Value::F32(x)) => x.to_le_bytes().to_vec(),
        (9, Value::F64(x)) => x.to_le_bytes().to_vec(),
        (10, Value::Vec3(x)) => floats(x),
        (12, Value::Vec4(x)) | (13, Value::Quat(x)) | (15, Value::Color(x)) => floats(x),
        (16, Value::Mat4(x)) => floats(x),
        _ => return Err(mismatch(format!("type {type_code} does not match {v:?}"))),
    })
}

fn encode_list(type_code: u16, l: &List) -> Result<Vec<u8>> {
    fn flat<T: Copy, const N: usize>(v: &[T], f: impl Fn(T) -> [u8; N]) -> Vec<u8> {
        v.iter().flat_map(|&x| f(x)).collect()
    }
    fn floats<const N: usize>(v: &[[f32; N]]) -> Vec<u8> {
        v.iter().flatten().flat_map(|x| x.to_le_bytes()).collect()
    }
    Ok(match (type_code, l) {
        (0, List::U8(v)) => v.clone(),
        (1, List::I8(v)) => v.iter().map(|&x| x as u8).collect(),
        (2, List::U16(v)) => flat(v, u16::to_le_bytes),
        (3, List::I16(v)) => flat(v, i16::to_le_bytes),
        (4, List::U32(v)) => flat(v, u32::to_le_bytes),
        (5, List::I32(v)) => flat(v, i32::to_le_bytes),
        (6, List::U64(v)) => flat(v, u64::to_le_bytes),
        (7, List::I64(v)) => flat(v, i64::to_le_bytes),
        (8, List::F32(v)) => flat(v, f32::to_le_bytes),
        (9, List::F64(v)) => flat(v, f64::to_le_bytes),
        (10, List::Vec3(v)) => floats(v),
        (12, List::Vec4(v)) | (13, List::Quat(v)) | (15, List::Color(v)) => floats(v),
        (16, List::Mat4(v)) => floats(v),
        _ => {
            return Err(mismatch(format!(
                "list type {type_code} does not match its values"
            )));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(label: u32, type_code: u16, flags: u16, offset: u32) -> FieldDef {
        FieldDef {
            label,
            type_code,
            flags,
            offset,
        }
    }

    /// A document exercising every storage kind, including a shared reference and a cycle.
    fn sample() -> Gff4 {
        let root = StructTemplate {
            label: FourCC::new(b"ROOT"),
            fields: vec![
                field(1, 4, 0, 0),                                      // u32
                field(2, 14, 0, 4),                                     // string
                field(3, 17, 0, 8),                                     // tlk
                field(4, 10, 0, 16),                                    // vec3
                field(5, 1, FLAG_STRUCT | FLAG_LIST, 28),               // inline struct list
                field(6, 1, FLAG_STRUCT | FLAG_REFERENCE, 32),          // struct ref
                field(7, 8, FLAG_LIST, 36),                             // f32 list
                field(8, 14, FLAG_LIST, 40),                            // string list
                field(9, TYPE_GENERIC, FLAG_LIST | FLAG_REFERENCE, 44), // generic list
                field(10, TYPE_GENERIC, FLAG_REFERENCE, 48),            // single generic
                field(11, 0, FLAG_LIST, 56),                            // null u8 list
                field(12, 5, 0, NULL),                                  // no storage
            ],
            size: 60,
        };
        let node = StructTemplate {
            label: FourCC::new(b"NODE"),
            fields: vec![
                field(20, 2, 0, 0),
                field(21, 1, FLAG_STRUCT | FLAG_REFERENCE, 4),
            ],
            size: 8,
        };
        // Structs: 0 root, 1 and 2 inline list nodes, 3 a referenced node that points back to 1.
        let structs = vec![
            StructData {
                template: 0,
                values: vec![
                    Value::U32(0xDEAD_BEEF),
                    Value::String(Some("Ostagar".into())),
                    Value::Tlk(TlkString {
                        id: 1234,
                        text: Some("override".into()),
                    }),
                    Value::Vec3([1.0, -2.5, f32::NAN]),
                    Value::List(Some(List::Struct(vec![
                        Some(StructId(1)),
                        Some(StructId(2)),
                    ]))),
                    Value::Struct(Some(StructId(3))),
                    Value::List(Some(List::F32(vec![0.5, 1.5]))),
                    Value::List(Some(List::String(vec![
                        Some("a".into()),
                        None,
                        Some("ünï".into()),
                    ]))),
                    Value::List(Some(List::Generic(vec![
                        GenericSlot {
                            type_code: 4,
                            flags: 0,
                            value: Some(Value::U32(7)),
                        },
                        GenericSlot {
                            type_code: 14,
                            flags: 0,
                            value: Some(Value::String(Some("generic text".into()))),
                        },
                        GenericSlot {
                            type_code: 1,
                            flags: FLAG_STRUCT,
                            value: Some(Value::Struct(Some(StructId(4)))),
                        },
                        GenericSlot {
                            type_code: 0,
                            flags: FLAG_LIST,
                            value: Some(Value::List(Some(List::U8(vec![1, 2, 3])))),
                        },
                        GenericSlot {
                            type_code: 4,
                            flags: 0,
                            value: None,
                        },
                    ]))),
                    Value::Generic(Box::new(GenericSlot {
                        type_code: 17,
                        flags: 0,
                        value: Some(Value::Tlk(TlkString { id: 9, text: None })),
                    })),
                    Value::List(None),
                    Value::Absent,
                ],
            },
            StructData {
                template: 1,
                values: vec![Value::U16(1), Value::Struct(Some(StructId(3)))],
            },
            StructData {
                template: 1,
                values: vec![Value::U16(2), Value::Struct(None)],
            },
            StructData {
                template: 1,
                values: vec![Value::U16(3), Value::Struct(Some(StructId(1)))],
            },
            StructData {
                template: 1,
                values: vec![Value::U16(4), Value::Struct(None)],
            },
        ];
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"TEST"),
            file_version: FourCC::new(b"V0.1"),
            templates: vec![root, node],
            structs,
            root: StructId(0),
            shared_strings: None,
        }
    }

    #[test]
    fn round_trip_preserves_graph() {
        let doc = sample();
        let bytes = doc.write().unwrap();
        let back = Gff4::parse(&bytes).unwrap();
        assert_eq!(
            format!("{:?}", back.templates),
            format!("{:?}", doc.templates)
        );
        assert_eq!(back.file_type, doc.file_type);

        let root = back.root;
        assert_eq!(back.field(root, 1).unwrap().as_u64(), Some(0xDEAD_BEEF));
        assert_eq!(back.field(root, 2).unwrap().as_str(), Some("Ostagar"));
        let list = back.field(root, 5).unwrap().as_list().unwrap();
        let List::Struct(items) = list else { panic!() };
        let first = items[0].unwrap();
        // The referenced node points back at the first inline node: the cycle survives.
        let referenced = back.field(root, 6).unwrap().as_struct().unwrap();
        assert_eq!(back.field(referenced, 21).unwrap().as_struct(), Some(first));
        assert_eq!(back.field(first, 21).unwrap().as_struct(), Some(referenced));
        assert!(matches!(back.field(root, 11), Some(Value::List(None))));
        assert!(matches!(back.field(root, 12), Some(Value::Absent)));

        // A second trip is byte-identical: the writer is canonical.
        assert_eq!(back.write().unwrap(), bytes);
    }

    #[test]
    fn round_trip_with_shared_strings() {
        let mut doc = sample();
        doc.version = Version::V41;
        doc.shared_strings = Some(vec!["pre-existing".into()]);
        let bytes = doc.write().unwrap();
        let back = Gff4::parse(&bytes).unwrap();
        assert_eq!(back.field(back.root, 2).unwrap().as_str(), Some("Ostagar"));
        let table = back.shared_strings.as_ref().unwrap();
        assert_eq!(table[0], "pre-existing");
        assert!(table.iter().any(|s| s == "generic text"));
        assert_eq!(back.write().unwrap(), bytes);
    }

    #[test]
    fn rejects_garbage() {
        assert!(matches!(Gff4::parse(b"nope"), Err(Gff4Error::NotGff4)));
        let mut bytes = sample().write().unwrap();
        bytes.truncate(60);
        assert!(Gff4::parse(&bytes).is_err());
    }
}
