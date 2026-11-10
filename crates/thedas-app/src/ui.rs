//! The Scaleform interface layer: movies played by `thedas-ui` into offscreen textures,
//! composited over the scene with premultiplied alpha.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use thedas_render::GpuContext;
use thedas_sim::Sim;
use thedas_ui::protocol::GameMessage;
use thedas_ui::stage::Stage;
use thedas_ui::{Gpu, Rgba};
use thedas_vfs::Vfs;

use crate::screens::Screens;

/// A movie pixel at least this opaque (of 255) takes a click from the world.
pub const COVER_ALPHA: u8 = 16;

const COMPOSITE_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}
"#;

struct Composite {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Composite {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui composite"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }
}

pub struct UiLayer {
    pub stage: Stage,
    composite: Option<Composite>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Messages the simulation did not take, already logged.
    ignored: std::collections::HashSet<String>,
    /// The screens' own choices (the inventory's category tab), shared with the message
    /// handler.
    shown: Arc<Mutex<thedas_ui::binding::Presentation>>,
    /// The conversation line on screen and how far it has played.
    line: Option<Line>,
    /// The level whose resources and minimap the interface has.
    level: Option<String>,
    /// Frames since the minimap's creatures were last sent.
    minimap_frame: u32,
    portraits: PortraitClock,
    /// The cutscene playing (its resource) and how long it has been up (milliseconds).
    cutscene: Option<(String, f64)>,
}

/// When portraits are drawn: the party they were drawn for, milliseconds since, and how many
/// times they have been drawn (each drawing gets new image names, so the movies load it
/// afresh).
#[derive(Default)]
struct PortraitClock {
    party: Vec<u32>,
    age_ms: f64,
    drawn: u32,
}

impl PortraitClock {
    /// Whether they are due: the party (`party`) changed, or they are [`PORTRAITS_EVERY_MS`]
    /// old. `dt_ms` is the time since the last call.
    fn due(&mut self, party: Vec<u32>, dt_ms: f64) -> bool {
        self.age_ms += dt_ms;
        if party != self.party || self.age_ms >= PORTRAITS_EVERY_MS {
            self.party = party;
            self.age_ms = 0.0;
            return true;
        }
        false
    }

    /// The image name of member `id`'s portrait in drawing `drawn`.
    fn name(id: u32, drawn: u32) -> String {
        format!("thedas_portrait_{id}_{drawn}.png")
    }
}

/// Portraits are drawn again this often while playing (milliseconds), and whenever the
/// party changes. Provisional.
const PORTRAITS_EVERY_MS: f64 = 3000.0;
/// Portraits' size (pixels, square).
pub const PORTRAIT_SIZE: u32 = 128;

/// minimap.gfx's creature flags (`SharedLibrary.CreatureMapIcon.FLAG_*`).
const MAP_HOSTILE: u32 = 1;
const MAP_IN_PARTY: u32 = 2;
const MAP_PLAYER: u32 = 8;
const MAP_WITHIN_RADIUS: u32 = 16;
/// The minimap's creatures are sent every this many frames.
const MINIMAP_EVERY: u32 = 3;

/// A conversation line: the dialog, the line's text and its replies' nodes.
type LineKey = (String, String, Vec<usize>);

/// The conversation line on screen: while it plays its replies are held back
/// (`Conversation.ResponsesReady` 0, which conversation.gfx watches to show them); once it
/// has played they show, or, with nothing to choose, the conversation goes on.
struct Line {
    key: LineKey,
    /// Milliseconds it has been up.
    t: f64,
    /// Nothing to choose: it continues by itself once played.
    waits: bool,
    continued: bool,
}

impl Line {
    fn played(&self) -> bool {
        self.t >= line_ms(&self.key.1)
    }
}

/// How long a line plays before the conversation goes on by itself (milliseconds), standing
/// in for its voice-over: a reading time for its text, a moment for a line without text (a
/// node that only moves the conversation on: across the game's conversations none of them
/// carries staging of its own). Provisional.
pub fn line_ms(text: &str) -> f64 {
    let chars = text.trim().chars().count() as f64;
    if chars == 0.0 {
        100.0
    } else {
        (1500.0 + 55.0 * chars).min(12_000.0)
    }
}

impl UiLayer {
    /// Loads the movies at `size`; failures are logged and the movie is skipped.
    pub fn new(ctx: &GpuContext, vfs: &Arc<Vfs>, names: &[String], size: (u32, u32)) -> Self {
        let gpu = Gpu {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
        };
        let mut stage = Stage::new(gpu, vfs, size);
        stage.open_all(names);
        Self {
            stage,
            composite: None,
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
            ignored: Default::default(),
            shown: Default::default(),
            line: None,
            level: None,
            minimap_frame: 0,
            portraits: PortraitClock::default(),
            cutscene: None,
        }
    }

    /// Plays the game's screens over a running game: the movies act on `sim` as they send
    /// messages (see [`thedas_ui::input`]).
    pub fn attach(&mut self, sim: &Arc<Mutex<Sim>>) {
        self.stage
            .bridge
            .set_message_handler(Some(thedas_ui::input::handler_for(
                sim.clone(),
                self.shown.clone(),
            )));
    }

    /// One interface frame while playing: binds the simulation's values, brings the screens
    /// up to it, runs the movies `dt_ms` and applies the messages nothing handled yet. The
    /// simulation is not locked while the movies run (their handler locks it).
    /// Returns the abilities the movies asked to use on a target the player is to pick (see
    /// [`thedas_ui::input::aim`]).
    pub fn play(
        &mut self,
        screens: &mut Screens,
        sim: &Arc<Mutex<Sim>>,
        dt_ms: f64,
    ) -> Vec<thedas_ui::input::Aim> {
        let (want, line) = {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            let shown = self.shown.lock().map(|s| s.clone()).unwrap_or_default();
            self.stage
                .bridge
                .bind(thedas_ui::binding::sim_values_for(&sim, &shown));
            let waits = thedas_ui::binding::awaits_continue(&sim);
            let line = thedas_ui::binding::conversation(&sim).map(|d| {
                let key: LineKey = (d.dialog, d.line, d.replies.iter().map(|r| r.0).collect());
                (key, waits)
            });
            (crate::screens::wanted(&sim, screens.inventory), line)
        };
        screens.sync(want, &mut self.stage);
        self.line = match (line, self.line.take()) {
            (Some((key, _)), Some(mut l)) if l.key == key => {
                l.t += dt_ms;
                Some(l)
            }
            (Some((key, waits)), _) => Some(Line {
                key,
                t: 0.0,
                waits,
                continued: false,
            }),
            (None, _) => None,
        };
        if let Some(l) = self.line.as_mut() {
            let played = l.played();
            self.stage.bridge.set_value(
                "Conversation.ResponsesReady",
                thedas_ui::UiValue::Number(played as u8 as f64),
            );
            if played && l.waits && !l.continued {
                l.continued = true;
                self.continue_conversation();
            }
        }
        self.play_cutscene(sim, dt_ms);
        let messages = self.stage.tick(dt_ms);
        let ignored = {
            let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            thedas_ui::input::apply_all(&mut sim, &messages)
        };
        // Messages for the interface itself, which the handler did not get to.
        let ignored: Vec<(String, Vec<String>)> = match self.shown.lock() {
            Ok(mut shown) => ignored
                .into_iter()
                .filter(|(n, a)| !thedas_ui::input::present(&mut shown, n, a))
                .collect(),
            Err(_) => ignored,
        };
        for m in &messages {
            if let GameMessage::Handled(name, args) | GameMessage::Send(name, args) = m {
                log::info!("ui: {name} {args:?}");
            }
        }
        let mut aims = Vec::new();
        {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            for (name, args) in &ignored {
                if let Some(a) = thedas_ui::input::aim(&sim, name, args) {
                    aims.push(a);
                }
            }
        }
        for (name, _) in ignored {
            // The stage closes a movie that asks for this itself; an ability to aim is the
            // app's to finish.
            if name != "GUI.HideFullScreenGUI"
                && name != thedas_ui::input::PERFORM_ABILITY
                && self.ignored.insert(name.clone())
            {
                log::warn!("ui: the simulation does not handle {name} yet");
            }
        }
        aims
    }

    /// A left click while playing reached the interface: a conversation line still playing is
    /// skipped to its end, and one with nothing to choose is continued (conversation.gfx's
    /// `ContinueButtonAction`). Provisional: the game's input bindings for the conversation's
    /// contexts are not traced; any click stands in for them.
    pub fn clicked(&mut self) {
        let Some(l) = self.line.as_mut() else {
            return;
        };
        if !l.played() {
            // Skip to the end of the line: its replies show (or it goes on) next frame.
            l.t = f64::INFINITY;
        } else if l.waits && !l.continued {
            l.continued = true;
            self.continue_conversation();
        }
    }

    /// conversation.gfx's continue action (it sends `Conversation.SelectResponse 0` when the
    /// line has nothing to choose).
    fn continue_conversation(&mut self) {
        if let Some(m) = self
            .stage
            .movies
            .iter_mut()
            .find(|m| m.name == "conversation.gfx")
        {
            m.call("_global.Conversation.ContinueButtonAction", &[]);
        }
    }

    /// Whether the party's portraits are due to be drawn: the party (`party`) changed, or they
    /// are [`PORTRAITS_EVERY_MS`] old. `dt_ms` is the time since the last call.
    pub fn wants_portraits(&mut self, party: Vec<u32>, dt_ms: f64) -> bool {
        self.portraits.due(party, dt_ms)
    }

    /// Hands the movies freshly drawn portraits (RGBA8, [`PORTRAIT_SIZE`] square, by member).
    pub fn set_portraits(&mut self, shots: Vec<(u32, Vec<u8>)>) {
        let old = self
            .shown
            .lock()
            .map(|s| s.portraits.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for name in old {
            self.stage.set_image(&name, None);
        }
        self.portraits.drawn += 1;
        let made = self.portraits.drawn;
        let mut names = std::collections::BTreeMap::new();
        for (id, pixels) in shots {
            let name = PortraitClock::name(id, made);
            let img = Rgba {
                width: PORTRAIT_SIZE,
                height: PORTRAIT_SIZE,
                pixels,
            };
            self.stage.set_image(&name, Some(&img));
            names.insert(id, name);
        }
        if let Ok(mut shown) = self.shown.lock() {
            shown.portraits = names;
        }
    }

    /// Plays the cutscene the simulation holds: its lines show as subtitles
    /// (`Cutscene.SubtitleText`, which cutscene.gfx shows) when they are said, and once it
    /// has run its length the simulation finishes it. Provisional: nothing is staged (no
    /// camera, animation or voice), so the world stands still behind the subtitles.
    fn play_cutscene(&mut self, sim: &Arc<Mutex<Sim>>, dt_ms: f64) {
        let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
        let Some(c) = sim.world.cutscene.clone() else {
            self.cutscene = None;
            return;
        };
        let t = match self.cutscene.take() {
            Some((name, t)) if name == c.name => t + dt_ms,
            _ => 0.0,
        };
        let text = c
            .line_at((t / 1000.0) as f32)
            .and_then(|l| sim.string(l.strref))
            .unwrap_or_default();
        self.stage
            .bridge
            .set_value("Cutscene.SubtitleText", thedas_ui::UiValue::String(text));
        if t >= c.seconds as f64 * 1000.0 {
            sim.finish_cutscene();
            sim.tick(0);
            self.stage.bridge.set_value(
                "Cutscene.SubtitleText",
                thedas_ui::UiValue::String(String::new()),
            );
            return;
        }
        self.cutscene = Some((c.name, t));
    }

    /// Skips the rest of the cutscene playing (the player clicked).
    pub fn skip_cutscene(&mut self) {
        if let Some((_, t)) = self.cutscene.as_mut() {
            *t = f64::INFINITY;
        }
    }

    /// The level being played (its layout and resources): the movies may load from them, and
    /// the minimap shows its picture. Does nothing when it is the same level.
    pub fn set_level(&mut self, layout: Option<&str>, vfs: Option<&Arc<Vfs>>) {
        if self.level.as_deref() == layout {
            return;
        }
        self.level = layout.map(str::to_string);
        self.stage.set_level_resources(vfs.cloned());
        let minimap = match (layout, vfs) {
            (Some(l), Some(v)) => thedas_ui::minimap::MinimapInfo::read(v, l),
            _ => None,
        };
        if let Ok(mut shown) = self.shown.lock() {
            shown.minimap = minimap;
        }
    }

    /// Sends the minimap where the camera and the area's creatures are (the engine writes
    /// `SharedLibrary.SharedMap`'s batch arrays: the camera first, then each creature's
    /// place on the map picture, in its pixels, and facing, three numbers each, and flags),
    /// then has it redraw. Provisional: every living creature in the area is shown, all
    /// within the map's radius.
    pub fn update_minimap(&mut self, sim: &Arc<Mutex<Sim>>, camera: &thedas_render::Camera) {
        self.minimap_frame += 1;
        if self.minimap_frame < MINIMAP_EVERY {
            return;
        }
        self.minimap_frame = 0;
        let Some(info) = self.shown.lock().ok().and_then(|s| s.minimap.clone()) else {
            return;
        };
        let Some(m) = self
            .stage
            .movies
            .iter_mut()
            .find(|m| m.name == "minimap.gfx")
        else {
            return;
        };
        let num = |x: f64| thedas_ui::UiValue::Number(x);
        // The camera's facing, as creatures' (degrees, 0 looking along +Y).
        let mut ids = vec![num(0.0)];
        let at = info.to_map(camera.position.x, camera.position.y);
        let mut places = vec![
            num(at[0] as f64),
            num(at[1] as f64),
            num(camera.yaw.to_degrees() as f64 - 90.0),
        ];
        let mut flags = vec![num(0.0)];
        {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            let area = sim.world.area;
            let party = thedas_sim::engine::party_of(&sim);
            let (hero, controlled) = (sim.world.hero, sim.world.controlled());
            for o in sim.world.objects.values() {
                if o.kind != thedas_sim::world::ObjectKind::Creature
                    || !o.active
                    || o.dead
                    || o.area.is_none()
                    || o.area != area
                {
                    continue;
                }
                let mut f = MAP_WITHIN_RADIUS;
                if party.contains(&o.id) {
                    f |= MAP_IN_PARTY;
                }
                if Some(o.id) == controlled {
                    f |= MAP_PLAYER;
                }
                if hero.is_some_and(|h| sim.hostile(h, o.id)) {
                    f |= MAP_HOSTILE;
                }
                ids.push(num(o.id as f64));
                let at = info.to_map(o.position[0], o.position[1]);
                places.extend([num(at[0] as f64), num(at[1] as f64), num(o.facing as f64)]);
                flags.push(num(f as f64));
            }
        }
        let map = "_global.SharedLibrary.SharedMap";
        m.set(
            &format!("{map}.BatchCreatureIDs"),
            thedas_ui::UiValue::List(ids),
        );
        m.set(
            &format!("{map}.BatchCreaturePositionsOrientations"),
            thedas_ui::UiValue::List(places),
        );
        m.set(
            &format!("{map}.BatchCreatureFlags"),
            thedas_ui::UiValue::List(flags),
        );
        // The map picture's clip is authored fully transparent and nothing in the movie
        // raises it once the picture has loaded. Provisional: the game shows it, so it is
        // shown here.
        m.set(
            "_global.MiniMap.Instance.m_mcMapImage._alpha",
            thedas_ui::UiValue::Number(100.0),
        );
        m.call("_global.MiniMap.Instance.UpdateMinimap", &[]);
    }

    /// Renders every movie into its texture (the window does this as it composites).
    pub fn render_movies(&mut self) {
        for m in &mut self.stage.movies {
            m.render();
        }
    }

    /// Whether a movie drew at window pixel (`x`, `y`) in its last rendered frame: a click
    /// there is the interface's, not the world's. Provisional: the game hit-tests the movies'
    /// shapes; an opaque pixel stands in for that.
    pub fn covers(&self, x: f64, y: f64) -> bool {
        let textures: Vec<wgpu::Texture> = self
            .stage
            .movies
            .iter()
            .filter_map(|m| m.texture())
            .filter(|t| x >= 0.0 && y >= 0.0 && (x as u32) < t.width() && (y as u32) < t.height())
            .collect();
        if textures.is_empty() {
            return false;
        }
        const ROW: u64 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui probe"),
            size: ROW * textures.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ui probe"),
            });
        for (i, t) in textures.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: t,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: x as u32,
                        y: y as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: ROW * i as u64,
                        bytes_per_row: Some(ROW as u32),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        if self
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .is_err()
            || !matches!(rx.recv(), Ok(Ok(())))
        {
            return false;
        }
        let Ok(data) = buffer.slice(..).get_mapped_range() else {
            return false;
        };
        // RGBA or BGRA: alpha is the fourth byte either way.
        (0..textures.len()).any(|i| {
            data.get(i * ROW as usize + 3)
                .is_some_and(|&a| a >= COVER_ALPHA)
        })
    }

    pub fn tick(&mut self, dt_ms: f64) -> Vec<GameMessage> {
        self.stage.tick(dt_ms)
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.stage.resize(size);
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.stage.mouse_move(x, y);
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        self.stage.mouse_button(x, y, down);
    }

    /// A click the movies see as Scaleform mouse `button` (a right click, a double click).
    pub fn click_as(&mut self, button: f64, x: f64, y: f64) {
        self.stage.click_as(button, x, y);
    }

    /// Renders every movie into its texture and draws them over `target`, in order.
    pub fn composite(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        if self.stage.movies.is_empty() {
            return;
        }
        let comp = self
            .composite
            .get_or_insert_with(|| Composite::new(&self.device, format));
        let mut groups = Vec::new();
        for m in &mut self.stage.movies {
            m.render();
            let Some(tex) = m.texture() else { continue };
            let view = tex.create_view(&Default::default());
            groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui movie"),
                layout: &comp.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&comp.sampler),
                    },
                ],
            }));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&comp.pipeline);
        for g in &groups {
            pass.set_bind_group(0, g, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Logs what the movies asked for and did not get: failed loads and unanswered values.
    pub fn log_gaps(&self) {
        for (movie, url) in self.stage.failed_loads() {
            log::warn!("ui: {movie}: load of {url} failed");
        }
        let gaps = self.stage.bridge.unanswered();
        log::debug!("ui: unanswered: {:?}", gaps.keys().collect::<Vec<_>>());
        if !gaps.is_empty() {
            log::info!(
                "ui: {} engine values unanswered: {:?}",
                gaps.len(),
                gaps.keys().take(40).collect::<Vec<_>>()
            );
        }
    }

    pub fn capture_over(&mut self, base: &mut Rgba) -> Result<()> {
        Ok(self.stage.capture_over(base)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_play_for_a_reading_time() {
        assert_eq!(line_ms(""), 100.0);
        assert_eq!(line_ms("  "), 100.0);
        assert!(line_ms("Short.") < line_ms("A much longer line that takes a while to read."));
        assert_eq!(line_ms(&"x".repeat(1000)), 12_000.0);
    }

    #[test]
    fn portraits_are_drawn_when_the_party_changes_and_every_few_seconds() {
        let mut clock = PortraitClock::default();
        assert!(clock.due(vec![1], 16.0));
        assert!(!clock.due(vec![1], 16.0));
        assert!(clock.due(vec![1, 2], 16.0));
        assert!(!clock.due(vec![1, 2], PORTRAITS_EVERY_MS - 1.0));
        assert!(clock.due(vec![1, 2], 1.0));
        assert_ne!(PortraitClock::name(1, 1), PortraitClock::name(1, 2));
    }
}
