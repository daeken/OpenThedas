//! Mod mode: tools over the running game, not an editor beside it. Opening it reloads and
//! switches nothing; the world keeps running (or stays paused, as the player left it), and
//! the panels can stay open while playing.
//!
//! This first slice inspects and scrubs:
//!
//! - **Inspector**: a creature or placeable picked with Ctrl+click (or the Pick button and a
//!   click) shows what it is and what it is animating now: each clip's tree, action,
//!   animation resource, time, frame and length.
//! - **Animation scrubber**: holds the picked creature's animation at a time of the panel's
//!   choosing, steps it a frame at a time, plays it slowed down and loops a range of it.
//!   "Report this frame" files an issue report naming the creature, the animation and the
//!   frame.
//!
//! The panels are drawn with egui over the game's frame, after its interface. Input goes to
//! them only while the pointer is over a panel (or a text field has the keyboard); the game
//! gets everything else, so it stays playable with the panels open.
//!
//! Mod mode writes no simulation state itself: changes to the game (plot flags, starting a
//! conversation) go through the simulation's own commands and are kept in its journal. The
//! scrubber only changes how a creature is drawn.

use thedas_script::vm::ObjectId;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::Window;

/// Frames per second the scrubber counts in. Provisional: the game's animations carry key
/// times, not a frame rate; 30 is the rate its own tools and its head-tracking poses use.
pub const FRAME_RATE: f32 = 30.0;

/// One clip a creature is animating with (its own motion first, then the layers over it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnimLine {
    /// Which layer: 0 for the creature's own motion.
    pub layer: usize,
    pub tree: String,
    pub action: String,
    /// The `.ani` resource, when the tree has the action.
    pub animation: Option<String>,
    /// Seconds into the animation (wrapped when it loops).
    pub time: f32,
    pub length: f32,
    pub looping: bool,
    pub weight: f32,
}

/// What the inspector shows of the picked object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Inspected {
    pub id: ObjectId,
    pub kind: String,
    pub tag: String,
    pub template: String,
    pub name: Option<String>,
    pub area: String,
    pub position: [f32; 3],
    pub facing: f32,
    pub active: bool,
    pub dead: bool,
    pub dialog: Option<String>,
    pub animations: Vec<AnimLine>,
}

/// The scrubber's hold on one creature's animation.
#[derive(Clone, Debug, PartialEq)]
pub struct Scrub {
    pub id: ObjectId,
    /// Seconds into the creature's own animation that it is drawn at.
    pub time: f32,
    pub length: f32,
    pub playing: bool,
    /// Speed while playing: 1 is real time.
    pub speed: f32,
    /// A range to loop while playing, in seconds.
    pub range: Option<(f32, f32)>,
}

impl Scrub {
    /// The frame the hold is at.
    pub fn frame(&self) -> u32 {
        (self.time * FRAME_RATE).round().max(0.0) as u32
    }

    /// The last frame of the animation.
    pub fn last_frame(&self) -> u32 {
        (self.length * FRAME_RATE).round().max(0.0) as u32
    }

    /// Moves to frame `n` (clamped to the animation).
    pub fn seek(&mut self, n: i64) {
        let n = n.clamp(0, self.last_frame() as i64);
        self.time = (n as f32 / FRAME_RATE).min(self.length);
    }

    /// Advances a playing hold by `dt` seconds, wrapping within its range (or the whole
    /// animation).
    pub fn advance(&mut self, dt: f32) {
        if !self.playing || self.length <= 0.0 {
            return;
        }
        let (lo, hi) = self
            .range
            .filter(|(a, b)| b > a)
            .unwrap_or((0.0, self.length));
        let span = (hi - lo).max(1e-3);
        let t = self.time + dt * self.speed;
        self.time = if t < lo || t > hi {
            lo + (t - lo).rem_euclid(span)
        } else {
            t
        };
    }
}

/// What the panels ask the app to do this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// File an issue report with this note.
    Report(String),
    /// The conversation panel's request (see [`crate::modconv`]).
    Conv(crate::modconv::ConvAction),
    /// The world-state browser's request (see [`crate::modworld`]).
    World(crate::modworld::WorldAction),
    /// The free camera on (true) or back to following the leader.
    Fly(bool),
    /// The free camera moved to look at this object.
    FlyTo(ObjectId),
    /// This object's models written for Blender (see [`crate::glb`]).
    Export(ObjectId),
    /// Live edits on or off (see [`crate::workspace`]).
    LiveEdits(bool),
    /// An object put at a place and facing (degrees).
    Move {
        id: ObjectId,
        position: [f32; 3],
        facing: f32,
    },
    /// An object put where the camera looks (the middle of the view).
    MoveToView(ObjectId),
    /// A creature (`creature`) or placeable made from a template where the camera looks.
    Spawn { creature: bool, template: String },
    /// An object removed (no undo).
    Remove(ObjectId),
}

/// Mod mode's state and its egui overlay.
pub struct ModMode {
    pub open: bool,
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    /// The last frame's panels, to paint (with their pixels per point). Kept, so a frame
    /// drawn offscreen (a screenshot of a hidden window) shows them too.
    shapes: Option<(Vec<egui::ClippedPrimitive>, f32)>,
    /// Texture changes not yet given to the renderer: they pile up while nothing is
    /// painted, as egui sends each (the font atlas) only once.
    textures: egui::TexturesDelta,
    /// A press went to a panel, so its release goes there too.
    pressed_in_overlay: bool,
    /// A press picked an object, so its release is swallowed too.
    pressed_to_pick: bool,
    /// The next world click picks an object (the Pick button).
    pub picking: bool,
    ctrl: bool,
    pub selected: Option<ObjectId>,
    pub scrub: Option<Scrub>,
    /// The conversation launcher.
    pub convs: crate::modconv::Convs,
    /// The world-state browser.
    pub world: crate::modworld::WorldBrowser,
    /// Whether the camera flies free (the app keeps this up to date).
    pub fly: bool,
    /// What the last export wrote, or why it failed.
    pub exported: Option<String>,
    /// Whether live edits are mounted (the app keeps this up to date).
    pub live: bool,
    /// What live edits last did.
    pub live_note: Option<String>,
    /// The inspected object's place as being typed: (object, position, facing).
    place: Option<(ObjectId, [f32; 3], f32)>,
    /// The spawn row: a template and whether it is a creature's.
    spawn_template: String,
    spawn_creature: bool,
    /// What the last move, spawn or removal did.
    pub placed: Option<String>,
    actions: Vec<Action>,
}

impl ModMode {
    pub fn new(window: &Window, device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let ctx = egui::Context::default();
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let renderer = egui_wgpu::Renderer::new(device, format, Default::default());
        ModMode {
            open: false,
            ctx,
            state,
            renderer,
            shapes: None,
            textures: Default::default(),
            pressed_in_overlay: false,
            pressed_to_pick: false,
            picking: false,
            ctrl: false,
            selected: None,
            scrub: None,
            convs: Default::default(),
            world: Default::default(),
            fly: false,
            exported: None,
            live: false,
            live_note: None,
            place: None,
            spawn_template: String::new(),
            spawn_creature: true,
            placed: None,
            actions: Vec::new(),
        }
    }

    /// Opens or closes the panels. Closing lets go of a held animation.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        if !self.open {
            self.scrub = None;
            self.picking = false;
        }
    }

    /// Offers a window event to the panels. Returns what the game should do with it.
    pub fn window_event(&mut self, window: &Window, event: &WindowEvent) -> Routed {
        if let WindowEvent::ModifiersChanged(m) = event {
            self.ctrl = m.state().control_key() || m.state().super_key();
        }
        if !self.open {
            return Routed::Game;
        }
        let response = self.state.on_window_event(window, event);
        match event {
            WindowEvent::KeyboardInput { event: key, .. } => {
                if key.state == ElementState::Pressed
                    && key.physical_key == PhysicalKey::Code(KeyCode::Escape)
                    && (self.picking || self.scrub.is_some())
                {
                    self.picking = false;
                    return Routed::Swallowed;
                }
                if self.ctx.egui_wants_keyboard_input() {
                    Routed::Swallowed
                } else {
                    Routed::Game
                }
            }
            WindowEvent::CursorMoved { .. } => {
                if self.ctx.is_pointer_over_egui() || self.pressed_in_overlay {
                    Routed::CursorOnly
                } else {
                    Routed::Game
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let pressed = *state == ElementState::Pressed;
                if pressed {
                    let over = response.consumed
                        || self.ctx.is_pointer_over_egui()
                        || self.ctx.egui_wants_pointer_input();
                    if over {
                        self.pressed_in_overlay = true;
                        return Routed::Swallowed;
                    }
                    if *button == MouseButton::Left && (self.picking || self.ctrl) {
                        self.pressed_to_pick = true;
                        self.picking = false;
                        return Routed::Pick;
                    }
                    Routed::Game
                } else if std::mem::take(&mut self.pressed_in_overlay)
                    | std::mem::take(&mut self.pressed_to_pick)
                {
                    Routed::Swallowed
                } else {
                    Routed::Game
                }
            }
            WindowEvent::MouseWheel { .. } => {
                if self.ctx.is_pointer_over_egui() {
                    Routed::Swallowed
                } else {
                    Routed::Game
                }
            }
            _ => Routed::Game,
        }
    }

    /// Whether egui shows its own cursor (the pointer is over a panel).
    pub fn owns_cursor(&self) -> bool {
        self.open && self.ctx.is_pointer_over_egui()
    }

    /// Lays out the panels for this frame from what the game shows now (`inspected`: the
    /// picked object), advancing a playing hold by `dt` seconds. Returns what the panels
    /// asked for.
    pub fn frame(
        &mut self,
        window: &Window,
        inspected: Option<&Inspected>,
        dt: f32,
    ) -> Vec<Action> {
        if !self.open {
            self.shapes = None;
            return Vec::new();
        }
        if let Some(s) = self.scrub.as_mut() {
            s.advance(dt);
        }
        // A hold on a creature that is no longer picked, or no longer there, is let go.
        if self
            .scrub
            .as_ref()
            .is_some_and(|s| Some(s.id) != self.selected || inspected.is_none())
        {
            self.scrub = None;
        }
        let input = self.state.take_egui_input(window);
        let ctx = self.ctx.clone();
        let output = ctx.run_pass(input, |ctx| self.panels(ctx, inspected));
        self.state
            .handle_platform_output(window, output.platform_output);
        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        self.shapes = Some((primitives, output.pixels_per_point));
        self.textures.append(output.textures_delta);
        std::mem::take(&mut self.actions)
    }

    fn panels(&mut self, ctx: &egui::Context, inspected: Option<&Inspected>) {
        egui::Window::new("Mod mode")
            .default_pos([16.0, 16.0])
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let label = if self.picking {
                        "Click an object…"
                    } else {
                        "Pick"
                    };
                    if ui.button(label).clicked() {
                        self.picking = !self.picking;
                    }
                    ui.label("or Ctrl+click in the world");
                });
                ui.horizontal(|ui| {
                    if ui
                        .selectable_label(self.convs.open, "Conversations")
                        .clicked()
                    {
                        self.convs.open = !self.convs.open;
                    }
                    if ui
                        .selectable_label(self.world.open, "World state")
                        .clicked()
                    {
                        self.world.open = !self.world.open;
                        // Opened again, it reads the world afresh.
                        self.world.snapshot = None;
                    }
                    if ui
                        .selectable_label(self.fly, "Fly")
                        .on_hover_text(
                            "The camera flies free: WASD to move, Q/E down and up, Shift \
                             faster, the right mouse button to look (F does the same)",
                        )
                        .clicked()
                    {
                        self.actions.push(Action::Fly(!self.fly));
                    }
                });
                if self.fly {
                    ui.weak("WASD move, Q/E down/up, Shift faster, right mouse looks");
                }
                let mut live = self.live;
                if ui
                    .checkbox(&mut live, "Live edits")
                    .on_hover_text(
                        "Mounts workspace/override above every mod. Textures saved over an \
                         export's PNGs are written there and drawn at once.",
                    )
                    .changed()
                {
                    self.actions.push(Action::LiveEdits(live));
                }
                if let Some(note) = &self.live_note {
                    ui.weak(note);
                }
                self.spawning(ui);
                ui.separator();
                match inspected {
                    None => {
                        ui.label("Nothing picked.");
                    }
                    Some(o) => self.inspector(ui, o),
                }
                if let Some(note) = &self.exported {
                    ui.separator();
                    ui.label(note);
                }
            });
        let mut asked = Vec::new();
        self.convs.window(ctx, &mut asked);
        self.actions.extend(asked.into_iter().map(Action::Conv));
        let mut asked = Vec::new();
        self.world.window(ctx, &self.convs.history, &mut asked);
        self.actions.extend(asked.into_iter().map(Action::World));
    }

    /// Where the inspected object stands, to change: typed, or where the camera looks;
    /// and removing it.
    fn placing(&mut self, ui: &mut egui::Ui, o: &Inspected) {
        // The typed place follows the object until it is edited.
        if self.place.is_none_or(|(id, ..)| id != o.id) {
            self.place = Some((o.id, o.position, o.facing.rem_euclid(360.0)));
        }
        let Some((_, mut p, mut f)) = self.place else {
            return;
        };
        ui.horizontal(|ui| {
            ui.label("place");
            for (v, l) in p.iter_mut().zip(["x", "y", "z"]) {
                ui.add(egui::DragValue::new(v).speed(0.05).prefix(format!("{l} ")));
            }
            ui.add(
                egui::DragValue::new(&mut f)
                    .speed(1.0)
                    .suffix("°")
                    .range(0.0..=360.0),
            );
        });
        self.place = Some((o.id, p, f));
        ui.horizontal(|ui| {
            if ui.button("Move").clicked() {
                self.actions.push(Action::Move {
                    id: o.id,
                    position: p,
                    facing: f,
                });
            }
            if ui
                .button("Move to view")
                .on_hover_text("Where the middle of the view meets the ground")
                .clicked()
            {
                self.actions.push(Action::MoveToView(o.id));
                self.place = None;
            }
            if ui
                .button("Remove")
                .on_hover_text("Takes it out of the world (cannot be undone)")
                .clicked()
            {
                self.actions.push(Action::Remove(o.id));
            }
        });
    }

    /// The spawn row: a creature or placeable from its template, where the camera looks.
    fn spawning(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.spawn_creature, true, "creature");
            ui.selectable_value(&mut self.spawn_creature, false, "placeable");
            ui.add(
                egui::TextEdit::singleline(&mut self.spawn_template)
                    .hint_text("template (.utc / .utp)")
                    .desired_width(150.0),
            );
            if ui
                .button("Spawn")
                .on_hover_text("Where the middle of the view meets the ground")
                .clicked()
                && !self.spawn_template.trim().is_empty()
            {
                self.actions.push(Action::Spawn {
                    creature: self.spawn_creature,
                    template: self.spawn_template.trim().to_string(),
                });
            }
        });
        if let Some(note) = &self.placed {
            ui.weak(note);
        }
    }

    fn inspector(&mut self, ui: &mut egui::Ui, o: &Inspected) {
        egui::Grid::new("inspected").num_columns(2).show(ui, |ui| {
            let row = |ui: &mut egui::Ui, k: &str, v: String| {
                ui.label(k);
                ui.monospace(v);
                ui.end_row();
            };
            ui.label("object");
            ui.horizontal(|ui| {
                ui.monospace(format!("{} ({})", o.id, o.kind));
                if ui
                    .small_button("Fly to")
                    .on_hover_text("The free camera looks at it")
                    .clicked()
                {
                    self.actions.push(Action::FlyTo(o.id));
                }
                if ui
                    .small_button("Export to Blender")
                    .on_hover_text("Its models as a .glb, textures as PNGs, and a manifest")
                    .clicked()
                {
                    self.actions.push(Action::Export(o.id));
                }
            });
            ui.end_row();
            row(ui, "tag", o.tag.clone());
            row(ui, "template", o.template.clone());
            if let Some(n) = &o.name {
                row(ui, "name", n.clone());
            }
            row(ui, "area", o.area.clone());
            row(
                ui,
                "position",
                format!(
                    "{:.2}, {:.2}, {:.2}  facing {:.0}°",
                    o.position[0],
                    o.position[1],
                    o.position[2],
                    o.facing.rem_euclid(360.0)
                ),
            );
            let mut state = Vec::new();
            if !o.active {
                state.push("inactive");
            }
            if o.dead {
                state.push("dead");
            }
            if !state.is_empty() {
                row(ui, "state", state.join(", "));
            }
            if let Some(d) = &o.dialog {
                row(ui, "conversation", d.clone());
            }
        });
        ui.separator();
        if o.animations.is_empty() {
            ui.label("Not animating.");
            return;
        }
        ui.label("Animating");
        for a in &o.animations {
            let what = match a.layer {
                0 => "own".to_string(),
                n => format!("layer {n}"),
            };
            ui.monospace(format!(
                "{what}: {} / {}  →  {}",
                a.tree,
                a.action,
                a.animation.as_deref().unwrap_or("(not in the tree)")
            ));
            ui.monospace(format!(
                "   {:.3} s of {:.3} s  frame {} of {}{}{}",
                a.time,
                a.length,
                (a.time * FRAME_RATE).round() as u32,
                (a.length * FRAME_RATE).round() as u32,
                if a.looping { "  looping" } else { "" },
                if (a.weight - 1.0).abs() > 1e-3 {
                    format!("  weight {:.2}", a.weight)
                } else {
                    String::new()
                }
            ));
        }
        ui.separator();
        self.placing(ui, o);
        ui.separator();
        self.scrubber(ui, o);
    }

    /// Holds the object's own animation where it is, or at frame `at`; false when it plays
    /// none.
    pub fn hold(&mut self, o: &Inspected, at: Option<i64>) -> bool {
        let Some(own) = o.animations.first().filter(|a| a.length > 0.0) else {
            return false;
        };
        let mut s = Scrub {
            id: o.id,
            time: own.time,
            length: own.length,
            playing: false,
            speed: 0.25,
            range: None,
        };
        if let Some(n) = at {
            s.seek(n);
        }
        self.scrub = Some(s);
        true
    }

    fn scrubber(&mut self, ui: &mut egui::Ui, o: &Inspected) {
        let Some(own) = o.animations.first().filter(|a| a.length > 0.0) else {
            return;
        };
        let Some(s) = self.scrub.as_mut() else {
            if ui.button("Hold this animation").clicked() {
                self.hold(o, None);
            }
            return;
        };
        s.length = own.length;
        ui.horizontal(|ui| {
            if ui.button("|<").on_hover_text("First frame").clicked() {
                s.seek(0);
                s.playing = false;
            }
            if ui.button("<").on_hover_text("One frame back").clicked() {
                s.seek(s.frame() as i64 - 1);
                s.playing = false;
            }
            let play = if s.playing { "Pause" } else { "Play" };
            if ui.button(play).clicked() {
                s.playing = !s.playing;
            }
            if ui.button(">").on_hover_text("One frame on").clicked() {
                s.seek(s.frame() as i64 + 1);
                s.playing = false;
            }
            if ui.button(">|").on_hover_text("Last frame").clicked() {
                s.seek(s.last_frame() as i64);
                s.playing = false;
            }
        });
        let mut frame = s.frame();
        let slider = egui::Slider::new(&mut frame, 0..=s.last_frame()).text("frame");
        if ui.add(slider).changed() {
            s.seek(frame as i64);
            s.playing = false;
        }
        ui.horizontal(|ui| {
            ui.label("speed");
            for (label, speed) in [("1/10", 0.1), ("1/4", 0.25), ("1/2", 0.5), ("1", 1.0)] {
                ui.selectable_value(&mut s.speed, speed, label);
            }
        });
        ui.horizontal(|ui| match s.range {
            None => {
                if ui.button("Loop from here").clicked() {
                    s.range = Some((s.time, s.length));
                }
            }
            Some((lo, hi)) => {
                ui.monospace(format!(
                    "looping frames {}–{}",
                    (lo * FRAME_RATE).round(),
                    (hi * FRAME_RATE).round()
                ));
                if ui.button("End here").clicked() {
                    s.range = Some((lo.min(s.time), s.time.max(lo)));
                }
                if ui.button("No loop").clicked() {
                    s.range = None;
                }
            }
        });
        let mut let_go = false;
        ui.horizontal(|ui| {
            if ui.button("Report this frame").clicked() {
                self.actions.push(Action::Report(frame_note(o, s)));
            }
            let_go = ui.button("Let go").clicked();
        });
        if let_go {
            self.scrub = None;
        }
    }

    /// Draws the panels laid out by [`ModMode::frame`] into `view`, over what is there.
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        size: [u32; 2],
    ) {
        let Some((primitives, ppp)) = self.shapes.as_ref() else {
            return;
        };
        let mut textures = std::mem::take(&mut self.textures);
        for (id, deltas) in &textures.set {
            for delta in deltas {
                self.renderer.update_texture(device, queue, *id, delta);
            }
        }
        let ppp = *ppp;
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: size,
            pixels_per_point: ppp,
        };
        self.renderer
            .update_buffers(device, queue, encoder, primitives, &screen);
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mod mode"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            let mut pass = pass.forget_lifetime();
            self.renderer.render(&mut pass, primitives, &screen);
        }
        for id in &textures.free {
            self.renderer.free_texture(id);
        }
        // Handled: egui asserts (in debug builds) that a delta is not dropped unapplied.
        textures.clear();
    }
}

/// How the game should treat a window event mod mode has seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Routed {
    /// The game handles it as usual.
    Game,
    /// A panel took it.
    Swallowed,
    /// The pointer moved over a panel: the game keeps the cursor position only.
    CursorOnly,
    /// A world click that picks the object under the cursor.
    Pick,
}

/// The issue report's note for a held frame.
pub fn frame_note(o: &Inspected, s: &Scrub) -> String {
    let own = o.animations.first();
    format!(
        "Animation frame: {} ({}, template {}) in {}, {} frame {} of {} ({:.3} s){}",
        o.tag,
        o.id,
        o.template,
        o.area,
        own.and_then(|a| a.animation.clone())
            .unwrap_or_else(|| "?".into()),
        s.frame(),
        s.last_frame(),
        s.time,
        own.map(|a| format!(", tree {} action {}", a.tree, a.action))
            .unwrap_or_default()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(length: f32) -> Scrub {
        Scrub {
            id: 7,
            time: 0.0,
            length,
            playing: true,
            speed: 1.0,
            range: None,
        }
    }

    #[test]
    fn frames_step_and_clamp() {
        let mut s = hold(1.0);
        assert_eq!(s.last_frame(), 30);
        s.seek(12);
        assert_eq!(s.frame(), 12);
        assert!((s.time - 0.4).abs() < 1e-6);
        s.seek(-3);
        assert_eq!(s.frame(), 0);
        s.seek(99);
        assert_eq!((s.frame(), s.time), (30, 1.0));
    }

    #[test]
    fn playing_wraps_within_the_animation_or_the_range() {
        let mut s = hold(1.0);
        s.speed = 0.5;
        s.advance(1.0);
        assert!((s.time - 0.5).abs() < 1e-6);
        s.advance(1.5);
        assert!((s.time - 0.25).abs() < 1e-5, "{}", s.time);
        s.range = Some((0.5, 0.75));
        s.time = 0.7;
        s.advance(0.2);
        assert!((s.time - 0.55).abs() < 1e-5, "{}", s.time);
        s.playing = false;
        s.advance(5.0);
        assert!((s.time - 0.55).abs() < 1e-5);
    }

    #[test]
    fn the_report_names_the_creature_animation_and_frame() {
        let o = Inspected {
            id: 42,
            tag: "bhn100cr_guard".into(),
            template: "bhn100cr_guard".into(),
            area: "bhn100ar_castle_cousland".into(),
            animations: vec![AnimLine {
                tree: "humanoid".into(),
                action: "pause".into(),
                animation: Some("mh_pause.ani".into()),
                length: 2.0,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut s = hold(2.0);
        s.seek(17);
        let note = frame_note(&o, &s);
        assert!(note.contains("bhn100cr_guard"), "{note}");
        assert!(note.contains("mh_pause.ani frame 17 of 60"), "{note}");
        assert!(note.contains("tree humanoid action pause"), "{note}");
    }
}
