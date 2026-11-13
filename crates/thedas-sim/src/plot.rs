//! Plots (`.plo`, GFF4 `PLTS`; Dragon Age II's are GFF4 `PLOT`, see [`Composite`]): named
//! flags grouped under a GUID, with an optional script
//! that runs when a flag is set (`EVENT_TYPE_SET_PLOT`) and answers "defined" flags
//! (`EVENT_TYPE_GET_PLOT`).
//!
//! Provisional, inferred from the plot files and the plot scripts' bytecode: the event carries
//! the plot GUID as string 0, the flag as int 1, the new and old values as ints 2 and 3, the
//! conversation owner as object 0 and the hero as its creator; a `GET_PLOT` answer is the
//! script's integer result. Flags above 127 are "defined" flags the script answers (see
//! [`DEFINED_FLAGS`]); the engine sets ordinary flags before running the script.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use thedas_formats::{Gff4, List, StructId, Value};
use thedas_script::vm::{OBJECT_INVALID, ObjectId, Value as V};
use thedas_vfs::Vfs;

use crate::sim::Sim;
use crate::trace::TraceEntry;

/// The first "defined" flag. DAOrigins.exe keeps flags 0 to 127 in the plot record's
/// four-dword flag set (the stored read, 0x75bc70, answers false past dword 3) and hands every
/// higher flag to the plot script: the flag read (0x75ea60, `cmp 0x7f` at 0x75eb5a) asks the
/// script with `EVENT_TYPE_GET_PLOT` and answers false for a plot without one; the flag set
/// (0x75f7e0, `cmp 0x7f` at 0x75f944) sends `EVENT_TYPE_SET_PLOT` whether or not the caller
/// asked for the script, and stores nothing. The shipped plots number their defined flags from
/// 256, so nothing between 128 and 255 exists in the data.
pub const DEFINED_FLAGS: i32 = 128;

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
    /// The plot's priority (field 13010, 0 to 3), which orders quest groups (provisional)
    /// and decides whether its first entry shows as a large alert (0 or 1: DAOrigins.exe
    /// 0x75ff0c for a quest, 0x75fde3 for a codex entry).
    pub priority: i32,
    /// The plot it belongs under (field 13017, a plot resource): a quest group for a quest, a
    /// quest for a sub-quest. Provisional.
    pub parent: Option<String>,
    /// The flags that end the quest (field 13005 non-zero: 255 on a quest's reward or ending
    /// flags). Provisional.
    pub finals: std::collections::BTreeSet<i32>,
    /// Whether the game pauses while this tutorial's popup shows (field 13023 non-zero; 255
    /// on `plt_tut_combat_basic` and `plt_tut_combat_basic_magic`), as DAOrigins.exe reads
    /// it (0x75f350, field 0x32df) when it queues a tutorial (0x693f10).
    pub pauses: bool,
    /// Dragon Age II only: the plot's composite flags (ids from 256), each a boolean
    /// expression over other flags that the plot file itself stores (see [`Composite`]).
    /// Read from the file but not yet consulted by the flag read.
    pub composites: BTreeMap<i32, Composite>,
}

/// A Dragon Age II composite flag: a boolean expression over flags of this or other plots.
///
/// The plot file stores the flags it reads (field 13502, each a plot GUID and a flag) and the
/// expression in prefix order (field 13520, clauses of an operator 13521 and an argument
/// 13522): operator 0 is an operand, its argument an index into the flag list; operators 1
/// and 2 take as many sub-expressions as their argument says; operator 3 takes one.
///
/// Provisional: read from the shipped files, not traced in the executable. That 1 is "all",
/// 2 "any" and 3 "not" is inferred from the expressions' shapes (most are operator 1 over
/// operands and operator-3 terms, the "this and not that" a quest state asks).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Composite {
    /// The flags the expression reads: `(plot GUID, flag)`.
    pub operands: Vec<(String, i32)>,
    /// The expression in prefix order: `(operator, argument)`.
    pub clauses: Vec<(u8, u16)>,
}

impl Composite {
    /// The expression's value, given each operand flag's value. `None` when the plot stores
    /// no expression (two shipped composite flags have none), when the stored expression is
    /// malformed (see [`Composite::well_formed`]), or when it holds a "not" without exactly
    /// one sub-expression (two shipped flags have a "not" of nothing; what the original
    /// makes of that is not known).
    pub fn eval(&self, flag: &mut dyn FnMut(&str, i32) -> bool) -> Option<bool> {
        let mut at = 0;
        let value = self.walk(&mut at, flag, 0)?;
        if at == self.clauses.len() {
            value
        } else {
            None
        }
    }

    /// Whether the stored clauses form one complete expression: every operand index inside
    /// the operand list, every operator known and followed by the sub-expressions it
    /// counts, and no clause left over. No expression at all is well formed.
    pub fn well_formed(&self) -> bool {
        if self.clauses.is_empty() {
            return true;
        }
        let mut at = 0;
        self.walk(&mut at, &mut |_, _| false, 0).is_some() && at == self.clauses.len()
    }

    /// Walks one sub-expression from clause `at`, leaving `at` past it. The outer `None` is
    /// a malformed expression; the inner one a value that cannot be told.
    fn walk(
        &self,
        at: &mut usize,
        flag: &mut dyn FnMut(&str, i32) -> bool,
        depth: usize,
    ) -> Option<Option<bool>> {
        // Shipped expressions nest three deep at most; this only bounds malformed input.
        if depth > 32 {
            return None;
        }
        let &(op, arg) = self.clauses.get(*at)?;
        *at += 1;
        if op == 0 {
            let (guid, id) = self.operands.get(arg as usize)?;
            return Some(Some(flag(guid, *id)));
        }
        if !(1..=3).contains(&op) {
            return None;
        }
        // Every sub-expression is walked, so the position ends past the whole term.
        let mut values = Vec::new();
        for _ in 0..arg {
            values.push(self.walk(at, flag, depth + 1)?);
        }
        Some(match op {
            // All: false if any is false, unknown if any is unknown, else true.
            1 if values.contains(&Some(false)) => Some(false),
            1 if values.contains(&None) => None,
            1 => Some(true),
            // Any: true if any is true, unknown if any is unknown, else false.
            2 if values.contains(&Some(true)) => Some(true),
            2 if values.contains(&None) => None,
            2 => Some(false),
            // Not: of exactly one sub-expression.
            _ => match values[..] {
                [Some(v)] => Some(!v),
                _ => None,
            },
        })
    }
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
    /// While a report is being made (see [`Sim::conversation_conditions`]), every flag read,
    /// outermost first.
    pub(crate) reads: Option<Vec<FlagRead>>,
    /// While a report is being made, every flag set: (plot GUID, flag, value).
    pub(crate) sets: Option<Vec<(String, i32, bool)>>,
}

/// A plot flag read while a condition was evaluated.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct FlagRead {
    /// The plot's GUID.
    pub plot: String,
    /// Its resource name, when the plot file was found.
    pub resref: Option<String>,
    pub flag: i32,
    /// The flag's name in the plot file.
    pub name: Option<String>,
    pub value: bool,
    /// A defined flag ([`DEFINED_FLAGS`] and up): its value is the plot script's answer (or a
    /// Dragon Age II composite's), not a stored one.
    pub defined: bool,
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

/// A Dragon Age II plot file (GFF4 type `PLOT`): one plot, the root struct.
///
/// Labels read from the shipped files (provisional, none traced): the root holds 13000 the
/// GUID, 13001 the name, 13002 the plot script (`plot_core` for most), 13003 a number 0 to 3
/// (read here as the priority), 13004 another plot's resource (read as the parent), 13006 a
/// number 1 to 14 (read as the type), 13050 to 13053 four words in the place of Dragon Age:
/// Origins' initial flag bits, 13300 the simple flags and 13501 the composite flags. A simple
/// flag holds 13200 its id, 13302 a reward-like number and 13303 its journal text; flag names
/// are not shipped, so every flag's name is empty. Not read yet: a simple flag's 13304
/// (a resource), 13306 (flags of other plots) and 13320 lists.
fn da2_plot(g: &Gff4, resref: &str) -> Option<PlotInfo> {
    let p = g.root;
    let guid = s(g, p, 13000)?;
    let int = |id: StructId, l: u32| g.field(id, l).and_then(|v| v.as_i64());
    let mut info = PlotInfo {
        guid: guid.to_ascii_uppercase(),
        resref: s(g, p, 3)
            .map(|r| r.to_ascii_lowercase())
            .filter(|_| resref.is_empty())
            .unwrap_or_else(|| resref.to_string()),
        script: s(g, p, 13002),
        name_strref: match g.field(p, 13001) {
            Some(Value::Tlk(t)) if t.id != u32::MAX => Some(t.id),
            _ => None,
        },
        kind: int(p, 13006).unwrap_or(0) as i32,
        priority: int(p, 13003).unwrap_or(0) as i32,
        parent: s(g, p, 13004).map(|r| r.to_ascii_lowercase()),
        ..Default::default()
    };
    for (i, label) in (13050..=13053).enumerate() {
        info.initial[i] = int(p, label).unwrap_or(0) as u32;
    }
    for f in structs(g, p, 13300) {
        let Some(id) = int(f, 13200) else { continue };
        let id = id as i32;
        info.flags.insert(id, String::new());
        if let Some(r) = int(f, 13302).filter(|&r| r != 0) {
            info.rewards.insert(id, r as i32);
        }
        if let Some(Value::Tlk(t)) = g.field(f, 13303) {
            if t.id != u32::MAX {
                info.journal.insert(id, t.id);
            }
        }
    }
    for c in structs(g, p, 13501) {
        let Some(id) = int(c, 13200) else { continue };
        let operands = structs(g, c, 13502)
            .into_iter()
            .map(|o| {
                (
                    s(g, o, 13100).unwrap_or_default().to_ascii_uppercase(),
                    int(o, 13101).unwrap_or(0) as i32,
                )
            })
            .collect();
        let clauses = structs(g, c, 13520)
            .into_iter()
            .map(|o| {
                (
                    int(o, 13521).unwrap_or(0) as u8,
                    int(o, 13522).unwrap_or(0) as u16,
                )
            })
            .collect();
        info.flags.insert(id as i32, String::new());
        info.composites
            .insert(id as i32, Composite { operands, clauses });
    }
    Some(info)
}

pub fn plots_from_gff4(g: &Gff4, resref: &str) -> Vec<PlotInfo> {
    if g.file_type.0 == *b"PLOT" {
        return da2_plot(g, resref).into_iter().collect();
    }
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
            pauses: g.field(p, 13023).and_then(|v| v.as_i64()).unwrap_or(0) != 0,
            composites: BTreeMap::new(),
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

    /// Reads a flag. Defined flags ([`DEFINED_FLAGS`] and up) are the plot script's answer
    /// (false without a script). An ordinary flag read with `call_script` runs the script's
    /// `GET_PLOT` too, as DAOrigins.exe does (0x75ec7b to 0x75ecf8), but answers with the stored
    /// value regardless (0x75ed36).
    pub fn get_plot_flag(
        &mut self,
        plot: &str,
        flag: i32,
        call_script: bool,
        owner: ObjectId,
    ) -> bool {
        // Only the condition's own reads are reported, not those of the plot scripts that
        // answer them.
        let outermost = self.plots.asking.is_empty();
        let value = self.read_plot_flag(plot, flag, call_script, owner);
        if outermost && self.plots.reads.is_some() {
            let info = self.plot_info(plot);
            let read = FlagRead {
                plot: self.plot_key(plot),
                resref: info.as_ref().map(|p| p.resref.clone()),
                flag,
                name: info.as_ref().and_then(|p| p.flags.get(&flag).cloned()),
                value,
                defined: flag >= DEFINED_FLAGS,
            };
            if let Some(r) = self.plots.reads.as_mut() {
                r.push(read);
            }
        }
        value
    }

    fn read_plot_flag(
        &mut self,
        plot: &str,
        flag: i32,
        call_script: bool,
        owner: ObjectId,
    ) -> bool {
        let key = self.plot_key(plot);
        let defined = flag >= DEFINED_FLAGS;
        // A Dragon Age II composite flag: the plot file states its value as an expression
        // over other flags (see [`Composite`]), so no script is asked. Provisional: the
        // evaluation is not traced in DragonAge2.exe. A flag already being answered (a cycle)
        // and an expression with no value read false.
        if defined {
            if let Some(c) = self
                .plot_info(plot)
                .and_then(|p| p.composites.get(&flag).cloned())
            {
                let asking = (key, flag);
                if self.plots.asking.contains(&asking) {
                    return false;
                }
                self.plots.asking.push(asking);
                let value = c.eval(&mut |guid, f| self.get_plot_flag(guid, f, false, owner));
                self.plots.asking.pop();
                return value.unwrap_or(false);
            }
        }
        if defined || call_script {
            if let Some(script) = self.plot_info(plot).and_then(|p| p.script.clone()) {
                // A plot script that reads the flag it is answering would recurse forever;
                // the inner read gets the stored value instead.
                let asking = (key.clone(), flag);
                if !self.plots.asking.contains(&asking) {
                    let ev = self.plot_event("GET_PLOT", &key, flag, 0, 0, owner);
                    self.plots.asking.push(asking);
                    let r = self.run_script(&script, self.plot_self(), Some(ev));
                    self.plots.asking.pop();
                    if defined {
                        return matches!(r, Some(V::Int(v)) if v != 0);
                    }
                }
            }
        }
        self.stored_plot_flag(plot, key, flag)
    }

    /// A flag's stored value: what was set, else the value the plot file starts it with.
    /// Defined flags store nothing and read false here.
    fn stored_plot_flag(&mut self, plot: &str, key: String, flag: i32) -> bool {
        if flag >= DEFINED_FLAGS {
            return false;
        }
        match self.world.plots.get(&(key, flag)) {
            Some(&v) => v,
            None => self.plot_info(plot).is_some_and(|p| p.initial_value(flag)),
        }
    }

    /// The value an ordinary flag holds now, as an undo puts it back.
    pub(crate) fn stored_flag_for_undo(&mut self, plot: &str, flag: i32) -> bool {
        let key = self.plot_key(plot);
        self.stored_plot_flag(plot, key, flag)
    }

    /// Whether a script has set an ordinary flag: the stored value only, not the value the
    /// plot file starts it with (the journal counts what happened; a Chanter's Board posting
    /// starts set). Defined flags are never stored and read false.
    pub(crate) fn script_set_plot_flag(&self, guid: &str, flag: i32) -> bool {
        flag < DEFINED_FLAGS
            && self
                .world
                .plots
                .get(&(guid.to_string(), flag))
                .copied()
                .unwrap_or(false)
    }

    /// Sets a flag. An ordinary flag is stored and its script runs when `call_script` is set; a
    /// defined flag ([`DEFINED_FLAGS`] and up) is not stored and always goes to the script, as
    /// DAOrigins.exe's flag set does (0x75f7e0; the defined branch at 0x75f951 to 0x75f9eb
    /// does not test the caller's flag).
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
        let defined = flag >= DEFINED_FLAGS;
        if !defined {
            self.world.plots.insert((key.clone(), flag), value);
            // A stored flag with journal text makes the quest's latest entry; a tutorial's
            // newly set flag queues its popup.
            if value {
                if let Some(info) = self.plot_info(plot) {
                    self.note_journal_flag(&info, flag);
                    if !old {
                        self.queue_tutorial(&info, flag);
                    }
                }
            }
        }
        if let Some(sets) = self.plots.sets.as_mut() {
            sets.push((key.clone(), flag, value));
        }
        let t = self.world.time_ms;
        self.world.trace.push(TraceEntry::PlotFlag {
            t,
            plot: key.clone(),
            flag,
            value,
        });
        if call_script || defined {
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
    fn flags_past_127_are_defined_and_never_stored() {
        use thedas_script::EngineTable;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let none = OBJECT_INVALID;
        s.set_plot_flag("plt_test", 127, true, false, none);
        assert!(
            s.get_plot_flag("plt_test", 127, false, none),
            "the last stored flag"
        );
        s.set_plot_flag("plt_test", 128, true, false, none);
        s.set_plot_flag("plt_test", 200, true, true, none);
        assert!(
            !s.get_plot_flag("plt_test", 128, false, none),
            "no script answers it"
        );
        assert!(!s.get_plot_flag("plt_test", 200, true, none));
        let key = s.plot_key("plt_test");
        assert!(
            !s.world.plots.contains_key(&(key, 128)),
            "defined flags store nothing"
        );
    }

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

    #[test]
    fn evaluates_composite_flags() {
        // all(A, not(B), any(C, D)) in prefix order.
        let c = Composite {
            operands: vec![
                ("P".into(), 1),
                ("P".into(), 2),
                ("Q".into(), 3),
                ("Q".into(), 4),
            ],
            clauses: vec![(1, 3), (0, 0), (3, 1), (0, 1), (2, 2), (0, 2), (0, 3)],
        };
        let with = |set: &'static [(&'static str, i32)]| {
            c.eval(&mut move |guid, flag| set.contains(&(guid, flag)))
        };
        assert_eq!(with(&[("P", 1), ("Q", 4)]), Some(true));
        assert_eq!(with(&[("P", 1), ("P", 2), ("Q", 4)]), Some(false));
        assert_eq!(with(&[("P", 1)]), Some(false));
        assert_eq!(with(&[("Q", 3)]), Some(false));

        // Malformed expressions answer nothing rather than a guess.
        let bad = |clauses: Vec<(u8, u16)>| Composite {
            operands: vec![("P".into(), 1)],
            clauses,
        };
        assert_eq!(bad(vec![]).eval(&mut |_, _| true), None, "no expression");
        assert!(bad(vec![]).well_formed());
        // A "not" of nothing (two shipped flags): well formed, value unknown, and an "all"
        // around it is false only if another term is.
        let odd = bad(vec![(1, 2), (0, 0), (3, 0)]);
        assert!(odd.well_formed());
        assert_eq!(odd.eval(&mut |_, _| true), None);
        assert_eq!(odd.eval(&mut |_, _| false), Some(false));
        assert!(!bad(vec![(1, 2), (0, 0)]).well_formed());
        assert!(!bad(vec![(0, 5)]).well_formed());
        assert!(!bad(vec![(9, 1), (0, 0)]).well_formed());
        assert_eq!(
            bad(vec![(0, 5)]).eval(&mut |_, _| true),
            None,
            "operand past the list"
        );
        assert_eq!(
            bad(vec![(1, 2), (0, 0)]).eval(&mut |_, _| true),
            None,
            "ends early"
        );
        assert_eq!(
            bad(vec![(0, 0), (0, 0)]).eval(&mut |_, _| true),
            None,
            "left over"
        );
        assert_eq!(
            bad(vec![(9, 1), (0, 0)]).eval(&mut |_, _| true),
            None,
            "unknown operator"
        );
    }

    #[test]
    fn parses_dragon_age_2_plot_files() {
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
        let list = |ids: &[usize]| {
            Value::List(Some(List::Struct(
                ids.iter().map(|&i| Some(StructId(i))).collect(),
            )))
        };
        let text = |t: &str| Value::String(Some(format!("{t}\0")));
        let g = Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"PLOT"),
            file_version: FourCC::new(b"V2.0"),
            templates: vec![
                tmpl(
                    b"PLOT",
                    &[
                        13000, 13001, 13002, 13003, 13004, 13006, 13050, 13300, 13501, 3,
                    ],
                ),
                tmpl(b"SIMP", &[13200, 13302, 13303]),
                tmpl(b"COMP", &[13200, 13502, 13520]),
                tmpl(b"PFLG", &[13100, 13101]),
                tmpl(b"CLAS", &[13521, 13522]),
            ],
            structs: vec![
                st(
                    0,
                    vec![
                        text("abcd"),
                        Value::Tlk(TlkString { id: 7, text: None }),
                        text("plot_core"),
                        Value::I32(2),
                        text("ACT00PT_Group"),
                        Value::I32(4),
                        Value::U32(1 << 3),
                        list(&[1]),
                        list(&[2]),
                        text("Act00PT_Main"),
                    ],
                ),
                st(
                    1,
                    vec![
                        Value::U8(3),
                        Value::I32(55),
                        Value::Tlk(TlkString { id: 9, text: None }),
                    ],
                ),
                st(2, vec![Value::U16(256), list(&[3]), list(&[4, 5])]),
                st(3, vec![text("ffee"), Value::U16(12)]),
                st(4, vec![Value::U8(3), Value::U16(1)]),
                st(4, vec![Value::U8(0), Value::U16(0)]),
            ],
            root: StructId(0),
            shared_strings: None,
        };
        let p = plots_from_gff4(&g, "");
        assert_eq!(p.len(), 1);
        let p = &p[0];
        assert_eq!(p.guid, "ABCD");
        assert_eq!(p.resref, "act00pt_main");
        assert_eq!(p.script.as_deref(), Some("plot_core"));
        assert_eq!(p.name_strref, Some(7));
        assert_eq!((p.kind, p.priority), (4, 2));
        assert_eq!(p.parent.as_deref(), Some("act00pt_group"));
        assert!(p.initial_value(3) && !p.initial_value(4));
        assert_eq!(p.flags.keys().copied().collect::<Vec<_>>(), vec![3, 256]);
        assert_eq!(p.rewards.get(&3), Some(&55));
        assert_eq!(p.journal.get(&3), Some(&9));
        let c = &p.composites[&256];
        assert_eq!(c.operands, vec![("FFEE".to_string(), 12)]);
        // not(FFEE flag 12)
        assert_eq!(c.eval(&mut |_, _| false), Some(true));
        assert_eq!(c.eval(&mut |g, f| g == "FFEE" && f == 12), Some(false));
        // The caller's resource name wins when it gives one.
        assert_eq!(plots_from_gff4(&g, "plt_x")[0].resref, "plt_x");
    }
}
