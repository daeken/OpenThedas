//! Area files (`.are`, GFF3 `ARE `): the gameplay side of an area. For rendering we only need
//! the name of its environment layout, which names the `.arl` and the level archives under
//! `packages/core/env/`.
//!
//! Provisional: the `Layout` field's meaning is inferred from the data (every `.are` in the
//! single-player campaign names an existing `env/<layout>.arl`), not checked against the
//! original executable.

use crate::gff3::{Gff3, Gff3Error, Struct, Value};

#[derive(Debug, thiserror::Error)]
pub enum AreaError {
    #[error(transparent)]
    Gff3(#[from] Gff3Error),
    #[error("not an area file (file type {0:?})")]
    WrongType(String),
    #[error("field {field} has an unexpected type")]
    WrongField { field: &'static str },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub tag: Option<String>,
    /// The environment layout (level) name, e.g. `lgt101d`.
    pub layout: Option<String>,
    /// Talk-table reference of the display name.
    pub name_strref: Option<u32>,
}

fn text(root: &Struct, field: &'static str) -> Result<Option<String>, AreaError> {
    match root
        .fields
        .iter()
        .find(|f| f.label == field)
        .map(|f| &f.value)
    {
        None => Ok(None),
        Some(Value::ExoString(b) | Value::ResRef(b)) => {
            let s = String::from_utf8_lossy(b)
                .trim_end_matches('\0')
                .to_string();
            Ok((!s.is_empty()).then_some(s))
        }
        Some(_) => Err(AreaError::WrongField { field }),
    }
}

impl Area {
    pub fn parse(data: &[u8]) -> Result<Self, AreaError> {
        Self::from_gff3(&Gff3::parse(data)?)
    }

    pub fn from_gff3(doc: &Gff3) -> Result<Self, AreaError> {
        if &doc.file_type != b"ARE " {
            return Err(AreaError::WrongType(
                String::from_utf8_lossy(&doc.file_type).into_owned(),
            ));
        }
        let Some(root) = doc.structs.first() else {
            return Ok(Area {
                tag: None,
                layout: None,
                name_strref: None,
            });
        };
        let name_strref = match root.fields.iter().find(|f| f.label == "Name") {
            None => None,
            Some(f) => match &f.value {
                Value::LocString(l) => Some(l.strref),
                Value::StrRef(r) => Some(*r),
                _ => return Err(AreaError::WrongField { field: "Name" }),
            },
        };
        Ok(Area {
            tag: text(root, "Tag")?,
            layout: text(root, "Layout")?,
            name_strref,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff3::{Field, LocString};

    fn doc(fields: Vec<Field>) -> Gff3 {
        Gff3 {
            file_type: *b"ARE ",
            file_version: *b"V3.2",
            structs: vec![Struct {
                id: 0xFFFF_FFFF,
                fields,
            }],
        }
    }

    #[test]
    fn reads_layout_and_tag() {
        let d = doc(vec![
            Field {
                label: "Name".into(),
                value: Value::LocString(LocString {
                    strref: 42,
                    strings: vec![],
                }),
            },
            Field {
                label: "Tag".into(),
                value: Value::ExoString(b"test_area".to_vec()),
            },
            Field {
                label: "Layout".into(),
                value: Value::ExoString(b"abc123d".to_vec()),
            },
        ]);
        let a = Area::from_gff3(&d).unwrap();
        assert_eq!(a.layout.as_deref(), Some("abc123d"));
        assert_eq!(a.tag.as_deref(), Some("test_area"));
        assert_eq!(a.name_strref, Some(42));
        // Survives a binary round trip too.
        let back = Area::parse(&d.write().unwrap()).unwrap();
        assert_eq!(back, a);
    }

    #[test]
    fn rejects_other_types_and_bad_fields() {
        let mut d = doc(vec![]);
        d.file_type = *b"UTC ";
        assert!(matches!(Area::from_gff3(&d), Err(AreaError::WrongType(_))));
        let d = doc(vec![Field {
            label: "Layout".into(),
            value: Value::Int(3),
        }]);
        assert!(Area::from_gff3(&d).is_err());
        assert_eq!(Area::from_gff3(&doc(vec![])).unwrap().layout, None);
    }
}
