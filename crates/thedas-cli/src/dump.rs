//! Readable dumps of GFF4, GDA and TLK resources.

use std::io::Write;

use anyhow::{Result, bail};
use serde_json::{Map, Value as Json, json};
use thedas_formats::gda::Cell;
use thedas_formats::gff4::{GenericSlot, Gff4, List, StructId, Value, is_gff4};
use thedas_formats::{Gda, Tlk};

pub fn dump(data: &[u8], raw: bool, out: &mut impl Write) -> Result<()> {
    if !is_gff4(data) {
        bail!(
            "not a GFF4 file (first bytes {:02x?})",
            &data[..data.len().min(8)]
        );
    }
    let doc = Gff4::parse(data)?;
    if !raw && &doc.file_type.0 == b"G2DA" {
        let gda = Gda::from_gff4(&doc)?;
        let header: Vec<String> = gda
            .columns
            .iter()
            .map(|c| format!("{:08x}", c.hash))
            .collect();
        writeln!(out, "{}", header.join("\t"))?;
        for row in &gda.rows {
            let cells: Vec<String> = row
                .iter()
                .map(|c| match c {
                    Cell::Empty => String::new(),
                    Cell::Int(n) => n.to_string(),
                    Cell::Float(f) => f.to_string(),
                    Cell::String(s) => s.replace(['\t', '\n'], " "),
                })
                .collect();
            writeln!(out, "{}", cells.join("\t"))?;
        }
        return Ok(());
    }
    if !raw && &doc.file_type.0 == b"TLK " {
        let tlk = Tlk::from_gff4(&doc)?;
        for (id, s) in &tlk.strings {
            writeln!(out, "{id}\t{}", s.replace('\n', "\\n"))?;
        }
        return Ok(());
    }
    serde_json::to_writer_pretty(&mut *out, &to_json(&doc))?;
    writeln!(out)?;
    Ok(())
}

/// The whole document as JSON. Structs reached more than once are printed once and then
/// referred to as `{"$ref": n}`.
pub fn to_json(doc: &Gff4) -> Json {
    let mut seen = vec![false; doc.structs.len()];
    json!({
        "version": format!("{:?}", doc.version),
        "file_type": doc.file_type.to_string(),
        "file_version": doc.file_version.to_string(),
        "root": struct_json(doc, doc.root, &mut seen),
    })
}

fn struct_json(doc: &Gff4, id: StructId, seen: &mut [bool]) -> Json {
    if std::mem::replace(&mut seen[id.0], true) {
        return json!({ "$ref": id.0 });
    }
    let t = doc.template_of(id);
    let mut m = Map::new();
    m.insert("$struct".into(), json!(format!("{} #{}", t.label, id.0)));
    for (f, v) in t.fields.iter().zip(&doc.get_struct(id).values) {
        m.insert(f.label.to_string(), value_json(doc, v, seen));
    }
    Json::Object(m)
}

fn floats(v: &[f32]) -> Json {
    json!(v.iter().map(|&f| f64::from(f)).collect::<Vec<_>>())
}

fn slot_json(doc: &Gff4, s: &GenericSlot, seen: &mut [bool]) -> Json {
    match &s.value {
        None => Json::Null,
        Some(v) => {
            json!({ "$type": s.type_code, "$flags": s.flags, "value": value_json(doc, v, seen) })
        }
    }
}

fn value_json(doc: &Gff4, v: &Value, seen: &mut [bool]) -> Json {
    match v {
        Value::Absent => json!("<absent>"),
        Value::U8(x) => json!(x),
        Value::I8(x) => json!(x),
        Value::U16(x) => json!(x),
        Value::I16(x) => json!(x),
        Value::U32(x) => json!(x),
        Value::I32(x) => json!(x),
        Value::U64(x) => json!(x),
        Value::I64(x) => json!(x),
        Value::F32(x) => json!(f64::from(*x)),
        Value::F64(x) => json!(x),
        Value::Vec3(x) => floats(x),
        Value::Vec4(x) | Value::Quat(x) | Value::Color(x) => floats(x),
        Value::Mat4(x) => floats(x),
        Value::String(s) => json!(s),
        Value::Tlk(t) => json!({ "strref": t.id, "text": t.text }),
        Value::Struct(None) => Json::Null,
        Value::Struct(Some(id)) => struct_json(doc, *id, seen),
        Value::Generic(s) => slot_json(doc, s, seen),
        Value::List(None) => Json::Null,
        Value::List(Some(l)) => list_json(doc, l, seen),
    }
}

fn list_json(doc: &Gff4, l: &List, seen: &mut [bool]) -> Json {
    const MAX_INLINE: usize = 64;
    let summarize = |len: usize, kind: &str| json!(format!("<{len} x {kind}>"));
    match l {
        List::U8(v) if v.len() > MAX_INLINE => summarize(v.len(), "u8"),
        List::U8(v) => json!(v),
        List::I8(v) => json!(v),
        List::U16(v) => json!(v),
        List::I16(v) => json!(v),
        List::U32(v) => json!(v),
        List::I32(v) => json!(v),
        List::U64(v) => json!(v),
        List::I64(v) => json!(v),
        List::F32(v) => floats(v),
        List::F64(v) => json!(v),
        List::Vec3(v) => json!(v.iter().map(|x| floats(x)).collect::<Vec<_>>()),
        List::Vec4(v) | List::Quat(v) | List::Color(v) => {
            json!(v.iter().map(|x| floats(x)).collect::<Vec<_>>())
        }
        List::Mat4(v) => json!(v.iter().map(|x| floats(x)).collect::<Vec<_>>()),
        List::String(v) => json!(v),
        List::Struct(v) => Json::Array(
            v.iter()
                .map(|s| match s {
                    None => Json::Null,
                    Some(id) => struct_json(doc, *id, seen),
                })
                .collect(),
        ),
        List::Generic(v) => Json::Array(v.iter().map(|s| slot_json(doc, s, seen)).collect()),
    }
}
