//! Plots (`.plo`, GFF4 `PLTS`): named flags grouped under a GUID, with an optional script
//! that runs when a flag is set (`EVENT_TYPE_SET_PLOT`) and answers "defined" flags
//! (`EVENT_TYPE_GET_PLOT`).
//!
//! Provisional, inferred from the plot files and the plot scripts' bytecode: the event carries
//! the plot GUID as string 0, the flag as int 1, the new and old values as ints 2 and 3, the
//! conversation owner as object 0 and the party as its creator; a `GET_PLOT` answer is the
//! script's integer result. Flags 256 and up are "defined" flags computed by the script; the
//! engine sets ordinary flags before running the script.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use thedas_formats::{Gff4, List, StructId, Value};
use thedas_script::vm::{OBJECT_INVALID, ObjectId, Value as V};
use thedas_vfs::Vfs;

use crate::sim::Sim;
use crate::trace::TraceEntry;

#[derive(Clone, Debug, PartialEq)]
pub struct PlotInfo {
    pub guid: String,
    pub resref: String,
    pub script: Option<String>,
    pub name_strref: Option<u32>,
    pub flags: BTreeMap<i32, String>,
}

#[derive(Default)]
pub struct Plots {
    by_key: HashMap<String, Arc<PlotInfo>>,
    loaded: bool,
}

fn s(g: &Gff4, id: StructId, l: u32) -> Option<String> {
    g.field(id, l)
        .and_then(|v| v.as_str())
        .map(|x| x.trim_end_matches('\0').to_string())
        .filter(|x| !x.is_empty())
}

fn structs(g: &Gff4, id: StructId, l: u32) -> Vec<StructId> {
    match g.field(id, l) {
        Some(Value::List(Some(List::Struct(v)))) => v.iter().flatten().copied().collect(),
        _ => Vec::new(),
    }
}

pub fn parse_plo(data: &[u8], resref: &str) -> Vec<PlotInfo> {
    match Gff4::parse(data) {
        Ok(g) => plots_from_gff4(&g, resref),
        Err(_) => Vec::new(),
    }
}

pub fn plots_from_gff4(g: &Gff4, resref: &str) -> Vec<PlotInfo> {
    let mut out = Vec::new();
    for p in structs(g, g.root, 13016) {
        let Some(guid) = s(g, p, 13007) else { continue };
        let mut flags = BTreeMap::new();
        for f in structs(g, p, 13000) {
            if let (Some(id), Some(name)) =
                (g.field(f, 13001).and_then(|v| v.as_i64()), s(g, f, 13002))
            {
                flags.insert(id as i32, name);
            }
        }
        let name_strref = match g.field(p, 13008) {
            Some(Value::Tlk(t)) => Some(t.id),
            _ => None,
        };
        out.push(PlotInfo {
            guid: guid.to_ascii_uppercase(),
            resref: resref.to_string(),
            script: s(g, p, 13009),
            name_strref,
            flags,
        });
    }
    out
}

impl Plots {
    fn load(&mut self, vfs: &Vfs) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        for name in vfs.names().into_iter().filter(|n| n.ends_with(".plo")) {
            let Ok((data, _)) = vfs.read(&name) else {
                continue;
            };
            let res = name.trim_end_matches(".plo").to_string();
            for p in parse_plo(&data, &res) {
                let p = Arc::new(p);
                self.by_key.insert(p.guid.clone(), p.clone());
                self.by_key.entry(res.clone()).or_insert(p);
            }
        }
    }

    /// A plot by GUID or resource name.
    pub fn get(&mut self, vfs: &Vfs, key: &str) -> Option<Arc<PlotInfo>> {
        self.load(vfs);
        let k = key.trim().trim_end_matches('\0');
        self.by_key
            .get(&k.to_ascii_uppercase())
            .or_else(|| self.by_key.get(&k.to_ascii_lowercase()))
            .cloned()
    }
}

impl Sim {
    /// The canonical key (GUID) for a plot named by GUID or resource.
    pub fn plot_key(&mut self, plot: &str) -> String {
        let vfs = self.vfs.clone();
        match self.plots.get(&vfs, plot) {
            Some(p) => p.guid.clone(),
            None => plot.trim_end_matches('\0').to_ascii_uppercase(),
        }
    }

    pub fn plot_info(&mut self, plot: &str) -> Option<Arc<PlotInfo>> {
        let vfs = self.vfs.clone();
        self.plots.get(&vfs, plot)
    }

    fn plot_event(
        &self,
        ty: &str,
        guid: &str,
        flag: i32,
        value: i32,
        old: i32,
        owner: ObjectId,
    ) -> crate::world::Event {
        let mut ev = self.new_event(ty, self.world.party);
        ev.strings = vec![guid.to_string()];
        ev.ints = vec![0, flag, value, old];
        ev.objects = vec![owner];
        ev
    }

    /// Reads a flag. Defined flags (256+) ask the plot script when `call_script` is set.
    pub fn get_plot_flag(
        &mut self,
        plot: &str,
        flag: i32,
        call_script: bool,
        owner: ObjectId,
    ) -> bool {
        let key = self.plot_key(plot);
        if flag >= 256 || call_script {
            if let Some(script) = self.plot_info(plot).and_then(|p| p.script.clone()) {
                if flag >= 256 {
                    let ev = self.plot_event("GET_PLOT", &key, flag, 0, 0, owner);
                    let r = self.run_script(
                        &script,
                        self.world.party.unwrap_or(OBJECT_INVALID),
                        Some(ev),
                    );
                    return matches!(r, Some(V::Int(v)) if v != 0);
                }
            }
        }
        self.world.plots.get(&(key, flag)).copied().unwrap_or(false)
    }

    pub fn set_plot_flag(
        &mut self,
        plot: &str,
        flag: i32,
        value: bool,
        call_script: bool,
        owner: ObjectId,
    ) {
        let key = self.plot_key(plot);
        let old = self
            .world
            .plots
            .get(&(key.clone(), flag))
            .copied()
            .unwrap_or(false);
        self.world.plots.insert((key.clone(), flag), value);
        let t = self.world.time_ms;
        self.world.trace.push(TraceEntry::PlotFlag {
            t,
            plot: key.clone(),
            flag,
            value,
        });
        if call_script {
            if let Some(script) = self.plot_info(plot).and_then(|p| p.script.clone()) {
                let ev = self.plot_event("SET_PLOT", &key, flag, value as i32, old as i32, owner);
                self.run_script(
                    &script,
                    self.world.party.unwrap_or(OBJECT_INVALID),
                    Some(ev),
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::gff4::{FieldDef, FourCC, StructData, StructTemplate, TlkString, Version};

    #[test]
    fn parses_plot_files() {
        // PLTS root -> one PLOT with two FLAGs.
        let tmpl = |label: &[u8; 4], fields: &[u32]| StructTemplate {
            label: FourCC::new(label),
            fields: fields
                .iter()
                .map(|&l| FieldDef {
                    label: l,
                    type_code: 0,
                    flags: 0,
                    offset: 0,
                })
                .collect(),
            size: 0,
        };
        let st = |t: u32, values: Vec<Value>| StructData {
            template: t,
            values,
        };
        let g = Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"PLTS"),
            file_version: FourCC::new(b"V0.1"),
            templates: vec![
                tmpl(b"PLTS", &[13016]),
                tmpl(b"PLOT", &[13000, 13007, 13008, 13009]),
                tmpl(b"FLAG", &[13001, 13002]),
            ],
            structs: vec![
                st(
                    0,
                    vec![Value::List(Some(List::Struct(vec![Some(StructId(1))])))],
                ),
                st(
                    1,
                    vec![
                        Value::List(Some(List::Struct(vec![
                            Some(StructId(2)),
                            Some(StructId(3)),
                        ]))),
                        Value::String(Some("abcd\0".into())),
                        Value::Tlk(TlkString { id: 7, text: None }),
                        Value::String(Some("plt_script\0".into())),
                    ],
                ),
                st(
                    2,
                    vec![Value::U32(0), Value::String(Some("FIRST\0".into()))],
                ),
                st(
                    2,
                    vec![Value::U32(256), Value::String(Some("DEFINED\0".into()))],
                ),
            ],
            root: StructId(0),
            shared_strings: None,
        };
        let p = plots_from_gff4(&g, "plt_test");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].guid, "ABCD");
        assert_eq!(p[0].script.as_deref(), Some("plt_script"));
        assert_eq!(p[0].name_strref, Some(7));
        assert_eq!(p[0].flags.get(&256).map(String::as_str), Some("DEFINED"));
    }
}
