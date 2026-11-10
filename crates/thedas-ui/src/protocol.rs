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

/// Looks up a talk-table string.
pub type StringLookup = Arc<dyn Fn(u32) -> Option<String> + Send + Sync>;

/// The function that starts a movie in game mode.
pub const START_SCENE: &str = "_global.GUISystem.StartSceneFromGame";
pub const FINISH_SCENE: &str = "_global.GUISystem.FinishSceneFromGame";

#[derive(Default)]
struct State {
    values: BTreeMap<String, UiValue>,
    contexts: Vec<String>,
    unanswered: BTreeMap<String, u64>,
    sounds: Vec<String>,
}

/// Engine values and strings the movies ask for, shared by every movie.
#[derive(Clone)]
pub struct GameBridge {
    state: Arc<Mutex<State>>,
    strings: StringLookup,
}

/// What a call means to the rest of the engine, once the bridge has handled its own part.
#[derive(Clone, Debug, PartialEq)]
pub enum GameMessage {
    /// `SendMessage`/`SendMessageArgs`: a message to a game system (name, arguments).
    Send(String, Vec<String>),
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
            "FormatFloat" => {
                let v = arg(0).parse::<f64>().unwrap_or(0.0);
                let digits = arg(1).parse::<usize>().unwrap_or(0).min(10);
                UiValue::String(format!("{v:.digits$}"))
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
                Some(GameMessage::Send(name, parts.collect()))
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
