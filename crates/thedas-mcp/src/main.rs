//! `thedas-mcp`: a Model Context Protocol server (stdio) for driving the OpenThedas
//! simulation: load a module and area, look at objects, step time, walk the hero around, use
//! things, talk to people, pick dialog replies, poke plot flags and locals, call engine
//! functions and scripts, and snapshot/restore the world. The `ui_*` tools play the game's
//! Scaleform interface on a headless GPU, bound to the simulation, with screenshots.
//!
//! Register it with Claude Code:
//!
//! ```sh
//! claude mcp add thedas -e THEDAS_INSTALL=/path/to/Dragon\ Age -- /path/to/thedas-mcp
//! ```

mod ui;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use rmcp::model::{CallToolResult, ContentBlock};
use thedas_ui::UiValue;
use thedas_ui::protocol::GameMessage;
use thedas_vfs::Vfs;

use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars::JsonSchema;
use rmcp::{ServiceExt, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value as J, json};
use thedas_script::ldf::Type;
use thedas_script::vm::{OBJECT_INVALID, ObjectId, Value};
use thedas_sim::Sim;
use thedas_sim::dialog::Conversation;
use thedas_sim::session::{SessionOptions, install_from_env, open};
use thedas_sim::trace::TraceEntry;
use thedas_sim::world::{Event, ObjectKind, World};

type Snapshot = (World, Option<Conversation>);

#[derive(Default)]
struct State {
    sim: Option<Sim>,
    slots: BTreeMap<String, Snapshot>,
    /// Trace entries already returned by `get_trace`.
    trace_cursor: usize,
    /// Resources for the interface when no simulation is loaded.
    ui_vfs: Option<Arc<Vfs>>,
}

/// Engine values bound for the interface.
type UiValues = Vec<(String, UiValue)>;

#[derive(Clone)]
struct Server {
    state: Arc<Mutex<State>>,
    ui: ui::UiThread,
}

fn ui_json(v: &UiValue) -> J {
    match v {
        UiValue::Undefined | UiValue::Null => J::Null,
        UiValue::Bool(b) => json!(b),
        UiValue::Number(n) => json!(n),
        UiValue::String(s) => json!(s),
        UiValue::List(l) => J::Array(l.iter().map(ui_json).collect()),
        UiValue::Object(o) => J::Object(o.iter().map(|(k, v)| (k.clone(), ui_json(v))).collect()),
    }
}

fn json_ui(v: &J) -> UiValue {
    match v {
        J::Null => UiValue::Null,
        J::Bool(b) => UiValue::Bool(*b),
        J::Number(n) => UiValue::Number(n.as_f64().unwrap_or(0.0)),
        J::String(s) => UiValue::String(s.clone()),
        J::Array(a) => UiValue::List(a.iter().map(json_ui).collect()),
        J::Object(o) => UiValue::Object(o.iter().map(|(k, v)| (k.clone(), json_ui(v))).collect()),
    }
}

fn messages_json(m: Vec<GameMessage>) -> J {
    J::Array(
        m.into_iter()
            .map(|m| match m {
                GameMessage::Send(name, args) => json!({ "send": name, "args": args }),
                GameMessage::Other(c) => json!({
                    "movie": c.movie, "kind": format!("{:?}", c.kind), "name": c.name,
                    "args": c.args.iter().map(ui_json).collect::<Vec<_>>(),
                }),
            })
            .collect(),
    )
}

#[derive(Deserialize, JsonSchema)]
struct UiOpenArgs {
    /// `menu` (in-game menu), `hud` (portraits, quick bar, minimap), `startmenu2` (title
    /// screen), or comma-separated movie names (`conversation,floatylayer`).
    movies: String,
    /// Stage width in pixels (default 1280).
    width: Option<u32>,
    /// Stage height in pixels (default 720).
    height: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct UiMovieArg {
    /// Movie name, e.g. `mainmenu.gfx`.
    movie: String,
}

#[derive(Deserialize, JsonSchema)]
struct UiStepArgs {
    /// Movie frames to run at 30 fps (default 1).
    frames: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
struct UiPointArgs {
    /// X in stage pixels (screenshot coordinates).
    x: f64,
    /// Y in stage pixels.
    y: f64,
}

#[derive(Deserialize, JsonSchema)]
struct UiInvokeArgs {
    /// Function path, e.g. `_global.GUISystem.FinishSceneFromGame`.
    path: String,
    /// Arguments (JSON values).
    #[serde(default)]
    args: Vec<J>,
    /// Movie to call into (default: the topmost).
    movie: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct UiValuesArgs {
    /// Only keys starting with this prefix.
    prefix: Option<String>,
}

fn err(msg: impl std::fmt::Display) -> String {
    format!("error: {msg}")
}

fn pretty(v: J) -> String {
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

#[derive(Deserialize, JsonSchema)]
struct LoadArgs {
    /// Install root (the folder with packages/ and modules/); defaults to $THEDAS_INSTALL.
    install: Option<String>,
    /// Module folder under modules/ (default "single player").
    module: Option<String>,
    /// Area resource, e.g. "cir110ar_inn" (the Spoiled Princess). Default: the module's start area.
    area: Option<String>,
    /// Waypoint tag to place the hero at.
    waypoint: Option<String>,
    /// Random seed (default 1).
    seed: Option<u64>,
    /// Character generation when a new game starts it: "quick" (default: the chargen
    /// script's pre-made character and origin) or "manual" (send events with chargen_event).
    chargen: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ChargenArgs {
    /// Event name without EVENT_TYPE_: CHARGEN_SELECT_GENDER, CHARGEN_SELECT_RACE,
    /// CHARGEN_SELECT_CLASS, CHARGEN_SELECT_BACKGROUND, CHARGEN_SELECT_NAME,
    /// CHARGEN_ASSIGN_ATTRIBUTES, CHARGEN_ASSIGN_ABILITIES, CHARGEN_END, ...
    event: String,
    /// Integer parameters (e.g. [race, 0]; CHARGEN_END takes [mode, quick start]).
    #[serde(default)]
    ints: Vec<i32>,
    /// String parameters (e.g. a name).
    #[serde(default)]
    strings: Vec<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ListArgs {
    /// Only objects whose tag contains this (case-insensitive).
    tag: Option<String>,
    /// Only this kind: creature, placeable, trigger, waypoint, store, sound, stage, item, area, module.
    kind: Option<String>,
    /// Include objects outside the current area.
    all_areas: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct ObjectArg {
    /// Object id (number) or tag.
    object: String,
}

#[derive(Deserialize, JsonSchema)]
struct StepArgs {
    /// Number of ticks (default 1).
    ticks: Option<u64>,
    /// Milliseconds per tick (default 100).
    tick_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
struct TraceArgs {
    /// Only entries of these kinds (event_delivered, script_start, script_end, unimplemented,
    /// log, missing, plot_flag, command, dialog, object_created, event_queued).
    kinds: Option<Vec<String>>,
    /// Maximum entries to return (default 100).
    limit: Option<usize>,
    /// Start from the beginning instead of after the last call.
    from_start: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct CallArgs {
    /// Engine function name from script.ldf, e.g. "GetObjectByTag".
    name: String,
    /// Arguments in declaration order: numbers, strings, and for objects an id or a tag.
    args: Option<Vec<J>>,
    /// OBJECT_SELF for the call (id or tag; default the hero).
    caller: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct ScriptArgs {
    /// Script resource name, e.g. "cir110ar_inn".
    script: String,
    /// OBJECT_SELF (id or tag; default the hero).
    caller: Option<String>,
    /// Run it as the handler of this event type (EVENT_TYPE_* name without the prefix, e.g.
    /// "SPAWN", or a number).
    event: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct MoveArgs {
    /// Move to this object (id or tag)...
    to_object: Option<String>,
    /// ...or to these coordinates in the current area.
    x: Option<f32>,
    y: Option<f32>,
    z: Option<f32>,
}

#[derive(Deserialize, JsonSchema)]
struct ReplyArgs {
    /// Index into the current replies (0-based).
    reply: usize,
}

#[derive(Deserialize, JsonSchema)]
struct PlotArgs {
    /// Plot GUID or resource name (e.g. "plt_cir000pt_talked_to").
    plot: String,
    flag: i32,
    /// Set the flag to this value (omit to read).
    value: Option<bool>,
    /// Run the plot's script (default true).
    call_script: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
struct LocalArgs {
    /// Object id or tag.
    object: String,
    name: String,
    /// int, float, string or object.
    kind: Option<String>,
    /// Set to this value (omit to read).
    value: Option<J>,
}

#[derive(Deserialize, JsonSchema)]
struct AreaArgs {
    area: String,
    waypoint: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
struct SlotArgs {
    /// Slot name (default "quick").
    slot: Option<String>,
    /// Also write to (save_state) or read from (load_state) this JSON file. Loading needs the
    /// same module loaded.
    path: Option<String>,
}

fn kind_name(k: ObjectKind) -> String {
    format!("{k:?}").to_ascii_lowercase()
}

fn resolve(sim: &Sim, s: &str) -> Option<ObjectId> {
    let s = s.trim();
    if let Ok(id) = s.parse::<u32>() {
        return sim.world.get(id).map(|o| o.id);
    }
    if s.eq_ignore_ascii_case("hero") || s.eq_ignore_ascii_case("player") {
        return sim.world.hero;
    }
    sim.world.by_tag(s).next().map(|o| o.id)
}

fn summary(sim: &Sim, id: ObjectId) -> J {
    match sim.world.get(id) {
        Some(o) => json!({
            "id": o.id, "kind": kind_name(o.kind), "tag": o.tag, "template": o.template,
            "position": o.position, "facing": o.facing, "active": o.active,
            "dialog": o.dialog, "script": o.script,
            "name": o.name_strref.and_then(|s| sim.string(s)),
        }),
        None => J::Null,
    }
}

fn value_json(v: &Value) -> J {
    match v {
        Value::Int(x) => json!(x),
        Value::Float(x) => json!(x),
        Value::String(s) => json!(s),
        Value::Resource(s) => json!({ "resource": s }),
        Value::Object(o) => json!({ "object": o }),
        Value::Vector(v) => json!(v),
        Value::Engine { ty, handle } => json!({ "engine": ty, "handle": handle }),
        Value::Array { items, .. } => J::Array(items.iter().map(value_json).collect()),
    }
}

fn arg_value(sim: &Sim, ty: &Type, j: &J) -> Value {
    match ty {
        Type::Int => Value::Int(j.as_i64().unwrap_or(0) as i32),
        Type::Float => Value::Float(j.as_f64().unwrap_or(0.0) as f32),
        Type::String | Type::Any => Value::String(
            j.as_str()
                .map(str::to_string)
                .unwrap_or_else(|| j.to_string()),
        ),
        Type::Resource => Value::Resource(j.as_str().unwrap_or_default().to_string()),
        Type::Object => Value::Object(match j {
            J::Number(n) => n.as_u64().unwrap_or(OBJECT_INVALID as u64) as u32,
            J::String(s) => resolve(sim, s).unwrap_or(OBJECT_INVALID),
            _ => OBJECT_INVALID,
        }),
        Type::Vector => {
            let a: Vec<f32> = j
                .as_array()
                .map(|a| a.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect())
                .unwrap_or_default();
            Value::Vector([
                a.first().copied().unwrap_or(0.0),
                a.get(1).copied().unwrap_or(0.0),
                a.get(2).copied().unwrap_or(0.0),
            ])
        }
        other => Value::default_for(other),
    }
}

fn dialog_json(sim: &Sim) -> J {
    match sim.dialog_state() {
        Some(st) => json!({
            "dialog": st.dialog, "owner": summary(sim, st.owner), "speaker": st.speaker,
            "line": st.line, "ended": st.ended,
            "replies": st.replies.iter().enumerate().map(|(i, (_, t))| json!({"index": i, "text": t})).collect::<Vec<_>>(),
        }),
        None => J::Null,
    }
}

impl Server {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            ui: ui::UiThread::spawn(),
        }
    }

    /// Resources and bound engine values for the interface: the simulation's when one is
    /// loaded, otherwise the install's global resources with no values.
    fn ui_inputs(&self) -> Result<(Arc<Vfs>, UiValues), String> {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(sim) = &st.sim {
            let values = thedas_ui::binding::sim_values(sim).into_iter().collect();
            return Ok((sim.vfs.clone(), values));
        }
        if st.ui_vfs.is_none() {
            let root = install_from_env().ok_or("no install: set THEDAS_INSTALL or call load")?;
            let install = thedas_vfs::DaoInstall::open(&root).map_err(|e| e.to_string())?;
            let vfs = install
                .global_vfs(thedas_vfs::install::TextureQuality::High)
                .map_err(|e| e.to_string())?;
            st.ui_vfs = Some(Arc::new(vfs));
        }
        Ok((st.ui_vfs.clone().ok_or("no resources")?, Vec::new()))
    }

    /// Binds the current engine values, runs `frames`, and returns the engine messages.
    async fn ui_frames(&self, frames: u32) -> Result<J, String> {
        let (_, values) = self.ui_inputs()?;
        let (mut out, messages) = self
            .ui
            .run(move |w| {
                w.step(frames, &values);
                let sounds = w
                    .stage
                    .as_ref()
                    .map(|s| s.bridge.drain_sounds())
                    .unwrap_or_default();
                let messages = std::mem::take(&mut w.messages);
                (
                    json!({
                        "messages": messages_json(messages.clone()),
                        "sounds": sounds,
                        "contexts": w.stage.as_ref().map(|s| s.bridge.contexts()),
                    }),
                    messages,
                )
            })
            .await?;
        let applied = self.apply_ui_messages(&messages);
        if !applied.is_empty() {
            out["applied"] = J::Array(applied);
        }
        Ok(out)
    }

    /// Acts on what the interface asked the game to do, as player input: picking a
    /// conversation response (`Conversation.SelectResponse N`).
    fn apply_ui_messages(&self, messages: &[GameMessage]) -> Vec<J> {
        let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let Some(sim) = st.sim.as_mut() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for m in messages {
            if let GameMessage::Send(name, args) = m {
                if name == "Conversation.SelectResponse" {
                    let n = args
                        .first()
                        .and_then(|a| a.parse::<usize>().ok())
                        .unwrap_or(0);
                    sim.choose_reply(n);
                    sim.tick(0);
                    out.push(json!({ "selected_response": n, "conversation": dialog_json(sim) }));
                }
            }
        }
        out
    }

    fn with_sim<F: FnOnce(&mut State) -> Result<J, String>>(&self, f: F) -> String {
        let mut st = match self.state.lock() {
            Ok(s) => s,
            Err(p) => p.into_inner(),
        };
        if st.sim.is_none() {
            return err("no module loaded; call load first");
        }
        match f(&mut st) {
            Ok(v) => pretty(v),
            Err(e) => err(e),
        }
    }
}

#[tool_router]
impl Server {
    #[tool(
        description = "Open interface movies (the game's Scaleform UI, played headless and bound to the simulation if one is loaded). Use ui_screenshot to see them, ui_click to press things."
    )]
    async fn ui_open(&self, Parameters(a): Parameters<UiOpenArgs>) -> String {
        let (vfs, values) = match self.ui_inputs() {
            Ok(x) => x,
            Err(e) => return err(e),
        };
        let size = (
            a.width.unwrap_or(1280).clamp(64, 4096),
            a.height.unwrap_or(720).clamp(64, 4096),
        );
        let names = thedas_ui::stage::movie_names(&a.movies);
        let r = self
            .ui
            .run(move |w| -> Result<J, String> {
                let stage = w.stage(&vfs, size)?;
                stage.bridge.set_values(values.clone());
                let mut failed = Vec::new();
                for n in &names {
                    stage.close(n);
                    if let Err(e) = stage.open(n) {
                        failed.push(format!("{n}: {e}"));
                    }
                }
                w.step(2, &values);
                let stage = w.stage.as_ref().ok_or("no stage")?;
                Ok(json!({
                    "open": stage.movies.iter().map(|m| m.name.clone()).collect::<Vec<_>>(),
                    "failed": failed,
                    "size": size,
                    "messages": messages_json(std::mem::take(&mut w.messages)),
                }))
            })
            .await;
        match r {
            Ok(Ok(v)) => pretty(v),
            Ok(Err(e)) | Err(e) => err(e),
        }
    }

    #[tool(description = "Close one interface movie.")]
    async fn ui_close(&self, Parameters(a): Parameters<UiMovieArg>) -> String {
        let movie = a.movie;
        match self
            .ui
            .run(move |w| w.stage.as_mut().is_some_and(|s| s.close(&movie)))
            .await
        {
            Ok(closed) => pretty(json!({ "closed": closed })),
            Err(e) => err(e),
        }
    }

    #[tool(
        description = "Run interface frames (30 fps) with engine values re-bound from the simulation. Returns the messages the movies sent to the engine, GUI sounds and the context stack."
    )]
    async fn ui_step(&self, Parameters(a): Parameters<UiStepArgs>) -> String {
        match self.ui_frames(a.frames.unwrap_or(1).min(600)).await {
            Ok(v) => pretty(v),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Move the mouse over the interface (stage pixels), then run a frame.")]
    async fn ui_hover(&self, Parameters(a): Parameters<UiPointArgs>) -> String {
        let (x, y) = (a.x, a.y);
        if let Err(e) = self
            .ui
            .run(move |w| {
                if let Some(s) = w.stage.as_mut() {
                    s.mouse_move(x, y);
                }
            })
            .await
        {
            return err(e);
        }
        match self.ui_frames(1).await {
            Ok(v) => pretty(v),
            Err(e) => err(e),
        }
    }

    #[tool(
        description = "Left-click the interface at stage pixels (as in ui_screenshot), then run a few frames. Returns what the movies sent to the engine."
    )]
    async fn ui_click(&self, Parameters(a): Parameters<UiPointArgs>) -> String {
        let (x, y) = (a.x, a.y);
        let (_, values) = match self.ui_inputs() {
            Ok(x) => x,
            Err(e) => return err(e),
        };
        let r = self
            .ui
            .run(move |w| {
                if let Some(s) = w.stage.as_mut() {
                    s.mouse_move(x, y);
                }
                w.step(1, &values);
                if let Some(s) = w.stage.as_mut() {
                    s.mouse_button(x, y, true);
                }
                w.step(1, &values);
                if let Some(s) = w.stage.as_mut() {
                    s.mouse_button(x, y, false);
                }
                w.step(3, &values);
            })
            .await;
        if let Err(e) = r {
            return err(e);
        }
        match self.ui_frames(1).await {
            Ok(v) => pretty(v),
            Err(e) => err(e),
        }
    }

    #[tool(description = "Screenshot of the open interface movies (PNG, over black).")]
    async fn ui_screenshot(&self) -> CallToolResult {
        match self.ui.run(|w| w.screenshot()).await {
            Ok(Ok(png)) => CallToolResult::success(vec![ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(png),
                "image/png",
            )]),
            Ok(Err(e)) | Err(e) => CallToolResult::error(vec![ContentBlock::text(err(e))]),
        }
    }

    #[tool(
        description = "Call a movie function by path, as the game engine does (e.g. _global.GUISystem.FinishSceneFromGame)."
    )]
    async fn ui_invoke(&self, Parameters(a): Parameters<UiInvokeArgs>) -> String {
        let args: Vec<UiValue> = a.args.iter().map(json_ui).collect();
        let (path, movie) = (a.path, a.movie);
        let r = self
            .ui
            .run(move |w| -> Result<J, String> {
                let stage = w.stage.as_mut().ok_or("no movies open")?;
                let m = match &movie {
                    Some(n) => stage.movies.iter_mut().find(|m| m.name == *n),
                    None => stage.movies.last_mut(),
                }
                .ok_or("no such movie")?;
                Ok(ui_json(&m.call(&path, &args)))
            })
            .await;
        match r {
            Ok(Ok(v)) => pretty(json!({ "result": v })),
            Ok(Err(e)) | Err(e) => err(e),
        }
    }

    #[tool(
        description = "Engine values the interface has read or been given, and the names it asked for that nothing answers."
    )]
    async fn ui_values(&self, Parameters(a): Parameters<UiValuesArgs>) -> String {
        let prefix = a.prefix.unwrap_or_default();
        let r = self
            .ui
            .run(move |w| -> Result<J, String> {
                let stage = w.stage.as_ref().ok_or("no movies open")?;
                let values: serde_json::Map<String, J> = stage
                    .bridge
                    .values()
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(k, v)| (k.clone(), ui_json(v)))
                    .collect();
                let unanswered: Vec<String> = stage
                    .bridge
                    .unanswered()
                    .into_keys()
                    .filter(|k| k.starts_with(&prefix))
                    .collect();
                Ok(json!({
                    "values": values,
                    "unanswered": unanswered,
                    "failed_loads": stage.failed_loads(),
                    "log": stage.log.iter().rev().take(40).collect::<Vec<_>>(),
                }))
            })
            .await;
        match r {
            Ok(Ok(v)) => pretty(v),
            Ok(Err(e)) | Err(e) => err(e),
        }
    }

    #[tool(
        description = "Load a module and area into a fresh simulation (replaces any running one). Returns the area, the hero and the objects."
    )]
    async fn load(&self, Parameters(a): Parameters<LoadArgs>) -> String {
        let install = match a
            .install
            .map(std::path::PathBuf::from)
            .or_else(install_from_env)
        {
            Some(p) => p,
            None => return err("no install: pass install or set THEDAS_INSTALL"),
        };
        let opts = SessionOptions {
            install,
            module: a.module.unwrap_or_else(|| "single player".into()),
            area: a.area,
            waypoint: a.waypoint,
            seed: a.seed.unwrap_or(1),
            keep_trace: true,
        };
        match open(&opts) {
            Ok(mut sim) => {
                if a.chargen.as_deref() == Some("manual") {
                    sim.chargen_policy = thedas_sim::sim::ChargenPolicy::Manual;
                }
                sim.tick(0);
                let area = sim.world.area;
                let objects: Vec<J> = sim
                    .world
                    .objects
                    .values()
                    .filter(|o| o.area == area && o.area.is_some())
                    .map(|o| summary(&sim, o.id))
                    .collect();
                let out = json!({
                    "area": area.map(|a| summary(&sim, a)),
                    "hero": sim.world.hero.map(|h| summary(&sim, h)),
                    "objects": objects,
                    "scripts_run": sim.world.trace.scripts_run,
                });
                let mut st = self.state.lock().unwrap_or_else(|p| p.into_inner());
                st.sim = Some(sim);
                st.trace_cursor = 0;
                pretty(out)
            }
            Err(e) => err(e),
        }
    }

    #[tool(description = "Current time, area, hero position, conversation and counters.")]
    async fn status(&self) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_ref().ok_or("no sim")?;
            let tr = &sim.world.trace;
            let mut un: Vec<(&String, &u64)> = tr.unimplemented.iter().collect();
            un.sort_by(|a, b| b.1.cmp(a.1));
            Ok(json!({
                "time_ms": sim.world.time_ms,
                "area": sim.world.area.map(|a| summary(sim, a)),
                "hero": sim.world.hero.map(|h| summary(sim, h)),
                "game_mode": sim.world.game_mode,
                "objects": sim.world.objects.len(),
                "queued_events": sim.world.queue.len(),
                "scripts_run": tr.scripts_run,
                "script_errors": tr.script_errors,
                "unimplemented_top": un.iter().take(40).map(|(f, n)| json!({"function": f, "calls": n})).collect::<Vec<_>>(),
                "conversation": dialog_json(sim),
            }))
        })
    }

    #[tool(description = "List objects, optionally filtered by tag substring and kind.")]
    async fn list_objects(&self, Parameters(a): Parameters<ListArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_ref().ok_or("no sim")?;
            let tag = a.tag.unwrap_or_default().to_ascii_lowercase();
            let kind = a.kind.unwrap_or_default().to_ascii_lowercase();
            let area = sim.world.area;
            let v: Vec<J> = sim
                .world
                .objects
                .values()
                .filter(|o| a.all_areas.unwrap_or(false) || o.area == area || Some(o.id) == area)
                .filter(|o| tag.is_empty() || o.tag.to_ascii_lowercase().contains(&tag))
                .filter(|o| kind.is_empty() || kind_name(o.kind) == kind)
                .map(|o| summary(sim, o.id))
                .collect();
            Ok(json!(v))
        })
    }

    #[tool(
        description = "Everything about one object: locals, properties, effects, commands, inventory."
    )]
    async fn inspect_object(&self, Parameters(a): Parameters<ObjectArg>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_ref().ok_or("no sim")?;
            let id = resolve(sim, &a.object).ok_or("no such object")?;
            let o = sim.world.get(id).ok_or("no such object")?;
            let mut v = serde_json::to_value(o).map_err(|e| e.to_string())?;
            v["name"] = json!(o.name_strref.and_then(|s| sim.string(s)));
            Ok(v)
        })
    }

    #[tool(
        description = "Advance time by ticks (default 1 x 100 ms), delivering events and running commands. Returns new trace entries."
    )]
    async fn step(&self, Parameters(a): Parameters<StepArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let before = sim.world.trace.entries.len();
            for _ in 0..a.ticks.unwrap_or(1).min(100_000) {
                sim.tick(a.tick_ms.unwrap_or(100));
            }
            let new: Vec<&TraceEntry> = sim.world.trace.entries[before..].iter().take(200).collect();
            Ok(json!({ "time_ms": sim.world.time_ms, "new_entries": sim.world.trace.entries.len() - before, "entries": new }))
        })
    }

    #[tool(
        description = "Trace entries since the last call (or from the start), optionally filtered by kind."
    )]
    async fn get_trace(&self, Parameters(a): Parameters<TraceArgs>) -> String {
        self.with_sim(|st| {
            let from = if a.from_start.unwrap_or(false) {
                0
            } else {
                st.trace_cursor
            };
            let sim = st.sim.as_ref().ok_or("no sim")?;
            let entries = &sim.world.trace.entries;
            let kinds = a.kinds.unwrap_or_default();
            let mut out = Vec::new();
            let mut last = from;
            for (i, e) in entries.iter().enumerate().skip(from) {
                let v = serde_json::to_value(e).map_err(|e| e.to_string())?;
                let k = v["kind"].as_str().unwrap_or("").to_string();
                last = i + 1;
                if kinds.is_empty() || kinds.contains(&k) {
                    out.push(v);
                    if out.len() >= a.limit.unwrap_or(100) {
                        break;
                    }
                }
            }
            st.trace_cursor = last;
            Ok(json!({ "entries": out, "remaining": entries.len() - last }))
        })
    }

    #[tool(
        description = "Call an engine function by name, as a script would. Arguments follow script.ldf's declaration order."
    )]
    async fn call_engine_function(&self, Parameters(a): Parameters<CallArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let f = sim
                .table
                .function_by_name(&a.name)
                .cloned()
                .ok_or_else(|| format!("no engine function {}", a.name))?;
            let caller = a
                .caller
                .as_deref()
                .and_then(|c| resolve(sim, c))
                .or(sim.world.hero)
                .unwrap_or(OBJECT_INVALID);
            let raw = a.args.unwrap_or_default();
            let args: Vec<Value> = f
                .params
                .iter()
                .enumerate()
                .map(|(i, p)| match raw.get(i) {
                    Some(j) => arg_value(sim, &p.ty, j),
                    None => Value::default_for(&p.ty),
                })
                .collect();
            let mut host = thedas_sim::sim::ScriptHost {
                sim,
                event: None,
                script: "mcp".into(),
            };
            let r = thedas_sim::engine::call(&mut host, caller, &f, &args);
            Ok(match r {
                None => json!({ "implemented": false }),
                Some(Ok(v)) => json!({ "implemented": true, "result": v.as_ref().map(value_json) }),
                Some(Err(e)) => json!({ "implemented": true, "error": e.to_string() }),
            })
        })
    }

    #[tool(
        description = "Run a compiled script on an object, optionally as an event handler. Returns its result and what it did."
    )]
    async fn run_script(&self, Parameters(a): Parameters<ScriptArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let caller = a
                .caller
                .as_deref()
                .and_then(|c| resolve(sim, c))
                .or(sim.world.hero)
                .unwrap_or(OBJECT_INVALID);
            let event = a.event.map(|e| {
                let ty = e
                    .parse::<i32>()
                    .unwrap_or_else(|_| sim.event_type(&e.to_ascii_uppercase()));
                Event {
                    ty,
                    creator: sim.world.hero,
                    target: Some(caller),
                    ..Default::default()
                }
            });
            let before = sim.world.trace.entries.len();
            let r = sim.run_script(&a.script, caller, event);
            sim.tick(0);
            let new: Vec<&TraceEntry> =
                sim.world.trace.entries[before..].iter().take(200).collect();
            Ok(json!({ "result": r.as_ref().map(value_json), "entries": new }))
        })
    }

    #[tool(
        description = "Move the hero to an object or to coordinates in the current area (instant)."
    )]
    async fn move_hero(&self, Parameters(a): Parameters<MoveArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let hero = sim.world.hero.ok_or("no hero")?;
            let target = match a.to_object.as_deref() {
                Some(t) => {
                    let id = resolve(sim, t).ok_or("no such object")?;
                    let o = sim.world.get(id).ok_or("no such object")?;
                    (o.position, o.area)
                }
                None => (
                    [a.x.unwrap_or(0.0), a.y.unwrap_or(0.0), a.z.unwrap_or(0.0)],
                    sim.world.area,
                ),
            };
            sim.move_object(hero, target.0, target.1);
            sim.tick(0);
            Ok(summary(sim, hero))
        })
    }

    #[tool(
        description = "Interact with an object as the hero: talk to a creature (starts its conversation) or use a placeable (sends it EVENT_TYPE_USE)."
    )]
    async fn interact(&self, Parameters(a): Parameters<ObjectArg>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let hero = sim.world.hero.ok_or("no hero")?;
            let id = resolve(sim, &a.object).ok_or("no such object")?;
            let kind = sim.world.get(id).map(|o| o.kind).ok_or("no such object")?;
            let before = sim.world.trace.entries.len();
            let dialog = sim.world.get(id).and_then(|o| o.dialog.clone());
            if kind == ObjectKind::Creature || dialog.is_some() {
                if !sim.start_conversation(hero, id, "") {
                    return Err("it has nothing to say".into());
                }
            } else {
                let ev = sim.new_event("USE", Some(hero));
                sim.deliver(id, ev, None);
            }
            sim.tick(0);
            let new: Vec<&TraceEntry> =
                sim.world.trace.entries[before..].iter().take(100).collect();
            Ok(json!({ "conversation": dialog_json(sim), "entries": new }))
        })
    }

    #[tool(
        description = "The current conversation: who is speaking, the line, and the replies you can pick."
    )]
    async fn dialog_state(&self) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_ref().ok_or("no sim")?;
            Ok(dialog_json(sim))
        })
    }

    #[tool(
        description = "Send a character-generation event for the creature in character generation (after load with chargen=manual), then run the events it causes."
    )]
    async fn chargen_event(&self, Parameters(a): Parameters<ChargenArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let event = a.event.trim_start_matches("EVENT_TYPE_").to_string();
            if !sim.chargen_event(&event, a.ints, a.strings) {
                return Err("no character generation in progress".into());
            }
            sim.tick(0);
            Ok(json!({
                "chargen_in_progress": sim.world.chargen.is_some(),
                "area": sim.world.area.map(|x| summary(sim, x)),
                "conversation": dialog_json(sim),
            }))
        })
    }

    #[tool(
        description = "Order the hero to attack a creature (as clicking an enemy does), then run time until the attack resolves. Returns the target's health and whether it died."
    )]
    async fn attack(&self, Parameters(a): Parameters<ObjectArg>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let hero = sim.world.hero.ok_or("no hero")?;
            let target = resolve(sim, &a.object).ok_or("no such object")?;
            sim.order_attack(hero, target);
            for _ in 0..30 {
                sim.tick(100);
                if sim.world.get(hero).is_none_or(|h| h.commands.is_empty()) {
                    break;
                }
            }
            let health = thedas_sim::sim::constant(&sim.table, "PROPERTY_DEPLETABLE_HEALTH");
            let hp = |id| {
                sim.world
                    .get(id)
                    .and_then(|o| o.properties.get(&health).copied())
            };
            Ok(json!({
                "target": summary(sim, target),
                "target_health": hp(target),
                "target_dead": sim.world.get(target).is_some_and(|o| o.dead),
                "hero_health": hp(hero),
                "hero_dead": sim.world.get(hero).is_some_and(|o| o.dead),
                "time_ms": sim.world.time_ms,
            }))
        })
    }

    #[tool(description = "Pick a reply in the current conversation by index.")]
    async fn choose_reply(&self, Parameters(a): Parameters<ReplyArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            if sim.dialog_state().is_none() {
                return Err("no conversation".into());
            }
            let before = sim.world.trace.entries.len();
            sim.choose_reply(a.reply);
            sim.tick(0);
            let effects: Vec<&TraceEntry> = sim.world.trace.entries[before..]
                .iter()
                .filter(|e| !matches!(e, TraceEntry::Dialog { .. }))
                .take(60)
                .collect();
            Ok(json!({ "conversation": dialog_json(sim), "effects": effects }))
        })
    }

    #[tool(description = "End the current conversation.")]
    async fn end_conversation(&self) -> String {
        self.with_sim(|st| {
            st.sim.as_mut().ok_or("no sim")?.end_conversation();
            Ok(json!("ended"))
        })
    }

    #[tool(description = "Read or set a plot flag.")]
    async fn plot_flag(&self, Parameters(a): Parameters<PlotArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let owner = sim.world.hero.unwrap_or(OBJECT_INVALID);
            let call = a.call_script.unwrap_or(true);
            if let Some(v) = a.value {
                sim.set_plot_flag(&a.plot, a.flag, v, call, owner);
                sim.tick(0);
            }
            let info = sim.plot_info(&a.plot);
            let value = sim.get_plot_flag(&a.plot, a.flag, call, owner);
            Ok(json!({
                "plot": info.as_ref().map(|p| p.resref.clone()),
                "guid": info.as_ref().map(|p| p.guid.clone()),
                "flag": a.flag,
                "flag_name": info.as_ref().and_then(|p| p.flags.get(&a.flag).cloned()),
                "value": value,
            }))
        })
    }

    #[tool(
        description = "Read or set a local variable on an object (kind: int, float, string, object)."
    )]
    async fn local(&self, Parameters(a): Parameters<LocalArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let id = resolve(sim, &a.object).ok_or("no such object")?;
            let kind = a.kind.unwrap_or_else(|| "int".into());
            let obj_val = match (&kind[..], &a.value) {
                ("object", Some(J::String(s))) => resolve(sim, s),
                ("object", Some(J::Number(n))) => n.as_u64().map(|x| x as u32),
                _ => None,
            };
            let o = sim.world.get_mut(id).ok_or("no such object")?;
            if let Some(v) = &a.value {
                match &kind[..] {
                    "int" => {
                        o.locals
                            .ints
                            .insert(a.name.clone(), v.as_i64().unwrap_or(0) as i32);
                    }
                    "float" => {
                        o.locals
                            .floats
                            .insert(a.name.clone(), v.as_f64().unwrap_or(0.0) as f32);
                    }
                    "string" => {
                        o.locals
                            .strings
                            .insert(a.name.clone(), v.as_str().unwrap_or_default().to_string());
                    }
                    "object" => {
                        o.locals
                            .objects
                            .insert(a.name.clone(), obj_val.unwrap_or(OBJECT_INVALID));
                    }
                    other => return Err(format!("unknown kind {other}")),
                }
            }
            Ok(match &kind[..] {
                "int" => json!(o.locals.ints.get(&a.name)),
                "float" => json!(o.locals.floats.get(&a.name)),
                "string" => json!(o.locals.strings.get(&a.name)),
                _ => json!(o.locals.objects.get(&a.name)),
            })
        })
    }

    #[tool(
        description = "Travel to another area of the module (loads it and sends its load events)."
    )]
    async fn enter_area(&self, Parameters(a): Parameters<AreaArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_mut().ok_or("no sim")?;
            let area = sim
                .enter_area(&a.area, a.waypoint.as_deref())
                .map_err(|e| e.to_string())?;
            sim.tick(0);
            Ok(summary(sim, area))
        })
    }

    #[tool(description = "List the module's areas (from its campaign file).")]
    async fn list_areas(&self) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_ref().ok_or("no sim")?;
            Ok(json!(sim.module_info.areas))
        })
    }

    #[tool(
        description = "Save the simulation state to an in-memory slot, and to a JSON file if path is given."
    )]
    async fn save_state(&self, Parameters(a): Parameters<SlotArgs>) -> String {
        self.with_sim(|st| {
            let sim = st.sim.as_ref().ok_or("no sim")?;
            let snap = sim.snapshot();
            let mut out = json!({});
            if let Some(path) = &a.path {
                let bytes = sim.save_json().map_err(|e| e.to_string())?;
                std::fs::write(path, &bytes).map_err(|e| format!("writing {path}: {e}"))?;
                out["file"] = json!(path);
                out["bytes"] = json!(bytes.len());
            }
            let slot = a.slot.unwrap_or_else(|| "quick".into());
            st.slots.insert(slot.clone(), snap);
            out["saved"] = json!(slot);
            Ok(out)
        })
    }

    #[tool(
        description = "Restore the simulation state from an in-memory slot, or from a JSON file written by save_state if path is given."
    )]
    async fn load_state(&self, Parameters(a): Parameters<SlotArgs>) -> String {
        self.with_sim(|st| {
            if let Some(path) = &a.path {
                let bytes = std::fs::read(path).map_err(|e| format!("reading {path}: {e}"))?;
                let sim = st.sim.as_mut().ok_or("no sim")?;
                sim.load_json(&bytes).map_err(|e| e.to_string())?;
                st.trace_cursor = 0;
                return Ok(json!({ "restored": path, "time_ms": sim.world.time_ms }));
            }
            let slot = a.slot.unwrap_or_else(|| "quick".into());
            let snap = st.slots.get(&slot).cloned().ok_or("no such slot")?;
            let sim = st.sim.as_mut().ok_or("no sim")?;
            sim.restore(snap);
            Ok(json!({ "restored": slot, "time_ms": sim.world.time_ms }))
        })
    }
}

#[tool_handler(
    name = "thedas",
    version = "0.1.0",
    instructions = "Drives the OpenThedas Dragon Age: Origins simulation. Start with load (e.g. area cir110ar_inn for the Spoiled Princess), then list_objects, interact to talk or use things, choose_reply to pick dialog options, step to advance time, and get_trace to see what scripts did. The game's own interface runs too: ui_open (menu, hud, startmenu2, or movie names), ui_screenshot to look, ui_click at screenshot pixels, ui_step, and ui_values for what it reads from the engine."
)]
impl rmcp::ServerHandler for Server {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = Server::new().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
