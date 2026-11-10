//! GFF3 (`V3.2`), the older Aurora generic file format. Dragon Age: Origins still uses it for
//! object templates and some area data (`.utc`, `.utp`, `.uti`, `.are`, ...).
//!
//! The layout is BioWare's published Aurora GFF format: a 56-byte header followed by struct,
//! field, label, field data, field index and list index sections. Strings are kept as raw bytes
//! because the format does not declare an encoding.

use std::fmt;

const NULL: u32 = 0xFFFF_FFFF;

#[derive(Debug, thiserror::Error)]
pub enum Gff3Error {
    #[error("not a GFF3 file")]
    NotGff3,
    #[error("read past end of data at offset {offset:#x} (+{len})")]
    OutOfBounds { offset: u64, len: u64 },
    #[error("struct index {0} out of range")]
    BadStruct(u32),
    #[error("field index {0} out of range")]
    BadField(u32),
    #[error("label index {0} out of range")]
    BadLabel(u32),
    #[error("unknown field type {0}")]
    UnknownType(u32),
    #[error("label {0:?} is longer than 16 bytes")]
    LabelTooLong(String),
}

pub type Result<T> = std::result::Result<T, Gff3Error>;

/// A localized string: a talk table reference plus optional embedded strings per language.
#[derive(Clone, Debug, PartialEq)]
pub struct LocString {
    pub strref: u32,
    /// (string id, raw bytes). The id encodes language and gender.
    pub strings: Vec<(u32, Vec<u8>)>,
}

#[derive(Clone, PartialEq)]
pub enum Value {
    Byte(u8),
    Char(i8),
    Word(u16),
    Short(i16),
    Dword(u32),
    Int(i32),
    Dword64(u64),
    Int64(i64),
    Float(f32),
    Double(f64),
    ExoString(Vec<u8>),
    ResRef(Vec<u8>),
    LocString(LocString),
    Void(Vec<u8>),
    /// Index into [`Gff3::structs`].
    Struct(u32),
    /// Indices into [`Gff3::structs`].
    List(Vec<u32>),
    Orientation([f32; 4]),
    Vector([f32; 3]),
    StrRef(u32),
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        match self {
            Value::Byte(v) => write!(f, "Byte({v})"),
            Value::Char(v) => write!(f, "Char({v})"),
            Value::Word(v) => write!(f, "Word({v})"),
            Value::Short(v) => write!(f, "Short({v})"),
            Value::Dword(v) => write!(f, "Dword({v})"),
            Value::Int(v) => write!(f, "Int({v})"),
            Value::Dword64(v) => write!(f, "Dword64({v})"),
            Value::Int64(v) => write!(f, "Int64({v})"),
            Value::Float(v) => write!(f, "Float({v:?})"),
            Value::Double(v) => write!(f, "Double({v:?})"),
            Value::ExoString(b) => write!(f, "ExoString({:?})", s(b)),
            Value::ResRef(b) => write!(f, "ResRef({:?})", s(b)),
            Value::LocString(l) => write!(
                f,
                "LocString({} {:?})",
                l.strref,
                l.strings.iter().map(|(i, b)| (i, s(b))).collect::<Vec<_>>()
            ),
            Value::Void(b) => write!(f, "Void({} bytes {:02x?})", b.len(), b),
            Value::Struct(i) => write!(f, "Struct(#{i})"),
            Value::List(v) => write!(f, "List({v:?})"),
            Value::Orientation(v) => write!(f, "Orientation({v:?})"),
            Value::Vector(v) => write!(f, "Vector({v:?})"),
            Value::StrRef(v) => write!(f, "StrRef({v})"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub label: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Struct {
    /// The struct's type id (`0xFFFFFFFF` for the root struct).
    pub id: u32,
    pub fields: Vec<Field>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gff3 {
    pub file_type: [u8; 4],
    pub file_version: [u8; 4],
    /// Struct 0 is the root.
    pub structs: Vec<Struct>,
}

/// Whether `data` starts with a GFF3 header.
pub fn is_gff3(data: &[u8]) -> bool {
    data.len() >= 56
        && &data[4..6] == b"V3"
        && data[..4].iter().all(|b| b.is_ascii_graphic() || *b == b' ')
}

struct Sections<'a> {
    d: &'a [u8],
}

impl<'a> Sections<'a> {
    fn bytes(&self, offset: u64, len: u64) -> Result<&'a [u8]> {
        match offset.checked_add(len) {
            Some(end) if end <= self.d.len() as u64 => Ok(&self.d[offset as usize..end as usize]),
            _ => Err(Gff3Error::OutOfBounds { offset, len }),
        }
    }
    fn u32(&self, offset: u64) -> Result<u32> {
        Ok(u32::from_le_bytes(
            self.bytes(offset, 4)?.try_into().unwrap(),
        ))
    }
}

impl Gff3 {
    pub fn parse(d: &[u8]) -> Result<Self> {
        if !is_gff3(d) {
            return Err(Gff3Error::NotGff3);
        }
        let s = Sections { d };
        let h = |i: u64| s.u32(8 + i * 4);
        let (struct_off, struct_count) = (h(0)? as u64, h(1)?);
        let (field_off, field_count) = (h(2)? as u64, h(3)?);
        let (label_off, label_count) = (h(4)? as u64, h(5)?);
        let field_data = h(6)? as u64;
        let field_indices = h(8)? as u64;
        let list_indices = h(10)? as u64;

        let mut labels = Vec::with_capacity(label_count.min(1 << 16) as usize);
        for i in 0..label_count as u64 {
            let raw = s.bytes(label_off + i * 16, 16)?;
            let end = raw.iter().position(|&b| b == 0).unwrap_or(16);
            labels.push(String::from_utf8_lossy(&raw[..end]).into_owned());
        }

        let read_field = |fi: u32| -> Result<Field> {
            if fi >= field_count {
                return Err(Gff3Error::BadField(fi));
            }
            let at = field_off + fi as u64 * 12;
            let ty = s.u32(at)?;
            let li = s.u32(at + 4)?;
            let data = s.u32(at + 8)?;
            let label = labels
                .get(li as usize)
                .ok_or(Gff3Error::BadLabel(li))?
                .clone();
            let complex = field_data + data as u64;
            let floats = |n: u64| -> Result<Vec<f32>> {
                Ok(s.bytes(complex, n * 4)?
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                    .collect())
            };
            let sized = |at: u64| -> Result<Vec<u8>> {
                let len = s.u32(at)? as u64;
                Ok(s.bytes(at + 4, len)?.to_vec())
            };
            let value = match ty {
                0 => Value::Byte(data as u8),
                1 => Value::Char(data as u8 as i8),
                2 => Value::Word(data as u16),
                3 => Value::Short(data as u16 as i16),
                4 => Value::Dword(data),
                5 => Value::Int(data as i32),
                6 => Value::Dword64(u64::from_le_bytes(s.bytes(complex, 8)?.try_into().unwrap())),
                7 => Value::Int64(i64::from_le_bytes(s.bytes(complex, 8)?.try_into().unwrap())),
                8 => Value::Float(f32::from_bits(data)),
                9 => Value::Double(f64::from_le_bytes(s.bytes(complex, 8)?.try_into().unwrap())),
                10 => Value::ExoString(sized(complex)?),
                11 => {
                    let len = s.bytes(complex, 1)?[0] as u64;
                    Value::ResRef(s.bytes(complex + 1, len)?.to_vec())
                }
                12 => {
                    let strref = s.u32(complex + 4)?;
                    let count = s.u32(complex + 8)?;
                    let mut at = complex + 12;
                    let mut strings = Vec::with_capacity(count.min(64) as usize);
                    for _ in 0..count {
                        let id = s.u32(at)?;
                        let text = sized(at + 4)?;
                        at += 8 + text.len() as u64;
                        strings.push((id, text));
                    }
                    Value::LocString(LocString { strref, strings })
                }
                13 => Value::Void(sized(complex)?),
                14 => Value::Struct(data),
                15 => {
                    let at = list_indices + data as u64;
                    let count = s.u32(at)? as u64;
                    let raw = s.bytes(at + 4, count * 4)?;
                    Value::List(
                        raw.chunks_exact(4)
                            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
                            .collect(),
                    )
                }
                16 => Value::Orientation(floats(4)?.try_into().unwrap()),
                17 => Value::Vector(floats(3)?.try_into().unwrap()),
                18 => Value::StrRef(s.u32(complex + 4)?),
                t => return Err(Gff3Error::UnknownType(t)),
            };
            Ok(Field { label, value })
        };

        let mut structs = Vec::with_capacity(struct_count.min(1 << 20) as usize);
        for si in 0..struct_count as u64 {
            let at = struct_off + si * 12;
            let id = s.u32(at)?;
            let data = s.u32(at + 4)?;
            let count = s.u32(at + 8)?;
            let mut fields = Vec::with_capacity(count.min(1 << 16) as usize);
            match count {
                0 => {}
                1 => fields.push(read_field(data)?),
                n => {
                    let raw = s.bytes(field_indices + data as u64, n as u64 * 4)?;
                    for c in raw.chunks_exact(4) {
                        fields.push(read_field(u32::from_le_bytes(c.try_into().unwrap()))?);
                    }
                }
            }
            structs.push(Struct { id, fields });
        }
        // Validate struct references so consumers can index without checks.
        for st in &structs {
            for f in &st.fields {
                match &f.value {
                    Value::Struct(i) if *i >= struct_count => return Err(Gff3Error::BadStruct(*i)),
                    Value::List(v) => {
                        if let Some(i) = v.iter().find(|&&i| i >= struct_count) {
                            return Err(Gff3Error::BadStruct(*i));
                        }
                    }
                    _ => {}
                }
            }
        }
        if structs.is_empty() {
            return Err(Gff3Error::BadStruct(0));
        }
        Ok(Gff3 {
            file_type: d[..4].try_into().unwrap(),
            file_version: d[4..8].try_into().unwrap(),
            structs,
        })
    }

    pub fn root(&self) -> &Struct {
        &self.structs[0]
    }

    /// Writes the document. Fields are laid out struct by struct, labels in first-use order.
    pub fn write(&self) -> Result<Vec<u8>> {
        let mut structs = Vec::new();
        let mut fields = Vec::new();
        let mut labels: Vec<[u8; 16]> = Vec::new();
        let mut label_index = std::collections::HashMap::new();
        let mut data = Vec::new();
        let mut field_indices = Vec::new();
        let mut list_indices = Vec::new();
        let mut field_count = 0u32;

        let put = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
        for st in &self.structs {
            let first = field_count;
            let n = st.fields.len() as u32;
            put(&mut structs, st.id);
            match n {
                0 => put(&mut structs, NULL),
                1 => put(&mut structs, first),
                _ => {
                    put(&mut structs, field_indices.len() as u32);
                    for i in 0..n {
                        put(&mut field_indices, first + i);
                    }
                }
            }
            put(&mut structs, n);
            field_count += n;

            for f in &st.fields {
                let li = match label_index.get(&f.label) {
                    Some(&i) => i,
                    None => {
                        if f.label.len() > 16 {
                            return Err(Gff3Error::LabelTooLong(f.label.clone()));
                        }
                        let mut raw = [0u8; 16];
                        raw[..f.label.len()].copy_from_slice(f.label.as_bytes());
                        labels.push(raw);
                        let i = labels.len() as u32 - 1;
                        label_index.insert(f.label.clone(), i);
                        i
                    }
                };
                let complex = |data: &mut Vec<u8>, bytes: &[u8]| {
                    let at = data.len() as u32;
                    data.extend_from_slice(bytes);
                    at
                };
                let sized = |bytes: &[u8]| {
                    let mut v = (bytes.len() as u32).to_le_bytes().to_vec();
                    v.extend_from_slice(bytes);
                    v
                };
                let (ty, v) = match &f.value {
                    Value::Byte(x) => (0, *x as u32),
                    Value::Char(x) => (1, *x as u8 as u32),
                    Value::Word(x) => (2, *x as u32),
                    Value::Short(x) => (3, *x as u16 as u32),
                    Value::Dword(x) => (4, *x),
                    Value::Int(x) => (5, *x as u32),
                    Value::Dword64(x) => (6, complex(&mut data, &x.to_le_bytes())),
                    Value::Int64(x) => (7, complex(&mut data, &x.to_le_bytes())),
                    Value::Float(x) => (8, x.to_bits()),
                    Value::Double(x) => (9, complex(&mut data, &x.to_le_bytes())),
                    Value::ExoString(b) => (10, complex(&mut data, &sized(b))),
                    Value::ResRef(b) => {
                        let mut v = vec![b.len().min(255) as u8];
                        v.extend_from_slice(&b[..b.len().min(255)]);
                        (11, complex(&mut data, &v))
                    }
                    Value::LocString(l) => {
                        let mut body = Vec::new();
                        body.extend_from_slice(&l.strref.to_le_bytes());
                        body.extend_from_slice(&(l.strings.len() as u32).to_le_bytes());
                        for (id, text) in &l.strings {
                            body.extend_from_slice(&id.to_le_bytes());
                            body.extend_from_slice(&sized(text));
                        }
                        (12, complex(&mut data, &sized(&body)))
                    }
                    Value::Void(b) => (13, complex(&mut data, &sized(b))),
                    Value::Struct(i) => (14, *i),
                    Value::List(v) => {
                        let at = list_indices.len() as u32;
                        put(&mut list_indices, v.len() as u32);
                        for &i in v {
                            put(&mut list_indices, i);
                        }
                        (15, at)
                    }
                    Value::Orientation(x) => {
                        let b: Vec<u8> = x.iter().flat_map(|f| f.to_le_bytes()).collect();
                        (16, complex(&mut data, &b))
                    }
                    Value::Vector(x) => {
                        let b: Vec<u8> = x.iter().flat_map(|f| f.to_le_bytes()).collect();
                        (17, complex(&mut data, &b))
                    }
                    Value::StrRef(x) => (18, complex(&mut data, &sized(&x.to_le_bytes()))),
                };
                put(&mut fields, ty);
                put(&mut fields, li);
                put(&mut fields, v);
            }
        }

        let mut out = Vec::new();
        out.extend_from_slice(&self.file_type);
        out.extend_from_slice(&self.file_version);
        let mut at = 56u32;
        let mut section = |out: &mut Vec<u8>, len: usize, count: u32| {
            out.extend_from_slice(&at.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            at += len as u32;
        };
        section(&mut out, structs.len(), self.structs.len() as u32);
        section(&mut out, fields.len(), field_count);
        section(&mut out, labels.len() * 16, labels.len() as u32);
        section(&mut out, data.len(), data.len() as u32);
        section(&mut out, field_indices.len(), field_indices.len() as u32);
        section(&mut out, list_indices.len(), list_indices.len() as u32);
        out.extend_from_slice(&structs);
        out.extend_from_slice(&fields);
        for l in &labels {
            out.extend_from_slice(l);
        }
        out.extend_from_slice(&data);
        out.extend_from_slice(&field_indices);
        out.extend_from_slice(&list_indices);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let doc = Gff3 {
            file_type: *b"UTC ",
            file_version: *b"V3.2",
            structs: vec![
                Struct {
                    id: NULL,
                    fields: vec![
                        Field {
                            label: "Tag".into(),
                            value: Value::ExoString(b"cultist".to_vec()),
                        },
                        Field {
                            label: "Appearance".into(),
                            value: Value::Short(-3),
                        },
                        Field {
                            label: "Speed".into(),
                            value: Value::Float(1.25),
                        },
                        Field {
                            label: "Big".into(),
                            value: Value::Int64(-1),
                        },
                        Field {
                            label: "LocName".into(),
                            value: Value::LocString(LocString {
                                strref: 0x0007_0918,
                                strings: vec![(0, b"Cultist".to_vec())],
                            }),
                        },
                        Field {
                            label: "Template".into(),
                            value: Value::ResRef(b"str500cr".to_vec()),
                        },
                        Field {
                            label: "Vars".into(),
                            value: Value::List(vec![1, 2]),
                        },
                        Field {
                            label: "Pos".into(),
                            value: Value::Vector([1.0, 2.0, 3.0]),
                        },
                    ],
                },
                Struct {
                    id: 7,
                    fields: vec![Field {
                        label: "Name".into(),
                        value: Value::ExoString(b"A".to_vec()),
                    }],
                },
                Struct {
                    id: 7,
                    fields: vec![],
                },
            ],
        };
        let bytes = doc.write().unwrap();
        let back = Gff3::parse(&bytes).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.write().unwrap(), bytes);
    }
}
