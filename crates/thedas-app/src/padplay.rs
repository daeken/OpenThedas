//! Carrying out the pad's [`Order`]s in the running game, through the paths the mouse and
//! keyboard already use: [`Live::click`] for anything clicked in the world (attack, talk,
//! use, aim an ability), the simulation's own walk order for the left stick (the order a
//! click on the ground gives), [`crate::use_quickslot`] for the shortcuts, the conversation
//! screen's reply and continue, and the interface's mouse for screens (a pointer the stick
//! moves). Nothing here changes what the simulation does with an order.
//!
//! A deliberate addition (see [`crate::pad`]): the PC original has no controller support.
//! The target the D-pad picks and A acts on stands in for the 360 version's auto-targeting;
//! its choice (the nearest sensible thing in front of the camera) is chosen, not traced.

use glam::{Vec2, Vec3};
use thedas_render::Camera;
use thedas_script::vm::ObjectId;
use thedas_sim::world::ObjectKind;

use crate::follow::Follow;
use crate::live::{Live, Pick};
use crate::pad::{Action, Context, Controller, Order, PadState};
use crate::screens::{Screen, Screens};

/// What the pad needs of the app (the window's, or the headless capture's).
pub trait Host {
    fn live(&mut self) -> Option<&mut Live>;
    fn camera(&self) -> Camera;
    fn follow(&mut self) -> Option<&mut Follow>;
    /// Turns the free camera (no leader followed).
    fn look(&mut self, yaw: f32, pitch: f32);
    fn size(&self) -> (u32, u32);
    fn cursor(&self) -> (f64, f64);
    /// The mouse moves there (the interface sees it).
    fn pointer_to(&mut self, x: f64, y: f64);
    /// The left button goes down or up at the cursor, as the mouse's does.
    fn left_button(&mut self, down: bool);
    /// The right button goes down or up at the cursor, as the mouse's does.
    fn right_button(&mut self, down: bool);
    /// A double click at the cursor, for the interface.
    fn double_click(&mut self);
    fn screens(&mut self) -> Option<&mut Screens>;
    /// Whether a popup's dialog box is up (it takes the input).
    fn popup(&mut self) -> bool;
    /// The save (1) or load (2) screen, as F5 and F9 open it.
    fn open_saves(&mut self, mode: i32);
    /// The conversation line on screen: whether it has played, and whether it has nothing
    /// to choose (it continues).
    fn line(&self) -> Option<(bool, bool)>;
    /// A click reached the interface (skips or continues a conversation line).
    fn interface_clicked(&mut self);
    /// The mouse wheel turns `lines` (up positive) at the cursor.
    fn scroll(&mut self, lines: f64);
    /// Highlights conversation reply `n` (from 0) as the reply buttons' own focus does;
    /// false when the conversation screen is not up.
    fn highlight_reply(&mut self, n: usize) -> bool;
}

/// How far ahead of the leader a walk order aims (metres): far enough that it never
/// arrives before the next order.
const WALK_AHEAD: [f32; 4] = [3.0, 2.0, 1.2, 0.6];
/// How far the pad's targets reach (metres).
pub const TARGET_RANGE: f32 = 15.0;
/// "In front": within this angle of the camera's forward (radians).
pub const TARGET_CONE: f32 = 1.2;
/// Anything this close counts as in front, whichever way it is.
pub const TARGET_NEAR: f32 = 2.0;
/// A pointer step (D-pad on a screen), pixels.
pub const POINTER_STEP: f64 = 40.0;

/// The player's screens LB and RB move between, in order (Back opens the first).
const PAGES: [Page; 10] = [
    Page::Menu(Screen::Inventory),
    Page::Menu(Screen::CharacterRecord),
    Page::Menu(Screen::Skills),
    Page::Menu(Screen::SpellsTalents),
    Page::Menu(Screen::Journal),
    Page::Menu(Screen::AreaMap),
    Page::Menu(Screen::Tactics),
    Page::Menu(Screen::Options),
    Page::Saves(1),
    Page::Saves(2),
];

#[derive(Clone, Copy, Debug, PartialEq)]
enum Page {
    Menu(Screen),
    Saves(i32),
}

/// A conversation line (dialog, line), the reply highlighted on it, and whether the
/// replies showed.
type Highlighted = (Option<(String, String)>, usize, bool);

/// The pad's part of the game: its controller, the target, the reply picked.
#[derive(Default)]
pub struct PadPlay {
    pub controller: Controller,
    /// What A acts on (the D-pad cycles it).
    pub target: Option<ObjectId>,
    /// The conversation reply picked (from 0), and the line it belongs to.
    pub reply: usize,
    reply_line: Option<(String, String)>,
    /// The quickbar picker is open (and whether the game was paused before it).
    picker: Option<bool>,
    /// The page of [`PAGES`] last opened.
    page: usize,
    /// The pad has been used (the pointer is drawn on screens).
    pub used: bool,
    /// The reply highlighted last: the line, the reply, and whether the replies showed.
    highlighted: Option<Highlighted>,
    /// The whole party is selected (LB+RB): A's attack and walking order everyone.
    pub group: bool,
}

/// Where the pad's input goes, from the screens up.
pub fn context(screens: Option<&Screens>, popup: bool, picker: bool) -> Context {
    let Some(s) = screens else {
        return if picker {
            Context::Picker
        } else {
            Context::Field
        };
    };
    if s.in_cutscene() {
        Context::Cutscene
    } else if s.shown().contains(&Screen::Conversation) && !popup {
        Context::Conversation
    } else if popup || !s.exploring() {
        Context::Screen
    } else if picker {
        Context::Picker
    } else {
        Context::Field
    }
}

impl PadPlay {
    /// Where the pad's input goes now.
    pub fn context_for(&self, screens: Option<&Screens>, popup: bool) -> Context {
        context(screens, popup, self.picker.is_some())
    }

    /// One frame of the pad: `state` now, `dt_ms` after the last. Returns what it did, for
    /// the log.
    pub fn frame(&mut self, state: &PadState, host: &mut dyn Host, dt_ms: f64) -> Vec<String> {
        if !state.is_idle() {
            self.used = true;
        }
        let popup = host.popup();
        let ctx = context(host.screens().map(|s| &*s), popup, self.picker.is_some());
        // The picker only lives over the field: anything else coming up closes it.
        if self.picker.is_some() && !matches!(ctx, Context::Picker | Context::Field) {
            self.close_picker(host);
        }
        self.keep_target(host);
        self.follow_line(host);
        if ctx == Context::Conversation && self.used {
            self.show_reply(host);
        }
        let mut log = Vec::new();
        for order in self.controller.update(state, ctx, dt_ms) {
            if let Some(m) = self.apply(order, host) {
                log.push(m);
            }
        }
        log
    }

    /// Drops the target when it is gone (dead, left, another area).
    fn keep_target(&mut self, host: &mut dyn Host) {
        let Some(t) = self.target else { return };
        let ok = host.live().is_some_and(|l| {
            let sim = l.sim();
            let leader = sim.world.controlled().and_then(|c| sim.world.get(c));
            candidates(&sim).iter().any(|c| c.id == t)
                && leader.is_some_and(|me| {
                    sim.world.get(t).is_some_and(|o| {
                        Vec3::from_array(o.position).distance(Vec3::from_array(me.position))
                            < TARGET_RANGE * 2.0
                    })
                })
        });
        if !ok {
            self.target = None;
        }
    }

    /// Highlights the reply picked, again whenever the line, the reply or whether the
    /// replies show changes (the screen redraws its buttons as they come up).
    fn show_reply(&mut self, host: &mut dyn Host) {
        let shown = host.line().is_some_and(|(played, _)| played);
        let now = (self.reply_line.clone(), self.reply, shown);
        if self.highlighted.as_ref() != Some(&now) && host.highlight_reply(self.reply) {
            self.highlighted = Some(now);
        }
    }

    /// A new conversation line starts with the first reply picked.
    fn follow_line(&mut self, host: &mut dyn Host) {
        let line = host.live().and_then(|l| {
            l.sim()
                .dialog_state()
                .map(|d| (d.dialog.clone(), d.line.clone()))
        });
        if line != self.reply_line {
            self.reply_line = line;
            self.reply = 0;
        }
    }

    fn apply(&mut self, order: Order, host: &mut dyn Host) -> Option<String> {
        match order {
            Order::Act(a) => self.act(a, host),
            Order::Walk(v) => walk(host, v, self.group),
            Order::Camera { yaw, zoom } => {
                match host.follow() {
                    Some(f) => {
                        f.orbit(yaw, 0.0);
                        f.zoom(zoom);
                    }
                    None => host.look(yaw, 0.0),
                }
                None
            }
            Order::Scroll(lines) => {
                host.scroll(lines as f64);
                None
            }
            Order::Pointer(d) => {
                let (x, y) = host.cursor();
                pointer_to(host, x + d.x as f64, y + d.y as f64);
                None
            }
        }
    }

    fn act(&mut self, a: Action, host: &mut dyn Host) -> Option<String> {
        Some(match a {
            Action::Interact => self.interact(host),
            Action::Quickslot(n) => self.quickslot(host, n),
            Action::PrevMember | Action::NextMember => {
                self.group = false;
                let live = host.live()?;
                let (party, now) = {
                    let sim = live.sim();
                    let party = thedas_sim::engine::party_of(&sim);
                    let now = sim
                        .world
                        .controlled()
                        .and_then(|c| party.iter().position(|&m| m == c));
                    (party.len(), now.unwrap_or(0))
                };
                if party == 0 {
                    return None;
                }
                let n = if a == Action::NextMember {
                    (now + 1) % party
                } else {
                    (now + party - 1) % party
                };
                format!("leading with {}", live.choose_leader(n)?)
            }
            Action::WholeParty => {
                self.group = !self.group;
                (if self.group {
                    "whole party selected (A and walking order everyone)"
                } else {
                    "leader only"
                })
                .into()
            }
            Action::Picker => self.open_picker(host),
            Action::ClosePicker => {
                self.close_picker(host);
                "quickbar picker closed".into()
            }
            Action::FocusTarget => {
                if self.target.is_none() {
                    self.target = best_in_front(host, false);
                }
                self.target_message(host)
            }
            Action::NextTarget | Action::PrevTarget => {
                let step = if a == Action::NextTarget { 1 } else { -1 };
                self.target = cycle_target(host, self.target, step);
                self.target_message(host)
            }
            Action::NearestHostile => {
                self.target = best_in_front(host, true);
                self.target_message(host)
            }
            Action::ClearTarget => {
                self.target = None;
                match host.live().and_then(|l| l.aiming.take()) {
                    Some(aim) => format!("target cleared; ability {} not aimed", aim.ability),
                    None => "target cleared".into(),
                }
            }
            Action::CenterCamera => {
                let heading = host.live().and_then(|l| {
                    let sim = l.sim();
                    let me = sim.world.controlled().and_then(|c| sim.world.get(c))?;
                    Some(thedas_sim::world::heading_of(me.facing).to_radians())
                })?;
                host.follow()?.yaw = heading;
                "camera behind the leader".into()
            }
            Action::Hub => {
                let s = host.screens()?;
                s.menu = match s.menu {
                    Some(_) => None,
                    None => Some(Screen::Inventory),
                };
                self.page = 0;
                format!("screens: {:?}", s.menu)
            }
            Action::Pause => {
                let live = host.live()?;
                live.paused = !live.paused;
                if self.picker.is_some() {
                    // Pausing by hand while the picker is up: it stays as set when it closes.
                    self.picker = Some(live.paused);
                }
                (if live.paused { "paused" } else { "running" }).into()
            }
            Action::PointerDown | Action::PointerUp => {
                host.left_button(a == Action::PointerDown);
                return None;
            }
            Action::RightDown | Action::RightUp => {
                host.right_button(a == Action::RightDown);
                return None;
            }
            Action::DoubleClick => {
                host.double_click();
                let (x, y) = host.cursor();
                format!("double click {x:.0},{y:.0}")
            }
            Action::PointerStep(dx, dy) => {
                let (x, y) = host.cursor();
                pointer_to(
                    host,
                    x + dx as f64 * POINTER_STEP,
                    y + dy as f64 * POINTER_STEP,
                );
                return None;
            }
            Action::CloseScreen => {
                if host.popup() {
                    return Some("a dialog box: point at its buttons and press A".into());
                }
                let s = host.screens()?;
                match s.menu.take() {
                    Some(m) => format!("{m:?} closed"),
                    None => "this screen closes from its own button".into(),
                }
            }
            Action::PrevPage | Action::NextPage => {
                let menu = host.screens()?.menu?;
                let at = PAGES
                    .iter()
                    .position(|p| matches!(p, Page::Menu(m) if *m == menu))
                    .unwrap_or(self.page);
                let n = PAGES.len();
                self.page = if a == Action::NextPage {
                    (at + 1) % n
                } else {
                    (at + n - 1) % n
                };
                match PAGES[self.page] {
                    Page::Menu(m) => host.screens()?.menu = Some(m),
                    Page::Saves(mode) => host.open_saves(mode),
                }
                format!("screen {:?}", PAGES[self.page])
            }
            Action::PrevReply | Action::NextReply => {
                let n = host
                    .live()
                    .and_then(|l| l.sim().dialog_state())
                    .map_or(0, |d| d.replies.len());
                if n == 0 {
                    return None;
                }
                self.reply = if a == Action::NextReply {
                    (self.reply + 1).min(n - 1)
                } else {
                    self.reply.saturating_sub(1)
                };
                let text = host
                    .live()
                    .and_then(|l| l.sim().dialog_state())
                    .and_then(|d| d.replies.get(self.reply).map(|r| r.1.clone()))
                    .unwrap_or_default();
                format!("reply {} picked: {text}", self.reply)
            }
            Action::ChooseReply => match host.line() {
                Some((played, waits)) if !played || waits => {
                    host.interface_clicked();
                    "line skipped or continued".into()
                }
                _ => {
                    let n = std::mem::take(&mut self.reply);
                    crate::reply(host.live()?, n)
                }
            },
            Action::Skip => {
                let cutscene = host.screens().is_some_and(|s| s.in_cutscene());
                if cutscene {
                    host.live()?.sim().skip_cutscene();
                    "cutscene skipped".into()
                } else {
                    host.interface_clicked();
                    "line skipped".into()
                }
            }
        })
    }

    /// A: a waiting ability goes to the target (or the ground ahead for one aimed at
    /// places); otherwise the leader acts on the target, or on the best thing in front.
    fn interact(&mut self, host: &mut dyn Host) -> String {
        let aiming = host.live().and_then(|l| l.aiming.clone());
        let target = match self.target {
            Some(t) => Some(t),
            None => best_in_front(host, aiming.is_some()),
        };
        let pick = match (target, &aiming) {
            (Some(t), _) => Some(Pick::Object(t)),
            (None, Some(aim)) => {
                let place = host
                    .live()
                    .is_some_and(|l| l.sim().ability_targets_place(aim.ability));
                if place { ground_ahead(host) } else { None }.map(Pick::Ground)
            }
            (None, None) => None,
        };
        let Some(pick) = pick else {
            return "nothing in front to act on".into();
        };
        if let Pick::Object(t) = pick {
            self.target = Some(t);
        }
        let order = host
            .live()
            .and_then(|l| l.click(pick))
            .unwrap_or_else(|| "nothing to do there".into());
        // The whole party selected: the rest attack the leader's target too, as the same
        // order given to each.
        if let (true, None, Pick::Object(t)) = (self.group, aiming, pick) {
            if let Some(live) = host.live() {
                let mut sim = live.sim();
                let leader = sim.world.controlled();
                let hostile = leader.is_some_and(|l| sim.hostile(l, t));
                if hostile {
                    for m in party_with(&sim) {
                        if Some(m) != leader {
                            sim.order_attack(m, t);
                        }
                    }
                    return format!("{order} (with the whole party)");
                }
            }
        }
        order
    }

    /// X, Y, B (RT for the second set): the quickbar's slot, aimed at once at the target (or
    /// the nearest hostile in front) when it needs one, as the 360's shortcuts are.
    fn quickslot(&mut self, host: &mut dyn Host, slot: i32) -> String {
        let Some(live) = host.live() else {
            return String::new();
        };
        let Some(ability) = crate::use_quickslot(live, slot) else {
            return format!("quickslot {}", slot + 1);
        };
        let target = self.target.or_else(|| best_in_front(host, true));
        match target {
            Some(t) => {
                self.target = Some(t);
                host.live()
                    .and_then(|l| l.click(Pick::Object(t)))
                    .unwrap_or_else(|| format!("ability {ability}: not aimed"))
            }
            None => format!("aiming ability {ability} (A on a target)"),
        }
    }

    fn open_picker(&mut self, host: &mut dyn Host) -> String {
        let Some(live) = host.live() else {
            return String::new();
        };
        self.picker = Some(live.paused);
        live.paused = true;
        // On the quickbar's first slot (it runs along the bottom of the HUD). Provisional:
        // placed by eye at 1280x720.
        let (w, h) = host.size();
        pointer_to(host, w as f64 * 0.065, h as f64 * 0.965);
        "quickbar picker: paused; point at a slot and press A (B or LT closes)".into()
    }

    fn close_picker(&mut self, host: &mut dyn Host) {
        if let Some(was) = self.picker.take() {
            if let Some(l) = host.live() {
                l.paused = was;
            }
        }
    }

    fn target_message(&self, host: &mut dyn Host) -> String {
        let tag = self.target.and_then(|t| {
            host.live()
                .and_then(|l| l.sim().world.get(t).map(|o| o.tag.clone()))
        });
        match tag {
            Some(t) => format!("target {t}"),
            None => "no target".into(),
        }
    }

    /// Whether the pointer is drawn: the pad drives it (on a screen or in the picker).
    pub fn pointer_shown(&self, ctx: Context) -> bool {
        self.used && matches!(ctx, Context::Screen | Context::Picker)
    }

    /// The pad's overlay in the world on a `size` view: a ring round the target. Pixel line
    /// segments, drawn under the interface as the aiming reticle is.
    pub fn overlay(
        &self,
        live: Option<&Live>,
        camera: &Camera,
        size: (u32, u32),
        ctx: Context,
    ) -> Vec<[Vec2; 2]> {
        let mut lines = Vec::new();
        if let (Some(t), Some(live), Context::Field | Context::Picker) = (self.target, live, ctx) {
            let sim = live.sim();
            if let Some(o) = sim.world.get(t) {
                let at = Vec3::from_array(o.position);
                let r = if o.kind == ObjectKind::Creature {
                    sim.body_radius(t).max(0.35) + 0.25
                } else {
                    0.8
                };
                let ring =
                    crate::reticle::outline(thedas_sim::quickbar::AbilityArea::Sphere(r), at, at);
                lines.extend(crate::reticle::project(camera, size, &ring));
            }
        }
        lines
    }

    /// The pointer, when the pad drives it: drawn over the interface.
    pub fn pointer(&self, cursor: (f64, f64), ctx: Context) -> Vec<[Vec2; 2]> {
        if self.pointer_shown(ctx) {
            pointer_arrow(cursor)
        } else {
            Vec::new()
        }
    }
}

/// An arrow pointer with its tip at `cursor` (pixels).
pub fn pointer_arrow((x, y): (f64, f64)) -> Vec<[Vec2; 2]> {
    let p = Vec2::new(x as f32, y as f32);
    let (a, b, c) = (p, p + Vec2::new(0.0, 20.0), p + Vec2::new(14.0, 14.0));
    vec![
        [a, b],
        [b, c],
        [c, a],
        [p + Vec2::new(5.0, 17.0), p + Vec2::new(9.0, 25.0)],
    ]
}

fn pointer_to(host: &mut dyn Host, x: f64, y: f64) {
    let (w, h) = host.size();
    host.pointer_to(
        x.clamp(0.0, w.saturating_sub(1) as f64),
        y.clamp(0.0, h.saturating_sub(1) as f64),
    );
}

/// The left stick: the leader walks toward a point a little ahead in the stick's direction
/// (relative to the camera), as a click on the ground there would order; let go, it stops
/// where it stands (a walk to its own feet).
fn walk(host: &mut dyn Host, v: Vec2, group: bool) -> Option<String> {
    let yaw = host.camera().yaw;
    let live = host.live()?;
    let mut sim = live.sim();
    let leader = sim.world.controlled()?;
    let area = sim.world.get(leader)?.area?;
    // The whole party selected: the others walk the same way, each from where it stands.
    let mut walkers = vec![leader];
    if group {
        walkers.extend(party_with(&sim).into_iter().filter(|&m| m != leader));
    }
    let dir = crate::pad::world_direction(v, yaw).normalize_or_zero();
    for who in walkers {
        let Some(at) = sim.world.get(who).map(|o| Vec3::from_array(o.position)) else {
            continue;
        };
        if v == Vec2::ZERO {
            sim.order_move(who, at.to_array());
            continue;
        }
        for ahead in WALK_AHEAD {
            let p = at + (dir * ahead).extend(0.0);
            let from = [p.x, p.y, p.z + 2.5];
            if let Some(g) = sim.ground_hit(area, from, [0.0, 0.0, -1.0], 6.0) {
                sim.order_move(who, g);
                break;
            }
        }
    }
    None
}

/// The living party members in the leader's area (the leader too).
fn party_with(sim: &thedas_sim::Sim) -> Vec<ObjectId> {
    let area = sim
        .world
        .controlled()
        .and_then(|c| sim.world.get(c))
        .and_then(|o| o.area);
    thedas_sim::engine::party_of(sim)
        .into_iter()
        .filter(|&m| {
            sim.world
                .get(m)
                .is_some_and(|o| !o.dead && o.area.is_some() && o.area == area)
        })
        .collect()
}

/// The ground a few metres ahead of the leader (camera-forward), for an ability aimed at a
/// place with nothing targeted.
fn ground_ahead(host: &mut dyn Host) -> Option<[f32; 3]> {
    let yaw = host.camera().yaw;
    let live = host.live()?;
    let mut sim = live.sim();
    let me = sim.world.controlled().and_then(|c| sim.world.get(c))?;
    let (area, at) = (me.area?, Vec3::from_array(me.position));
    let dir = Vec2::new(yaw.cos(), yaw.sin());
    [4.0, 2.5, 1.0].into_iter().find_map(|d: f32| {
        let p = at + (dir * d).extend(0.0);
        sim.ground_hit(area, [p.x, p.y, p.z + 2.5], [0.0, 0.0, -1.0], 6.0)
    })
}

/// Something the pad can target.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub id: ObjectId,
    pub at: Vec2,
    pub hostile: bool,
}

/// What A could sensibly act on in the leader's area: living hostiles, creatures with
/// something to say, and interactive placeables (doors, containers); not the party.
pub fn candidates(sim: &thedas_sim::Sim) -> Vec<Candidate> {
    let Some(leader) = sim.world.controlled() else {
        return Vec::new();
    };
    let Some(area) = sim.world.get(leader).and_then(|o| o.area) else {
        return Vec::new();
    };
    let party = thedas_sim::engine::party_of(sim);
    sim.world
        .objects
        .values()
        .filter(|o| o.area == Some(area) && o.active && !party.contains(&o.id))
        .filter_map(|o| {
            let hostile = o.kind == ObjectKind::Creature && !o.dead && sim.hostile(leader, o.id);
            let sensible = match o.kind {
                ObjectKind::Creature => !o.dead && (hostile || o.dialog.is_some()),
                ObjectKind::Placeable => o.interactive,
                _ => false,
            };
            sensible.then(|| Candidate {
                id: o.id,
                at: Vec2::new(o.position[0], o.position[1]),
                hostile,
            })
        })
        .collect()
}

/// The best of `cands` for a leader at `from` looking along `facing` (unit): the nearest in
/// front, weighing how far off to the side it is; `hostile` only hostiles.
pub fn choose_in_front(
    cands: &[Candidate],
    from: Vec2,
    facing: Vec2,
    hostile: bool,
) -> Option<ObjectId> {
    cands
        .iter()
        .filter(|c| !hostile || c.hostile)
        .filter_map(|c| {
            let d = c.at - from;
            let dist = d.length();
            let angle = if dist > 1e-4 {
                facing.angle_to(d).abs()
            } else {
                0.0
            };
            let ok = dist <= TARGET_RANGE && (angle <= TARGET_CONE || dist <= TARGET_NEAR);
            ok.then_some((dist * (1.0 + angle), c.id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// The next (`step` 1) or previous (-1) of `cands` in view after `current`, ordered left to
/// right as seen looking along `facing`; the first in front when nothing is targeted.
pub fn cycle(
    cands: &[Candidate],
    from: Vec2,
    facing: Vec2,
    current: Option<ObjectId>,
    step: i32,
) -> Option<ObjectId> {
    let mut seen: Vec<(f32, ObjectId)> = cands
        .iter()
        .filter_map(|c| {
            let d = c.at - from;
            let angle = if d.length() > 1e-4 {
                facing.angle_to(d)
            } else {
                0.0
            };
            (d.length() <= TARGET_RANGE && angle.abs() <= std::f32::consts::FRAC_PI_2)
                .then_some((angle, c.id))
        })
        .collect();
    // Anticlockwise is to the left: left to right is from the largest angle down.
    seen.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
    let ids: Vec<ObjectId> = seen.into_iter().map(|(_, id)| id).collect();
    if ids.is_empty() {
        return None;
    }
    match current.and_then(|c| ids.iter().position(|&i| i == c)) {
        Some(i) => {
            let n = ids.len() as i32;
            Some(ids[(i as i32 + step).rem_euclid(n) as usize])
        }
        None => choose_in_front(cands, from, facing, false).filter(|id| ids.contains(id)),
    }
}

/// The leader's place and the camera's forward on the ground.
fn viewpoint(host: &mut dyn Host) -> Option<(Vec2, Vec2, Vec<Candidate>)> {
    let yaw = host.camera().yaw;
    let live = host.live()?;
    let sim = live.sim();
    let me = sim.world.controlled().and_then(|c| sim.world.get(c))?;
    let from = Vec2::new(me.position[0], me.position[1]);
    Some((from, Vec2::new(yaw.cos(), yaw.sin()), candidates(&sim)))
}

fn best_in_front(host: &mut dyn Host, hostile: bool) -> Option<ObjectId> {
    let (from, facing, cands) = viewpoint(host)?;
    choose_in_front(&cands, from, facing, hostile)
}

fn cycle_target(host: &mut dyn Host, current: Option<ObjectId>, step: i32) -> Option<ObjectId> {
    let (from, facing, cands) = viewpoint(host)?;
    cycle(&cands, from, facing, current, step)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(id: ObjectId, x: f32, y: f32, hostile: bool) -> Candidate {
        Candidate {
            id,
            at: Vec2::new(x, y),
            hostile,
        }
    }

    #[test]
    fn picks_the_nearest_in_front() {
        let cands = [
            c(1, -3.0, 0.0, true), // behind
            c(2, 6.0, 0.5, false), // ahead, further
            c(3, 3.0, 2.5, false), // ahead, off to the left
            c(4, 4.0, 0.0, true),  // ahead
            c(5, 40.0, 0.0, true), // out of range
        ];
        let id = choose_in_front(&cands, Vec2::ZERO, Vec2::X, false);
        assert_eq!(id, Some(4));
        assert_eq!(choose_in_front(&cands, Vec2::ZERO, Vec2::X, true), Some(4));
        // Looking the other way: only the one behind is in front now.
        assert_eq!(
            choose_in_front(&cands, Vec2::ZERO, -Vec2::X, false),
            Some(1)
        );
        // Something right beside the leader counts whichever way it is.
        let near = [c(9, 0.0, -1.0, false)];
        assert_eq!(choose_in_front(&near, Vec2::ZERO, Vec2::X, false), Some(9));
        assert_eq!(choose_in_front(&[], Vec2::ZERO, Vec2::X, false), None);
    }

    #[test]
    fn cycles_left_to_right_and_wraps() {
        let cands = [
            c(1, 5.0, 3.0, true),  // left
            c(2, 5.0, 0.0, true),  // middle
            c(3, 5.0, -3.0, true), // right
            c(4, -5.0, 0.0, true), // behind: not in view
        ];
        let f = Vec2::X;
        assert_eq!(cycle(&cands, Vec2::ZERO, f, None, 1), Some(2));
        assert_eq!(cycle(&cands, Vec2::ZERO, f, Some(2), 1), Some(3));
        assert_eq!(cycle(&cands, Vec2::ZERO, f, Some(3), 1), Some(1));
        assert_eq!(cycle(&cands, Vec2::ZERO, f, Some(1), -1), Some(3));
        assert_eq!(cycle(&cands, Vec2::ZERO, f, Some(2), -1), Some(1));
        assert_eq!(cycle(&[], Vec2::ZERO, f, Some(2), 1), None);
    }

    #[test]
    fn contexts_follow_the_screens() {
        let s = Screens::default();
        // Nothing shown yet counts as exploring.
        assert_eq!(context(Some(&s), false, false), Context::Field);
        assert_eq!(context(Some(&s), false, true), Context::Picker);
        assert_eq!(context(Some(&s), true, false), Context::Screen);
        assert_eq!(context(None, false, false), Context::Field);
    }

    #[test]
    fn the_pointer_is_an_arrow_at_the_cursor() {
        let a = pointer_arrow((10.0, 20.0));
        assert_eq!(a[0][0], Vec2::new(10.0, 20.0));
        assert!(a.iter().all(|[p, q]| p.x >= 10.0 && q.y >= 20.0));
    }
}
