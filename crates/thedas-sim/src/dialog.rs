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

/// A conversation said in passing, under way (DAOrigins.exe's ambient dialogs, updated at
/// 0x6ff570): one line at a time, each for its own time, around a game that goes on.
#[derive(Clone, Debug, PartialEq)]
pub struct AmbientTalk {
    pub(crate) c: Conversation,
    /// The lines and replies it has used: it stops when it comes back to one (chants loop).
    pub(crate) used: Vec<usize>,
    /// How long its line still has, in milliseconds.
    pub(crate) left_ms: i64,
    /// Begun on a line flagged ambient (rather than between creatures outside the party):
    /// it ends at the first line that is not (see [`Sim::ambient_next`]).
    pub(crate) flagged: bool,
}

/// How long a line said in passing lasts without a voice, in milliseconds (0xb15250, set
/// at 0x6fe4f8).
pub const AMBIENT_LINE_MS: u32 = 7000;
/// The least a voiced line said in passing lasts, in milliseconds (0xae1998, set at
/// 0x6fe728 when its voice starts): it ends when this is over and its voice has stopped
/// (0x6ff713 to 0x6ff748).
pub const AMBIENT_VOICED_MIN_MS: u32 = 2000;

/// One line or reply of a conversation as its conditions stand now (see
/// [`Sim::conversation_conditions`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct NodeCondition {
    /// Its index in the conversation's line list.
    pub node: usize,
    /// A start of the conversation (one of its first lines).
    pub start: bool,
    /// A reply of the player's rather than a line.
    pub reply: bool,
    pub text: String,
    /// Whether it could be used now: its condition passes and, if it shows once, it has
    /// not been shown.
    pub usable: bool,
    /// The plot flags its condition read, in order.
    pub reads: Vec<crate::plot::FlagRead>,
    /// Plot flags its condition's scripts would set (a condition is not meant to change
    /// anything, but scripts can). The report itself changes nothing: they are undone.
    pub sets: Vec<crate::plot::FlagChange>,
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
        // A line said in passing takes its time: what follows it is said when it is over
        // (see [`Sim::advance_ambient`]). Presentation is told of it.
        if c.ambient_run {
            let (voiced, ms) = self.ambient_line_time(&c, line);
            if self.world.ambient_lines.len() >= 64 {
                self.world.ambient_lines.remove(0);
            }
            let text = self.node_text(&node);
            self.world.ambient_lines.push(crate::world::AmbientLine {
                speaker,
                dialog: c.dlg.name.clone(),
                strref: node.strref,
                text,
                female: self.player_is_female(c.player),
                voiced,
                seconds: ms as f32 / 1000.0,
            });
            c.ended = replies.is_empty();
            c.replies = replies;
            return c;
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

    /// Every line and reply of conversation `dialog` (empty: `owner`'s own) with the plot
    /// flags its condition reads and whether it could be used now, as the conversation would
    /// be opened by the hero with `owner`. The conditions run as they would when the
    /// conversation is walked, then the world is put back as it was: what a condition
    /// script would set is reported in `sets` and not kept (the world, the random numbers,
    /// the conversations going on). Simulation state outside the world (`training_done`,
    /// `following`, `next_follow_ms`) is not put back. `None` when there is no such
    /// conversation.
    pub fn conversation_conditions(
        &mut self,
        owner: ObjectId,
        dialog: &str,
    ) -> Option<Vec<NodeCondition>> {
        let name = if dialog.is_empty() {
            self.world.get(owner).and_then(|o| o.dialog.clone())?
        } else {
            dialog.trim_end_matches(".dlg").to_string()
        };
        let d = self
            .vfs
            .read(&format!("{}.dlg", name.to_ascii_lowercase()))
            .ok()
            .and_then(|(data, _)| Dlg::parse(&data, &name))?;
        Some(self.dlg_conditions(owner, &d))
    }

    /// [`Sim::conversation_conditions`] for a conversation already read.
    pub(crate) fn dlg_conditions(&mut self, owner: ObjectId, d: &Dlg) -> Vec<NodeCondition> {
        let player = self.world.hero.unwrap_or(owner);
        let c = Conversation {
            dlg: d.clone(),
            owner,
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
        // Conditions' scripts can set flags, draw random numbers and change objects: all of
        // it is undone when the report is made.
        let world = self.world.clone();
        let talk = self.talk.clone();
        let ambient = self.ambient.clone();
        let ambient_before = self.dialog_ambient;
        let mut out = Vec::new();
        for (i, n) in d.nodes.iter().enumerate() {
            self.plots.reads = Some(Vec::new());
            self.plots.sets = Some(Vec::new());
            let usable = self.usable(&c, i);
            let reads = self.plots.reads.take().unwrap_or_default();
            let sets = self.plots.sets.take().unwrap_or_default();
            out.push(NodeCondition {
                node: i,
                start: d.starts.contains(&i),
                reply: n.reply,
                text: self.node_text(n),
                usable,
                reads,
                sets,
            });
        }
        // The trace keeps what ran (it is not part of the world as saved).
        let trace = std::mem::take(&mut self.world.trace);
        self.world = world;
        self.world.trace = trace;
        self.talk = talk;
        self.ambient = ambient;
        self.dialog_ambient = ambient_before;
        out
    }

    /// Starts a conversation between `initiator` and `target` (the owner); an empty `dlg`
    /// uses the target's own conversation. Returns whether it started.
    pub fn start_conversation(&mut self, initiator: ObjectId, target: ObjectId, dlg: &str) -> bool {
        match self.load_conversation(initiator, target, dlg) {
            Some(d) => self.open_conversation(initiator, target, d, None),
            None => false,
        }
    }

    /// The conversation `dlg` names (empty: `target`'s own), read; `None` (traced as
    /// missing) when there is none.
    pub(crate) fn load_conversation(
        &mut self,
        initiator: ObjectId,
        target: ObjectId,
        dlg: &str,
    ) -> Option<Dlg> {
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
                    return None;
                }
            }
        } else {
            // Dragon Age II names its conversations with their `.cnv` extension.
            dlg.trim_end_matches(".dlg")
                .trim_end_matches(".cnv")
                .to_string()
        };
        let Some(d) = self
            .vfs
            .read(&format!("{}.dlg", name.to_ascii_lowercase()))
            .ok()
            .and_then(|(data, _)| Dlg::parse(&data, &name))
            .or_else(|| {
                // Dragon Age II's conversations are `.cnv` files (see `crate::cnv`).
                let (data, _) = self
                    .vfs
                    .read(&format!("{}.cnv", name.to_ascii_lowercase()))
                    .ok()?;
                crate::cnv::dlg_from_cnv(&Gff4::parse(&data).ok()?, &name)
            })
        else {
            let t = self.world.time_ms;
            self.world.trace.push(TraceEntry::Missing {
                t,
                what: format!("conversation {name}"),
            });
            return None;
        };
        Some(d)
    }

    /// Starts conversation `d` (read when it was asked for, as DAOrigins.exe loads it with
    /// the request, 0x68cee0 -> 0x7ef310) between `initiator` and `target`. `at` starts it
    /// at that line instead of the first usable start (mod mode's out-of-order testing):
    /// its condition is not asked, and it opens on the dialog screen unless the line itself
    /// is said in passing.
    pub(crate) fn open_conversation(
        &mut self,
        initiator: ObjectId,
        target: ObjectId,
        d: Dlg,
        at: Option<usize>,
    ) -> bool {
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
        let start = match at {
            Some(s) => Some(s),
            None => d.starts.iter().copied().find(|&s| self.usable(&c, s)),
        };
        // Between creatures outside the party it is an ambient conversation: it plays out
        // around the player without waiting for them. Provisional: all at once.
        let ambient_start = start.is_some_and(|s| d.nodes.get(s).is_some_and(|n| n.ambient));
        let ambient = |sim: &Self| {
            if at.is_some() {
                // A chosen line plays as it is: in passing only if it is said so.
                return ambient_start;
            }
            let party = crate::engine::party_of(sim);
            ambient_start || (!party.contains(&initiator) && !party.contains(&target))
        };
        // One asked for while another is going on waits for it to end: DAOrigins.exe's
        // conversation is a game mode request (0x697765), and a running conversation does
        // not give way (its +0x28 is false), so the request waits in the mode controller's
        // pending list (0x6414a0, 0x640c60). Its start is chosen when it starts.
        if !ambient(self) && self.talk.as_ref().is_some_and(|t| !t.ended) {
            self.world.pending_talks.push((initiator, target, d));
            return true;
        }
        // The speaker is already saying this conversation in passing: it goes on, and is
        // not begun again (the sweep clicked Alistair during his exchange with the mage at
        // Ostagar, and the exchange started over every time). Provisional: what
        // DAOrigins.exe does when a conversation is asked of a creature saying one in
        // passing is not traced.
        if ambient(self)
            && self
                .ambient
                .iter()
                .any(|a| a.c.owner == target && a.c.dlg.name == d.name)
        {
            return true;
        }
        let c = Conversation {
            ambient_run: ambient(self),
            ..c
        };
        let c = match start {
            Some(s) => self.speak(c, s),
            None => Conversation { ended: true, ..c },
        };
        if ambient(self) {
            // One at a time for a speaker: a new one takes the place of the one it was
            // saying. Provisional: what DAOrigins.exe does with a second ambient
            // conversation of the same owner is not traced.
            let owner = c.owner;
            self.ambient.retain(|a| a.c.owner != owner);
            if let Some(line) = c.line.filter(|_| start.is_some()) {
                let (_, ms) = self.ambient_line_time(&c, line);
                self.ambient.push(AmbientTalk {
                    used: vec![line],
                    left_ms: i64::from(ms),
                    flagged: ambient_start,
                    c,
                });
            }
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
        // A conversation waiting for this one starts in its place, in the same mode.
        while self.talk.as_ref().is_none_or(|c| c.ended) && !self.world.pending_talks.is_empty() {
            let (initiator, target, d) = self.world.pending_talks.remove(0);
            // Its owner must still be there to talk: an active, living creature, as when it
            // was asked for (DAOrigins.exe 0x6976b0). The City Elf's Valora, asked during
            // Vaughan's scene, is sent off with the bridal party when it ends. Provisional:
            // the exe makes these tests when the conversation is asked for; whether it makes
            // them again when a waiting one starts is not traced.
            let there = target == OBJECT_INVALID
                || self.world.get(target).is_some_and(|o| {
                    o.kind == crate::world::ObjectKind::Creature && o.active && !o.dead
                });
            if there {
                self.open_conversation(initiator, target, d, None);
            }
        }
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

    /// Whether line `line` of `c` has a voice, and how long it lasts said in passing, in
    /// milliseconds: [`AMBIENT_LINE_MS`] without a voice, else the voice's length and at
    /// least [`AMBIENT_VOICED_MIN_MS`] (DAOrigins.exe 0x6fe4a0 and 0x6ff6e5: the line's
    /// timer, then its voice still playing). The voice's length is read from the
    /// conversation's bank in the install ([`crate::voice`]); without an install no line is
    /// voiced. Provisional: the exe counts a line as voiced when its voice started to play,
    /// so with voices off or a sample that fails it takes the unvoiced time.
    fn ambient_line_time(&mut self, c: &Conversation, line: usize) -> (bool, u32) {
        let female = self.player_is_female(c.player);
        let voice = match (&self.install_root, c.dlg.nodes[line].strref) {
            (Some(root), Some(strref)) => self.voices.line_ms(root, &c.dlg.name, strref, female),
            _ => None,
        };
        match voice {
            Some(ms) => (true, ms.max(AMBIENT_VOICED_MIN_MS)),
            None => (false, AMBIENT_LINE_MS),
        }
    }

    /// Whether the player character of a conversation is female (her readings of the lines
    /// are the `_f` samples).
    fn player_is_female(&self, player: ObjectId) -> bool {
        self.world
            .get(player)
            .is_some_and(|o| o.fields.get("Gender").copied() == Some(2))
    }

    /// The conversations said in passing go on by `ms`: one whose line is over says its
    /// next, taking the first reply each time, or ends (DAOrigins.exe 0x6ff6e5 to 0x6ff7a8:
    /// the line's floaty goes, then 0x7f1910 picks what follows). Provisional: 0x7f1910 is
    /// not traced ("the first usable reply, then its first usable line" is how a
    /// conversation with nobody choosing reads); the exe holds these still in some game
    /// modes (0x6ff5a3 to 0x6ff633), here they run with the simulation's clock; they are
    /// not saved.
    pub(crate) fn advance_ambient(&mut self, ms: u64) {
        if self.ambient.is_empty() {
            return;
        }
        let mut going = Vec::new();
        for mut a in std::mem::take(&mut self.ambient) {
            // Its owner gone or dead, the rest is not said (provisional: not traced).
            if !self.world.get(a.c.owner).is_some_and(|o| !o.dead) {
                continue;
            }
            a.left_ms -= ms as i64;
            // A long step says the lines that fall within it, each for its own time.
            let mut a = Some(a);
            while let Some(t) = a.take() {
                if t.left_ms > 0 {
                    going.push(t);
                    break;
                }
                let over = t.left_ms;
                a = self.ambient_next(t).map(|mut n| {
                    n.left_ms += over;
                    n
                });
            }
        }
        // Those a line's action began meanwhile take their speakers' places.
        let begun = std::mem::take(&mut self.ambient);
        going.retain(|a| !begun.iter().any(|b| b.c.owner == a.c.owner));
        going.extend(begun);
        self.ambient = going;
    }

    /// The line of an ambient conversation is over: its next line is said, or `None` when
    /// it has ended (no reply left, or one it has used already).
    fn ambient_next(&mut self, a: AmbientTalk) -> Option<AmbientTalk> {
        let AmbientTalk {
            mut c,
            mut used,
            flagged,
            ..
        } = a;
        if c.ended {
            return None;
        }
        let &r = c.replies.first()?;
        if used.contains(&r) {
            return None;
        }
        // A choice for the player is not made in passing: there the conversation turns to
        // the player and opens as any other (Alistair, the mage gone: "You know, one good
        // thing about the Blight..." and the player's replies), or ends when another is
        // open. Provisional: 0x7f1910 is not traced.
        if c.dlg.nodes[r].reply && !self.node_text(&c.dlg.nodes[r]).trim().is_empty() {
            if self.talk.as_ref().is_none_or(|t| t.ended) {
                c.ambient_run = false;
                c.chain.clear();
                self.talk = Some(c);
                self.enter_conversation_mode();
            }
            return None;
        }
        used.push(r);
        // A reply nobody chooses is taken; a line that follows a line directly is said.
        let next = if c.dlg.nodes[r].reply {
            let node = c.dlg.nodes[r].clone();
            self.mark_shown(&mut c, r);
            self.node_action(&node, c.owner);
            let n = node
                .children
                .iter()
                .copied()
                .find(|&n| self.usable(&c, n))?;
            if used.contains(&n) {
                return None;
            }
            used.push(n);
            n
        } else {
            r
        };
        // One begun on an ambient line is said in passing as far as its ambient lines go:
        // Alistair's exchange with the mage at Ostagar runs on into his own conversation
        // with the player ("You know, one good thing about the Blight..."), which waits
        // for the player to speak to him. Provisional: 0x7f1910 is not traced.
        if flagged && !c.dlg.nodes[next].ambient {
            return None;
        }
        let c = self.speak(c, next);
        let (_, ms) = self.ambient_line_time(&c, next);
        Some(AmbientTalk {
            c,
            used,
            left_ms: i64::from(ms),
            flagged,
        })
    }

    /// Stops the conversations `owner` is saying in passing (all of them for
    /// [`OBJECT_INVALID`]): what was still to be said is not (`ClearAmbientDialogs`,
    /// DAOrigins.exe 0x6510f0).
    pub(crate) fn clear_ambient(&mut self, owner: ObjectId) {
        self.ambient
            .retain(|a| owner != OBJECT_INVALID && a.c.owner != owner);
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
    /// The conversation line whose cutscene is playing or last played (the line that
    /// queued it; the conversation may have moved on to a later line meanwhile).
    pub fn cutscene_line(&self) -> Option<usize> {
        self.talk.as_ref().and_then(|c| c.cut_from)
    }

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
    fn a_conversation_asked_for_during_another_waits_for_it() {
        // The City Elf's hooligans: the hero's talk with Valora, ordered before the trigger,
        // came due during Vaughan's conversation and cut it short.
        let d = Dlg::from_gff4(&doc(), "test").unwrap();
        let named = |name: &str| Dlg {
            name: name.into(),
            ..d.clone()
        };
        let table = std::sync::Arc::new(thedas_script::EngineTable::default());
        let mut sim = Sim::new(std::sync::Arc::new(thedas_vfs::Vfs::new()), table, 1);
        let mut spawn = |tag: &str| sim.world.spawn(crate::world::ObjectKind::Creature, tag);
        let (hero, vaughan, valora) = (spawn("player"), spawn("vaughan"), spawn("valora"));
        sim.world.hero = Some(hero);
        assert!(sim.open_conversation(hero, vaughan, named("vaughan"), None));
        assert!(
            sim.open_conversation(hero, valora, named("valora"), None),
            "asked for"
        );
        let talk = sim.talk.as_ref().unwrap();
        assert_eq!((talk.dlg.name.as_str(), talk.owner), ("vaughan", vaughan));
        assert_eq!(sim.dialog_state().unwrap().line, "line 10", "still going");
        sim.tick(100);
        assert_eq!(sim.talk.as_ref().unwrap().owner, vaughan, "it waits");
        // Vaughan's ends; Valora's starts in its place.
        assert!(sim.choose_reply(0).unwrap().ended);
        sim.tick(0);
        let talk = sim.talk.as_ref().unwrap();
        assert_eq!((talk.dlg.name.as_str(), talk.owner), ("valora", valora));
        assert!(!talk.ended);
        assert!(sim.world.pending_talks.is_empty());
        // One whose owner has gone by then (set inactive, as the bridal party is after the
        // scene) does not start.
        assert!(sim.choose_reply(0).unwrap().ended);
        sim.tick(0);
        assert!(sim.open_conversation(hero, vaughan, named("vaughan"), None));
        assert!(sim.open_conversation(hero, valora, named("valora"), None));
        sim.world.get_mut(valora).unwrap().active = false;
        assert!(sim.choose_reply(0).unwrap().ended);
        sim.tick(0);
        assert!(sim.talk.as_ref().is_none_or(|t| t.ended), "nothing starts");
        assert!(sim.world.pending_talks.is_empty());
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
    /// A conversation of remarks said in passing: each line is followed by an unspoken
    /// reply that leads to the next, or (`direct`) by the next line itself.
    fn remarks(name: &str, lines: &[&str], direct: bool) -> Dlg {
        let node = |reply: bool, text: Option<&str>, children: Vec<usize>| Node {
            reply,
            strref: None,
            text: text.map(Into::into),
            speaker: String::new(),
            listener: String::new(),
            condition: PlotRef::default(),
            action: PlotRef::default(),
            children,
            show_once: 0,
            ambient: !reply,
            cutscene: None,
        };
        let mut nodes = Vec::new();
        for (i, l) in lines.iter().enumerate() {
            let last = i + 1 == lines.len();
            let at = nodes.len();
            match (last, direct) {
                (true, _) => nodes.push(node(false, Some(l), vec![])),
                (false, true) => nodes.push(node(false, Some(l), vec![at + 1])),
                (false, false) => {
                    nodes.push(node(false, Some(l), vec![at + 1]));
                    nodes.push(node(true, None, vec![at + 2]));
                }
            }
        }
        Dlg {
            name: name.into(),
            starts: vec![0],
            nodes,
        }
    }

    fn passers_by() -> (Sim, ObjectId, ObjectId) {
        let table = std::sync::Arc::new(thedas_script::EngineTable::default());
        let mut sim = Sim::new(std::sync::Arc::new(thedas_vfs::Vfs::new()), table, 1);
        let hero = sim
            .world
            .spawn(crate::world::ObjectKind::Creature, "player");
        let npc = sim.world.spawn(crate::world::ObjectKind::Creature, "guard");
        sim.world.hero = Some(hero);
        (sim, hero, npc)
    }

    fn said(sim: &Sim) -> Vec<&str> {
        sim.world
            .ambient_lines
            .iter()
            .map(|l| l.text.as_str())
            .collect()
    }

    #[test]
    fn a_conversation_said_in_passing_says_one_line_at_a_time() {
        // King Endrin at the feast answered "Quickly, find Trian and send him to me." and
        // nobody heard it: the whole conversation was run at once and kept from the
        // presentation.
        for direct in [false, true] {
            let (mut sim, hero, npc) = passers_by();
            assert!(sim.open_conversation(
                hero,
                npc,
                remarks("guard", &["one", "two"], direct),
                None
            ));
            // No dialog screen: the game goes on, and the presentation is told the line.
            assert!(sim.talk.is_none());
            assert_eq!(said(&sim), ["one"]);
            let l = &sim.world.ambient_lines[0];
            assert_eq!((l.speaker, l.voiced), (npc, false));
            assert_eq!(l.seconds, AMBIENT_LINE_MS as f32 / 1000.0);
            // Unvoiced, a line lasts seven seconds; then the next is said.
            sim.tick(6000);
            assert_eq!(said(&sim), ["one"]);
            sim.tick(1500);
            assert_eq!(said(&sim), ["one", "two"]);
            sim.tick(8000);
            assert_eq!(said(&sim), ["one", "two"]);
            assert!(sim.ambient.is_empty());
        }
    }

    #[test]
    fn a_speaker_says_one_ambient_conversation_and_can_be_stopped() {
        let (mut sim, hero, npc) = passers_by();
        let d = remarks("guard", &["one", "two"], false);
        // Asked again while saying it, the speaker goes on rather than starting over (the
        // sweep's click on Alistair restarted his exchange with the mage every time).
        assert!(sim.open_conversation(hero, npc, d.clone(), None));
        sim.tick(1000);
        assert!(sim.open_conversation(hero, npc, d.clone(), None));
        assert_eq!(sim.ambient.len(), 1);
        sim.tick(6500);
        assert_eq!(said(&sim), ["one", "two"]);
        sim.tick(8000);
        // `ClearAmbientDialogs` stops what was still to be said; so does the speaker's death.
        assert!(sim.open_conversation(hero, npc, d.clone(), None));
        sim.clear_ambient(npc);
        sim.tick(8000);
        assert_eq!(said(&sim).len(), 3);
        assert!(sim.open_conversation(hero, npc, d, None));
        sim.world.get_mut(npc).unwrap().dead = true;
        sim.tick(8000);
        assert_eq!(said(&sim).len(), 4);
        assert!(sim.ambient.is_empty());
    }
    #[test]
    fn a_conversation_starts_at_a_chosen_line() {
        // Said in passing, a chosen line is said that way, and the walk goes on from it.
        let (mut sim, hero, npc) = passers_by();
        let mut d = remarks("guard", &["one", "two", "three"], false);
        assert!(sim.open_conversation(hero, npc, d.clone(), Some(2)));
        sim.tick(20000);
        assert_eq!(said(&sim), ["two", "three"]);
        // Not said in passing, it opens on the dialog screen at that line, though neither
        // speaker is in the party.
        let (mut sim, hero, npc) = passers_by();
        d.nodes[2].ambient = false;
        assert!(sim.open_conversation(hero, npc, d, Some(2)));
        let st = sim.dialog_state().expect("the dialog is open");
        assert_eq!(st.line, "two");
        assert!(said(&sim).is_empty());
    }

    #[test]
    fn a_conversations_conditions_report_the_flags_they_read() {
        let (mut sim, _hero, npc) = passers_by();
        let mut d = remarks("guard", &["one", "two"], false);
        d.nodes[0].condition = PlotRef {
            plot: Some("some_plot".into()),
            flag: 3,
            value: true,
            script: None,
        };
        let report = sim.dlg_conditions(npc, &d);
        assert_eq!(report.len(), 3);
        let first = &report[0];
        assert!(first.start && !first.reply && !first.usable);
        assert_eq!(first.text, "one");
        assert_eq!(first.reads.len(), 1);
        assert_eq!((first.reads[0].flag, first.reads[0].value), (3, false));
        // The unconditioned reply and line read nothing and are usable.
        assert!(report[1].reply && report[1].usable && report[1].reads.is_empty());
        assert!(report[2].usable && report[2].sets.is_empty());
        // Nothing is left recording.
        assert!(sim.plots.reads.is_none() && sim.plots.sets.is_none());
    }

    #[test]
    fn an_ambient_run_ends_where_the_player_is_spoken_to() {
        // Alistair's exchange with the mage at Ostagar is ambient, and runs on into his own
        // conversation with the player, which waits for the player to talk to him.
        let (mut sim, hero, npc) = passers_by();
        let mut d = remarks("alistair", &["one", "two", "three"], false);
        // "three" is said to the player, not in passing.
        d.nodes[4].ambient = false;
        assert!(sim.open_conversation(hero, npc, d.clone(), None));
        sim.tick(20000);
        assert_eq!(said(&sim), ["one", "two"]);
        assert!(sim.ambient.is_empty());
        // At a choice for the player it turns to the player: the dialog opens on its line.
        let (mut sim, hero, npc) = passers_by();
        let mut d = remarks("alistair", &["one", "two"], false);
        d.nodes[1].text = Some("A choice".into());
        assert!(sim.open_conversation(hero, npc, d, None));
        sim.tick(20000);
        assert_eq!(said(&sim), ["one"]);
        assert!(sim.ambient.is_empty());
        let st = sim.dialog_state().expect("the dialog is open");
        assert_eq!((st.line.as_str(), st.replies.len()), ("one", 1));
    }
}
