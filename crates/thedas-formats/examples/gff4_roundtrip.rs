//! Round-trips one GFF4 file and reports the first struct that differs, plus shared structs.
//! Usage: cargo run --example gff4_roundtrip -- FILE
use thedas_formats::Gff4;

fn main() {
    let path = std::env::args().nth(1).expect("usage: gff4_roundtrip FILE");
    let data = std::fs::read(&path).expect("read");
    let a = Gff4::parse(&data).expect("parse");
    let bytes = a.write().expect("write");
    let b = Gff4::parse(&bytes).expect("re-parse");
    println!(
        "structs: {} -> {}; bytes {} -> {}",
        a.structs.len(),
        b.structs.len(),
        data.len(),
        bytes.len()
    );
    for (i, (x, y)) in a.structs.iter().zip(&b.structs).enumerate() {
        let (dx, dy) = (format!("{:?}", x), format!("{:?}", y));
        if dx != dy {
            println!("first difference at struct {i}:\n  original: {dx}\n  re-read:  {dy}");
            refs(&a);
            return;
        }
    }
    println!("graphs match");
}

/// Lists structs that are reached from more than one place, which the writer must keep shared.
fn refs(doc: &Gff4) {
    use thedas_formats::gff4::{List, Value};
    let mut seen: std::collections::HashMap<usize, Vec<String>> = Default::default();
    for (si, s) in doc.structs.iter().enumerate() {
        let t = &doc.templates[s.template as usize];
        for (f, v) in t.fields.iter().zip(&s.values) {
            let kind = format!("s{si}.f{} flags {:#x}", f.label, f.flags);
            match v {
                Value::Struct(Some(c)) => seen.entry(c.0).or_default().push(kind),
                Value::List(Some(List::Struct(items))) => {
                    for (k, c) in items.iter().enumerate() {
                        if let Some(c) = c {
                            seen.entry(c.0).or_default().push(format!("{kind}[{k}]"));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut multi: Vec<_> = seen.into_iter().filter(|(_, v)| v.len() > 1).collect();
    multi.sort();
    for (id, v) in multi.iter().take(10) {
        println!("struct {id} reached from {v:?}");
    }
    println!("{} structs reached more than once", multi.len());
}
