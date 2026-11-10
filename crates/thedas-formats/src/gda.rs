//! GDA tables: Dragon Age's replacement for 2DA, stored as a GFF4 of type `G2DA`.
//!
//! The root struct holds a column list (each column has a name hash and a type) and a row list.
//! Row structs store their cells in fields `10005 + column index`. Column names are not stored:
//! only the CRC-32 of the lowercased name in UTF-16LE, so lookups by name hash the name.

use crate::gff4::{FourCC, Gff4, Gff4Error, List, StructId, Value};

pub const FIELD_COLUMN_NAME: u32 = 10000;
pub const FIELD_COLUMN_HASH: u32 = 10001;
pub const FIELD_COLUMN_LIST: u32 = 10002;
pub const FIELD_ROW_LIST: u32 = 10003;
pub const FIELD_COLUMN_FIRST: u32 = 10005;
pub const FIELD_COLUMN_TYPE: u32 = 10999;

#[derive(Debug, thiserror::Error)]
pub enum GdaError {
    #[error(transparent)]
    Gff4(#[from] Gff4Error),
    #[error("not a GDA table (file type {0})")]
    WrongType(FourCC),
    #[error("unsupported GDA version {0}")]
    UnsupportedVersion(FourCC),
    #[error("malformed GDA: {0}")]
    Malformed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnKind {
    Empty,
    String,
    Int,
    Float,
    Bool,
    Resource,
}

impl ColumnKind {
    fn from_code(code: u64) -> Option<Self> {
        Some(match code as i64 {
            -1 => ColumnKind::Empty,
            0 => ColumnKind::String,
            1 => ColumnKind::Int,
            2 => ColumnKind::Float,
            3 => ColumnKind::Bool,
            4 => ColumnKind::Resource,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Column {
    pub hash: u32,
    /// The type the column declares, if it declares one.
    pub kind: Option<ColumnKind>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Empty,
    Int(i64),
    Float(f64),
    String(String),
}

#[derive(Clone, Debug)]
pub struct Gda {
    pub version: FourCC,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Cell>>,
}

/// The hash Dragon Age stores for a column name.
pub fn column_hash(name: &str) -> u32 {
    let lower = name.to_lowercase();
    let bytes: Vec<u8> = lower.encode_utf16().flat_map(u16::to_le_bytes).collect();
    crc32fast::hash(&bytes)
}

fn struct_list(doc: &Gff4, id: StructId, label: u32) -> Result<Vec<Option<StructId>>, GdaError> {
    match doc.field(id, label) {
        Some(Value::List(Some(List::Struct(v)))) => Ok(v.clone()),
        Some(Value::List(None)) | None => Ok(Vec::new()),
        Some(other) => Err(GdaError::Malformed(format!(
            "field {label} is not a struct list: {other:?}"
        ))),
    }
}

impl Gda {
    pub fn parse(data: &[u8]) -> Result<Self, GdaError> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(doc: &Gff4) -> Result<Self, GdaError> {
        if &doc.file_type.0 != b"G2DA" {
            return Err(GdaError::WrongType(doc.file_type));
        }
        if !matches!(&doc.file_version.0, b"V0.1" | b"V0.2") {
            return Err(GdaError::UnsupportedVersion(doc.file_version));
        }
        let mut columns = Vec::new();
        for c in struct_list(doc, doc.root, FIELD_COLUMN_LIST)? {
            let Some(c) = c else {
                columns.push(Column {
                    hash: 0,
                    kind: None,
                });
                continue;
            };
            let hash = doc
                .field(c, FIELD_COLUMN_HASH)
                .and_then(Value::as_u64)
                .ok_or_else(|| GdaError::Malformed("column without a hash".into()))?
                as u32;
            let kind = doc
                .field(c, FIELD_COLUMN_TYPE)
                .and_then(Value::as_u64)
                .map(|code| {
                    ColumnKind::from_code(code)
                        .ok_or_else(|| GdaError::Malformed(format!("bad column type {code}")))
                })
                .transpose()?;
            columns.push(Column { hash, kind });
        }

        let mut rows = Vec::new();
        for r in struct_list(doc, doc.root, FIELD_ROW_LIST)? {
            let Some(r) = r else {
                rows.push(vec![Cell::Empty; columns.len()]);
                continue;
            };
            let mut row = Vec::with_capacity(columns.len());
            for i in 0..columns.len() as u32 {
                let cell = match doc.field(r, FIELD_COLUMN_FIRST + i) {
                    None | Some(Value::Absent) => Cell::Empty,
                    Some(v) => {
                        if let Some(n) = v.as_i64() {
                            Cell::Int(n)
                        } else if let Some(f) = v.as_f64() {
                            Cell::Float(f)
                        } else if let Value::String(s) = v {
                            Cell::String(s.clone().unwrap_or_default())
                        } else {
                            return Err(GdaError::Malformed(format!(
                                "unexpected cell value {v:?}"
                            )));
                        }
                    }
                };
                row.push(cell);
            }
            rows.push(row);
        }
        Ok(Gda {
            version: doc.file_version,
            columns,
            rows,
        })
    }

    /// The index of a column by name.
    pub fn column(&self, name: &str) -> Option<usize> {
        let h = column_hash(name);
        self.columns.iter().position(|c| c.hash == h)
    }

    /// The row whose `ID` column equals `id`.
    pub fn row_by_id(&self, id: i64) -> Option<&[Cell]> {
        let c = self.column("ID")?;
        self.rows
            .iter()
            .find(|r| r[c] == Cell::Int(id))
            .map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_crc32_of_lowercase_utf16() {
        // CRC-32 of "id" as UTF-16LE (69 00 64 00).
        assert_eq!(column_hash("ID"), crc32fast::hash(&[0x69, 0, 0x64, 0]));
        assert_eq!(column_hash("Label"), column_hash("label"));
    }
}
