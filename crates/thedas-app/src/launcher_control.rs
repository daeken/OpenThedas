//! Driving the launcher from a script or a test client, through its accessibility tree: the
//! same tree (AccessKit) that screen readers and Windows UI Automation see, so a script clicks
//! a button the way an assistive tool would, by its name.
//!
//! - `thedas-app --launcher --control 127.0.0.1:8020` takes JSON requests a line at a time, as
//!   the game's own `--control` does (see `control.rs`), each answered by one JSON line.
//! - `thedas-app --launcher --launcher-steps "click Search this computer; wait; tree"` plays
//!   steps in order (a `;`-separated list), printing what `tree` and `status` answer.
//!
//! Commands (`"cmd"` in a request; the first word of a step):
//! - `click <name>`: clicks the control with that name (a button, checkbox, radio button, link,
//!   tab or combo box). Exact names first, then ignoring case, then a unique part of a name.
//!   A failed step ends `--launcher-steps` (the process exits non-zero); a client gets an
//!   error naming what the page has instead.
//! - `type <name>=<text>` (`{"cmd":"type","name":...,"text":...}`): focuses the text field
//!   with that name (its label or its hint) and types the text. No name: the focused field.
//! - `key <name>`: presses a key (`Enter`, `Tab`, `Escape`, ...).
//! - `choose <path>`: the next file or folder dialog answers `path` instead of opening (native
//!   dialogs can't be driven), e.g. `choose C:\Games\DAO; click Pick a folder…`.
//! - `wait [seconds]`: until no search or download is running (default 300 s).
//! - `run <ms>`: lets the launcher run that long.
//! - `tree`: every named control and text on the page: role, name, value, enabled, checked.
//! - `status`: the page, the footer's message, the installs, whether a search or download runs.
//! - `screenshot` (`shot <file.png>` as a step): the window as drawn, a PNG (base64 under
//!   `"png"` in a reply).
//! - `quit`: closes the launcher.
//!
//! Clicking "Play" hands off to the game as a player's click does (a new process), and the
//! reply carries the command line it ran.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use egui::accesskit::{self, Action, Node, NodeId, Role, Toggled, TreeId};
use serde_json::{Value as J, json};

use crate::control::{self, Control};

/// One thing to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    Click(String),
    Type { name: Option<String>, text: String },
    Key(String),
    Choose(PathBuf),
    Wait(Duration),
    Run(Duration),
    Tree,
    Status,
    Screenshot(Option<PathBuf>),
    Quit,
}

impl Cmd {
    /// The command for a log line. Typed text is left out: it may be a password.
    fn describe(&self) -> String {
        match self {
            Cmd::Type { name, text } => format!(
                "type into {} ({} characters)",
                name.as_deref().unwrap_or("the focused field"),
                text.chars().count()
            ),
            c => format!("{c:?}"),
        }
    }
}

/// Parses `--launcher-steps`: commands separated by `;`.
pub fn parse_steps(s: &str) -> Result<Vec<Cmd>> {
    s.split(';')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(parse_step)
        .collect()
}

fn parse_step(t: &str) -> Result<Cmd> {
    let (word, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
    let rest = rest.trim();
    let need = |what: &str| -> Result<String> {
        if rest.is_empty() {
            bail!("step {t:?} needs {what}");
        }
        Ok(rest.to_string())
    };
    Ok(match word {
        "click" => Cmd::Click(need("a name")?),
        "type" => match rest.split_once('=') {
            Some((n, text)) if !n.trim().is_empty() => Cmd::Type {
                name: Some(n.trim().to_string()),
                text: text.to_string(),
            },
            Some((_, text)) => Cmd::Type {
                name: None,
                text: text.to_string(),
            },
            None => Cmd::Type {
                name: None,
                text: rest.to_string(),
            },
        },
        "key" => Cmd::Key(need("a key name")?),
        "choose" => Cmd::Choose(PathBuf::from(need("a path")?)),
        "wait" => Cmd::Wait(secs(rest, 300.0)?),
        "run" => Cmd::Run(Duration::from_millis(need("milliseconds")?.parse()?)),
        "tree" => Cmd::Tree,
        "status" => Cmd::Status,
        "shot" | "screenshot" => Cmd::Screenshot((!rest.is_empty()).then(|| PathBuf::from(rest))),
        "quit" => Cmd::Quit,
        _ => bail!("unknown launcher step {t:?}"),
    })
}

fn secs(s: &str, default: f64) -> Result<Duration> {
    let v = if s.is_empty() { default } else { s.parse()? };
    if !(0.0..=86_400.0).contains(&v) {
        bail!("{v} seconds is out of range");
    }
    Ok(Duration::from_secs_f64(v))
}

/// A control request as a command.
pub fn parse_request(body: &J) -> Result<Cmd> {
    let s = |k: &str| body.get(k).and_then(J::as_str).map(str::to_string);
    let cmd = s("cmd").unwrap_or_default();
    Ok(match cmd.as_str() {
        "click" => Cmd::Click(s("name").or(s("label")).unwrap_or_default()),
        "type" => Cmd::Type {
            name: s("name").or(s("label")),
            text: s("text").unwrap_or_default(),
        },
        "key" => Cmd::Key(s("key").unwrap_or_default()),
        "choose" => Cmd::Choose(PathBuf::from(s("path").unwrap_or_default())),
        "wait" => Cmd::Wait(Duration::from_secs_f64(
            body.get("seconds")
                .and_then(J::as_f64)
                .unwrap_or(300.0)
                .clamp(0.0, 3600.0),
        )),
        "run" => Cmd::Run(Duration::from_millis(
            body.get("ms").and_then(J::as_u64).unwrap_or(0).min(600_000),
        )),
        "tree" => Cmd::Tree,
        "status" => Cmd::Status,
        "screenshot" => Cmd::Screenshot(None),
        "quit" => Cmd::Quit,
        other => bail!("unknown launcher command {other:?}"),
    })
}

/// One control on the page, as the accessibility tree has it.
#[derive(Clone, Debug, PartialEq)]
pub struct Widget {
    pub id: NodeId,
    pub role: Role,
    /// What a screen reader announces: the label, else the label of what labels it, else the
    /// text of a plain label, else a text field's hint.
    pub name: String,
    pub value: Option<String>,
    pub enabled: bool,
    pub checked: Option<bool>,
    pub clickable: bool,
    pub focusable: bool,
}

/// The page's accessibility tree, kept from egui's updates.
#[derive(Default)]
pub struct Tree {
    nodes: HashMap<NodeId, Node>,
    root: Option<NodeId>,
    pub focus: Option<NodeId>,
}

impl Tree {
    pub fn update(&mut self, u: &accesskit::TreeUpdate) {
        if u.tree_id != TreeId::ROOT {
            return;
        }
        if let Some(t) = &u.tree {
            self.root = Some(t.root);
        }
        for (id, n) in &u.nodes {
            self.nodes.insert(*id, n.clone());
        }
        self.focus = Some(u.focus);
        // Drop nodes no longer reachable, so the map follows the page.
        let mut seen = std::collections::HashSet::new();
        let mut stack: Vec<NodeId> = self.root.into_iter().collect();
        while let Some(id) = stack.pop() {
            if seen.insert(id) {
                if let Some(n) = self.nodes.get(&id) {
                    stack.extend(n.children().iter().copied());
                }
            }
        }
        self.nodes.retain(|id, _| seen.contains(id));
    }

    /// The page's widgets in reading order.
    pub fn widgets(&self) -> Vec<Widget> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.root.into_iter().collect();
        while let Some(id) = stack.pop() {
            let Some(n) = self.nodes.get(&id) else {
                continue;
            };
            stack.extend(n.children().iter().rev().copied());
            // A label's text runs repeat the label's own text.
            if n.is_hidden() || n.role() == Role::TextRun {
                continue;
            }
            let name = self.name(n);
            let value = n.value().map(str::to_string);
            let clickable = n.supports_action(Action::Click);
            let focusable = n.supports_action(Action::Focus);
            if name.is_empty() && value.is_none() && !clickable {
                continue;
            }
            out.push(Widget {
                id,
                role: n.role(),
                name,
                value: if n.role() == Role::Label { None } else { value },
                enabled: !n.is_disabled(),
                checked: n.toggled().map(|t| t == Toggled::True),
                clickable,
                focusable,
            });
        }
        out
    }

    fn name(&self, n: &Node) -> String {
        if let Some(l) = n.label() {
            return l.trim().to_string();
        }
        for id in n.labelled_by() {
            if let Some(l) = self.nodes.get(id) {
                if let Some(t) = l.label().or(l.value()) {
                    return t.trim().to_string();
                }
            }
        }
        if n.role() == Role::Label {
            return n.value().unwrap_or("").trim().to_string();
        }
        n.placeholder().unwrap_or("").trim().to_string()
    }
}

/// Finds the widget named `name` among those `ok` accepts: an exact name, then ignoring
/// case, then the only one whose name contains it.
pub fn find<'a>(
    widgets: &'a [Widget],
    name: &str,
    ok: impl Fn(&Widget) -> bool,
) -> std::result::Result<&'a Widget, String> {
    let name = name.trim();
    let cands: Vec<&Widget> = widgets.iter().filter(|w| ok(w)).collect();
    if let Some(w) = cands.iter().find(|w| w.name == name) {
        return Ok(w);
    }
    if let Some(w) = cands.iter().find(|w| w.name.eq_ignore_ascii_case(name)) {
        return Ok(w);
    }
    let low = name.to_lowercase();
    let part: Vec<&&Widget> = cands
        .iter()
        .filter(|w| w.name.to_lowercase().contains(&low))
        .collect();
    match part.as_slice() {
        [w] => Ok(w),
        [] => Err(format!(
            "nothing named {name:?} here; there is: {}",
            cands
                .iter()
                .map(|w| format!("{:?}", w.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        many => Err(format!(
            "{name:?} could be any of {}",
            many.iter()
                .map(|w| format!("{:?}", w.name))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

pub fn widgets_json(ws: &[Widget]) -> J {
    J::Array(
        ws.iter()
            .map(|w| {
                let mut o = json!({
                    "role": format!("{:?}", w.role),
                    "name": w.name,
                    "enabled": w.enabled,
                });
                if let Some(v) = &w.value {
                    o["value"] = json!(v);
                }
                if let Some(c) = w.checked {
                    o["checked"] = json!(c);
                }
                if w.clickable && w.role != Role::Label {
                    o["clickable"] = json!(true);
                }
                o
            })
            .collect(),
    )
}

/// Where a command came from, and so where its answer goes.
enum Asker {
    Client(mpsc::Sender<J>),
    Steps,
}

struct Pending {
    cmd: Cmd,
    asker: Asker,
    /// When the command started (for `wait` and `run`), once it has.
    since: Option<Instant>,
}

/// Commands waiting to be played, from steps and from a control client.
pub struct Script {
    queue: VecDeque<Pending>,
    control: Option<Control>,
    pub tree: Tree,
    /// Set by a step that failed: the rest of the steps are dropped.
    failed: bool,
    /// Frames drawn since the last command finished. A command that reads the page waits for
    /// two: the frame that took the input, and the one that drew its effect (a page changes
    /// after the frame its button was clicked in).
    frames: u32,
}

/// What the frame loop must do for the command now playing.
pub enum Need {
    Nothing,
    /// A screenshot of this frame, for the command at the head of the queue.
    Screenshot,
    Quit,
}

impl Script {
    pub fn new(steps: Vec<Cmd>, control: Option<Control>) -> Script {
        Script {
            queue: steps
                .into_iter()
                .map(|cmd| Pending {
                    cmd,
                    asker: Asker::Steps,
                    since: None,
                })
                .collect(),
            control,
            tree: Tree::default(),
            failed: false,
            frames: 0,
        }
    }

    /// Whether anything is waiting (the window keeps redrawing while it is).
    pub fn busy(&self) -> bool {
        !self.queue.is_empty() || (self.failed && self.control.is_none())
    }

    pub fn has_control(&self) -> bool {
        self.control.is_some()
    }

    /// Takes in new control requests.
    pub fn poll(&mut self) {
        let Some(c) = &self.control else { return };
        for r in c.poll() {
            match parse_request(&r.body) {
                Ok(cmd) => self.queue.push_back(Pending {
                    cmd,
                    asker: Asker::Client(r.reply),
                    since: None,
                }),
                Err(e) => {
                    let _ = r.reply.send(control::failure(e));
                }
            }
        }
    }

    /// Before a frame: plays the next command into `events` (input for this frame), using the
    /// launcher's state through `host`. Returns what the frame must also do.
    pub fn before_frame(&mut self, events: &mut Vec<egui::Event>, host: &mut dyn Host) -> Need {
        // A failed step with no client attached: nothing more will happen, so close (the
        // process then exits non-zero).
        if self.failed && self.control.is_none() {
            return Need::Quit;
        }
        // Several instant commands may play in one frame; one that needs the frame stops.
        for _ in 0..16 {
            let Some(p) = self.queue.front_mut() else {
                return Need::Nothing;
            };
            let reads_page = matches!(
                p.cmd,
                Cmd::Click(_) | Cmd::Type { .. } | Cmd::Tree | Cmd::Status | Cmd::Screenshot(_)
            );
            if reads_page && self.frames < 2 {
                return Need::Nothing;
            }
            let since = *p.since.get_or_insert_with(Instant::now);
            let widgets = self.tree.widgets();
            let result: Option<std::result::Result<J, String>> = match &p.cmd {
                Cmd::Click(name) => Some(
                    find(&widgets, name, |w| {
                        w.clickable && !w.name.is_empty() && w.role != Role::Label
                    })
                    .and_then(|w| {
                        if !w.enabled {
                            return Err(format!("{:?} is turned off right now", w.name));
                        }
                        events.push(action(Action::Click, w.id));
                        Ok(json!({ "clicked": w.name }))
                    }),
                ),
                Cmd::Type { name, text } => Some({
                    let target = match name {
                        Some(n) => find(&widgets, n, |w| {
                            w.focusable
                                && matches!(
                                    w.role,
                                    Role::TextInput
                                        | Role::PasswordInput
                                        | Role::MultilineTextInput
                                )
                        })
                        .map(|w| Some(w.id)),
                        None => Ok(None),
                    };
                    target.map(|id| {
                        if let Some(id) = id {
                            events.push(action(Action::Focus, id));
                        }
                        events.push(egui::Event::Text(text.clone()));
                        json!({ "typed": text.chars().count() })
                    })
                }),
                Cmd::Key(k) => Some(match egui::Key::from_name(k) {
                    Some(key) => {
                        for pressed in [true, false] {
                            events.push(egui::Event::Key {
                                key,
                                physical_key: None,
                                pressed,
                                repeat: false,
                                modifiers: Default::default(),
                            });
                        }
                        Ok(json!({ "key": key.name() }))
                    }
                    None => Err(format!("no key named {k:?}")),
                }),
                Cmd::Choose(path) => {
                    host.choose(path.clone());
                    Some(Ok(json!({ "next_dialog": path })))
                }
                Cmd::Wait(limit) => {
                    if !host.working() {
                        Some(Ok(
                            json!({ "waited_ms": since.elapsed().as_millis() as u64 }),
                        ))
                    } else if since.elapsed() > *limit {
                        Some(Err(format!("still busy after {} s", limit.as_secs())))
                    } else {
                        None
                    }
                }
                Cmd::Run(d) => (since.elapsed() >= *d).then(|| Ok(json!({}))),
                Cmd::Tree => Some(Ok(json!({ "widgets": widgets_json(&widgets) }))),
                Cmd::Status => Some(Ok(host.status())),
                Cmd::Screenshot(_) => return Need::Screenshot,
                Cmd::Quit => {
                    let p = self.queue.pop_front();
                    if let Some(p) = p {
                        self.answer(p.asker, Ok(json!({})), "quit");
                    }
                    return Need::Quit;
                }
            };
            let Some(result) = result else {
                return Need::Nothing;
            };
            let wants_frame = matches!(p.cmd, Cmd::Click(_) | Cmd::Type { .. } | Cmd::Key(_));
            let Some(p) = self.queue.pop_front() else {
                return Need::Nothing;
            };
            let what = p.cmd.describe();
            self.answer(p.asker, result, &what);
            self.frames = 0;
            if wants_frame {
                // Input plays this frame; the next command sees its effect.
                return Need::Nothing;
            }
        }
        Need::Nothing
    }

    /// After a frame: the tree it drew.
    pub fn after_frame(&mut self, update: Option<&accesskit::TreeUpdate>) {
        self.frames = self.frames.saturating_add(1);
        if let Some(u) = update {
            self.tree.update(u);
        }
    }

    /// Answers the screenshot at the head of the queue with `png`.
    pub fn screenshot_taken(&mut self, png: Result<Vec<u8>>) {
        let Some(p) = self.queue.pop_front() else {
            return;
        };
        self.frames = 0;
        let result = match (png, &p.cmd) {
            (Err(e), _) => Err(e.to_string()),
            (Ok(png), Cmd::Screenshot(Some(path))) => std::fs::write(path, &png)
                .map(|_| json!({ "written": path }))
                .map_err(|e| format!("could not write {}: {e}", path.display())),
            (Ok(png), _) => {
                use base64::Engine;
                Ok(json!({ "png": base64::engine::general_purpose::STANDARD.encode(png) }))
            }
        };
        self.answer(p.asker, result, "screenshot");
    }

    /// Answers a client (or, for steps, prints `tree` and `status` and stops on a failure).
    fn answer(&mut self, asker: Asker, result: std::result::Result<J, String>, what: &str) {
        match asker {
            Asker::Client(tx) => {
                let reply = match result {
                    Ok(mut v) => {
                        v["ok"] = json!(true);
                        v
                    }
                    Err(e) => control::failure(e),
                };
                let _ = tx.send(reply);
            }
            Asker::Steps => match result {
                Ok(v) => {
                    if what == "Tree" || what == "Status" {
                        println!("{v}");
                    }
                    log::info!("launcher step {what}: done");
                }
                Err(e) => {
                    log::error!("launcher step {what} failed: {e}");
                    eprintln!("launcher step {what} failed: {e}");
                    self.failed = true;
                    self.queue.retain(|p| !matches!(p.asker, Asker::Steps));
                }
            },
        }
    }

    /// Whether a step failed (the process exits non-zero once the window closes).
    pub fn failed(&self) -> bool {
        self.failed
    }
}

fn action(action: Action, target: NodeId) -> egui::Event {
    egui::Event::AccessKitActionRequest(accesskit::ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: target,
        data: None,
    })
}

/// What a script needs from the launcher.
pub trait Host {
    /// A search or download is running.
    fn working(&self) -> bool;
    fn status(&self) -> J;
    /// The next file or folder dialog answers `path`.
    fn choose(&mut self, path: PathBuf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_text_is_never_logged() {
        let c = Cmd::Type {
            name: Some("Password".into()),
            text: "hunter2".into(),
        };
        assert!(!c.describe().contains("hunter2"));
    }

    #[test]
    fn steps_parse() {
        let s = parse_steps(
            "click Search this computer; wait; wait 5; type Account name=me; type =x; \
             key Enter; choose /g/dao; run 250; tree; status; shot a.png; quit",
        )
        .unwrap();
        assert_eq!(s[0], Cmd::Click("Search this computer".into()));
        assert_eq!(s[1], Cmd::Wait(Duration::from_secs(300)));
        assert_eq!(s[2], Cmd::Wait(Duration::from_secs(5)));
        assert_eq!(
            s[3],
            Cmd::Type {
                name: Some("Account name".into()),
                text: "me".into()
            }
        );
        assert_eq!(
            s[4],
            Cmd::Type {
                name: None,
                text: "x".into()
            }
        );
        assert_eq!(s[6], Cmd::Choose("/g/dao".into()));
        assert_eq!(s[7], Cmd::Run(Duration::from_millis(250)));
        assert_eq!(s[10], Cmd::Screenshot(Some("a.png".into())));
        assert_eq!(s[11], Cmd::Quit);
        for bad in ["click", "dance", "run x", "wait -1", "choose"] {
            assert!(parse_steps(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn requests_parse() {
        let c = |v: J| parse_request(&v).unwrap();
        assert_eq!(
            c(json!({"cmd":"click","name":"Continue"})),
            Cmd::Click("Continue".into())
        );
        assert_eq!(
            c(json!({"cmd":"click","label":"Back"})),
            Cmd::Click("Back".into())
        );
        assert_eq!(c(json!({"cmd":"screenshot"})), Cmd::Screenshot(None));
        assert!(parse_request(&json!({"cmd":"fly"})).is_err());
    }

    fn w(name: &str, clickable: bool) -> Widget {
        Widget {
            id: NodeId(name.len() as u64),
            role: Role::Button,
            name: name.into(),
            value: None,
            enabled: true,
            checked: None,
            clickable,
            focusable: clickable,
        }
    }

    #[test]
    fn names_are_found() {
        let ws = vec![
            w("Search this computer", true),
            w("Search again", true),
            w("Continue", true),
            w("Welcome", false),
        ];
        let f = |n| find(&ws, n, |w| w.clickable).map(|w| w.name.clone());
        assert_eq!(f("Continue").unwrap(), "Continue");
        assert_eq!(f("continue").unwrap(), "Continue");
        assert_eq!(f("this computer").unwrap(), "Search this computer");
        assert!(f("Search").unwrap_err().contains("could be any of"));
        assert!(f("Welcome").unwrap_err().contains("nothing named"));
    }

    #[test]
    fn the_tree_follows_updates() {
        let mut root = Node::new(Role::Window);
        root.set_children(vec![NodeId(2), NodeId(3)]);
        let mut b = Node::new(Role::Button);
        b.set_label("Play");
        b.add_action(Action::Click);
        let mut l = Node::new(Role::Label);
        l.set_value("Welcome");
        let mut t = Tree::default();
        t.update(&accesskit::TreeUpdate {
            nodes: vec![(NodeId(1), root.clone()), (NodeId(2), b), (NodeId(3), l)],
            tree: Some(accesskit::TreeInfo::new(NodeId(1))),
            tree_id: TreeId::ROOT,
            focus: NodeId(1),
        });
        let ws = t.widgets();
        assert_eq!(ws.len(), 2);
        assert_eq!((ws[0].name.as_str(), ws[0].clickable), ("Play", true));
        assert_eq!(ws[1].name, "Welcome");
        // The button leaves the page.
        root.set_children(vec![NodeId(3)]);
        t.update(&accesskit::TreeUpdate {
            nodes: vec![(NodeId(1), root)],
            tree: None,
            tree_id: TreeId::ROOT,
            focus: NodeId(1),
        });
        assert_eq!(t.widgets().len(), 1);
    }
}
