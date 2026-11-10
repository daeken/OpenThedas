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
            let cond = c.dlg.nodes[r].condition.clone();
            if self.condition(&cond, c.owner) {
                replies.push(r);
            }
        }
        // An NPC line followed directly by another NPC line continues on its own.
        if let Some(&next) = replies.first() {
            if !c.dlg.nodes[next].reply {
                return self.speak(c, next);
            }
        }
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
                None => return false,
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
        };
        let start = d.starts.iter().copied().find(|&s| {
            let cond = d.nodes[s].condition.clone();
            self.condition(&cond, target)
        });
        let c = match start {
            Some(s) => self.speak(c, s),
            None => Conversation { ended: true, ..c },
        };
        self.talk = Some(c);
        true
    }

    /// Picks reply `choice` (an index into the current replies).
    pub fn choose_reply(&mut self, choice: usize) -> Option<DialogState> {
        let mut c = self.talk.take()?;
        let Some(&reply) = c.replies.get(choice) else {
            self.talk = Some(c);
            return self.dialog_state();
        };
        let node = c.dlg.nodes[reply].clone();
        self.action(&node.action, c.owner);
        let next = node.children.iter().copied().find(|&n| {
            let cond = c.dlg.nodes[n].condition.clone();
            self.condition(&cond, c.owner)
        });
        c = match next {
            Some(n) => self.speak(c, n),
            None => Conversation {
                ended: true,
                replies: Vec::new(),
                ..c
            },
        };
        self.talk = Some(c);
        self.dialog_state()
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
}
