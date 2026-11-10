//! The engine's side of the game's GUI protocol, as far as the movies' ActionScript shows it.
//!
//! Provisional, inferred from the movies' bytecode (the `GUISystem`, `ExternalCommands` and
//! `Localization` classes every movie carries):
//! - The engine starts a movie by calling `_global.GUISystem.StartSceneFromGame()` within
//!   100 ms; otherwise the movie assumes it runs in a standalone Flash player.
//! - `ExternalInterface.call("GetValue" | "GetNumber" | "GetString", name)` reads an engine
//!   value by dotted name (`Scaleform.Platform`, `Party.MemberSelected.0`, ...);
//!   `fscommand("SetValue" | "SetValueUnicode", "name value")` writes one.
//! - `ExternalInterface.call("GetTlkString", id, "#id")` returns a talk-table string; labels
//!   are `Localization.Strings.LOC_*` names mapped to string ids in the movie.
//! - `fscommand` also carries `PlayGUISound`, context stack changes (`CreateContext`,
//!   `PushContext`, `PopContext`) and `SendMessage`/`SendMessageArgs` to game systems.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::{CallKind, Responder, UiCall, UiValue};

/// Messages whose engine side reads a value the screen set just before sending them (the
/// name typed in character creation): the bridge passes the value on as one more argument.
/// Provisional: inferred from the movies, which set the value and send the message together.
const MESSAGE_VALUES: &[(&str, &str)] =
    &[("GUICharGen.CharGenSelectName", "GUICharGen.TempCharName")];

/// `args` with the value [`MESSAGE_VALUES`] names for message `name`, if any, added.
fn with_message_values(
    values: &BTreeMap<String, UiValue>,
    name: &str,
    mut args: Vec<String>,
) -> Vec<String> {
    for (message, value) in MESSAGE_VALUES {
        if *message == name {
            args.push(values.get(*value).map(as_string).unwrap_or_default());
        }
    }
    args
}

/// The engine value holding the hero's name, for designer tags.
pub const HERO_NAME: &str = "Party.Hero.Name";

/// Text with the game's designer tags filled in: `<FirstName/>` is the hero's name.
/// Provisional: other tags (gendered text and the like) are left as written.
pub fn replace_designer_tags(text: &str, hero: &str) -> String {
    text.replace("<FirstName/>", hero)
}

/// Acts on a game message as the game does when the movie sends it, before the movie's next
/// instruction: returns the engine values that changed, or `None` when it did not act (the
/// message is then passed on as a [`GameMessage::Send`]).
pub type MessageHandler =
    Arc<dyn Fn(&str, &[String]) -> Option<Vec<(String, UiValue)>> + Send + Sync>;

/// Looks up a talk-table string.
pub type StringLookup = Arc<dyn Fn(u32) -> Option<String> + Send + Sync>;

/// A question about the game's 2DA tables (M2DA table ids, column hashes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TableQuery {
    /// `Get2DAValue`: the cell at a row id.
    Cell { table: i32, row: i64, column: u32 },
    /// `Get2DAValueByRowIndexColumnHash`: the cell at a row's position in the table.
    CellAt {
        table: i32,
        index: usize,
        column: u32,
    },
    /// `GetNum2DARows`.
    Rows { table: i32 },
}

/// Answers [`TableQuery`]s.
pub type TableLookup = Arc<dyn Fn(TableQuery) -> Option<UiValue> + Send + Sync>;

/// The function that starts a movie in game mode.
pub const START_SCENE: &str = "_global.GUISystem.StartSceneFromGame";
pub const FINISH_SCENE: &str = "_global.GUISystem.FinishSceneFromGame";
/// Lays a movie out against the current screen (safe zone, anchors); the engine calls it
/// after starting a movie and when the resolution changes.
pub const LAYOUT: &str = "_global.MovieUtils.UpdateAllLayouts";

#[derive(Default)]
struct State {
    values: BTreeMap<String, UiValue>,
    contexts: Vec<String>,
    unanswered: BTreeMap<String, u64>,
    sounds: Vec<String>,
    /// Messages the handler already acted on, oldest first (their text).
    handled: std::collections::VecDeque<String>,
    /// The names the last [`GameBridge::bind`] set.
    bound: std::collections::BTreeSet<String>,
}

/// Engine values and strings the movies ask for, shared by every movie.
#[derive(Clone)]
pub struct GameBridge {
    state: Arc<Mutex<State>>,
    strings: StringLookup,
    handler: Arc<Mutex<Option<MessageHandler>>>,
    tables: Arc<Mutex<Option<TableLookup>>>,
}

/// What a call means to the rest of the engine, once the bridge has handled its own part.
#[derive(Clone, Debug, PartialEq)]
pub enum GameMessage {
    /// `SendMessage`/`SendMessageArgs`: a message to a game system (name, arguments).
    Send(String, Vec<String>),
    /// A message the bridge's handler acted on when the movie sent it.
    Handled(String, Vec<String>),
    /// Anything the bridge does not know.
    Other(UiCall),
}

fn as_string(v: &UiValue) -> String {
    match v {
        UiValue::String(s) => s.clone(),
        UiValue::Number(n) if n.fract() == 0.0 => format!("{}", *n as i64),
        UiValue::Number(n) => n.to_string(),
        UiValue::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

/// `"12"` becomes a number; other text stays a string.
fn parse_value(s: &str) -> UiValue {
    match s.parse::<f64>() {
        Ok(n) => UiValue::Number(n),
        Err(_) => UiValue::String(s.to_string()),
    }
}

impl GameBridge {
    pub fn new(strings: StringLookup) -> Self {
        let mut state = State::default();
        for (k, v) in [
            ("Scaleform.Platform", UiValue::String("PC".into())),
            ("SaveLoad.SavingEnabled", UiValue::Number(0.0)),
            ("DebugOptions.EnableController", UiValue::Number(0.0)),
            ("DebugOptions.EnableDebugHelpers", UiValue::Number(0.0)),
            ("DebugOptions.E3Mode", UiValue::Number(0.0)),
            (
                "DebugOptions.ShowConversationResponsesImmediately",
                UiValue::Number(0.0),
            ),
            ("Scaleform.IsMac", UiValue::Number(0.0)),
            ("OnlineFeatures.LoggedIn", UiValue::Number(0.0)),
            (
                "ClientOptions.OnlineOptions.EnableOnlineFeatures",
                UiValue::Number(0.0),
            ),
        ] {
            state.values.insert(k.to_string(), v);
        }
        Self {
            state: Arc::new(Mutex::new(state)),
            strings,
            handler: Arc::new(Mutex::new(None)),
            tables: Arc::new(Mutex::new(None)),
        }
    }

    /// Installs (or removes) the handler for game messages the movies send.
    /// Where `Get2DAValue` finds the game's tables.
    pub fn set_table_lookup(&self, tables: Option<TableLookup>) {
        if let Ok(mut t) = self.tables.lock() {
            *t = tables;
        }
    }

    pub fn set_message_handler(&self, handler: Option<MessageHandler>) {
        if let Ok(mut h) = self.handler.lock() {
            *h = handler;
        }
    }

    pub fn set_value(&self, name: &str, value: UiValue) {
        if let Ok(mut s) = self.state.lock() {
            s.values.insert(name.to_string(), value);
        }
    }

    /// Replaces many values at once (e.g. [`crate::binding::sim_values`] every frame).
    pub fn set_values(&self, values: impl IntoIterator<Item = (String, UiValue)>) {
        if let Ok(mut s) = self.state.lock() {
            for (k, v) in values {
                s.unanswered.remove(&k);
                s.values.insert(k, v);
            }
        }
    }

    /// Binds the engine's whole state (e.g. [`crate::binding::sim_values`] every frame):
    /// values the last bind set that are not in `values` are gone (a conversation's earlier
    /// replies, a party member who left); values the movies set themselves are kept.
    pub fn bind(&self, values: impl IntoIterator<Item = (String, UiValue)>) {
        if let Ok(mut s) = self.state.lock() {
            let mut bound = std::collections::BTreeSet::new();
            for (k, v) in values {
                s.unanswered.remove(&k);
                s.values.insert(k.clone(), v);
                bound.insert(k);
            }
            let stale: Vec<String> = s.bound.difference(&bound).cloned().collect();
            for k in stale {
                s.values.remove(&k);
            }
            s.bound = bound;
        }
    }

    /// Records names a movie wanted that nothing answers.
    pub fn note_unanswered(&self, names: impl IntoIterator<Item = String>) {
        if let Ok(mut s) = self.state.lock() {
            for n in names {
                *s.unanswered.entry(n).or_default() += 1;
            }
        }
    }

    /// Every value set so far.
    pub fn values(&self) -> BTreeMap<String, UiValue> {
        self.state
            .lock()
            .map(|s| s.values.clone())
            .unwrap_or_default()
    }

    pub fn value(&self, name: &str) -> Option<UiValue> {
        self.state.lock().ok()?.values.get(name).cloned()
    }

    /// The context stack (`MainMenu`, ...), innermost last.
    pub fn contexts(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|s| s.contexts.clone())
            .unwrap_or_default()
    }

    /// `GetValue` names nobody answered, with counts.
    pub fn unanswered(&self) -> BTreeMap<String, u64> {
        self.state
            .lock()
            .map(|s| s.unanswered.clone())
            .unwrap_or_default()
    }

    /// GUI sounds requested since the last drain.
    pub fn drain_sounds(&self) -> Vec<String> {
        self.state
            .lock()
            .map(|mut s| std::mem::take(&mut s.sounds))
            .unwrap_or_default()
    }

    /// The answer to an `ExternalInterface.call` or a value-returning fscommand (whose single
    /// argument string holds space-separated arguments).
    pub fn respond(&self, call: &UiCall) -> UiValue {
        let args: Vec<String> = match call.kind {
            CallKind::ExternalInterface => call.args.iter().map(as_string).collect(),
            CallKind::FsCommand => call
                .args
                .first()
                .map(as_string)
                .unwrap_or_default()
                .split(' ')
                .map(str::to_string)
                .collect(),
        };
        let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
        match call.name.as_str() {
            // The game acts on a message before the movie goes on (a movie reads the result
            // right after sending it, as chanters.gfx does after accepting a job).
            "SendMessage" | "SendMessageArgs" if call.kind == CallKind::FsCommand => {
                let handler = self.handler.lock().ok().and_then(|h| h.clone());
                let name = arg(0);
                let message_args = match self.state.lock() {
                    Ok(s) => {
                        with_message_values(&s.values, &name, args[1.min(args.len())..].to_vec())
                    }
                    Err(_) => args[1.min(args.len())..].to_vec(),
                };
                if let Some(changed) = handler.and_then(|h| h(&name, &message_args)) {
                    self.set_values(changed);
                    if let Ok(mut s) = self.state.lock() {
                        s.handled.push_back(args.join(" "));
                    }
                }
                UiValue::Undefined
            }
            "GetValue" | "GetNumber" | "GetString" | "GetBool" => {
                let name = arg(0);
                let Ok(mut s) = self.state.lock() else {
                    return UiValue::Undefined;
                };
                match s.values.get(&name) {
                    Some(v) => v.clone(),
                    None => {
                        *s.unanswered.entry(name).or_default() += 1;
                        UiValue::Undefined
                    }
                }
            }
            // SharedLibrary.Ability.LoadFromToken's call (see `avm1::PRE_CALLS`).
            "GetAbilityAttributes" => {
                let Ok(s) = self.state.lock() else {
                    return UiValue::Undefined;
                };
                crate::chains::ability_attributes(|k| s.values.get(k).cloned(), &arg(0))
            }
            "GetTlkString" => {
                let id = arg(0).parse::<u32>().ok();
                match id.and_then(|id| (self.strings)(id)) {
                    Some(text) => UiValue::String(text),
                    None => UiValue::String(arg(1)),
                }
            }
            "GetNumMembers" => {
                let prefix = format!("{}.", arg(0));
                let Ok(s) = self.state.lock() else {
                    return UiValue::Undefined;
                };
                // A child counts when it has members of its own or a non-zero value (an empty
                // party slot is a bare 0).
                let n = s
                    .values
                    .range(prefix.clone()..)
                    .take_while(|(k, _)| k.starts_with(&prefix))
                    .filter(|(k, v)| k[prefix.len()..].contains('.') || **v != UiValue::Number(0.0))
                    .filter_map(|(k, _)| k[prefix.len()..].split('.').next())
                    .collect::<std::collections::BTreeSet<_>>()
                    .len();
                UiValue::Number(n as f64)
            }
            // The whole argument string is the text (it has spaces of its own).
            "ReplaceDesignerTags" => {
                let text = match call.kind {
                    CallKind::FsCommand => call.args.first().map(as_string).unwrap_or_default(),
                    CallKind::ExternalInterface => arg(0),
                };
                let hero = match self
                    .state
                    .lock()
                    .ok()
                    .and_then(|s| s.values.get(HERO_NAME).cloned())
                {
                    Some(UiValue::String(n)) => n,
                    _ => String::new(),
                };
                UiValue::String(replace_designer_tags(&text, &hero))
            }
            "FormatFloat" => {
                let v = arg(0).parse::<f64>().unwrap_or(0.0);
                let digits = arg(1).parse::<usize>().unwrap_or(0).min(10);
                UiValue::String(format!("{v:.digits$}"))
            }
            // A 2DA cell: the table id, the row id and the column hash (worldmap.gfx reads
            // map names and pin images this way).
            // The 2DA fscommands: (table id, row id or row position, column hash) and
            // (table id) for the row count.
            name @ ("Get2DAValue" | "Get2DAValueByRowIndexColumnHash" | "GetNum2DARows")
                if call.kind == CallKind::FsCommand =>
            {
                let lookup = self.tables.lock().ok().and_then(|t| t.clone());
                let table = arg(0).trim().parse::<i32>().ok();
                let row = arg(1)
                    .trim()
                    .parse::<f64>()
                    .ok()
                    .filter(|r| r.fract() == 0.0);
                let column = arg(2).trim().parse::<u32>().ok();
                let query = match (name, table, row, column) {
                    ("Get2DAValue", Some(table), Some(row), Some(column)) => {
                        Some(TableQuery::Cell {
                            table,
                            row: row as i64,
                            column,
                        })
                    }
                    ("Get2DAValueByRowIndexColumnHash", Some(table), Some(row), Some(column))
                        if row >= 0.0 =>
                    {
                        Some(TableQuery::CellAt {
                            table,
                            index: row as usize,
                            column,
                        })
                    }
                    ("GetNum2DARows", Some(table), _, _) => Some(TableQuery::Rows { table }),
                    _ => None,
                };
                let found = match (lookup, query) {
                    (Some(l), Some(q)) => l(q),
                    _ => None,
                };
                found.unwrap_or_else(|| {
                    if let Ok(mut s) = self.state.lock() {
                        *s.unanswered.entry(name.into()).or_default() += 1;
                    }
                    UiValue::Undefined
                })
            }
            name if call.kind == CallKind::FsCommand
                && crate::avm1::RETURNING_FSCOMMANDS.contains(&name) =>
            {
                if let Ok(mut s) = self.state.lock() {
                    *s.unanswered.entry(name.to_string()).or_default() += 1;
                }
                UiValue::Undefined
            }
            _ => UiValue::Undefined,
        }
    }

    pub fn responder(&self) -> Responder {
        let me = self.clone();
        Arc::new(move |call: &UiCall| me.respond(call))
    }

    /// Applies a call's effect on engine values and contexts; returns what remains for the
    /// rest of the engine.
    pub fn handle(&self, call: UiCall) -> Option<GameMessage> {
        if matches!(
            call.name.as_str(),
            "GetValue" | "GetNumber" | "GetString" | "GetBool"
        ) || crate::avm1::RETURNING_FSCOMMANDS.contains(&call.name.as_str())
        {
            return None;
        }
        let text = call.args.first().map(as_string).unwrap_or_default();
        let Ok(mut s) = self.state.lock() else {
            return None;
        };
        match call.name.as_str() {
            "SetValue" | "SetValueUnicode" => {
                let (k, v) = text.split_once(' ').unwrap_or((&text, ""));
                s.values.insert(k.to_string(), parse_value(v));
                None
            }
            "PlayGUISound" => {
                s.sounds.push(text);
                None
            }
            "CreateContext" => None,
            "PushContext" => {
                s.contexts.push(text);
                None
            }
            "PopContext" => {
                if let Some(i) = s.contexts.iter().rposition(|c| *c == text) {
                    s.contexts.remove(i);
                } else {
                    s.contexts.pop();
                }
                None
            }
            "BlockGameMessage" | "UnblockGameMessage" => None,
            "SendMessage" | "SendMessageArgs" => {
                let mut parts = text.split(' ').map(str::to_string);
                let name = parts.next().unwrap_or_default();
                let args = with_message_values(&s.values, &name, parts.collect());
                if s.handled.front() == Some(&text) {
                    s.handled.pop_front();
                    return Some(GameMessage::Handled(name, args));
                }
                Some(GameMessage::Send(name, args))
            }
            _ => Some(GameMessage::Other(call)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(kind: CallKind, name: &str, args: &[&str]) -> UiCall {
        UiCall {
            movie: "m.gfx".into(),
            kind,
            name: name.into(),
            args: args
                .iter()
                .map(|a| UiValue::String(a.to_string()))
                .collect(),
        }
    }

    fn bridge() -> GameBridge {
        GameBridge::new(Arc::new(|id| (id == 5).then(|| "Five".to_string())))
    }

    #[test]
    fn binding_drops_what_the_engine_no_longer_has() {
        let b = bridge();
        let n = |x: f64| UiValue::Number(x);
        b.bind([("R.0.Text".into(), n(1.0)), ("R.1.Text".into(), n(2.0))]);
        b.set_value("Movie.Own", n(3.0));
        b.bind([("R.0.Text".into(), n(4.0))]);
        assert_eq!(b.value("R.0.Text"), Some(n(4.0)));
        assert_eq!(b.value("R.1.Text"), None);
        assert_eq!(b.value("Movie.Own"), Some(n(3.0)));
        // The defaults every movie reads stay.
        assert!(b.value("Scaleform.Platform").is_some());
    }

    #[test]
    fn the_name_typed_goes_with_its_message() {
        let b = bridge();
        let fs = CallKind::FsCommand;
        b.handle(call(
            fs,
            "SetValueUnicode",
            &["GUICharGen.TempCharName Elissa"],
        ));
        assert_eq!(
            b.handle(call(
                fs,
                "SendMessageArgs",
                &["GUICharGen.CharGenSelectName 12"]
            )),
            Some(GameMessage::Send(
                "GUICharGen.CharGenSelectName".into(),
                vec!["12".into(), "Elissa".into()]
            ))
        );
    }

    #[test]
    fn designer_tags_take_the_heros_name() {
        let b = bridge();
        b.set_value(HERO_NAME, UiValue::String("Aeducan".into()));
        let text = "I am the Grey Warden <FirstName/>.";
        assert_eq!(
            b.respond(&call(CallKind::FsCommand, "ReplaceDesignerTags", &[text])),
            UiValue::String("I am the Grey Warden Aeducan.".into())
        );
        assert_eq!(
            replace_designer_tags("<male>x</male>", "A"),
            "<male>x</male>"
        );
    }

    #[test]
    fn handled_messages_update_values_before_the_movie_reads_them() {
        let b = bridge();
        b.set_message_handler(Some(Arc::new(|name: &str, args: &[String]| {
            (name == "Journal.AcceptChantersQuest").then(|| {
                vec![(
                    format!(
                        "Board.{}.Accepted",
                        args.first().cloned().unwrap_or_default()
                    ),
                    UiValue::Number(1.0),
                )]
            })
        })));
        let fs = CallKind::FsCommand;
        let accept = call(fs, "SendMessage", &["Journal.AcceptChantersQuest 2"]);
        b.respond(&accept);
        assert_eq!(
            b.respond(&call(
                CallKind::ExternalInterface,
                "GetValue",
                &["Board.2.Accepted"]
            )),
            UiValue::Number(1.0)
        );
        assert_eq!(
            b.handle(accept),
            Some(GameMessage::Handled(
                "Journal.AcceptChantersQuest".into(),
                vec!["2".into()]
            ))
        );
        // What the handler does not act on goes to the rest of the engine as before.
        let other = call(fs, "SendMessage", &["GUI.HideFullScreenGUI"]);
        b.respond(&other);
        assert_eq!(
            b.handle(other),
            Some(GameMessage::Send("GUI.HideFullScreenGUI".into(), vec![]))
        );
    }

    #[test]
    fn table_fscommands_and_ability_attributes_are_answered() {
        let b = bridge();
        b.set_table_lookup(Some(Arc::new(|q| match q {
            TableQuery::Cell {
                table: 111,
                row: 117,
                column: 7,
            } => Some(UiValue::Number(371207.0)),
            TableQuery::CellAt {
                table: 3,
                index: 2,
                column: 9,
            } => Some(UiValue::Number(1.0)),
            TableQuery::Rows { table: 3 } => Some(UiValue::Number(26.0)),
            _ => None,
        })));
        let fs = CallKind::FsCommand;
        let n = |x: f64| UiValue::Number(x);
        assert_eq!(
            b.respond(&call(fs, "Get2DAValue", &["111 117 7"])),
            n(371207.0)
        );
        assert_eq!(
            b.respond(&call(fs, "Get2DAValueByRowIndexColumnHash", &["3 2 9"])),
            n(1.0)
        );
        assert_eq!(b.respond(&call(fs, "GetNum2DARows", &["3"])), n(26.0));
        assert_eq!(
            b.respond(&call(fs, "Get2DAValueByRowIndexColumnHash", &["3 -1 9"])),
            UiValue::Undefined
        );
        assert_eq!(
            b.unanswered().get("Get2DAValueByRowIndexColumnHash"),
            Some(&1)
        );
        let t = "CharGenCreature.Stats.TalentChains.0";
        b.set_values([
            (format!("{t}.ID"), n(14.0)),
            (format!("{t}.UseType"), n(4.0)),
            (
                format!("{t}.Icon"),
                UiValue::String("tlt_ico_powerful".into()),
            ),
            (format!("{t}.GUIType"), n(101.0)),
            (format!("{t}.PrereqAbilityID"), n(4022.0)),
            (format!("{t}.DisplayState"), n(2.0)),
        ]);
        assert_eq!(
            b.respond(&call(
                CallKind::ExternalInterface,
                "GetAbilityAttributes",
                &[t]
            )),
            UiValue::List(vec![
                n(14.0),
                n(4.0),
                UiValue::String("tlt_ico_powerful".into()),
                n(101.0),
                n(4022.0),
                n(2.0)
            ])
        );
    }

    #[test]
    fn answers_values_and_strings() {
        let b = bridge();
        let ei = CallKind::ExternalInterface;
        assert_eq!(
            b.respond(&call(ei, "GetValue", &["Scaleform.Platform"])),
            UiValue::String("PC".into())
        );
        assert_eq!(
            b.respond(&call(ei, "GetValue", &["Nope"])),
            UiValue::Undefined
        );
        assert_eq!(b.unanswered().get("Nope"), Some(&1));
        assert_eq!(
            b.respond(&call(ei, "GetTlkString", &["5", "#5"])),
            UiValue::String("Five".into())
        );
        assert_eq!(
            b.respond(&call(ei, "GetTlkString", &["6", "#6"])),
            UiValue::String("#6".into())
        );
        assert_eq!(
            b.respond(&call(CallKind::FsCommand, "GetTlkString", &["5 #5"])),
            UiValue::String("Five".into())
        );
        assert_eq!(
            b.respond(&call(CallKind::FsCommand, "FormatFloat", &["2.345 1"])),
            UiValue::String("2.3".into())
        );
        b.set_values([
            ("Party.Members.0.ID".to_string(), UiValue::Number(3.0)),
            ("Party.Members.0.Name".to_string(), UiValue::Null),
            ("Party.Members.1.ID".to_string(), UiValue::Number(4.0)),
            ("Party.MembersX".to_string(), UiValue::Null),
        ]);
        assert_eq!(
            b.respond(&call(
                CallKind::FsCommand,
                "GetNumMembers",
                &["Party.Members"]
            )),
            UiValue::Number(2.0)
        );
    }

    #[test]
    fn applies_fscommands() {
        let b = bridge();
        let fs = CallKind::FsCommand;
        assert_eq!(b.handle(call(fs, "SetValue", &["GUI.Page 20"])), None);
        assert_eq!(b.value("GUI.Page"), Some(UiValue::Number(20.0)));
        b.handle(call(fs, "PushContext", &["MainMenu"]));
        b.handle(call(fs, "PushContext", &["Options"]));
        b.handle(call(fs, "PopContext", &["MainMenu"]));
        assert_eq!(b.contexts(), vec!["Options".to_string()]);
        b.handle(call(fs, "PlayGUISound", &["101 Click"]));
        assert_eq!(b.drain_sounds(), vec!["101 Click".to_string()]);
        assert_eq!(
            b.handle(call(
                fs,
                "SendMessage",
                &["GameModeController.HideGUIScreen 3"]
            )),
            Some(GameMessage::Send(
                "GameModeController.HideGUIScreen".into(),
                vec!["3".into()]
            ))
        );
        assert!(matches!(
            b.handle(call(fs, "Mystery", &[])),
            Some(GameMessage::Other(_))
        ));
    }
}
