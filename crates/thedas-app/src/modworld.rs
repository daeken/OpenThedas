//! Mod mode's world-state browser: the module's variables (the game's globals), the
//! current area's and the inspected object's, the objects in the area, and the plot flags
//! the world has stored. Reading only, from a snapshot taken when the panel opens or is
//! refreshed; a plot flag set from here goes through the simulation's commands like the
//! conversation panel's (see [`crate::modconv::set_flag`]) and joins its history.
//!
//! Also the free camera's "fly to": a camera placed to look at an object.

use glam::Vec3;
use thedas_render::camera::Camera;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
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
    pub module: Vec<Var>,
    pub area: Option<(String, Vec<Var>)>,
    /// The inspected object's tag and variables.
    pub object: Option<(String, Vec<Var>)>,
    pub things: Vec<Thing>,
    /// The plots with stored flags.
    pub plots: Vec<Plot>,
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
}

impl WorldBrowser {
    /// The panel, when open.
    pub fn window(&mut self, ctx: &egui::Context, actions: &mut Vec<WorldAction>) {
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
            .show(ctx, |ui| self.contents(ui, actions));
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
        let Some(snap) = &self.snapshot else {
            ui.label("Reading the world…");
            return;
        };
        ui.weak(format!(
            "as of {:.1} s of game time",
            snap.time_ms as f64 / 1000.0
        ));
        ui.separator();
        let needle = self.filter.to_ascii_lowercase();
        let keep = |s: &str| needle.is_empty() || s.to_ascii_lowercase().contains(&needle);
        match self.tab {
            Tab::Globals => {
                ui.label("The module's variables");
                vars(ui, "globals", &snap.module, &keep);
            }
            Tab::Area => match &snap.area {
                None => {
                    ui.label("No area.");
                }
                Some((tag, v)) => {
                    ui.label(format!("{tag}'s variables"));
                    vars(ui, "area", v, &keep);
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
                Some((tag, v)) => {
                    ui.label(format!("{tag}'s variables"));
                    vars(ui, "object", v, &keep);
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

/// A list of variables, filtered.
fn vars(ui: &mut egui::Ui, id: &str, v: &[Var], keep: &dyn Fn(&str) -> bool) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(260.0)
        .show(ui, |ui| {
            egui::Grid::new(id).striped(true).show(ui, |ui| {
                let shown = v.iter().filter(|x| keep(&x.name) || keep(&x.value));
                let mut n = 0;
                for x in shown.take(ROWS_MAX) {
                    ui.monospace(&x.name);
                    ui.weak(x.kind);
                    ui.monospace(&x.value);
                    ui.end_row();
                    n += 1;
                }
                if n == 0 {
                    ui.weak("none");
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
            if let Some(Ok(p)) = &browser.looked {
                let guid = p.guid.clone();
                browser.looked = Some(lookup(sim, &guid));
            }
        }
    }
    None
}

/// The world now, with `selected`'s variables.
pub fn snapshot(sim: &mut Sim, selected: Option<ObjectId>) -> Snapshot {
    let w = &sim.world;
    let module = w
        .module
        .and_then(|m| w.get(m))
        .map(|m| locals(&m.locals, w))
        .unwrap_or_default();
    let area = w
        .area
        .and_then(|a| w.get(a))
        .map(|a| (a.tag.clone(), locals(&a.locals, w)));
    let object = selected
        .and_then(|s| w.get(s))
        .map(|o| (o.tag.clone(), locals(&o.locals, w)));
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
    v.extend(l.ints.iter().map(|(k, x)| var(k, "int", x.to_string())));
    v.extend(
        l.floats
            .iter()
            .map(|(k, x)| var(k, "float", format!("{x}"))),
    );
    v.extend(
        l.strings
            .iter()
            .map(|(k, x)| var(k, "string", format!("{x:?}"))),
    );
    v.extend(l.objects.iter().map(|(k, &x)| var(k, "object", object(x))));
    v.extend(
        l.resources
            .iter()
            .map(|(k, x)| var(k, "resource", x.clone())),
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

fn var(name: &str, kind: &'static str, value: String) -> Var {
    Var {
        name: name.to_string(),
        kind,
        value,
    }
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
        assert_eq!(v[0], var("COUNT", "int", "3".into()));
        assert_eq!(v[1], var("NAME", "string", "\"x\"".into()));
        assert_eq!(v[2], var("WHO", "object", "(999999, gone)".into()));
    }
}
