//! Dragon Age II conversations (`.cnv`, GFF4 `CONV` version `3.0`), read into the same
//! [`Dlg`] model Dragon Age: Origins' `.dlg` files use, so one walker runs both.
//!
//! A DA2 conversation is a list of lines (field 30002) and the links between them: the root's
//! starting links (30001) and each line's onward links (30204). A link names its target line
//! (30100); a link the player picks on the dialogue wheel also carries the wheel's paraphrase
//! (30101, a talk-table id) and an icon (30300). A line holds a flag word (30200), its text
//! (30201), a condition (30202) and an action (30203); a condition or action is a plot GUID
//! (30400), a flag (30401) and a value (30402, 255 set or 0 clear).
//!
//! In the [`Dlg`] model the player's choices are reply nodes. So each DA2 line becomes a line
//! node, and each link with a paraphrase becomes a reply node of its own: its text is the
//! paraphrase, its condition its target line's (a choice whose line cannot be said is not
//! offered), and its one child the target line. A link without a paraphrase points straight
//! at its target line, which the walker says next on its own.
//!
//! Provisional throughout: the fields are read from the shipped files (see
//! `OpenThedas-testdata/notes/da2-conversations.md`), not traced in DragonAge2.exe. Field
//! 30200 is a set of flags, not a speaker: bit 0 marks a line of the other side and bit 1 a
//! line of the player (over 30,000 lines of 250 conversations, every line a wheel choice
//! leads to has bit 1 and every other line bit 0, without exception). Its other bits (2, 3,
//! 5, 6, 8) are not worked out. Which creature says a line of the other side is not in the
//! conversation file: its performance names it ([`performance_speaker`], [`apply_speakers`]). Not carried yet: the wheel icon, the
//! conversation-level condition (30003), fields 30301 and 30303, and whether a line is
//! ambient (no such field is known).

use thedas_formats::gff4::{FourCC, List};
use thedas_formats::{Gff4, StructId, Value};

use crate::dialog::{Dlg, Node, PlotRef};

/// Field 30200's bit for a line the player says.
const FLAG_PLAYER_LINE: i64 = 0x2;

/// Whether a parsed GFF4 is a Dragon Age II conversation.
pub fn is_cnv(g: &Gff4) -> bool {
    g.file_type.0 == *b"CONV" && g.file_version == FourCC::new(b"3.0 ")
}

fn structs(g: &Gff4, id: StructId, label: u32) -> Vec<StructId> {
    match g.field(id, label) {
        Some(Value::List(Some(List::Struct(v)))) => v.iter().flatten().copied().collect(),
        Some(Value::List(Some(List::Generic(slots)))) => slots
            .iter()
            .filter_map(|s| match &s.value {
                Some(Value::Struct(Some(id))) => Some(*id),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn int(g: &Gff4, id: StructId, label: u32) -> Option<i64> {
    g.field(id, label).and_then(|v| v.as_i64())
}

fn strref(g: &Gff4, id: StructId, label: u32) -> Option<u32> {
    match g.field(id, label) {
        Some(Value::Tlk(t)) if t.id != u32::MAX => Some(t.id),
        _ => None,
    }
}

fn plotref(g: &Gff4, id: StructId, label: u32) -> PlotRef {
    let Some(Value::Struct(Some(p))) = g.field(id, label) else {
        return PlotRef::default();
    };
    let plot = g
        .field(*p, 30400)
        .and_then(|v| v.as_str())
        .map(|s| s.trim_end_matches('\0').trim().to_string())
        .filter(|s| !s.is_empty());
    PlotRef {
        flag: if plot.is_some() {
            int(g, *p, 30401).unwrap_or(-1) as i32
        } else {
            -1
        },
        plot,
        value: int(g, *p, 30402).unwrap_or(255) != 0,
        script: None,
    }
}

/// A Dragon Age II conversation as a [`Dlg`]; `None` when `g` is not one.
pub fn dlg_from_cnv(g: &Gff4, name: &str) -> Option<Dlg> {
    if !is_cnv(g) {
        return None;
    }
    let lines = structs(g, g.root, 30002);
    let mut nodes: Vec<Node> = lines
        .iter()
        .map(|&l| Node {
            reply: false,
            strref: strref(g, l, 30201),
            text: None,
            speaker: match int(g, l, 30200) {
                Some(flags) if flags & FLAG_PLAYER_LINE != 0 => "PLAYER".into(),
                _ => "OWNER".into(),
            },
            listener: String::new(),
            condition: plotref(g, l, 30202),
            action: plotref(g, l, 30203),
            children: Vec::new(),
            show_once: 0,
            ambient: false,
            cutscene: None,
        })
        .collect();
    let line_count = nodes.len();
    // A link: the node it leads to, adding a reply node for a wheel choice.
    let follow = |nodes: &mut Vec<Node>, link: StructId| -> Option<usize> {
        let target = int(g, link, 30100)? as usize;
        if target >= line_count {
            return None;
        }
        match strref(g, link, 30101) {
            None => Some(target),
            Some(paraphrase) => {
                nodes.push(Node {
                    reply: true,
                    strref: Some(paraphrase),
                    text: None,
                    speaker: "PLAYER".into(),
                    listener: String::new(),
                    condition: nodes[target].condition.clone(),
                    action: PlotRef::default(),
                    children: vec![target],
                    show_once: 0,
                    ambient: false,
                    cutscene: None,
                });
                Some(nodes.len() - 1)
            }
        }
    };
    for (i, &l) in lines.iter().enumerate() {
        let children: Vec<usize> = structs(g, l, 30204)
            .into_iter()
            .filter_map(|link| follow(&mut nodes, link))
            .collect();
        nodes[i].children = children;
    }
    let starts: Vec<usize> = structs(g, g.root, 30001)
        .into_iter()
        .filter_map(|link| follow(&mut nodes, link))
        .collect();
    Some(Dlg {
        name: name.to_string(),
        starts,
        nodes,
    })
}

/// Field 31102 of a line performance (`.cl`, GFF4 `LINE`): the talk-table id of the line it
/// performs.
const FIELD_PERFORMANCE_LINE: u32 = 31102;
/// A cutscene actor's list of actors (5200), its tracks (5206) and the object it plays
/// (5216: `PLAYER`, `OWNER` or a tag).
const FIELD_ACTORS: u32 = 5200;
const FIELD_TRACKS: u32 = 5206;
const FIELD_ACTOR_OBJECT: u32 = 5216;

/// Who says the line a performance (`.cl`) performs: the talk-table id of the line, and the
/// object of the one actor with a speak-line track (template `ASLN`), as its cutscene binds
/// actors (`PLAYER`, `OWNER` or a tag). `None` for a performance nobody speaks in.
///
/// Dragon Age II's conversation files do not say who speaks a line of the other side; its
/// performance does. Provisional: read from the shipped files (in the prologue's
/// `pro000_opening_pc`, the `ASLN` actor is the speaker of every spoken line checked:
/// Bethany's "Here they come", the player's "Allow me" and "All yours, Sister"), not traced
/// in DragonAge2.exe.
pub fn performance_speaker(g: &Gff4) -> Option<(u32, String)> {
    let line = match g.field(g.root, FIELD_PERFORMANCE_LINE)? {
        Value::Tlk(t) => t.id,
        other => other.as_i64()? as u32,
    };
    let cut = crate::cutscene::cut_root(g)?;
    structs(g, cut, FIELD_ACTORS).into_iter().find_map(|actor| {
        let speaks = structs(g, actor, FIELD_TRACKS)
            .into_iter()
            .any(|t| g.template_of(t).label.0 == *b"ASLN");
        if !speaks {
            return None;
        }
        let who = g.field(actor, FIELD_ACTOR_OBJECT)?.as_str()?;
        let who = who.trim_end_matches('\0').trim();
        (!who.is_empty()).then(|| (line, who.to_string()))
    })
}

/// A line's speaker that is chosen when the line is said: [`PARTNER`] then the candidates'
/// tags, `|`-separated (see [`Sim::line_speaker_tag`](crate::sim::Sim)).
pub const PARTNER: &str = "PARTNER:";

/// Gives the lines of conversation `d` the speakers their performances name (see
/// [`performance_speaker`]), by talk-table id. The player's lines keep `PLAYER`. A line of
/// the other side with no performance, or one nobody speaks in, goes to the conversation's
/// other participant: of the speakers its performances name other than the player and the
/// owner, the first present when the line is said ([`PARTNER`]); with none, the owner.
///
/// Provisional: Dragon Age II's short lines with no performance ("Scouts.") name no speaker
/// anywhere in the files; in the conversations read they are the other participant's (the
/// prologue's sibling conversation names Bethany and Carver, and only one of them is there).
pub fn apply_speakers(d: &mut Dlg, speakers: &std::collections::HashMap<u32, String>) {
    let mut partners: Vec<String> = Vec::new();
    for n in &d.nodes {
        if let Some(w) = n.strref.and_then(|s| speakers.get(&s)) {
            if !w.eq_ignore_ascii_case("player")
                && !w.eq_ignore_ascii_case("owner")
                && !partners.iter().any(|p| p.eq_ignore_ascii_case(w))
            {
                partners.push(w.clone());
            }
        }
    }
    let partner = (!partners.is_empty()).then(|| format!("{PARTNER}{}", partners.join("|")));
    for n in d
        .nodes
        .iter_mut()
        .filter(|n| !n.reply && n.speaker != "PLAYER")
    {
        match n.strref.and_then(|s| speakers.get(&s)) {
            Some(who) if who.eq_ignore_ascii_case("player") => n.speaker = "PLAYER".into(),
            Some(who) if who.eq_ignore_ascii_case("owner") => n.speaker = "OWNER".into(),
            Some(who) => n.speaker = who.clone(),
            None => {
                if let Some(p) = &partner {
                    n.speaker = p.clone();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::gff4::{FieldDef, StructData, StructTemplate, TlkString, Version};

    fn template(label: &[u8; 4], fields: &[u32]) -> StructTemplate {
        StructTemplate {
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
        }
    }

    /// Owner: "Who are you?" (line 0) -> wheel: [peaceful "A friend." -> player line 1,
    /// (plot-gated) "None of your business." -> player line 2]; line 1 -> owner line 3 directly.
    fn sample() -> Gff4 {
        let tlk = |id: u32| Value::Tlk(TlkString { id, text: None });
        let list = |ids: &[usize]| {
            Value::List(Some(List::Struct(
                ids.iter().map(|&i| Some(StructId(i))).collect(),
            )))
        };
        let st = |template: u32, values: Vec<Value>| StructData { template, values };
        let plot = |guid: Option<&str>, flag: i32| {
            vec![
                Value::String(guid.map(|g| format!("{g}\0"))),
                Value::I32(flag),
                Value::U8(255),
            ]
        };
        let link =
            |to: u16, para: u32, icon: u8| st(2, vec![Value::U16(to), tlk(para), Value::U8(icon)]);
        let line = |speaker: u16, text: u32, cond: usize, act: usize, links: &[usize]| {
            st(
                1,
                vec![
                    Value::U16(speaker),
                    tlk(text),
                    Value::Struct(Some(StructId(cond))),
                    Value::Struct(Some(StructId(act))),
                    list(links),
                ],
            )
        };
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"CONV"),
            file_version: FourCC::new(b"3.0 "),
            templates: vec![
                template(b"CONV", &[30001, 30002]),
                template(b"LINE", &[30200, 30201, 30202, 30203, 30204]),
                template(b"LINK", &[30100, 30101, 30300]),
                template(b"PLOT", &[30400, 30401, 30402]),
            ],
            structs: vec![
                // 0: root
                st(0, vec![list(&[1]), list(&[2, 3, 4, 5])]),
                // 1: start link -> line 0
                link(0, u32::MAX, 255),
                // 2..5: lines 0..3
                line(0x101, 100, 10, 10, &[6, 7]),
                line(0x4e, 101, 10, 11, &[8]),
                line(0x0a, 102, 12, 10, &[]),
                line(0x45, 103, 10, 10, &[]),
                // 6, 7: wheel links from line 0
                link(1, 200, 2),
                link(2, 201, 7),
                // 8: line 1 -> line 3, no paraphrase
                link(3, u32::MAX, 255),
                // 9: unused
                st(3, plot(None, -1)),
                // 10: no plot
                st(3, plot(None, -1)),
                // 11: action: set PLOT flag 4
                st(3, plot(Some("ABCD"), 4)),
                // 12: condition: PLOT flag 300 (a composite)
                st(3, plot(Some("ABCD"), 300)),
            ],
            root: StructId(0),
            shared_strings: None,
        }
    }

    #[test]
    fn reads_lines_and_wheel_choices_into_the_dialog_model() {
        let d = dlg_from_cnv(&sample(), "test").unwrap();
        assert_eq!(d.starts, vec![0]);
        // Four lines, then a reply node for each wheel link.
        assert_eq!(d.nodes.len(), 6);
        let line0 = &d.nodes[0];
        assert!(!line0.reply);
        assert_eq!(line0.strref, Some(100));
        assert_eq!(line0.speaker, "OWNER");
        assert_eq!(line0.children, vec![4, 5]);
        let (a, b) = (&d.nodes[4], &d.nodes[5]);
        assert!(a.reply && b.reply);
        assert_eq!((a.strref, a.children.clone()), (Some(200), vec![1]));
        assert_eq!((b.strref, b.children.clone()), (Some(201), vec![2]));
        // A choice is gated by its line's condition.
        assert_eq!(b.condition.plot.as_deref(), Some("ABCD"));
        assert_eq!(b.condition.flag, 300);
        assert!(a.condition.is_empty());
        // The player's line, its action, and a link straight to the next line.
        assert_eq!(d.nodes[1].speaker, "PLAYER");
        assert_eq!(d.nodes[1].action.flag, 4);
        assert!(d.nodes[1].action.value);
        assert_eq!(d.nodes[1].children, vec![3]);
        assert!(d.nodes[3].children.is_empty());
    }

    #[test]
    fn other_files_are_not_dragon_age_2_conversations() {
        let mut g = sample();
        g.file_version = FourCC::new(b"V0.2");
        assert!(dlg_from_cnv(&g, "x").is_none());
        // A link past the line list is dropped.
        let mut g = sample();
        g.structs[6].values[0] = Value::U16(99);
        let d = dlg_from_cnv(&g, "x").unwrap();
        assert_eq!(d.nodes[0].children, vec![4]);
    }

    #[test]
    fn performances_name_the_speakers_and_the_rest_go_to_whoever_is_there() {
        let mut d = dlg_from_cnv(&sample(), "test").unwrap();
        let speakers: std::collections::HashMap<u32, String> = [
            (100, "pro000cr_carver_fake".to_string()),
            (101, "PLAYER".to_string()),
            (102, "pro000cr_bethany_fake".to_string()),
        ]
        .into_iter()
        .collect();
        apply_speakers(&mut d, &speakers);
        assert_eq!(d.nodes[0].speaker, "pro000cr_carver_fake");
        assert_eq!(d.nodes[1].speaker, "PLAYER");
        // A player's line stays the player's, whatever its performance names.
        assert_eq!(d.nodes[2].speaker, "PLAYER");
        assert_eq!(
            d.nodes[3].speaker,
            "PARTNER:pro000cr_carver_fake|pro000cr_bethany_fake"
        );

        // Said, the line goes to the candidate who is there and active.
        let table = std::sync::Arc::new(thedas_script::EngineTable::default());
        let mut s = crate::sim::Sim::new(std::sync::Arc::new(thedas_vfs::Vfs::new()), table, 1);
        let carver = s
            .world
            .spawn(crate::world::ObjectKind::Creature, "pro000cr_carver_fake");
        let bethany = s
            .world
            .spawn(crate::world::ObjectKind::Creature, "pro000cr_bethany_fake");
        s.world.get_mut(carver).unwrap().active = false;
        s.world.get_mut(bethany).unwrap().active = true;
        assert_eq!(s.line_speaker_tag(&d.nodes[3]), "pro000cr_bethany_fake");
        s.world.get_mut(bethany).unwrap().dead = true;
        assert_eq!(s.line_speaker_tag(&d.nodes[3]), "pro000cr_carver_fake");
        assert_eq!(s.line_speaker_tag(&d.nodes[1]), "PLAYER");
    }
}
