//! Talk tables: Dragon Age's localized strings, stored as a GFF4 of type `TLK `.
//!
//! - V0.2 stores each string directly: a list of `{id, string}` structs.
//! - V0.4 and V0.5 Huffman-compress all strings into one bit stream. Each entry holds an id and
//!   a bit offset; the tree is a flat array of `i32` node pairs where a negative value is a leaf
//!   holding the bitwise complement of a UTF-16 code unit.

use std::collections::BTreeMap;

use crate::gff4::{FourCC, Gff4, Gff4Error, List, Value};

const FIELD_STRING_LIST: u32 = 19001;
const FIELD_STRING_ID: u32 = 19002;
const FIELD_STRING: u32 = 19003;
const FIELD_HUFF_ID: u32 = 19004;
const FIELD_HUFF_BIT_OFFSET: u32 = 19005;
const FIELD_HUFF_LIST: u32 = 19006;
const FIELD_HUFF_TREE: u32 = 19007;
const FIELD_HUFF_BITSTREAM: u32 = 19008;

#[derive(Debug, thiserror::Error)]
pub enum TlkError {
    #[error(transparent)]
    Gff4(#[from] Gff4Error),
    #[error("not a talk table (file type {0})")]
    WrongType(FourCC),
    #[error("unsupported talk table version {0}")]
    UnsupportedVersion(FourCC),
    #[error("malformed talk table: {0}")]
    Malformed(String),
}

#[derive(Clone, Debug)]
pub struct Tlk {
    pub version: FourCC,
    pub strings: BTreeMap<u32, String>,
}

/// The raw 32-bit words of a list field, whatever integer width it was declared with.
fn words(doc: &Gff4, label: u32) -> Result<Vec<u32>, TlkError> {
    match doc.field(doc.root, label) {
        Some(Value::List(Some(List::U32(v)))) => Ok(v.clone()),
        Some(Value::List(Some(List::I32(v)))) => Ok(v.iter().map(|&x| x as u32).collect()),
        Some(Value::List(None)) | None => Ok(Vec::new()),
        Some(other) => Err(TlkError::Malformed(format!(
            "field {label} is not a 32-bit list: {}",
            match other {
                Value::List(Some(l)) => format!("list of {} items", l.len()),
                v => format!("{v:?}"),
            }
        ))),
    }
}

impl Tlk {
    pub fn parse(data: &[u8]) -> Result<Self, TlkError> {
        Self::from_gff4(&Gff4::parse(data)?)
    }

    pub fn from_gff4(doc: &Gff4) -> Result<Self, TlkError> {
        if &doc.file_type.0 != b"TLK " {
            return Err(TlkError::WrongType(doc.file_type));
        }
        let strings = match &doc.file_version.0 {
            b"V0.2" => Self::load_plain(doc)?,
            b"V0.4" | b"V0.5" => Self::load_huffman(doc)?,
            _ => return Err(TlkError::UnsupportedVersion(doc.file_version)),
        };
        Ok(Tlk {
            version: doc.file_version,
            strings,
        })
    }

    pub fn get(&self, id: u32) -> Option<&str> {
        self.strings.get(&id).map(String::as_str)
    }

    fn entries(doc: &Gff4, label: u32) -> Result<Vec<crate::gff4::StructId>, TlkError> {
        match doc.field(doc.root, label) {
            Some(Value::List(Some(List::Struct(v)))) => Ok(v.iter().flatten().copied().collect()),
            Some(Value::List(None)) | None => Ok(Vec::new()),
            Some(_) => Err(TlkError::Malformed(format!(
                "field {label} is not a struct list"
            ))),
        }
    }

    fn load_plain(doc: &Gff4) -> Result<BTreeMap<u32, String>, TlkError> {
        let mut out = BTreeMap::new();
        for e in Self::entries(doc, FIELD_STRING_LIST)? {
            let Some(id) = doc.field(e, FIELD_STRING_ID).and_then(Value::as_u64) else {
                continue;
            };
            let text = doc
                .field(e, FIELD_STRING)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            out.entry(id as u32).or_insert(text);
        }
        Ok(out)
    }

    fn load_huffman(doc: &Gff4) -> Result<BTreeMap<u32, String>, TlkError> {
        let tree: Vec<i32> = words(doc, FIELD_HUFF_TREE)?
            .into_iter()
            .map(|w| w as i32)
            .collect();
        let bits = words(doc, FIELD_HUFF_BITSTREAM)?;
        let mut out = BTreeMap::new();
        for e in Self::entries(doc, FIELD_HUFF_LIST)? {
            let Some(id) = doc.field(e, FIELD_HUFF_ID).and_then(Value::as_u64) else {
                continue;
            };
            let offset = doc
                .field(e, FIELD_HUFF_BIT_OFFSET)
                .and_then(Value::as_u64)
                .ok_or_else(|| TlkError::Malformed(format!("string {id} has no bit offset")))?;
            let text = decode_huffman(&tree, &bits, offset as u32)
                .ok_or_else(|| TlkError::Malformed(format!("string {id} does not decode")))?;
            out.entry(id as u32).or_insert(text);
        }
        Ok(out)
    }
}

/// Decodes one NUL-terminated string starting at `bit_offset`.
fn decode_huffman(tree: &[i32], bits: &[u32], bit_offset: u32) -> Option<String> {
    let root = (tree.len() / 2) as i64 - 1;
    let mut pos = bit_offset as u64;
    let mut units = Vec::new();
    loop {
        let mut node = root;
        while node >= 0 {
            let word = *bits.get((pos >> 5) as usize)?;
            let bit = (word >> (pos & 31)) & 1;
            node = *tree.get((node * 2 + bit as i64) as usize)? as i64;
            pos += 1;
        }
        let unit = !(node as i32) as u16;
        if unit == 0 {
            break;
        }
        units.push(unit);
        if units.len() > 1 << 20 {
            return None;
        }
    }
    String::from_utf16(&units).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_a_tiny_tree() {
        // Leaves: 'a' = !0x61, 'b' = !0x62, NUL = !0. Tree:
        //   node 0: [leaf a, leaf b]
        //   node 1 (root): [node 0, leaf NUL]
        let leaf = |u: u16| !(u as i32);
        let tree = vec![leaf(0x61), leaf(0x62), 0, leaf(0)];
        // "ab\0": a = bits 0,0 ; b = 0,1 ; NUL = 1. LSB-first: 0b1_10_00 = 0x18.
        let bits = vec![0b1_10_00u32];
        assert_eq!(decode_huffman(&tree, &bits, 0).as_deref(), Some("ab"));
        // Truncated bit stream fails cleanly.
        assert_eq!(decode_huffman(&tree, &[], 0), None);
    }
}
