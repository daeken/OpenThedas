//! Mod mode's world-state browser: the module's variables (the game's globals), the
//! current area's and the inspected object's, the objects in the area, and the plot flags
//! the world has stored, read from a snapshot taken when the panel opens or is refreshed.
//! Variables and plot flags changed from here go through the simulation's commands
//! ([`crate::modconv::set_local`], [`crate::modconv::set_flag`]) and join the conversation
//! panel's history, whose undo sets back what they replaced.
//!
//! Also the free camera's "fly to": a camera placed to look at an object.

use glam::Vec3;
use thedas_render::camera::Camera;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::commands::LocalValue;
use thedas_sim::plot::DEFINED_FLAGS;
use thedas_sim::world::{Locals, ObjectKind, World};

/// The most rows a list shows at once (the filter narrows it).
const ROWS_MAX: usize = 500;

/// One variable: its name, its type and its value as text.
#[derive(Clone, Debug, PartialEq)]
pub struct Var {
    pub name: String,
    pub kind: &'static str,
    pub value: String,
    /// The value as it is typed to change it; `None` for a type the panel does not change
    /// (a location).
    pub raw: Option<String>,
}

/// An object's variables.
#[derive(Clone, Debug, PartialEq)]
pub struct VarSet {
    pub id: ObjectId,
    pub tag: String,
    pub vars: Vec<Var>,
}

/// The variable types the panel sets.
const KINDS: [&str; 5] = ["int", "float", "string", "object", "resource"];

/// A value typed for a variable of type `kind` (one of [`KINDS`]); an object is its id.
pub fn parse_value(kind: &str, text: &str) -> Result<LocalValue, String> {
    let t = text.trim();
    Ok(match kind {
        "int" => LocalValue::Int(
            t.parse()
                .map_err(|_| format!("{t:?} is not a whole number"))?,
        ),
        "float" => LocalValue::Float(t.parse().map_err(|_| format!("{t:?} is not a number"))?),
        "string" => LocalValue::String(text.to_string()),
        "object" => LocalValue::Object(
            t.parse()
                .map_err(|_| format!("{t:?} is not an object id"))?,
        ),
        "resource" => LocalValue::Resource(t.to_string()),
        _ => return Err(format!("variables of type {kind} are not set here")),
    })
}

/// A variable being changed in place.
#[derive(Clone, Debug, Default, PartialEq)]
struct Editing {
    target: ObjectId,
    name: String,
    kind: &'static str,
    text: String,
    error: Option<String>,
}

/// The new-variable row.
#[derive(Clone, Debug, PartialEq)]
struct Adding {
    name: String,
    kind: &'static str,
    text: String,
    error: Option<String>,
}

impl Default for Adding {
    fn default() -> Self {
        Adding {
            name: String::new(),
            kind: "int",
            text: String::new(),
            error: None,
        }
    }
}

/// An object of the area, for the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Thing {
    pub id: ObjectId,
    pub kind: String,
    pub tag: String,
    pub active: bool,
    pub dead: bool,
}

/// One flag of a plot: its number, its name in the plot file and its value (`None`: not
/// stored, or a defined flag, whose plot script answers for it).
#[derive(Clone, Debug, PartialEq)]
pub struct Flag {
    pub flag: i32,
    pub name: Option<String>,
    pub value: Option<bool>,
    /// The value it starts with in the plot file (flags 0 to 127), which it reads as
    /// while not stored.
    pub initial: bool,
}

/// A plot and its flags.
#[derive(Clone, Debug, PartialEq)]
pub struct Plot {
    pub guid: String,
    pub resref: Option<String>,
    pub flags: Vec<Flag>,
}

/// The world as the browser shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub time_ms: u64,
    pub module: Option<VarSet>,
    pub area: Option<VarSet>,
    /// The inspected object's variables.
    pub object: Option<VarSet>,
    /// The inspected creature's properties and the effects on it.
    pub stats: Option<Stats>,
    pub things: Vec<Thing>,
    /// The plots with stored flags.
    pub plots: Vec<Plot>,
}

/// One of a creature's properties.
#[derive(Clone, Debug, PartialEq)]
pub struct Stat {
    pub id: i32,
    /// Its name from the scripts' `PROPERTY_*` constants.
    pub name: String,
    pub base: f32,
    /// A depletable's current value (health, mana or stamina).
    pub current: Option<f32>,
    /// What effects and worn items add.
    pub modifier: f32,
    pub total: f32,
}

/// One effect on a creature.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectLine {
    /// Its type's name from the scripts' `EFFECT_TYPE_*` constants.
    pub kind: String,
    pub ability: i32,
    pub creator: Option<String>,
    /// Seconds left, for one that runs out.
    pub left_s: Option<f32>,
    pub hostile: bool,
    pub numbers: String,
}

/// A creature's properties and effects.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub id: ObjectId,
    pub stats: Vec<Stat>,
    pub effects: Vec<EffectLine>,
}

/// What the browser asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum WorldAction {
    /// Take the snapshot again.
    Refresh,
    /// Every flag of the plot with this name or GUID.
    Lookup(String),
    /// Pick this object for the inspector.
    Select(ObjectId),
    /// Set or clear a plot flag.
    SetFlag {
        plot: String,
        flag: i32,
        value: bool,
    },
    /// Set one of an object's variables, or remove it (`None`).
    SetLocal {
        target: ObjectId,
        name: String,
        value: Option<LocalValue>,
    },
    /// Undo history entry n (see [`crate::modconv::Convs::history`]).
    Undo(usize),
    /// Set a creature's property: its base, its current value, or both.
    SetStat {
        target: ObjectId,
        property: i32,
        base: Option<f32>,
        current: Option<f32>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tab {
    #[default]
    Globals,
    Area,
    Object,
    Plots,
}

/// The browser's state.
#[derive(Default)]
pub struct WorldBrowser {
    pub open: bool,
    tab: Tab,
    filter: String,
    lookup: String,
    pub snapshot: Option<Snapshot>,
    /// The plot looked up, every flag of it.
    pub looked: Option<Result<Plot, String>>,
    /// Plot flags set from here run their plot's script.
    call_script: bool,
    editing: Option<Editing>,
    adding: Adding,
    /// A property being typed: (property, base, current).
    stat_edit: Option<(i32, f32, Option<f32>)>,
}

impl WorldBrowser {
    /// The panel, when open.
    pub fn window(
        &mut self,
        ctx: &egui::Context,
        history: &[crate::modconv::Edit],
        actions: &mut Vec<WorldAction>,
    ) {
        if !self.open {
            return;
        }
        if self.snapshot.is_none() && !actions.contains(&WorldAction::Refresh) {
            actions.push(WorldAction::Refresh);
        }
        let mut open = self.open;
        egui::Window::new("World state")
            .open(&mut open)
            .default_pos([420.0, 60.0])
            .default_width(520.0)
            .default_height(560.0)
            .show(ctx, |ui| {
                self.contents(ui, actions);
                history_list(ui, history, actions);
            });
        self.open = open;
    }

    fn contents(&mut self, ui: &mut egui::Ui, actions: &mut Vec<WorldAction>) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.tab, Tab::Globals, "Globals");
            ui.selectable_value(&mut self.tab, Tab::Area, "Area");
            ui.selectable_value(&mut self.tab, Tab::Object, "Inspected");
            ui.selectable_value(&mut self.tab, Tab::Plots, "Plots");
            if ui.button("Refresh").clicked() {
                actions.push(WorldAction::Refresh);
            }
        });
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.text_edit_singleline(&mut self.filter);
        });
        let Some(snap) = self.snapshot.as_ref() else {
            ui.label("Reading the world…");
            return;
        };
        let (editing, adding) = (&mut self.editing, &mut self.adding);
        ui.weak(format!(
            "as of {:.1} s of game time",
            snap.time_ms as f64 / 1000.0
        ));
        ui.separator();
        let needle = self.filter.to_ascii_lowercase();
        let keep = |s: &str| needle.is_empty() || s.to_ascii_lowercase().contains(&needle);
        match self.tab {
            Tab::Globals => match &snap.module {
                None => {
                    ui.label("No module.");
                }
                Some(set) => {
                    ui.label("The module's variables");
                    vars(ui, "globals", set, &keep, editing, adding, actions);
                }
            },
            Tab::Area => match &snap.area {
                None => {
                    ui.label("No area.");
                }
                Some(set) => {
                    ui.label(format!("{}'s variables", set.tag));
                    vars(ui, "area", set, &keep, editing, adding, actions);
                    ui.separator();
                    ui.label("Objects here (click one to inspect it)");
                    egui::ScrollArea::vertical()
                        .id_salt("things")
                        .max_height(220.0)
                        .show(ui, |ui| {
                            let shown =
                                snap.things.iter().filter(|t| keep(&t.tag) || keep(&t.kind));
                            for t in shown.take(ROWS_MAX) {
                                let state = match (t.active, t.dead) {
                                    (_, true) => " dead",
                                    (false, _) => " inactive",
                                    _ => "",
                                };
                                let text = format!("{} ({}, {}){state}", t.tag, t.id, t.kind);
                                if ui.selectable_label(false, text).clicked() {
                                    actions.push(WorldAction::Select(t.id));
                                }
                            }
                        });
                }
            },
            Tab::Object => match &snap.object {
                None => {
                    ui.label("Nothing picked: Ctrl+click an object, or pick one under Area.");
                }
                Some(set) => {
                    ui.label(format!("{}'s variables", set.tag));
                    vars(ui, "object", set, &keep, editing, adding, actions);
                    if let Some(st) = &snap.stats {
                        ui.separator();
                        stats(ui, st, &keep, &mut self.stat_edit, actions);
                    }
                }
            },
            Tab::Plots => {
                let mut flips = Vec::new();
                ui.horizontal(|ui| {
                    ui.label("Look up a plot");
                    let r = ui.text_edit_singleline(&mut self.lookup);
                    let go = ui.button("Show").clicked()
                        || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    if go && !self.lookup.trim().is_empty() {
                        actions.push(WorldAction::Lookup(self.lookup.trim().to_string()));
                    }
                });
                ui.checkbox(&mut self.call_script, "flags run their plot script");
                match &self.looked {
                    Some(Ok(p)) => {
                        plot_rows(ui, p, &|_: &str| true, &mut flips);
                        ui.separator();
                    }
                    Some(Err(e)) => {
                        ui.label(e);
                        ui.separator();
                    }
                    None => {}
                }
                ui.label("Stored flags");
                egui::ScrollArea::vertical()
                    .id_salt("plots")
                    .max_height(320.0)
                    .show(ui, |ui| {
                        let mut rows = 0;
                        for p in &snap.plots {
                            let name = p.resref.as_deref().unwrap_or(&p.guid);
                            let whole = keep(name);
                            let flag_keep = |n: &str| whole || keep(n);
                            if !whole
                                && !p
                                    .flags
                                    .iter()
                                    .any(|f| f.name.as_deref().is_some_and(flag_keep))
                            {
                                continue;
                            }
                            rows += plot_rows(ui, p, &flag_keep, &mut flips);
                            if rows > ROWS_MAX {
                                ui.weak("… filter to see more");
                                break;
                            }
                        }
                    });
                actions.extend(
                    flips
                        .into_iter()
                        .map(|(plot, flag, value)| WorldAction::SetFlag { plot, flag, value }),
                );
            }
        }
    }

    /// Whether flags set from here run their plot script.
    pub fn call_script(&self) -> bool {
        self.call_script
    }
}

/// An object's variables, filtered, each with edit and remove, and a row to add one.
fn vars(
    ui: &mut egui::Ui,
    id: &str,
    set: &VarSet,
    keep: &dyn Fn(&str) -> bool,
    editing: &mut Option<Editing>,
    adding: &mut Adding,
    actions: &mut Vec<WorldAction>,
) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new(id).striped(true).show(ui, |ui| {
                let shown = set.vars.iter().filter(|x| keep(&x.name) || keep(&x.value));
                let mut n = 0;
                for x in shown.take(ROWS_MAX) {
                    ui.monospace(&x.name);
                    ui.weak(x.kind);
                    let here = editing.as_ref().is_some_and(|e| {
                        e.target == set.id && e.name == x.name && e.kind == x.kind
                    });
                    if here && let Some(e) = editing.as_mut() {
                        if edit_row(ui, e, actions) {
                            *editing = None;
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.monospace(&x.value);
                            let Some(raw) = &x.raw else {
                                return;
                            };
                            if ui.small_button("edit").clicked() {
                                *editing = Some(Editing {
                                    target: set.id,
                                    name: x.name.clone(),
                                    kind: x.kind,
                                    text: raw.clone(),
                                    error: None,
                                });
                            }
                            if ui.small_button("remove").clicked() {
                                actions.push(WorldAction::SetLocal {
                                    target: set.id,
                                    name: x.name.clone(),
                                    value: None,
                                });
                            }
                        });
                    }
                    ui.end_row();
                    n += 1;
                }
                if n == 0 {
                    ui.weak("none");
                    ui.end_row();
                }
            });
        });
    add_row(ui, id, set.id, adding, actions);
}

/// A variable's value being typed: Apply (or Enter) sets it, Cancel leaves it. True when
/// the edit is over.
fn edit_row(ui: &mut egui::Ui, e: &mut Editing, actions: &mut Vec<WorldAction>) -> bool {
    let mut done = false;
    ui.horizontal(|ui| {
        let r = ui.text_edit_singleline(&mut e.text);
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.small_button("Apply").clicked() || enter {
            match parse_value(e.kind, &e.text) {
                Ok(v) => {
                    actions.push(WorldAction::SetLocal {
                        target: e.target,
                        name: e.name.clone(),
                        value: Some(v),
                    });
                    done = true;
                }
                Err(err) => e.error = Some(err),
            }
        }
        if ui.small_button("Cancel").clicked() {
            done = true;
        }
        if let Some(err) = &e.error {
            ui.colored_label(ui.visuals().error_fg_color, err);
        }
    });
    done
}

/// The row that adds a variable to object `target`.
fn add_row(
    ui: &mut egui::Ui,
    id: &str,
    target: ObjectId,
    adding: &mut Adding,
    actions: &mut Vec<WorldAction>,
) {
    ui.horizontal(|ui| {
        ui.label("new");
        ui.add(
            egui::TextEdit::singleline(&mut adding.name)
                .hint_text("name")
                .desired_width(140.0),
        );
        egui::ComboBox::from_id_salt(("kind", id))
            .selected_text(adding.kind)
            .show_ui(ui, |ui| {
                for k in KINDS {
                    ui.selectable_value(&mut adding.kind, k, k);
                }
            });
        ui.add(
            egui::TextEdit::singleline(&mut adding.text)
                .hint_text("value")
                .desired_width(140.0),
        );
        if ui.button("Add").clicked() {
            let name = adding.name.trim().to_string();
            match (name.is_empty(), parse_value(adding.kind, &adding.text)) {
                (true, _) => adding.error = Some("give it a name".into()),
                (false, Err(e)) => adding.error = Some(e),
                (false, Ok(v)) => {
                    actions.push(WorldAction::SetLocal {
                        target,
                        name,
                        value: Some(v),
                    });
                    *adding = Adding {
                        kind: adding.kind,
                        ..Adding::default()
                    };
                }
            }
        }
    });
    if let Some(e) = &adding.error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
}

/// Mod mode's changes this session, each with Undo while it can be undone.
fn history_list(
    ui: &mut egui::Ui,
    history: &[crate::modconv::Edit],
    actions: &mut Vec<WorldAction>,
) {
    if history.is_empty() {
        return;
    }
    ui.separator();
    egui::CollapsingHeader::new(format!("History ({})", history.len()))
        .default_open(true)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("world history")
                .max_height(120.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for (n, e) in history.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if !e.undo.is_empty() && ui.small_button("Undo").clicked() {
                                actions.push(WorldAction::Undo(n));
                            }
                            ui.label(&e.text);
                        });
                    }
                });
        });
}

/// A plot's flags, each stored one with a button that flips it. Returns the rows shown.
fn plot_rows(
    ui: &mut egui::Ui,
    p: &Plot,
    keep: &dyn Fn(&str) -> bool,
    flips: &mut Vec<(String, i32, bool)>,
) -> usize {
    ui.strong(p.resref.as_deref().unwrap_or(&p.guid));
    let mut n = 1;
    for f in &p.flags {
        let name = f.name.clone().unwrap_or_else(|| format!("flag {}", f.flag));
        if !keep(&name) {
            continue;
        }
        n += 1;
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let value = match (f.flag >= DEFINED_FLAGS, f.value) {
                (true, _) => "defined".to_string(),
                (false, Some(v)) => v.to_string(),
                (false, None) => format!("{} (as the plot starts)", f.initial),
            };
            ui.monospace(format!("{:>4} {name} = {value}", f.flag));
            if f.flag < DEFINED_FLAGS {
                let now = f.value.unwrap_or(f.initial);
                if ui.small_button(if now { "clear" } else { "set" }).clicked() {
                    flips.push((p.guid.clone(), f.flag, !now));
                }
            }
        });
    }
    n
}

/// Carries out what the browser asked for. `selected` is the inspected object; a
/// `Select` returns the object to inspect.
pub fn carry_out(
    sim: &mut Sim,
    browser: &mut WorldBrowser,
    convs: &mut crate::modconv::Convs,
    a: WorldAction,
    selected: Option<ObjectId>,
) -> Option<ObjectId> {
    match a {
        WorldAction::Refresh => browser.snapshot = Some(snapshot(sim, selected)),
        WorldAction::Lookup(name) => {
            browser.looked = Some(lookup(sim, &name));
            browser.lookup = name;
            browser.tab = Tab::Plots;
        }
        WorldAction::Select(id) => {
            browser.snapshot = Some(snapshot(sim, Some(id)));
            return Some(id);
        }
        WorldAction::SetFlag { plot, flag, value } => {
            let edit = crate::modconv::set_flag(sim, &plot, flag, value, browser.call_script());
            convs.status = edit.text.clone();
            convs.history.push(edit);
            browser.snapshot = Some(snapshot(sim, selected));
            relook(sim, browser);
        }
        WorldAction::SetLocal {
            target,
            name,
            value,
        } => {
            let edit = crate::modconv::set_local(sim, target, &name, value);
            convs.status = edit.text.clone();
            convs.history.push(edit);
            browser.snapshot = Some(snapshot(sim, selected));
        }
        WorldAction::SetStat {
            target,
            property,
            base,
            current,
        } => {
            let edit = crate::modconv::set_property(sim, target, property, base, current);
            convs.status = edit.text.clone();
            convs.history.push(edit);
            browser.snapshot = Some(snapshot(sim, selected));
        }
        WorldAction::Undo(n) => {
            crate::modconv::carry_out(sim, convs, crate::modconv::ConvAction::Undo(n), selected);
            browser.snapshot = Some(snapshot(sim, selected));
            relook(sim, browser);
        }
    }
    None
}

/// The plot looked up, read again.
fn relook(sim: &mut Sim, browser: &mut WorldBrowser) {
    if let Some(Ok(p)) = &browser.looked {
        let guid = p.guid.clone();
        browser.looked = Some(lookup(sim, &guid));
    }
}

/// The world now, with `selected`'s variables.
pub fn snapshot(sim: &mut Sim, selected: Option<ObjectId>) -> Snapshot {
    let w = &sim.world;
    let set = |id: Option<ObjectId>| {
        id.and_then(|i| w.get(i)).map(|o| VarSet {
            id: o.id,
            tag: o.tag.clone(),
            vars: locals(&o.locals, w),
        })
    };
    let (module, area, object) = (set(w.module), set(w.area), set(selected));
    let stats = selected.and_then(|s| creature_stats(sim, s));
    let mut things: Vec<Thing> = w
        .objects
        .values()
        .filter(|o| o.area.is_some() && o.area == w.area)
        .filter(|o| !matches!(o.kind, ObjectKind::Area | ObjectKind::Module))
        .map(|o| Thing {
            id: o.id,
            kind: format!("{:?}", o.kind),
            tag: o.tag.clone(),
            active: o.active,
            dead: o.dead,
        })
        .collect();
    things.sort_by(|a, b| a.tag.cmp(&b.tag).then(a.id.cmp(&b.id)));
    let time_ms = w.time_ms;
    let mut stored: Vec<(String, Vec<(i32, bool)>)> = Vec::new();
    for ((guid, flag), value) in &w.plots {
        match stored.last_mut() {
            Some((g, flags)) if g == guid => flags.push((*flag, *value)),
            _ => stored.push((guid.clone(), vec![(*flag, *value)])),
        }
    }
    let mut plots: Vec<Plot> = stored
        .into_iter()
        .map(|(guid, flags)| {
            let info = sim.plot_info(&guid);
            Plot {
                resref: info.as_ref().map(|i| i.resref.clone()),
                flags: flags
                    .into_iter()
                    .map(|(flag, value)| Flag {
                        flag,
                        name: info.as_ref().and_then(|i| i.flags.get(&flag).cloned()),
                        value: Some(value),
                        initial: info.as_ref().is_some_and(|i| initial(&i.initial, flag)),
                    })
                    .collect(),
                guid,
            }
        })
        .collect();
    plots.sort_by(|a, b| {
        let key = |p: &Plot| p.resref.clone().unwrap_or_else(|| p.guid.clone());
        key(a).cmp(&key(b))
    });
    Snapshot {
        time_ms,
        module,
        area,
        object,
        stats,
        things,
        plots,
    }
}

/// Every flag the plot file names, with what the world has stored.
fn lookup(sim: &mut Sim, name: &str) -> Result<Plot, String> {
    let info = sim
        .plot_info(name)
        .ok_or_else(|| format!("No plot named {name}."))?;
    let flags = info
        .flags
        .iter()
        .map(|(&flag, n)| Flag {
            flag,
            name: Some(n.clone()),
            value: sim.world.plots.get(&(info.guid.clone(), flag)).copied(),
            initial: initial(&info.initial, flag),
        })
        .collect();
    Ok(Plot {
        guid: info.guid.clone(),
        resref: Some(info.resref.clone()),
        flags,
    })
}

/// Whether flag `flag` starts set in a plot file's starting values (bit `flag & 31` of
/// word `flag >> 5`, flags 0 to 127; see [`thedas_sim::plot::PlotInfo::initial`]).
fn initial(words: &[u32; 4], flag: i32) -> bool {
    (0..128).contains(&flag) && words[(flag >> 5) as usize] & (1 << (flag & 31)) != 0
}

/// An object's variables as text, sorted by name within each type.
pub fn locals(l: &Locals, w: &World) -> Vec<Var> {
    let object = |id: ObjectId| match w.get(id) {
        Some(o) => format!("{} ({id})", o.tag),
        None => format!("({id}, gone)"),
    };
    let mut v = Vec::new();
    let typed = |k: &String, kind, value: String, raw: String| Var {
        name: k.clone(),
        kind,
        value,
        raw: Some(raw),
    };
    v.extend(
        l.ints
            .iter()
            .map(|(k, x)| typed(k, "int", x.to_string(), x.to_string())),
    );
    v.extend(
        l.floats
            .iter()
            .map(|(k, x)| typed(k, "float", format!("{x}"), format!("{x}"))),
    );
    v.extend(
        l.strings
            .iter()
            .map(|(k, x)| typed(k, "string", format!("{x:?}"), x.clone())),
    );
    v.extend(
        l.objects
            .iter()
            .map(|(k, &x)| typed(k, "object", object(x), x.to_string())),
    );
    v.extend(
        l.resources
            .iter()
            .map(|(k, x)| typed(k, "resource", x.clone(), x.clone())),
    );
    v.extend(l.locations.iter().map(|(k, x)| {
        let p = x.position;
        let at = format!(
            "{} at {:.2}, {:.2}, {:.2}",
            object(x.area),
            p[0],
            p[1],
            p[2]
        );
        var(k, "location", at)
    }));
    v
}

/// A variable the panel shows but does not change.
fn var(name: &str, kind: &'static str, value: String) -> Var {
    Var {
        name: name.to_string(),
        kind,
        value,
        raw: None,
    }
}

/// The names the scripts give numbers, by prefix: `PROPERTY_ATTRIBUTE_STRENGTH` ->
/// `STRENGTH` (first name wins).
fn constant_names(sim: &Sim, prefixes: &[&str]) -> std::collections::BTreeMap<i32, String> {
    use thedas_script::ldf::ConstValue;
    let mut out = std::collections::BTreeMap::new();
    for c in &sim.table.constants {
        let Some(p) = prefixes.iter().find(|p| c.name.starts_with(**p)) else {
            continue;
        };
        if let ConstValue::Int(v) = c.value {
            out.entry(v as i32)
                .or_insert_with(|| c.name[p.len()..].to_string());
        }
    }
    out
}

/// A creature's properties (each with its name, base, current, modifier and total) and the
/// effects on it.
pub fn creature_stats(sim: &Sim, id: ObjectId) -> Option<Stats> {
    let o = sim.world.get(id)?;
    if o.kind != ObjectKind::Creature {
        return None;
    }
    let names = constant_names(
        sim,
        &[
            "PROPERTY_ATTRIBUTE_",
            "PROPERTY_SIMPLE_",
            "PROPERTY_DEPLETABLE_",
            "PROPERTY_DERIVED_",
        ],
    );
    let kinds = constant_names(sim, &["EFFECT_TYPE_"]);
    let depletable = |p: i32| {
        thedas_sim::engine::property_kind(sim, p) == thedas_sim::engine::PropertyKind::Depletable
    };
    let mut ids: Vec<i32> = o.properties.keys().copied().collect();
    ids.extend(names.keys().filter(|p| !o.properties.contains_key(p)));
    ids.sort();
    ids.dedup();
    let stats = ids
        .into_iter()
        .filter(|p| o.properties.contains_key(p) || names.contains_key(p))
        .map(|p| {
            let (base, current) = o.properties.get(&p).copied().unwrap_or((0.0, 0.0));
            Stat {
                id: p,
                name: names
                    .get(&p)
                    .cloned()
                    .unwrap_or_else(|| format!("property {p}")),
                base,
                current: depletable(p).then_some(current),
                modifier: o.modifiers.get(&p).copied().unwrap_or(0.0),
                total: sim.property_total(id, p),
            }
        })
        .collect();
    let now = sim.world.time_ms;
    let effects = o
        .effects
        .iter()
        .map(|e| EffectLine {
            kind: kinds
                .get(&e.ty)
                .cloned()
                .unwrap_or_else(|| format!("type {}", e.ty)),
            ability: e.ability,
            creator: e
                .creator
                .and_then(|c| sim.world.get(c))
                .map(|c| c.tag.clone()),
            left_s: e.expires.map(|t| t.saturating_sub(now) as f32 / 1000.0),
            hostile: e.hostile,
            numbers: e
                .ints
                .iter()
                .map(|i| i.to_string())
                .chain(e.floats.iter().map(|f| format!("{f}")))
                .collect::<Vec<_>>()
                .join(", "),
        })
        .collect();
    Some(Stats { id, stats, effects })
}

/// The inspected creature's properties (filtered; each editable) and its effects.
fn stats(
    ui: &mut egui::Ui,
    st: &Stats,
    keep: &dyn Fn(&str) -> bool,
    edit: &mut Option<(i32, f32, Option<f32>)>,
    actions: &mut Vec<WorldAction>,
) {
    ui.label("Properties");
    egui::ScrollArea::vertical()
        .id_salt("stats")
        .max_height(220.0)
        .show(ui, |ui| {
            egui::Grid::new("stats grid").striped(true).show(ui, |ui| {
                ui.weak("property");
                ui.weak("base");
                ui.weak("current");
                ui.weak("+ effects");
                ui.weak("total");
                ui.end_row();
                for s in st.stats.iter().filter(|s| keep(&s.name)) {
                    ui.monospace(&s.name);
                    let here = edit.is_some_and(|(p, ..)| p == s.id);
                    if here && let Some((_, b, c)) = edit.as_mut() {
                        ui.add(egui::DragValue::new(b).speed(0.5));
                        match c {
                            Some(c) => {
                                ui.add(egui::DragValue::new(c).speed(0.5));
                            }
                            None => {
                                ui.weak("—");
                            }
                        }
                        let (b, c) = (*b, *c);
                        ui.horizontal(|ui| {
                            if ui.small_button("Apply").clicked() {
                                actions.push(WorldAction::SetStat {
                                    target: st.id,
                                    property: s.id,
                                    base: (b != s.base).then_some(b),
                                    current: c.filter(|c| Some(*c) != s.current),
                                });
                                *edit = None;
                            }
                            if ui.small_button("Cancel").clicked() {
                                *edit = None;
                            }
                        });
                    } else {
                        ui.monospace(format!("{}", s.base));
                        ui.monospace(s.current.map_or("—".into(), |c| format!("{c}")));
                        ui.horizontal(|ui| {
                            ui.monospace(format!("{:+}", s.modifier));
                            if ui.small_button("edit").clicked() {
                                *edit = Some((s.id, s.base, s.current));
                            }
                        });
                    }
                    ui.monospace(format!("{}", s.total));
                    ui.end_row();
                }
            });
        });
    ui.label(format!("Effects ({})", st.effects.len()));
    egui::ScrollArea::vertical()
        .id_salt("effects")
        .max_height(140.0)
        .show(ui, |ui| {
            for e in &st.effects {
                let left = e
                    .left_s
                    .map_or("lasting".into(), |s| format!("{s:.1} s left"));
                let by = e
                    .creator
                    .as_deref()
                    .map_or(String::new(), |c| format!(", from {c}"));
                let hostile = if e.hostile { ", hostile" } else { "" };
                ui.monospace(format!(
                    "{} (ability {}{by}{hostile}): {left}; {}",
                    e.kind, e.ability, e.numbers
                ));
            }
            if st.effects.is_empty() {
                ui.weak("none");
            }
        });
}

/// The free camera moved to look at `target` (an object's feet) from 5 m off, keeping
/// the direction it looks in and looking down a little.
pub fn fly_to(cam: &Camera, target: Vec3) -> Camera {
    let pitch = -0.35f32;
    let dir = Vec3::new(
        pitch.cos() * cam.yaw.cos(),
        pitch.cos() * cam.yaw.sin(),
        pitch.sin(),
    );
    let at = target + Vec3::Z * 1.2;
    Camera {
        position: at - dir * 5.0,
        pitch,
        ..*cam
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flying_to_keeps_the_heading_and_looks_at_the_object() {
        let cam = Camera::new(
            Vec3::new(100.0, 0.0, 50.0),
            std::f32::consts::FRAC_PI_2,
            0.4,
        );
        let c = fly_to(&cam, Vec3::new(10.0, 20.0, 0.0));
        assert_eq!(c.yaw, cam.yaw);
        assert!(c.pitch < 0.0);
        // Behind the object along -Y (the camera looks along +Y), above it.
        assert!((c.position.x - 10.0).abs() < 1e-4);
        assert!(c.position.y < 20.0 && c.position.z > 1.2);
        assert!((c.position - Vec3::new(10.0, 20.0, 1.2)).length() - 5.0 < 1e-4);
    }

    #[test]
    fn starting_values_are_a_bit_per_flag() {
        // lot100pt_bandits2's field 13011 = 32: flag 5.
        let w = [32, 0, 0, 1 << 31];
        assert!(initial(&w, 5));
        assert!(!initial(&w, 4));
        assert!(initial(&w, 127));
        assert!(!initial(&w, 128));
        assert!(!initial(&w, -1));
    }

    #[test]
    fn variables_read_with_their_types() {
        let mut l = Locals::default();
        l.ints.insert("COUNT".into(), 3);
        l.strings.insert("NAME".into(), "x".into());
        l.objects.insert("WHO".into(), 999_999);
        let w = World::new(std::sync::Arc::new(thedas_sim::m2da::Tables::empty()), 0);
        let v = locals(&l, &w);
        let raw = |name: &str, kind, value: &str, raw: &str| Var {
            raw: Some(raw.into()),
            ..var(name, kind, value.into())
        };
        assert_eq!(v[0], raw("COUNT", "int", "3", "3"));
        assert_eq!(v[1], raw("NAME", "string", "\"x\"", "x"));
        assert_eq!(v[2], raw("WHO", "object", "(999999, gone)", "999999"));
    }

    #[test]
    fn typed_values_parse_by_type() {
        assert_eq!(parse_value("int", " 42 "), Ok(LocalValue::Int(42)));
        assert!(parse_value("int", "4.2").is_err());
        assert_eq!(parse_value("float", "4.25"), Ok(LocalValue::Float(4.25)));
        assert_eq!(
            parse_value("string", " a b"),
            Ok(LocalValue::String(" a b".into()))
        );
        assert_eq!(parse_value("object", "124"), Ok(LocalValue::Object(124)));
        assert!(parse_value("object", "player").is_err());
        assert_eq!(
            parse_value("resource", "x.dlg"),
            Ok(LocalValue::Resource("x.dlg".into()))
        );
        assert!(parse_value("location", "1").is_err());
    }
}
