//! The simulation: a [`World`] plus the resources and scripts that drive it.
//!
//! Scripts run on the NWScript VM with [`ScriptHost`] answering engine calls (see
//! [`crate::engine`]). Events are delivered to an object's event script; `DelayEvent` and
//! non-immediate `SignalEvent` queue them; [`Sim::tick`] advances time and drains the queue.
//!
//! Provisional (not yet checked against the original): the order of load events (module
//! `MODULE_LOAD`; then, for an area, `SPAWN` to each creature and placeable,
//! `AREALOAD_SPECIAL`, `AREALOAD_PRELOADEXIT`, `ENTER` for the hero, `AREALOAD_POSTLOADEXIT`),
//! and the command executor, which completes commands as soon as it reaches them (waits last
//! their duration) and then sends `COMMAND_COMPLETE`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use thedas_formats::gff3::{Gff3, Struct, Value as G3};
use thedas_formats::{Gff4, StructId};
use thedas_script::ldf::{ConstValue, EngineTable, Function};
use thedas_script::vm::{Host, ObjectId, Value, VmError, VmResult};
use thedas_script::{Ncs, Vm};
use thedas_vfs::Vfs;

use crate::trace::TraceEntry;
use crate::world::{Event, Location, Object, ObjectKind, World};

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("{0}: not found")]
    NotFound(String),
    #[error("{0}: {1}")]
    Parse(String, String),
    #[error(transparent)]
    Vfs(#[from] thedas_vfs::Error),
}

/// One area in the module's area tree.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AreaEntry {
    pub resref: String,
    pub layout: Option<String>,
    pub area_list: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ModuleInfo {
    pub name: String,
    pub script: Option<String>,
    pub start_area: Option<String>,
    pub start_position: [f32; 3],
    pub areas: Vec<AreaEntry>,
}

/// Indexes of engine structures in the engine table (`ENGINE_STRUCTURE_n`).
#[derive(Clone, Copy, Debug)]
pub struct EngineTypes {
    pub event: u8,
    pub location: u8,
    pub command: u8,
    pub effect: u8,
}

pub struct Sim {
    pub world: World,
    pub vfs: Arc<Vfs>,
    pub table: Arc<EngineTable>,
    pub module_info: ModuleInfo,
    scripts: HashMap<String, Option<Arc<Ncs>>>,
    pub(crate) types: EngineTypes,
    event_names: HashMap<i32, String>,
    depth: usize,
    /// Instructions a single script run may take.
    pub script_budget: u64,
    pub(crate) talk: Option<crate::dialog::Conversation>,
    pub(crate) plots: crate::plot::Plots,
    strings: std::sync::OnceLock<std::collections::BTreeMap<u32, String>>,
}

pub const MAX_DEPTH: usize = 24;

/// Looks up an integer constant from the engine table (0 if absent).
pub fn constant(table: &EngineTable, name: &str) -> i32 {
    match table.constant(name).map(|c| &c.value) {
        Some(ConstValue::Int(v)) => *v as i32,
        _ => 0,
    }
}

impl Sim {
    pub fn new(vfs: Arc<Vfs>, table: Arc<EngineTable>, seed: u64) -> Self {
        let tables = Arc::new(crate::m2da::Tables::new(vfs.clone()));
        let pos = |n: &str| table.structures.iter().position(|s| s == n).unwrap_or(0) as u8;
        let types = EngineTypes {
            event: pos("event"),
            location: pos("location"),
            command: pos("command"),
            effect: pos("effect"),
        };
        let event_names = table
            .constants
            .iter()
            .filter_map(|c| match (&c.value, c.name.strip_prefix("EVENT_TYPE_")) {
                (ConstValue::Int(v), Some(n)) => Some((*v as i32, n.to_string())),
                _ => None,
            })
            .collect();
        Sim {
            world: World::new(tables, seed),
            vfs,
            table,
            module_info: ModuleInfo::default(),
            scripts: HashMap::new(),
            types,
            event_names,
            depth: 0,
            script_budget: 5_000_000,
            talk: None,
            plots: crate::plot::Plots::default(),
            strings: std::sync::OnceLock::new(),
        }
    }

    /// A talk-table string (English tables: `*_en-us.tlk` in the stack, merged).
    pub fn string(&self, strref: u32) -> Option<String> {
        let map = self.strings.get_or_init(|| {
            let mut m = std::collections::BTreeMap::new();
            for name in self
                .vfs
                .names()
                .into_iter()
                .filter(|n| n.ends_with("en-us.tlk"))
            {
                if let Ok((data, _)) = self.vfs.read(&name) {
                    if let Ok(t) = thedas_formats::Tlk::parse(&data) {
                        m.extend(t.strings);
                    }
                }
            }
            m
        });
        map.get(&strref).cloned()
    }

    pub fn event_name(&self, ty: i32) -> String {
        self.event_names
            .get(&ty)
            .cloned()
            .unwrap_or_else(|| ty.to_string())
    }

    pub fn event_type(&self, name: &str) -> i32 {
        constant(&self.table, &format!("EVENT_TYPE_{name}"))
    }

    /// Registers a compiled script under a name, ahead of the resource stack (tools, tests).
    pub fn add_script(&mut self, name: &str, ncs: Ncs) {
        self.scripts
            .insert(name.to_ascii_lowercase(), Some(Arc::new(ncs)));
    }

    fn script(&mut self, name: &str) -> Option<Arc<Ncs>> {
        let key = name.to_ascii_lowercase();
        if let Some(s) = self.scripts.get(&key) {
            return s.clone();
        }
        let res = if key.ends_with(".ncs") {
            key.clone()
        } else {
            format!("{key}.ncs")
        };
        let loaded = self
            .vfs
            .read(&res)
            .ok()
            .and_then(|(d, _)| Ncs::parse(&d).ok())
            .map(Arc::new);
        self.scripts.insert(key, loaded.clone());
        loaded
    }

    /// Runs a script on `caller` with an optional current event. Errors are traced, not
    /// returned: a failing script stops, the simulation goes on.
    pub fn run_script(
        &mut self,
        name: &str,
        caller: ObjectId,
        event: Option<Event>,
    ) -> Option<Value> {
        let t = self.world.time_ms;
        if self.depth >= MAX_DEPTH {
            self.world.trace.push(TraceEntry::Missing {
                t,
                what: format!("script {name}: nesting deeper than {MAX_DEPTH}"),
            });
            return None;
        }
        let Some(ncs) = self.script(name) else {
            self.world.trace.push(TraceEntry::Missing {
                t,
                what: format!("script {name}"),
            });
            return None;
        };
        self.world.trace.push(TraceEntry::ScriptStart {
            t,
            script: name.to_string(),
            caller,
            depth: self.depth,
        });
        let event = event.map(|e| self.world.store(crate::world::EngineData::Event(e)));
        let mut vm = Vm::new(ncs, self.table.clone());
        vm.budget = self.script_budget;
        self.depth += 1;
        let result = {
            let mut host = ScriptHost {
                sim: self,
                event,
                script: name.to_string(),
            };
            vm.run(&mut host, caller)
        };
        self.depth -= 1;
        let (steps, error, value) = match result {
            Ok(r) => (r.steps, None, r.value),
            Err(e) => (0, Some(e.to_string()), None),
        };
        self.world.trace.push(TraceEntry::ScriptEnd {
            t: self.world.time_ms,
            script: name.to_string(),
            steps,
            error,
        });
        value
    }

    /// Delivers an event to an object's event script (or `script`, when given).
    pub fn deliver(&mut self, target: ObjectId, mut event: Event, script: Option<String>) {
        if event.target.is_none() {
            event.target = Some(target);
        }
        let Some(obj) = self.world.get(target) else {
            self.world.trace.push(TraceEntry::Missing {
                t: self.world.time_ms,
                what: format!("event {} for invalid object {target:#x}", event.ty),
            });
            return;
        };
        let Some(script) = script.or_else(|| obj.script.clone()) else {
            return;
        };
        self.world.trace.push(TraceEntry::EventDelivered {
            t: self.world.time_ms,
            target,
            ty: event.ty,
            event_name: self.event_name(event.ty),
            script: script.clone(),
        });
        self.run_script(&script, target, Some(event));
    }

    pub fn new_event(&self, name: &str, creator: Option<ObjectId>) -> Event {
        Event {
            ty: self.event_type(name),
            creator,
            ..Default::default()
        }
    }

    /// Advances time by `ms`, delivering every event that falls due and running commands.
    pub fn tick(&mut self, ms: u64) {
        let until = self.world.time_ms + ms;
        loop {
            while let Some((due, q)) = self.world.pop_due(until) {
                // Events run at their own due time, so delays chain correctly.
                self.world.time_ms = self.world.time_ms.max(due);
                self.deliver(q.target, q.event, q.script);
            }
            if !self.run_commands(until) {
                break;
            }
        }
        self.world.time_ms = until;
    }

    /// Runs the front command of every object whose command is ready; returns whether any
    /// command finished (which may have queued more events).
    fn run_commands(&mut self, until: u64) -> bool {
        let ids: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|o| !o.commands.is_empty())
            .map(|o| o.id)
            .collect();
        let mut any = false;
        let now = self.world.time_ms;
        for id in ids {
            let Some(obj) = self.world.get_mut(id) else {
                continue;
            };
            let Some(cmd) = obj.commands.first().cloned() else {
                continue;
            };
            // A wait starts when it reaches the front and records its end time in ints[0].
            if cmd.name == "CommandWait" {
                let end = match cmd.ints.first() {
                    Some(&end) => end as u64,
                    None => {
                        let end = now + (cmd.seconds.max(0.0) * 1000.0) as u64;
                        if let Some(c) = obj.commands.first_mut() {
                            c.ints.insert(0, end as i32);
                        }
                        end
                    }
                };
                if end > until {
                    continue;
                }
            }
            let obj = self.world.get_mut(id).expect("checked above");
            obj.commands.remove(0);
            if let (Some(t), true) = (cmd.target, cmd.name.starts_with("CommandMoveToObject")) {
                if let Some(p) = self.world.get(t).map(|o| (o.position, o.area)) {
                    let obj = self.world.get_mut(id).expect("checked above");
                    obj.position = p.0;
                    obj.area = p.1;
                }
            }
            if let Some(l) = cmd.location {
                let obj = self.world.get_mut(id).expect("checked above");
                obj.position = l.position;
                obj.area = Some(l.area);
                obj.facing = l.facing;
            }
            self.world.trace.push(TraceEntry::Command {
                t: self.world.time_ms,
                object: id,
                command: cmd.name.clone(),
                status: "complete".into(),
            });
            let mut ev = self.new_event("COMMAND_COMPLETE", Some(id));
            ev.ints = vec![cmd.ty, 1];
            self.world.queue_event(0, id, ev, None);
            any = true;
        }
        any
    }

    // -----------------------------------------------------------------------------------------
    // Loading

    fn read_gff3(&self, name: &str) -> Result<Gff3, SimError> {
        let (data, _) = self
            .vfs
            .read(name)
            .map_err(|_| SimError::NotFound(name.to_string()))?;
        Gff3::parse(&data).map_err(|e| SimError::Parse(name.to_string(), e.to_string()))
    }

    /// Loads a module's campaign file (`modules/<dir>/*.cif`): module variables, script, start
    /// area and the area tree. Creates the module object, the hero and the party.
    pub fn load_module(&mut self, install_root: &Path, module_dir: &str) -> Result<(), SimError> {
        let dir = install_root.join("modules").join(module_dir);
        let cif = std::fs::read_dir(&dir)
            .map_err(|_| SimError::NotFound(dir.display().to_string()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cif")))
            .ok_or_else(|| SimError::NotFound(format!("{module_dir}/*.cif")))?;
        let data = std::fs::read(&cif)
            .map_err(|e| SimError::Parse(cif.display().to_string(), e.to_string()))?;
        let g = Gff4::parse(&data).map_err(|e| SimError::Parse("cif".into(), e.to_string()))?;
        let mut info = parse_cif(&g);
        info.name = module_dir.to_string();
        let module = self.world.spawn(ObjectKind::Module, "module");
        self.created(module);
        {
            let m = self.world.get_mut(module).expect("just spawned");
            m.script = info.script.clone();
            m.template = info.script.clone().unwrap_or_default();
            for (name, value) in cif_vars(&g) {
                match value {
                    thedas_formats::gff4::Value::I32(v) => {
                        m.locals.ints.insert(name, v);
                    }
                    thedas_formats::gff4::Value::F32(v) => {
                        m.locals.floats.insert(name, v);
                    }
                    thedas_formats::gff4::Value::String(Some(s)) => {
                        m.locals
                            .strings
                            .insert(name, s.trim_end_matches('\0').to_string());
                    }
                    other => {
                        if let Some(v) = other.as_i64() {
                            m.locals.ints.insert(name, v as i32);
                        }
                    }
                }
            }
        }
        self.world.module = Some(module);
        // The hero, with a default identity until character creation is simulated.
        let party = self.world.spawn(ObjectKind::Party, "party");
        let hero = self.world.spawn(ObjectKind::Creature, "player");
        {
            let h = self.world.get_mut(hero).expect("just spawned");
            h.script = Some("player_core".into());
            h.template = "player".into();
            h.rank = constant(&self.table, "CREATURE_RANK_PLAYER");
        }
        self.created(hero);
        self.world.hero = Some(hero);
        self.world.party = Some(party);
        self.module_info = info;
        // A new game: the module loads, then starts (provisional order).
        for name in ["MODULE_LOAD", "MODULE_START"] {
            let ev = self.new_event(name, Some(module));
            self.world.queue_event(0, module, ev, None);
        }
        Ok(())
    }

    fn created(&mut self, id: ObjectId) {
        if let Some(o) = self.world.get(id) {
            let e = TraceEntry::ObjectCreated {
                t: self.world.time_ms,
                id,
                object_kind: format!("{:?}", o.kind),
                tag: o.tag.clone(),
                template: o.template.clone(),
            };
            self.world.trace.push(e);
        }
    }

    /// Loads an area and its objects, places the hero (at the waypoint with tag `waypoint`, or
    /// the area's first waypoint) and queues the load events.
    pub fn enter_area(
        &mut self,
        resref: &str,
        waypoint: Option<&str>,
    ) -> Result<ObjectId, SimError> {
        let are = self.read_gff3(&format!("{}.are", resref.to_ascii_lowercase()))?;
        let root = are
            .structs
            .first()
            .ok_or_else(|| SimError::Parse(resref.into(), "empty area".into()))?;
        let area = self.world.spawn(
            ObjectKind::Area,
            &text(root, "Tag").unwrap_or_else(|| resref.to_string()),
        );
        {
            let a = self.world.get_mut(area).expect("just spawned");
            a.template = resref.to_string();
            a.script = resref_field(root, "ScriptResRefID");
            a.name_strref = strref(root, "Name");
            read_vars(&are, root, &mut a.locals);
        }
        self.created(area);
        self.world.area = Some(area);

        let lists = [
            ("CreatureList", ObjectKind::Creature, "utc"),
            ("PlaceableList", ObjectKind::Placeable, "utp"),
            ("TriggerList", ObjectKind::Trigger, "utt"),
            ("WaypointList", ObjectKind::Waypoint, ""),
            ("StoreList", ObjectKind::Store, "utm"),
            ("SoundList", ObjectKind::Sound, ""),
            ("StageList", ObjectKind::Stage, ""),
        ];
        let mut spawned = Vec::new();
        for (list, kind, ext) in lists {
            for &si in list_of(root, list) {
                let Some(inst) = are.structs.get(si as usize) else {
                    continue;
                };
                let id = self.spawn_instance(&are, inst, kind, ext, area);
                spawned.push(id);
            }
        }

        // Place the hero.
        if let Some(hero) = self.world.hero {
            let wp = self
                .world
                .objects
                .values()
                .filter(|o| o.kind == ObjectKind::Waypoint && o.area == Some(area))
                .find(|o| waypoint.is_none_or(|w| o.tag.eq_ignore_ascii_case(w)))
                .map(|o| (o.position, o.facing));
            let h = self.world.get_mut(hero).expect("hero exists");
            h.area = Some(area);
            if let Some((p, f)) = wp {
                h.position = p;
                h.facing = f;
            }
        }

        // Load events (order provisional, see the module docs).
        for id in &spawned {
            let kind = self.world.get(*id).map(|o| o.kind);
            if matches!(kind, Some(ObjectKind::Creature | ObjectKind::Placeable)) {
                let ev = self.new_event("SPAWN", Some(*id));
                self.world.queue_event(0, *id, ev, None);
            }
        }
        for name in ["AREALOAD_SPECIAL", "AREALOAD_PRELOADEXIT"] {
            let ev = self.new_event(name, Some(area));
            self.world.queue_event(0, area, ev, None);
        }
        if let Some(hero) = self.world.hero {
            let mut ev = self.new_event("ENTER", Some(hero));
            ev.objects = vec![hero];
            self.world.queue_event(0, area, ev, None);
        }
        let ev = self.new_event("AREALOAD_POSTLOADEXIT", Some(area));
        self.world.queue_event(0, area, ev, None);
        Ok(area)
    }

    fn spawn_instance(
        &mut self,
        are: &Gff3,
        inst: &Struct,
        kind: ObjectKind,
        ext: &str,
        area: ObjectId,
    ) -> ObjectId {
        let tag = text(inst, "Tag").unwrap_or_default();
        let template = resref_field(inst, "TemplateResRef").unwrap_or_default();
        let id = self.world.spawn(kind, &tag);
        let tmpl = if !ext.is_empty() && !template.is_empty() {
            match self.read_gff3(&format!("{}.{ext}", template.to_ascii_lowercase())) {
                Ok(g) => Some(g),
                Err(e) => {
                    self.world.trace.push(TraceEntry::Missing {
                        t: self.world.time_ms,
                        what: e.to_string(),
                    });
                    None
                }
            }
        } else {
            None
        };
        let q = [
            float(inst, "XOrientation"),
            float(inst, "YOrientation"),
            float(inst, "ZOrientation"),
            float(inst, "WOrientation"),
        ];
        let o = self.world.get_mut(id).expect("just spawned");
        o.template = template;
        o.area = Some(area);
        o.position = [
            float(inst, "XPosition"),
            float(inst, "YPosition"),
            float(inst, "ZPosition"),
        ];
        // Rotation about Z from the quaternion, in degrees (provisional convention).
        o.facing = (2.0 * q[2].atan2(q[3])).to_degrees().rem_euclid(360.0);
        if let Some(a) = int(inst, "Active") {
            o.active = a != 0;
        }
        if let Some(t) = &tmpl {
            if let Some(r) = t.structs.first() {
                o.script =
                    resref_field(r, "EventScript").or_else(|| resref_field(r, "ScriptResRefID"));
                o.dialog =
                    resref_field(r, "DialogResRef").or_else(|| resref_field(r, "Conversation"));
                o.name_strref = strref(r, "LocName");
                o.immortal = int(r, "Immortal").unwrap_or(0) != 0;
                o.plot = int(r, "Plot").unwrap_or(0) != 0;
                o.team = int(r, "TeamId").unwrap_or(-1);
                o.group = int(r, "GroupId").unwrap_or(0);
                o.rank = int(r, "Rank").unwrap_or(0);
                o.appearance = int(r, "Appearance").unwrap_or(0);
                read_vars(t, r, &mut o.locals);
                for fl in &r.fields {
                    if let Some(v) = int(r, &fl.label) {
                        o.fields.insert(fl.label.clone(), v);
                    }
                }
            }
        }
        // Instance values override the template.
        if let Some(t) = int(inst, "TeamId") {
            o.team = t;
        }
        if let Some(r) = int(inst, "Rank") {
            o.rank = r;
        }
        read_vars(are, inst, &mut o.locals);
        if kind == ObjectKind::Trigger {
            for &p in list_of(inst, "Geometry") {
                if let Some(s) = are.structs.get(p as usize) {
                    o.geometry
                        .push([float(s, "XPoint"), float(s, "YPoint"), float(s, "ZPoint")]);
                }
            }
        }
        self.created(id);
        id
    }
}

// ---------------------------------------------------------------------------------------------
// GFF helpers

fn field<'a>(s: &'a Struct, label: &str) -> Option<&'a G3> {
    s.fields.iter().find(|f| f.label == label).map(|f| &f.value)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .trim_end_matches('\0')
        .to_string()
}

pub(crate) fn text(s: &Struct, label: &str) -> Option<String> {
    match field(s, label)? {
        G3::ExoString(b) | G3::ResRef(b) => Some(lossy(b)),
        _ => None,
    }
}

pub(crate) fn resref_field(s: &Struct, label: &str) -> Option<String> {
    text(s, label).filter(|t| !t.is_empty())
}

fn strref(s: &Struct, label: &str) -> Option<u32> {
    match field(s, label)? {
        G3::LocString(l) => Some(l.strref),
        G3::StrRef(r) => Some(*r),
        _ => None,
    }
}

pub(crate) fn int(s: &Struct, label: &str) -> Option<i32> {
    Some(match field(s, label)? {
        G3::Byte(v) => *v as i32,
        G3::Char(v) => *v as i32,
        G3::Word(v) => *v as i32,
        G3::Short(v) => *v as i32,
        G3::Dword(v) => *v as i32,
        G3::Int(v) => *v,
        G3::Dword64(v) => *v as i32,
        G3::Int64(v) => *v as i32,
        _ => return None,
    })
}

fn float(s: &Struct, label: &str) -> f32 {
    match field(s, label) {
        Some(G3::Float(v)) => *v,
        Some(G3::Double(v)) => *v as f32,
        _ => 0.0,
    }
}

fn list_of<'a>(s: &'a Struct, label: &str) -> &'a [u32] {
    match field(s, label) {
        Some(G3::List(v)) => v,
        _ => &[],
    }
}

/// `VarTable` entries: type 1 int, 2 float, 3 string, 4 object, 12 resource (provisional).
fn read_vars(doc: &Gff3, s: &Struct, locals: &mut crate::world::Locals) {
    for &i in list_of(s, "VarTable") {
        let Some(v) = doc.structs.get(i as usize) else {
            continue;
        };
        let Some(name) = text(v, "Name") else {
            continue;
        };
        match int(v, "Type") {
            Some(1) => {
                locals.ints.insert(name, int(v, "Value").unwrap_or(0));
            }
            Some(2) => {
                locals.floats.insert(name, float(v, "Value"));
            }
            Some(3) => {
                locals
                    .strings
                    .insert(name, text(v, "Value").unwrap_or_default());
            }
            Some(4) => {
                locals
                    .objects
                    .insert(name, thedas_script::vm::OBJECT_INVALID);
            }
            Some(12) => {
                locals
                    .resources
                    .insert(name, text(v, "Value").unwrap_or_default());
            }
            _ => {}
        }
    }
}

fn cif_vars(g: &Gff4) -> Vec<(String, thedas_formats::gff4::Value)> {
    let mut out = Vec::new();
    let Some(thedas_formats::gff4::Value::List(Some(thedas_formats::List::Struct(vars)))) =
        g.field(g.root, 17000)
    else {
        return out;
    };
    for v in vars.iter().flatten() {
        let name = g
            .field(*v, 17001)
            .and_then(|x| x.as_str())
            .map(|s| s.trim_end_matches('\0').to_string());
        let value = match g.field(*v, 17003) {
            Some(thedas_formats::gff4::Value::Generic(slot)) => slot.value.clone(),
            other => other.cloned(),
        };
        if let (Some(n), Some(val)) = (name, value) {
            out.push((n, val));
        }
    }
    out
}

/// The campaign file: script (17104), start area (17101), start position (17102) and the area
/// tree (3290). Field meanings provisional, read from `singleplayer.cif`.
fn parse_cif(g: &Gff4) -> ModuleInfo {
    let s = |l: u32| {
        g.field(g.root, l)
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('\0').to_string())
            .filter(|s| !s.is_empty())
    };
    let mut info = ModuleInfo {
        script: s(17104),
        start_area: s(17101).or_else(|| s(17100)),
        ..Default::default()
    };
    if let Some(thedas_formats::gff4::Value::Vec3(p)) = g.field(g.root, 17102) {
        info.start_position = *p;
    }
    fn walk(g: &Gff4, id: StructId, out: &mut Vec<AreaEntry>, depth: usize) {
        if depth > 32 {
            return;
        }
        let names: Vec<String> = match g.field(id, 3291) {
            Some(thedas_formats::gff4::Value::List(Some(thedas_formats::List::String(v)))) => v
                .iter()
                .flatten()
                .map(|s| s.trim_end_matches('\0').to_string())
                .collect(),
            _ => Vec::new(),
        };
        let resref = g
            .field(id, 3294)
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('\0').to_string())
            .filter(|s| !s.is_empty());
        if let Some(r) = resref {
            out.push(AreaEntry {
                resref: r,
                layout: names.first().cloned(),
                area_list: names.get(1).cloned(),
            });
        }
        let children: Vec<StructId> = match g.field(id, 3292) {
            Some(thedas_formats::gff4::Value::List(Some(thedas_formats::List::Struct(v)))) => {
                v.iter().flatten().copied().collect()
            }
            Some(thedas_formats::gff4::Value::Struct(Some(c))) => vec![*c],
            _ => Vec::new(),
        };
        for c in children {
            walk(g, c, out, depth + 1);
        }
    }
    if let Some(thedas_formats::gff4::Value::Struct(Some(r))) = g.field(g.root, 3290) {
        walk(g, *r, &mut info.areas, 0);
    }
    info
}

// ---------------------------------------------------------------------------------------------
// The engine host

pub struct ScriptHost<'a> {
    pub sim: &'a mut Sim,
    /// The current event's handle.
    pub event: Option<u64>,
    pub script: String,
}

impl Host for ScriptHost<'_> {
    fn action(
        &mut self,
        caller: ObjectId,
        f: &Function,
        args: Vec<Value>,
    ) -> VmResult<Option<Value>> {
        match crate::engine::call(self, caller, f, &args) {
            Some(r) => {
                *self
                    .sim
                    .world
                    .trace
                    .implemented_calls
                    .entry(f.name.clone())
                    .or_default() += 1;
                r
            }
            None => {
                let t = self.sim.world.time_ms;
                self.sim.world.trace.push(TraceEntry::Unimplemented {
                    t,
                    function: f.name.clone(),
                    script: self.script.clone(),
                });
                Ok(None)
            }
        }
    }
}

impl ScriptHost<'_> {
    pub fn err(&self, f: &Function, msg: impl Into<String>) -> VmError {
        VmError::Host {
            name: f.name.clone(),
            message: msg.into(),
        }
    }

    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.sim.world.get(id)
    }

    pub fn location_of(&self, id: ObjectId) -> Option<Location> {
        let o = self.object(id)?;
        Some(Location {
            area: o.area.unwrap_or(thedas_script::vm::OBJECT_INVALID),
            position: o.position,
            facing: o.facing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LDF: &str = "#define ENGINE_STRUCTURE_0 event\n\
        int GetLocalInt(object o, string s) = 63;\n\
        void SetLocalInt(object o, string s, int n) = 64;\n";

    /// n = GetLocalInt(OBJECT_SELF, "n") + 1; SetLocalInt(OBJECT_SELF, "n", n).
    fn counter_script() -> Ncs {
        let mut b: Vec<u8> = Vec::new();
        let s = |b: &mut Vec<u8>| b.extend([0x04, 0x05, 0x00, 0x01, b'n']);
        let self_ = |b: &mut Vec<u8>| b.extend([0x04, 0x06, 0, 0, 0, 0]);
        s(&mut b);
        self_(&mut b);
        b.extend([0x05, 0x00, 0x00, 63, 0x02]);
        b.extend([0x04, 0x03, 0, 0, 0, 1, 0x14, 0x20]);
        s(&mut b);
        self_(&mut b);
        b.extend([0x05, 0x00, 0x00, 64, 0x03, 0x20, 0x00]);
        let mut v = b"NCS V1.0B".to_vec();
        v.extend(((13 + b.len()) as u32).to_be_bytes());
        v.extend(b);
        Ncs::parse(&v).unwrap()
    }

    fn sim() -> Sim {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 7);
        s.world.trace.keep = true;
        s.add_script("counter", counter_script());
        s
    }

    #[test]
    fn delayed_events_run_in_order_at_their_time() {
        let mut s = sim();
        let id = s.world.spawn(ObjectKind::Creature, "c");
        s.world.get_mut(id).unwrap().script = Some("counter".into());
        s.world.queue_event(
            1000,
            id,
            Event {
                ty: 1,
                ..Default::default()
            },
            None,
        );
        s.world.queue_event(
            500,
            id,
            Event {
                ty: 2,
                ..Default::default()
            },
            None,
        );
        let n = |s: &Sim| {
            s.world
                .get(id)
                .unwrap()
                .locals
                .ints
                .get("n")
                .copied()
                .unwrap_or(0)
        };
        s.tick(400);
        assert_eq!(n(&s), 0);
        s.tick(200);
        assert_eq!(n(&s), 1);
        assert_eq!(s.world.time_ms, 600);
        s.tick(1000);
        assert_eq!(n(&s), 2);
        let delivered: Vec<(u64, i32)> = s
            .world
            .trace
            .entries
            .iter()
            .filter_map(|e| match e {
                TraceEntry::EventDelivered { t, ty, .. } => Some((*t, *ty)),
                _ => None,
            })
            .collect();
        assert_eq!(delivered, vec![(500, 2), (1000, 1)]);
    }

    #[test]
    fn runs_are_deterministic() {
        let run = || {
            let mut s = sim();
            let id = s.world.spawn(ObjectKind::Creature, "c");
            s.world.get_mut(id).unwrap().script = Some("counter".into());
            for k in 0..20u64 {
                s.world
                    .queue_event((k * 37) % 300, id, Event::default(), None);
            }
            s.tick(1000);
            (s.world.trace.to_jsonl(), s.world.rng.next_u64())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn missing_scripts_and_objects_are_traced() {
        let mut s = sim();
        let id = s.world.spawn(ObjectKind::Creature, "c");
        s.world.get_mut(id).unwrap().script = Some("nope".into());
        s.world.queue_event(0, id, Event::default(), None);
        s.world.queue_event(0, 9999, Event::default(), None);
        s.tick(10);
        let missing = s
            .world
            .trace
            .entries
            .iter()
            .filter(|e| matches!(e, TraceEntry::Missing { .. }))
            .count();
        assert_eq!(missing, 2);
    }
}
