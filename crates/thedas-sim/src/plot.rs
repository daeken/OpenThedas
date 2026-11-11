//! Plots (`.plo`, GFF4 `PLTS`): named flags grouped under a GUID, with an optional script
//! that runs when a flag is set (`EVENT_TYPE_SET_PLOT`) and answers "defined" flags
//! (`EVENT_TYPE_GET_PLOT`).
//!
//! Provisional, inferred from the plot files and the plot scripts' bytecode: the event carries
//! the plot GUID as string 0, the flag as int 1, the new and old values as ints 2 and 3, the
//! conversation owner as object 0 and the hero as its creator; a `GET_PLOT` answer is the
//! script's integer result. Flags 256 and up are "defined" flags computed by the script; the
//! engine sets ordinary flags before running the script.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use thedas_formats::{Gff4, List, StructId, Value};
use thedas_script::vm::{OBJECT_INVALID, ObjectId, Value as V};
use thedas_vfs::Vfs;

use crate::sim::Sim;
use crate::trace::TraceEntry;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlotInfo {
    pub guid: String,
    pub resref: String,
    pub script: Option<String>,
    pub name_strref: Option<u32>,
    pub flags: BTreeMap<i32, String>,
    /// Each flag's reward id (field 13003, a row of the rewards table; 0 for none), as
    /// `GetPlotFlagRewardId` answers it. Provisional: the field is inferred from the data.
    pub rewards: BTreeMap<i32, i32>,
    /// Each flag's journal text (field 13004, a talk-table id). Provisional: the field is
    /// inferred from the data.
    pub journal: BTreeMap<i32, u32>,
    /// The values flags 0 to 127 start with: fields 13011 to 13014, a bit per flag, 32 flags
    /// a field. DAOrigins.exe's plot loader (0x75db80) copies them into the plot record's
    /// four-dword flag set (+0x8 to +0x14, at 0x75dff9), which the flag read (0x75bc70,
    /// bit `flag & 31` of dword `flag >> 5`) answers from. Lothering's first Chanter's Board
    /// job is posted this way (`lot100pt_bandits2` has field 13011 = 32: flag 5,
    /// `BANDITS2_CHANTER_BOARD`; no script sets it).
    pub initial: [u32; 4],
    /// The plot's type (field 13022, a `plottypes.gda` row): 1 a quest, 2 the story so far,
    /// 3 to 11 and 16 codex categories, 12 a Chanter's Board job, 13 and 15 tutorials, 14 a
    /// quest group. Provisional: the field is inferred from the data (every `plt_cod_*` plot
    /// has a codex type, every `*_plot_group` plot 14).
    pub kind: i32,
    /// The plot's priority (field 13010, 0 to 3), which orders quest groups. Provisional.
    pub priority: i32,
    /// The plot it belongs under (field 13017, a plot resource): a quest group for a quest, a
    /// quest for a sub-quest. Provisional.
    pub parent: Option<String>,
    /// The flags that end the quest (field 13005 non-zero: 255 on a quest's reward or ending
    /// flags). Provisional.
    pub finals: std::collections::BTreeSet<i32>,
}

impl PlotInfo {
    /// A flag's value before any script has set it (see [`PlotInfo::initial`]).
    pub fn initial_value(&self, flag: i32) -> bool {
        (0..128).contains(&flag) && self.initial[(flag >> 5) as usize] & (1 << (flag & 31)) != 0
    }
}

#[derive(Default)]
pub struct Plots {
    by_key: HashMap<String, Arc<PlotInfo>>,
    loaded: bool,
    /// Defined flags being answered by their plot scripts, innermost last.
    asking: Vec<(String, i32)>,
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
        let mut rewards = BTreeMap::new();
        let mut journal = BTreeMap::new();
        let mut finals = std::collections::BTreeSet::new();
        for f in structs(g, p, 13000) {
            if let (Some(id), Some(name)) =
                (g.field(f, 13001).and_then(|v| v.as_i64()), s(g, f, 13002))
            {
                flags.insert(id as i32, name);
                if let Some(r) = g
                    .field(f, 13003)
                    .and_then(|v| v.as_i64())
                    .filter(|&r| r != 0)
                {
                    rewards.insert(id as i32, r as i32);
                }
                if g.field(f, 13005).and_then(|v| v.as_i64()).unwrap_or(0) != 0 {
                    finals.insert(id as i32);
                }
                if let Some(Value::Tlk(t)) = g.field(f, 13004) {
                    if t.id != u32::MAX {
                        journal.insert(id as i32, t.id);
                    }
                }
            }
        }
        let mut initial = [0u32; 4];
        for (i, label) in (13011..=13014).enumerate() {
            initial[i] = g.field(p, label).and_then(|v| v.as_i64()).unwrap_or(0) as u32;
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
            rewards,
            journal,
            initial,
            kind: g.field(p, 13022).and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            priority: g.field(p, 13010).and_then(|v| v.as_i64()).unwrap_or(0) as i32,
            parent: s(g, p, 13017).map(|r| r.to_ascii_lowercase()),
            finals,
        });
    }
    out
}

impl Plots {
    /// Registers a plot by its GUID and resource name, for tests.
    #[cfg(test)]
    pub(crate) fn insert(&mut self, p: PlotInfo) {
        self.loaded = true;
        let p = Arc::new(p);
        self.by_key.insert(p.guid.clone(), p.clone());
        self.by_key.insert(p.resref.clone(), p);
    }

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

    /// Every plot, once each (by GUID order).
    pub fn all(&mut self, vfs: &Vfs) -> Vec<Arc<PlotInfo>> {
        self.load(vfs);
        let mut v: Vec<Arc<PlotInfo>> = self
            .by_key
            .iter()
            .filter(|(k, p)| **k == p.guid)
            .map(|(_, p)| p.clone())
            .collect();
        v.sort_by(|a, b| a.guid.cmp(&b.guid));
        v
    }

    /// A plot by GUID or resource name.
    pub fn get(&mut self, vfs: &Vfs, key: &str) -> Option<Arc<PlotInfo>> {
        self.load(vfs);
        let k = key.trim().trim_end_matches('\0');
        // Tables name plots without the file prefix (`lot100pt_bears` for
        // `plt_lot100pt_bears.plo`).
        self.by_key
            .get(&k.to_ascii_uppercase())
            .or_else(|| self.by_key.get(&k.to_ascii_lowercase()))
            .or_else(|| self.by_key.get(&format!("plt_{}", k.to_ascii_lowercase())))
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
        // The creator is the player: plot scripts search from it for nearby creatures
        // (`UT_GetNearestCreatureByTag(GetEventCreator(ev), ...)`).
        let mut ev = self.new_event(ty, self.world.hero.or(self.world.party));
        ev.strings = vec![guid.to_string()];
        ev.ints = vec![0, flag, value, old];
        ev.objects = vec![owner];
        ev
    }

    /// `OBJECT_SELF` for plot scripts. Provisional: the hero, since plot scripts pass
    /// `OBJECT_SELF` to `UT_Talk` as the listener.
    fn plot_self(&self) -> ObjectId {
        self.world
            .hero
            .or(self.world.party)
            .unwrap_or(OBJECT_INVALID)
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
                // A plot script that reads the flag it is answering would recurse forever;
                // the inner read gets the stored value instead.
                let asking = (key.clone(), flag);
                if flag >= 256 && !self.plots.asking.contains(&asking) {
                    let ev = self.plot_event("GET_PLOT", &key, flag, 0, 0, owner);
                    self.plots.asking.push(asking);
                    let r = self.run_script(&script, self.plot_self(), Some(ev));
                    self.plots.asking.pop();
                    return matches!(r, Some(V::Int(v)) if v != 0);
                }
            }
        }
        self.stored_plot_flag(plot, key, flag)
    }

    /// A flag's stored value: what was set, else the value the plot file starts it with.
    fn stored_plot_flag(&mut self, plot: &str, key: String, flag: i32) -> bool {
        match self.world.plots.get(&(key, flag)) {
            Some(&v) => v,
            None => self.plot_info(plot).is_some_and(|p| p.initial_value(flag)),
        }
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
        let old = self.stored_plot_flag(plot, key.clone(), flag);
        self.world.plots.insert((key.clone(), flag), value);
        if value {
            if let Some(info) = self.plot_info(plot) {
                self.note_journal_flag(&info, flag);
            }
        }
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
                self.run_script(&script, self.plot_self(), Some(ev));
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
                tmpl(b"PLOT", &[13000, 13007, 13008, 13009, 13011, 13012]),
                tmpl(b"FLAG", &[13001, 13002, 13003]),
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
                        Value::U32(1 << 5),
                        Value::U32(1),
                    ],
                ),
                st(
                    2,
                    vec![
                        Value::U32(0),
                        Value::String(Some("FIRST\0".into())),
                        Value::U32(142),
                    ],
                ),
                st(
                    2,
                    vec![
                        Value::U32(256),
                        Value::String(Some("DEFINED\0".into())),
                        Value::U32(0),
                    ],
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
        assert_eq!(p[0].rewards.get(&0), Some(&142));
        assert_eq!(p[0].rewards.get(&256), None, "no reward is not a reward");
        // Fields 13011.. start flags set, 32 to a field.
        assert_eq!(p[0].initial, [32, 1, 0, 0]);
        assert!(p[0].initial_value(5) && p[0].initial_value(32));
        assert!(!p[0].initial_value(4) && !p[0].initial_value(256));
    }
}
