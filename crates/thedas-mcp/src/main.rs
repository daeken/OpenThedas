//! `thedas-mcp`: a Model Context Protocol server (stdio) for driving the OpenThedas
//! simulation: load a module and area, look at objects, step time, walk the hero around, use
//! things, talk to people, pick dialog replies, poke plot flags and locals, call engine
//! functions and scripts, and snapshot/restore the world.
//!
//! Register it with Claude Code:
//!
//! ```sh
//! claude mcp add thedas -e THEDAS_INSTALL=/path/to/Dragon\ Age -- /path/to/thedas-mcp
//! ```

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

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
}

#[derive(Clone)]
struct Server {
    state: Arc<Mutex<State>>,
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
        }
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
                "unimplemented_top": un.iter().take(15).map(|(f, n)| json!({"function": f, "calls": n})).collect::<Vec<_>>(),
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
            let h = sim.world.get_mut(hero).ok_or("no hero")?;
            h.position = target.0;
            if target.1.is_some() {
                h.area = target.1;
            }
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

    #[tool(description = "Save the simulation state to an in-memory slot.")]
    async fn save_state(&self, Parameters(a): Parameters<SlotArgs>) -> String {
        self.with_sim(|st| {
            let snap = st.sim.as_ref().ok_or("no sim")?.snapshot();
            let slot = a.slot.unwrap_or_else(|| "quick".into());
            st.slots.insert(slot.clone(), snap);
            Ok(json!({ "saved": slot }))
        })
    }

    #[tool(description = "Restore the simulation state from an in-memory slot.")]
    async fn load_state(&self, Parameters(a): Parameters<SlotArgs>) -> String {
        self.with_sim(|st| {
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
    instructions = "Drives the OpenThedas Dragon Age: Origins simulation. Start with load (e.g. area cir110ar_inn for the Spoiled Princess), then list_objects, interact to talk or use things, choose_reply to pick dialog options, step to advance time, and get_trace to see what scripts did."
)]
impl rmcp::ServerHandler for Server {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let service = Server::new().serve(rmcp::transport::stdio()).await?;
    service.waiting().await?;
    Ok(())
}
