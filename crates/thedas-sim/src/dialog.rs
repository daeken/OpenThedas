//! Conversations (`.dlg`, GFF4 `CONV`) and the state machine that walks them.
//!
//! A conversation is a list of nodes: NPC lines (`NTRY`) and player replies (`RPLY`). Field
//! `12000` lists candidate start nodes, `12400` on each node its children (indices into the
//! node list). Each node has a condition (`12208`) and an action (`12209`): a plot flag (GUID
//! `12300`, flag `12301`, value `12302`: 255 = set, 0 = clear) and/or a script (`12303`).
//! Text is a talk-table reference (`12201`); `12202`/`12203` name speaker and listener
//! (`OWNER`, `PLAYER` or a tag).
//!
//! Walking (provisional): the first start node whose condition passes is spoken; its action
//! runs; the player's choices are its children whose conditions pass. Choosing a reply runs the
//! reply's action, then the first passing child line is spoken. The conversation ends when a
//! line has no available replies or a reply leads nowhere.

use thedas_formats::{Gff4, List, StructId, Value};
use thedas_script::vm::{OBJECT_INVALID, ObjectId, Value as V};

use crate::sim::Sim;
use crate::trace::TraceEntry;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlotRef {
    pub plot: Option<String>,
    pub flag: i32,
    /// The value tested or set.
    pub value: bool,
    pub script: Option<String>,
}

impl PlotRef {
    pub fn is_empty(&self) -> bool {
        (self.plot.is_none() || self.flag < 0) && self.script.is_none()
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub reply: bool,
    pub strref: Option<u32>,
    pub text: Option<String>,
    pub speaker: String,
    pub listener: String,
    pub condition: PlotRef,
    pub action: PlotRef,
    pub children: Vec<usize>,
    /// Field 12206, taken to be the toolset's "show once": 0 always, 1 once per
    /// conversation, 2 once per game. Provisional.
    #[serde(default)]
    pub show_once: u8,
    /// Field 12207, taken to mark an ambient line (said in passing, without the dialog
    /// screen): chants and barks. Provisional.
    #[serde(default)]
    pub ambient: bool,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Dlg {
    pub name: String,
    pub starts: Vec<usize>,
    pub nodes: Vec<Node>,
}

fn plotref(g: &Gff4, id: Option<StructId>) -> PlotRef {
    let Some(id) = id else {
        return PlotRef::default();
    };
    let s = |l| {
        g.field(id, l)
            .and_then(|v| v.as_str())
            .map(|x| x.trim_end_matches('\0').to_string())
            .filter(|x| !x.is_empty())
    };
    PlotRef {
        plot: s(12300),
        flag: g.field(id, 12301).and_then(|v| v.as_i64()).unwrap_or(-1) as i32,
        value: g.field(id, 12302).and_then(|v| v.as_i64()).unwrap_or(255) != 0,
        script: s(12303),
    }
}

impl Dlg {
    pub fn parse(data: &[u8], name: &str) -> Option<Dlg> {
        Self::from_gff4(&Gff4::parse(data).ok()?, name)
    }

    pub fn from_gff4(g: &Gff4, name: &str) -> Option<Dlg> {
        if g.file_type.0 != *b"CONV" {
            return None;
        }
        let ints = |id: StructId, l: u32| -> Vec<usize> {
            match g.field(id, l) {
                Some(Value::List(Some(List::U32(v)))) => v.iter().map(|&x| x as usize).collect(),
                Some(Value::List(Some(List::I32(v)))) => {
                    v.iter().map(|&x| x.max(0) as usize).collect()
                }
                Some(Value::List(Some(List::U16(v)))) => v.iter().map(|&x| x as usize).collect(),
                _ => Vec::new(),
            }
        };
        let node_ids: Vec<StructId> = match g.field(g.root, 12002) {
            Some(Value::List(Some(List::Generic(slots)))) => slots
                .iter()
                .filter_map(|s| match &s.value {
                    Some(Value::Struct(Some(id))) => Some(*id),
                    _ => None,
                })
                .collect(),
            Some(Value::List(Some(List::Struct(v)))) => v.iter().flatten().copied().collect(),
            _ => Vec::new(),
        };
        let mut nodes = Vec::new();
        for id in &node_ids {
            let label = g.template_of(*id).label;
            let st = |l| match g.field(*id, l) {
                Some(Value::Struct(s)) => *s,
                _ => None,
            };
            let (strref, text) = match g.field(*id, 12201) {
                Some(Value::Tlk(t)) => (
                    (t.id != u32::MAX).then_some(t.id),
                    t.text.clone().map(|x| x.trim_end_matches('\0').to_string()),
                ),
                _ => (None, None),
            };
            let s = |l| {
                g.field(*id, l)
                    .and_then(|v| v.as_str())
                    .map(|x| x.trim_end_matches('\0').to_string())
                    .unwrap_or_default()
            };
            nodes.push(Node {
                reply: label.0 == *b"RPLY",
                strref,
                text,
                speaker: s(12202),
                listener: s(12203),
                condition: plotref(g, st(12208)),
                action: plotref(g, st(12209)),
                children: ints(*id, 12400),
                show_once: g
                    .field(*id, 12206)
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    .clamp(0, 2) as u8,
                ambient: g.field(*id, 12207).and_then(|v| v.as_i64()).unwrap_or(0) != 0,
            });
        }
        let n = nodes.len();
        for node in &mut nodes {
            node.children.retain(|&c| c < n);
        }
        Some(Dlg {
            name: name.to_string(),
            starts: ints(g.root, 12000).into_iter().filter(|&c| c < n).collect(),
            nodes,
        })
    }
}

/// A conversation in progress.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Conversation {
    pub dlg: Dlg,
    pub owner: ObjectId,
    pub player: ObjectId,
    /// The NPC line being shown.
    pub line: Option<usize>,
    /// The player's available replies (node indices).
    pub replies: Vec<usize>,
    pub ended: bool,
    /// Show-once-per-conversation nodes already used in this one.
    #[serde(default)]
    pub shown: Vec<usize>,
    /// NPC lines just spoken one after another (guards against looping chains).
    #[serde(default)]
    pub chain: Vec<usize>,
}

/// An open conversation as presentation sees it: its dialog (resource name without
/// `.dlg`), the node being shown (an index into the dialog's line list), its owner and the
/// player character in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpenLine<'a> {
    pub dialog: &'a str,
    pub line: Option<usize>,
    pub owner: ObjectId,
    pub player: ObjectId,
}

/// What a conversation looks like to a player: the current line and the choices.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct DialogState {
    pub dialog: String,
    pub owner: ObjectId,
    pub speaker: String,
    pub line: String,
    pub replies: Vec<(usize, String)>,
    pub ended: bool,
}

impl Sim {
    /// Whether a node may still be used: its condition passes and, if it shows once, it
    /// has not been shown (in this conversation, or ever).
    fn usable(&mut self, c: &Conversation, node: usize) -> bool {
        let n = &c.dlg.nodes[node];
        let fresh = match n.show_once {
            1 => !c.shown.contains(&node),
            2 => !self
                .world
                .dialog_shown
                .contains(&format!("{}:{node}", c.dlg.name)),
            _ => true,
        };
        fresh && {
            let cond = n.condition.clone();
            self.condition(&cond, c.owner)
        }
    }

    /// Records that a node was used, for show-once nodes.
    fn mark_shown(&mut self, c: &mut Conversation, node: usize) {
        match c.dlg.nodes[node].show_once {
            1 => c.shown.push(node),
            2 => {
                self.world
                    .dialog_shown
                    .insert(format!("{}:{node}", c.dlg.name));
            }
            _ => {}
        }
    }

    fn condition(&mut self, c: &PlotRef, owner: ObjectId) -> bool {
        if let Some(script) = &c.script {
            let r = self.run_script(script, owner, None);
            if !matches!(r, Some(V::Int(v)) if v != 0) {
                return false;
            }
        }
        match &c.plot {
            Some(plot) if c.flag >= 0 => self.get_plot_flag(plot, c.flag, true, owner) == c.value,
            _ => true,
        }
    }

    fn action(&mut self, a: &PlotRef, owner: ObjectId) {
        if let Some(plot) = &a.plot {
            if a.flag >= 0 {
                self.set_plot_flag(plot, a.flag, a.value, true, owner);
            }
        }
        if let Some(script) = &a.script {
            self.run_script(script, owner, None);
        }
    }

    pub fn node_text(&self, n: &Node) -> String {
        n.text
            .clone()
            .or_else(|| n.strref.and_then(|s| self.string(s)))
            .unwrap_or_default()
    }

    /// Speaks a line: runs its action and collects the replies.
    fn speak(&mut self, mut c: Conversation, line: usize) -> Conversation {
        let node = c.dlg.nodes[line].clone();
        self.mark_shown(&mut c, line);
        self.action(&node.action, c.owner);
        let text = self.node_text(&node);
        let speaker = match node.speaker.as_str() {
            "OWNER" | "" => c.owner,
            "PLAYER" => c.player,
            tag => self.world.by_tag(tag).next().map_or(c.owner, |o| o.id),
        };
        let t = self.world.time_ms;
        self.world
            .trace
            .push(TraceEntry::Dialog { t, speaker, text });
        c.line = Some(line);
        let mut replies = Vec::new();
        for &r in &node.children {
            if self.usable(&c, r) {
                replies.push(r);
            }
        }
        // An NPC line followed directly by another NPC line continues on its own (but not
        // round a loop of lines already spoken in this exchange).
        if let Some(&next) = replies.first() {
            if !c.dlg.nodes[next].reply
                && next != line
                && c.chain.len() < 64
                && !c.chain.contains(&next)
            {
                c.chain.push(line);
                return self.speak(c, next);
            }
        }
        c.chain.clear();
        c.ended = replies.is_empty();
        c.replies = replies;
        c
    }

    /// Starts a conversation between `initiator` and `target` (the owner); an empty `dlg`
    /// uses the target's own conversation. Returns whether it started.
    pub fn start_conversation(&mut self, initiator: ObjectId, target: ObjectId, dlg: &str) -> bool {
        let name = if dlg.is_empty() {
            match self.world.get(target).and_then(|o| o.dialog.clone()) {
                Some(d) => d,
                None => {
                    let t = self.world.time_ms;
                    self.world.trace.push(TraceEntry::Missing {
                        t,
                        what: format!(
                            "conversation: {initiator} -> {target} names no dialog and the target has none"
                        ),
                    });
                    return false;
                }
            }
        } else {
            dlg.trim_end_matches(".dlg").to_string()
        };
        let Some(d) = self
            .vfs
            .read(&format!("{}.dlg", name.to_ascii_lowercase()))
            .ok()
            .and_then(|(data, _)| Dlg::parse(&data, &name))
        else {
            let t = self.world.time_ms;
            self.world.trace.push(TraceEntry::Missing {
                t,
                what: format!("conversation {name}"),
            });
            return false;
        };
        let player = if Some(initiator) == self.world.hero || target == OBJECT_INVALID {
            initiator
        } else {
            self.world.hero.unwrap_or(initiator)
        };
        let c = Conversation {
            dlg: d.clone(),
            owner: target,
            player,
            line: None,
            replies: Vec::new(),
            ended: false,
            shown: Vec::new(),
            chain: Vec::new(),
        };
        let start = d.starts.iter().copied().find(|&s| self.usable(&c, s));
        let c = match start {
            Some(s) => self.speak(c, s),
            None => Conversation { ended: true, ..c },
        };
        // Between creatures outside the party it is an ambient conversation: it plays out
        // around the player without waiting for them. Provisional: all at once.
        let party = crate::engine::party_of(self);
        let ambient_start = start.is_some_and(|s| d.nodes[s].ambient);
        if ambient_start || (!party.contains(&initiator) && !party.contains(&target)) {
            self.run_ambient(c);
            return true;
        }
        self.talk = Some(c);
        true
    }

    /// Plays an ambient conversation to its end, taking the first reply each time and
    /// stopping if it comes back to a node it has used (chants loop).
    fn run_ambient(&mut self, mut c: Conversation) {
        let mut used: Vec<usize> = c.line.into_iter().collect();
        for _ in 0..64 {
            if c.ended {
                break;
            }
            let Some(&r) = c.replies.first() else { break };
            if used.contains(&r) {
                break;
            }
            used.push(r);
            let node = c.dlg.nodes[r].clone();
            self.mark_shown(&mut c, r);
            self.action(&node.action, c.owner);
            match node.children.iter().copied().find(|&n| self.usable(&c, n)) {
                Some(n) if !used.contains(&n) => {
                    used.push(n);
                    c = self.speak(c, n);
                }
                _ => break,
            }
        }
    }

    /// Picks reply `choice` (an index into the current replies).
    pub fn choose_reply(&mut self, choice: usize) -> Option<DialogState> {
        let mut c = self.talk.take()?;
        let Some(&reply) = c.replies.get(choice) else {
            self.talk = Some(c);
            return self.dialog_state();
        };
        let node = c.dlg.nodes[reply].clone();
        self.mark_shown(&mut c, reply);
        self.action(&node.action, c.owner);
        // The action started another conversation (a plot script's UT_Talk): it replaces
        // this one, as in the game.
        if self.talk.is_some() {
            return self.dialog_state();
        }
        let next = node.children.iter().copied().find(|&n| self.usable(&c, n));
        c = match next {
            Some(n) => self.speak(c, n),
            None => Conversation {
                ended: true,
                replies: Vec::new(),
                ..c
            },
        };
        if self.talk.is_none() {
            self.talk = Some(c);
        }
        self.dialog_state()
    }

    /// The open conversation, for presentation.
    pub fn conversation_line(&self) -> Option<OpenLine<'_>> {
        let c = self.talk.as_ref().filter(|c| !c.ended)?;
        Some(OpenLine {
            dialog: c.dlg.name.as_str(),
            line: c.line,
            owner: c.owner,
            player: c.player,
        })
    }

    pub fn dialog_state(&self) -> Option<DialogState> {
        let c = self.talk.as_ref()?;
        let line = c.line.map(|l| &c.dlg.nodes[l]);
        Some(DialogState {
            dialog: c.dlg.name.clone(),
            owner: c.owner,
            speaker: line.map(|n| n.speaker.clone()).unwrap_or_default(),
            line: line.map(|n| self.node_text(n)).unwrap_or_default(),
            replies: c
                .replies
                .iter()
                .map(|&r| {
                    let t = self.node_text(&c.dlg.nodes[r]);
                    (
                        r,
                        if t.is_empty() {
                            "[continue]".to_string()
                        } else {
                            t
                        },
                    )
                })
                .collect(),
            ended: c.ended,
        })
    }

    pub fn end_conversation(&mut self) {
        self.talk = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::gff4::{
        FieldDef, FourCC, GenericSlot, StructData, StructTemplate, TlkString, Version,
    };

    fn doc() -> Gff4 {
        let tmpl = |label: &[u8; 4], fields: &[u32]| StructTemplate {
            label: FourCC::new(label),
            fields: fields
                .iter()
                .map(|&l| FieldDef {
                    label: l,
                    type_code: 0,
                    flags: 0,
                    offset: 0,
                })
                .collect(),
            size: 0,
        };
        let node = |t: u32, strref: u32, children: Vec<u32>| StructData {
            template: t,
            values: vec![
                Value::Tlk(TlkString {
                    id: strref,
                    text: Some(format!("line {strref}")),
                }),
                Value::String(Some("OWNER".into())),
                Value::List(Some(List::U32(children))),
            ],
        };
        let slot = |i: usize| GenericSlot {
            type_code: 0,
            flags: 0,
            value: Some(Value::Struct(Some(StructId(i)))),
        };
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"CONV"),
            file_version: FourCC::new(b"V0.1"),
            templates: vec![
                tmpl(b"CONV", &[12000, 12002]),
                tmpl(b"NTRY", &[12201, 12202, 12400]),
                tmpl(b"RPLY", &[12201, 12202, 12400]),
            ],
            structs: vec![
                StructData {
                    template: 0,
                    values: vec![
                        Value::List(Some(List::U32(vec![0, 99]))),
                        Value::List(Some(List::Generic(vec![
                            slot(1),
                            slot(2),
                            slot(3),
                            slot(4),
                        ]))),
                    ],
                },
                node(1, 10, vec![1, 2]),
                node(2, 11, vec![3]),
                node(2, 12, vec![]),
                node(1, 13, vec![]),
            ],
            root: StructId(0),
            shared_strings: None,
        }
    }

    #[test]
    fn parses_and_walks_a_conversation() {
        let d = Dlg::from_gff4(&doc(), "test").unwrap();
        assert_eq!(d.starts, vec![0], "out-of-range start dropped");
        assert_eq!(d.nodes.len(), 4);
        assert!(d.nodes[1].reply && !d.nodes[0].reply);
        assert_eq!(d.nodes[0].children, vec![1, 2]);

        let table = std::sync::Arc::new(thedas_script::EngineTable::default());
        let mut sim = Sim::new(std::sync::Arc::new(thedas_vfs::Vfs::new()), table, 1);
        let npc = sim.world.spawn(crate::world::ObjectKind::Creature, "npc");
        let hero = sim
            .world
            .spawn(crate::world::ObjectKind::Creature, "player");
        sim.world.hero = Some(hero);
        sim.talk = Some(Conversation {
            dlg: d.clone(),
            owner: npc,
            player: hero,
            line: None,
            replies: vec![],
            ended: false,
            shown: vec![],
            chain: vec![],
        });
        let c = sim.talk.take().unwrap();
        let c = sim.speak(c, 0);
        sim.talk = Some(c);
        let st = sim.dialog_state().unwrap();
        assert_eq!(st.line, "line 10");
        assert_eq!(st.replies.len(), 2);
        let st = sim.choose_reply(0).unwrap();
        assert_eq!(st.line, "line 13");
        assert!(st.ended);
    }

    #[test]
    fn show_once_lines_are_skipped_the_second_time() {
        let node = |reply: bool, text: &str, children: Vec<usize>, show_once: u8| Node {
            reply,
            strref: None,
            text: Some(text.into()),
            speaker: String::new(),
            listener: String::new(),
            condition: PlotRef::default(),
            action: PlotRef::default(),
            children,
            show_once,
            ambient: false,
        };
        let d = Dlg {
            name: "cage".into(),
            starts: vec![0],
            nodes: vec![
                node(false, "...", vec![1], 0),
                node(true, "", vec![2, 3], 0),
                node(false, "a prayer", vec![], 2),
                node(false, "what do you want?", vec![], 0),
            ],
        };
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let talk_once = |sim: &mut Sim| {
            let c = Conversation {
                dlg: d.clone(),
                owner: 1,
                player: 2,
                line: None,
                replies: vec![],
                ended: false,
                shown: vec![],
                chain: vec![],
            };
            let c = sim.speak(c, 0);
            sim.talk = Some(c);
            sim.choose_reply(0).unwrap().line
        };
        assert_eq!(talk_once(&mut sim), "a prayer");
        assert_eq!(talk_once(&mut sim), "what do you want?");
    }
}
