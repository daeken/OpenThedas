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
    /// Field 12207, the line's ambient flag (said in passing, without the dialog screen:
    /// chants and barks). DAOrigins.exe reads it before running the line's scripts and
    /// answers `DialogIsAmbient` from it (0x7f049a).
    #[serde(default)]
    pub ambient: bool,
    /// The cutscene the line plays (field 12211 when it is a `CUTR` reference: its 12210,
    /// a `.cut` resource). An inline `CUT` (the line's own staging) is not one: the
    /// presentation stages those (`thedas_formats::stage`).
    #[serde(default)]
    pub cutscene: Option<String>,
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
                cutscene: match g.field(*id, 12211) {
                    Some(Value::Struct(s)) => *s,
                    Some(Value::Generic(slot)) => match &slot.value {
                        Some(Value::Struct(s)) => *s,
                        _ => None,
                    },
                    _ => None,
                }
                .filter(|&c| g.template_of(c).label.0 == *b"CUTR")
                .and_then(|c| g.field(c, 12210))
                .and_then(|v| v.as_str())
                .map(|x| x.trim_end_matches('\0').trim().to_string())
                .filter(|x| !x.is_empty()),
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
    /// The line whose cutscene was queued last, which the cutscene's spoken lines are
    /// counted from (see [`Sim::dialog_cutscene_lines`]).
    #[serde(default)]
    pub cut_from: Option<usize>,
    /// Lines of the conversation the queued cutscene still says after the one that queued
    /// it: the conversation walks on through them (single unspoken replies) as it speaks.
    #[serde(default)]
    pub cut_lines_left: u32,
    /// The creatures the shown line's own cutscene took, and where it leaves them when the
    /// line ends (see [`crate::cutactors`]).
    #[serde(default)]
    pub staged: Vec<crate::cutactors::Bound>,
    /// Where the shown line's own cutscene plays.
    #[serde(default)]
    pub staged_at: Option<crate::cuttracks::Placement>,
    /// Played out around the player without the dialog screen (see
    /// [`Sim::start_conversation`]): its lines stage nobody.
    #[serde(default)]
    pub ambient_run: bool,
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
    /// Whether the player has replies to choose among (not only an unspoken continue).
    pub choosing: bool,
    /// The shown line's talk-table reference: its voice-over is the sample named after it.
    pub strref: Option<u32>,
    /// The shown line is ambient (said in passing).
    pub ambient: bool,
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
            self.dialog_ambient = n.ambient;
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

    /// Runs a line's action, with `DialogIsAmbient` telling whether the line is ambient.
    fn node_action(&mut self, node: &Node, owner: ObjectId) {
        self.dialog_ambient = node.ambient;
        self.action(&node.action, owner);
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
        // The line shown before ends, and its cutscene with it.
        self.end_line_staging(&mut c);
        let node = c.dlg.nodes[line].clone();
        self.mark_shown(&mut c, line);
        self.node_action(&node, c.owner);
        if !c.ambient_run && !node.ambient {
            self.begin_line_staging(&mut c, line);
        }
        if let Some(cut) = node.cutscene.as_ref().filter(|_| !node.ambient) {
            // The line's cutscene plays (GameModeDialog 0x7f1d46 -> 0x690b70); the lines it
            // says of this conversation are walked as it is spoken.
            let says = self.dialog_cutscene_lines(cut, &c.dlg.name);
            c.cut_from = Some(line);
            c.cut_lines_left = says.saturating_sub(1);
            // Once: a conversation begun again at the same line (scripts begin it more
            // than once) does not queue it twice.
            let res = crate::cutscene::Cutscene::resource(cut);
            let queued = self.world.dialog_cutscenes.contains(&res)
                || self.world.cutscene.as_ref().is_some_and(|p| p.name == res);
            if !queued {
                self.world.cutscenes.push(cut.clone());
                self.world.dialog_cutscenes.push(res);
            }
        }
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
        // A line its cutscene says goes by on its own: a single unspoken reply is taken.
        if c.cut_lines_left > 0
            && let [r] = replies[..]
            && c.dlg.nodes[r].reply
            && self.node_text(&c.dlg.nodes[r]).trim().is_empty()
        {
            c.cut_lines_left -= 1;
            let reply = c.dlg.nodes[r].clone();
            self.mark_shown(&mut c, r);
            self.node_action(&reply, c.owner);
            let next = reply.children.iter().copied().find(|&n| self.usable(&c, n));
            if let Some(next) = next.filter(|&n| !c.dlg.nodes[n].reply && n != line) {
                return self.speak(c, next);
            }
        }
        c.cut_lines_left = 0;
        c.ended = replies.is_empty();
        c.replies = replies;
        c
    }

    /// Where conversation `c`'s line `line` plays its cutscene `tracks` (see
    /// [`crate::cutactors`]): on the line's stage when the cutscene was made on one, else
    /// where it was made. `None` when the stage is not in the area.
    pub fn line_context(
        &mut self,
        c: &Conversation,
        line: usize,
        tracks: &crate::cuttracks::Tracks,
    ) -> Option<crate::cutactors::LineContext> {
        let cuts = self.conversation_line_cuts(&c.dlg.name);
        let lc = cuts.get(line)?;
        let speaker = c
            .dlg
            .nodes
            .get(line)
            .map(|n| n.speaker.trim_end_matches('\0').to_string())
            .filter(|s| !s.is_empty())
            .and_then(|s| self.cutscene_actor_object(&s, Some(c.owner), Some(c.player)))
            .unwrap_or(c.owner);
        let placement = match tracks.stage_file {
            Some(_) => {
                self.conversation_placement(lc.stage.as_deref(), lc.unstaged, speaker, c.player)?
            }
            None => crate::cuttracks::Placement::NONE,
        };
        Some(crate::cutactors::LineContext {
            owner: c.owner,
            player: c.player,
            unstaged: lc.unstaged,
            placement,
        })
    }

    /// Line `line`'s own cutscene starts: the creatures it stages are taken.
    fn begin_line_staging(&mut self, c: &mut Conversation, line: usize) {
        let cuts = self.conversation_line_cuts(&c.dlg.name);
        let Some(tracks) = cuts.get(line).and_then(|l| l.tracks.clone()) else {
            return;
        };
        let Some(ctx) = self.line_context(c, line, &tracks) else {
            return;
        };
        c.staged = self.bind_cutscene_actors(&tracks, Some(ctx));
        c.staged_at = Some(ctx.placement);
    }

    /// The shown line's own cutscene ends: the creatures it took stay on their marks.
    fn end_line_staging(&mut self, c: &mut Conversation) {
        let staged = std::mem::take(&mut c.staged);
        c.staged_at = None;
        self.release_cutscene_actors(staged);
    }

    /// How many lines of conversation `dlg` cutscene `cut` says (its `ASLN` tracks naming
    /// that conversation, 5562). Provisional: a cutscene's spoken lines are taken to be
    /// the conversation's lines in the order it walks them, from the line that plays it.
    pub fn dialog_cutscene_lines(&self, cut: &str, dlg: &str) -> u32 {
        let res = crate::cutscene::Cutscene::resource(cut);
        let Some(g) = self
            .vfs
            .read(&res)
            .ok()
            .and_then(|(data, _)| Gff4::parse(&data).ok())
        else {
            return 0;
        };
        crate::cutscene::dialog_refs(&g)
            .iter()
            .filter(|r| r.dialog.eq_ignore_ascii_case(dlg))
            .count() as u32
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
            cut_from: None,
            cut_lines_left: 0,
            staged: Vec::new(),
            staged_at: None,
            ambient_run: false,
        };
        let start = d.starts.iter().copied().find(|&s| self.usable(&c, s));
        // Between creatures outside the party it is an ambient conversation: it plays out
        // around the player without waiting for them. Provisional: all at once.
        let ambient_start = start.is_some_and(|s| d.nodes[s].ambient);
        let ambient = |sim: &Self| {
            let party = crate::engine::party_of(sim);
            ambient_start || (!party.contains(&initiator) && !party.contains(&target))
        };
        let c = Conversation {
            ambient_run: ambient(self),
            ..c
        };
        let c = match start {
            Some(s) => self.speak(c, s),
            None => Conversation { ended: true, ..c },
        };
        if ambient(self) {
            let c = Conversation {
                ambient_run: true,
                staged: Vec::new(),
                staged_at: None,
                ..c
            };
            self.run_ambient(c);
            return true;
        }
        // A conversation begun over another ends the other's line.
        if let Some(mut old) = self.talk.take() {
            self.end_line_staging(&mut old);
        }
        let live = !c.ended;
        self.talk = Some(c);
        if live {
            self.enter_conversation_mode();
        }
        true
    }

    /// The conversation takes the game (DAOrigins.exe 0x697765: `SetGameMode(4)`, which the
    /// mode controller grants at once even over combat, 0x640c60): the module hears
    /// GAMEMODE_CHANGE (GM_DIALOG, the old mode), on which module_core takes the party out
    /// of combat. Provisional: the mode the game returns to afterwards (the one replaced, or
    /// one a script asked for meanwhile) is not traced.
    fn enter_conversation_mode(&mut self) {
        let Some(talk_mode) = self
            .table
            .constant("GM_DIALOG")
            .and_then(|c| match &c.value {
                thedas_script::ldf::ConstValue::Int(v) => Some(*v as i32),
                _ => None,
            })
        else {
            return;
        };
        let old = self.world.game_mode;
        if old == talk_mode {
            return;
        }
        self.world.mode_before_talk = Some(old);
        self.world.game_mode = talk_mode;
        if let Some(m) = self.world.module {
            let mut ev = self.new_event("GAMEMODE_CHANGE", Some(m));
            ev.ints = vec![talk_mode, old];
            self.world.queue_event(0, m, ev, None);
        }
    }

    /// Once no conversation is going on, the game goes back to the mode the conversation
    /// replaced (or one a script asked for meanwhile), and the module hears it.
    pub(crate) fn leave_conversation_mode(&mut self) {
        let Some(before) = self.world.mode_before_talk else {
            return;
        };
        if self.talk.as_ref().is_some_and(|c| !c.ended) {
            return;
        }
        let next = self.world.mode_after_talk.take().unwrap_or(before);
        self.world.mode_before_talk = None;
        let old = self.world.game_mode;
        self.world.game_mode = next;
        if let Some(m) = self.world.module {
            let mut ev = self.new_event("GAMEMODE_CHANGE", Some(m));
            ev.ints = vec![next, old];
            self.world.queue_event(0, m, ev, None);
        }
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
            self.node_action(&node, c.owner);
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
        // The line answered ends. Provisional: before the reply's action runs.
        self.end_line_staging(&mut c);
        let node = c.dlg.nodes[reply].clone();
        self.mark_shown(&mut c, reply);
        self.node_action(&node, c.owner);
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

    /// The shown line's own cutscene, for presentation: the conversation (resource name
    /// without `.dlg`), the line, where its cutscene plays and the creatures it took.
    pub fn line_staging(
        &self,
    ) -> Option<(
        &str,
        usize,
        crate::cuttracks::Placement,
        &[crate::cutactors::Bound],
    )> {
        let c = self.talk.as_ref()?;
        Some((c.dlg.name.as_str(), c.line?, c.staged_at?, &c.staged))
    }

    /// The open conversation, for presentation.
    pub fn conversation_line(&self) -> Option<OpenLine<'_>> {
        let c = self.talk.as_ref().filter(|c| !c.ended)?;
        let node = c.line.and_then(|l| c.dlg.nodes.get(l));
        Some(OpenLine {
            dialog: c.dlg.name.as_str(),
            line: c.line,
            strref: node.and_then(|n| n.strref),
            ambient: node.is_some_and(|n| n.ambient),
            owner: c.owner,
            player: c.player,
            choosing: c
                .replies
                .iter()
                .any(|&r| !self.node_text(&c.dlg.nodes[r]).trim().is_empty()),
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
        if let Some(mut c) = self.talk.take() {
            self.end_line_staging(&mut c);
        }
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
            cut_from: None,
            cut_lines_left: 0,
            staged: Vec::new(),
            staged_at: None,
            ambient_run: false,
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
            cutscene: None,
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
                cut_from: None,
                cut_lines_left: 0,
                staged: Vec::new(),
                staged_at: None,
                ambient_run: false,
            };
            let c = sim.speak(c, 0);
            sim.talk = Some(c);
            sim.choose_reply(0).unwrap().line
        };
        assert_eq!(talk_once(&mut sim), "a prayer");
        assert_eq!(talk_once(&mut sim), "what do you want?");
    }

    #[test]
    fn a_lines_scripts_run_knowing_whether_it_is_ambient() {
        let node = |ambient: bool| Node {
            reply: false,
            strref: None,
            text: Some("...".into()),
            speaker: String::new(),
            listener: String::new(),
            condition: PlotRef::default(),
            action: PlotRef::default(),
            children: vec![],
            show_once: 0,
            ambient,
            cutscene: None,
        };
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let c = Conversation {
            dlg: Dlg {
                name: "bark".into(),
                starts: vec![0],
                nodes: vec![node(true), node(false)],
            },
            owner: 1,
            player: 2,
            line: None,
            replies: vec![],
            ended: false,
            shown: vec![],
            chain: vec![],
            cut_from: None,
            cut_lines_left: 0,
            staged: Vec::new(),
            staged_at: None,
            ambient_run: false,
        };
        assert!(sim.usable(&c, 0) && sim.dialog_ambient);
        assert!(sim.usable(&c, 1) && !sim.dialog_ambient);
        let c = sim.speak(c, 0);
        assert!(sim.dialog_ambient, "the ambient line's action ran");
        sim.speak(c, 1);
        assert!(!sim.dialog_ambient);
    }

    #[test]
    fn a_line_that_plays_a_cutscene_queues_it_once() {
        let node = |cutscene: Option<&str>, ambient: bool| Node {
            reply: false,
            strref: None,
            text: Some("...".into()),
            speaker: String::new(),
            listener: String::new(),
            condition: PlotRef::default(),
            action: PlotRef::default(),
            children: vec![],
            show_once: 0,
            ambient,
            cutscene: cutscene.map(Into::into),
        };
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let c = Conversation {
            dlg: Dlg {
                name: "father".into(),
                starts: vec![0],
                nodes: vec![node(Some("intro"), false), node(Some("bark_cut"), true)],
            },
            owner: 1,
            player: 2,
            line: None,
            replies: vec![],
            ended: false,
            shown: vec![],
            chain: vec![],
            cut_from: None,
            cut_lines_left: 0,
            staged: Vec::new(),
            staged_at: None,
            ambient_run: false,
        };
        let c = sim.speak(c, 0);
        assert_eq!(c.cut_from, Some(0));
        // Begun again at the same line, it is not queued twice; an ambient line plays none.
        let c = sim.speak(c, 0);
        sim.speak(c, 1);
        assert_eq!(sim.world.cutscenes, ["intro"]);
        assert_eq!(sim.world.dialog_cutscenes, ["intro.cut"]);
    }
}
