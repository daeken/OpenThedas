//! The world's objects that play a cutscene's actors, and where a cutscene leaves them.
//!
//! Traced in DAOrigins.exe (`EC::Cutscene`):
//!
//! - When a cutscene starts, each actor naming an object (field 5216, actor +0x44) takes
//!   one (0x7e36b0): `OWNER` is the conversation's owner, `PLAYER` the player's creature,
//!   `HENCH...` a follower, anything else the object with that tag (0x7cd0a0 -> 0x7f2d80).
//!   An object already playing another actor of the cutscene plays no second one. An actor
//!   whose name finds nothing, or that names none, is the cutscene's own: a model (5202) or
//!   a creature built from its template (5208) that only the presentation shows.
//! - Taking an object (0x7e30b0, a creature or a placeable) records where it stands and
//!   faces, the actor's end pose (5221, 5222) and whether to put it back where it stood
//!   (record +0x38). That flag is set for every object a cutscene a script loaded takes,
//!   and for a conversation's cutscene (cutscene +0x23c, set by 0x7f0b0d) only for the
//!   player's creature on a line that is not staged (the line's 12104, cutscene +0x11d).
//! - While the cutscene plays its tracks move the object as drawn (0x7cf450 sets the
//!   model's position), not the object itself.
//! - When it ends (0x7e10a0, from `GameModeDialog` and `GameModeCutscene`), each object it
//!   took (0x7e12b0's loop) is put back where it stood when the flag is set or it is a
//!   placeable; otherwise, if it is still exactly where it stood and faced (0x4d4eb0, no
//!   tolerance), it is put at the actor's end pose in the world (0x695540, 0x694ce0): the
//!   cutscene's placement (+0xa0, +0xb0) applied to 5221 and 5222, facing where the turned
//!   +Y points, flattened. An object something else moved meanwhile stays where it is.
//! - A conversation's cutscene made on a stage (5017, cutscene +0x48; 0x7d2adf) is placed
//!   (0x7d2aa0, from `GameModeDialog` 0x68e584) on the area stage with the line's tag
//!   (12100) nearest the player's creature (0x67cdd0: by distance on the ground plane; the
//!   first of equals), turned as the stage faces; a line that is not staged (12104) is
//!   placed at its speaker (12202, else the owner), facing the player's creature.
//!
//! So a creature a conversation's line stages stays on its mark once the line ends, and a
//! cutscene a script loads leaves the world as it found it.
//!
//! Provisional: `HENCHMAN` names (the follower slots 0x7e3760 counts through) take no
//! object; a line staged on a stage the area does not have moves nobody (the executable
//! would place the cutscene at the world's origin); a creature put at its end pose is set
//! on the pathing grid's floor there when its actor snaps to the walkmesh (kind 38) - the
//! end placement (0x695540) does not itself snap, and what the creature's own position
//! handler (vtable +0x7c) does is not traced. Like `SetPosition` here, the move tells no
//! trigger (a creature put inside or outside one is noticed when it next moves).

use std::sync::Arc;

use thedas_formats::Gff4;
use thedas_formats::gff4::{StructId, Value};
use thedas_script::vm::ObjectId;

use crate::cuttracks::{Placement, Tracks, facing_of};
use crate::sim::Sim;
use crate::world::ObjectKind;

/// An object a cutscene took for one of its actors.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bound {
    pub object: ObjectId,
    /// The actor it plays (an index into the cutscene's [`Tracks::actors`]).
    #[serde(default)]
    pub actor: usize,
    /// Where it stood and faced when the cutscene took it.
    pub position: [f32; 3],
    pub facing: f32,
    /// Where the cutscene leaves it: the actor's end pose in the world.
    pub end: Option<([f32; 3], f32)>,
    /// Put back where it stood when the cutscene ends.
    pub restore: bool,
    /// The actor keeps to the walkmesh (see the module docs).
    #[serde(default)]
    pub snaps: bool,
}

/// What a conversation line's cutscene is played in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineContext {
    pub owner: ObjectId,
    pub player: ObjectId,
    /// The line is not staged (12104): the player's creature is put back.
    pub unstaged: bool,
    pub placement: Placement,
}

/// A conversation line's staging: its stage (12100), whether it is unstaged (12104) and its
/// own cutscene (12211 when it is a `CUT ` struct).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LineCut {
    pub stage: Option<String>,
    pub unstaged: bool,
    pub tracks: Option<Arc<Tracks>>,
}

const FIELD_LINES: u32 = 12002;
const FIELD_LINE_STAGE: u32 = 12100;
const FIELD_LINE_UNSTAGED: u32 = 12104;
const FIELD_LINE_CUT: u32 = 12211;

/// Each line of a conversation file (in its line list's order, 12002) and its staging.
pub fn line_cuts(g: &Gff4) -> Vec<LineCut> {
    if g.file_type.0 != *b"CONV" {
        return Vec::new();
    }
    crate::cutscene::structs(g, g.root, FIELD_LINES)
        .into_iter()
        .map(|line| {
            let cut = match g.field(line, FIELD_LINE_CUT) {
                Some(Value::Struct(s)) => *s,
                Some(Value::Generic(slot)) => match &slot.value {
                    Some(Value::Struct(s)) => *s,
                    _ => None,
                },
                _ => None,
            }
            .filter(|&c: &StructId| g.template_of(c).label.0 == *b"CUT ");
            LineCut {
                stage: g
                    .field(line, FIELD_LINE_STAGE)
                    .and_then(Value::as_str)
                    .map(|s| s.trim_end_matches('\0').trim().to_string())
                    .filter(|s| !s.is_empty()),
                unstaged: g
                    .field(line, FIELD_LINE_UNSTAGED)
                    .and_then(Value::as_i64)
                    .is_some_and(|v| v != 0),
                tracks: cut.map(|c| Arc::new(Tracks::from_cut(g, c))),
            }
        })
        .collect()
}

impl Sim {
    /// The staging of conversation `dlg`'s lines (read once and kept).
    pub fn conversation_line_cuts(&mut self, dlg: &str) -> Arc<Vec<LineCut>> {
        let key = dlg.to_ascii_lowercase();
        if let Some(c) = self.line_cuts.get(&key) {
            return c.clone();
        }
        let cuts = self
            .vfs
            .read(&format!("{key}.dlg"))
            .ok()
            .and_then(|(data, _)| Gff4::parse(&data).ok())
            .map(|g| line_cuts(&g))
            .unwrap_or_default();
        let cuts = Arc::new(cuts);
        self.line_cuts.insert(key, cuts.clone());
        cuts
    }

    /// The object a cutscene actor names (see the module docs), in the current area.
    pub fn cutscene_actor_object(
        &self,
        name: &str,
        owner: Option<ObjectId>,
        player: Option<ObjectId>,
    ) -> Option<ObjectId> {
        let name = name.trim();
        if name.eq_ignore_ascii_case("OWNER") {
            return owner;
        }
        if name.eq_ignore_ascii_case("PLAYER") {
            return player;
        }
        if name.len() >= 8 && name[..8].eq_ignore_ascii_case("HENCHMAN") {
            return None;
        }
        let tag = match name.split_once(':') {
            Some((kind, tag)) if kind.eq_ignore_ascii_case("HENCH") => tag,
            Some(_) => return None,
            None => name,
        };
        let area = self.world.area;
        self.world
            .by_tag(tag)
            .find(|o| {
                o.area.is_some()
                    && o.area == area
                    && matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable)
            })
            .map(|o| o.id)
    }

    /// Where a conversation line's cutscene plays (0x7d2aa0): the stage `stage` nearest
    /// `player`, or for an unstaged line its speaker facing `player`. `None` when there is
    /// no such stage (or speaker).
    pub fn conversation_placement(
        &self,
        stage: Option<&str>,
        unstaged: bool,
        speaker: ObjectId,
        player: ObjectId,
    ) -> Option<Placement> {
        let p = self.world.get(player)?.position;
        if unstaged {
            let s = self.world.get(speaker)?.position;
            let (dx, dy) = (p[0] - s[0], p[1] - s[1]);
            let facing = if dx * dx + dy * dy > 1e-12 {
                crate::world::facing_toward(dx, dy)
            } else {
                0.0
            };
            return Some(Placement::facing(s, facing));
        }
        let area = self.world.area;
        let stage = stage?;
        let mut best: Option<(f32, [f32; 3], f32)> = None;
        for o in self.world.by_tag(stage) {
            if o.kind != ObjectKind::Stage || o.area.is_none() || o.area != area {
                continue;
            }
            let d = (o.position[0] - p[0]).powi(2) + (o.position[1] - p[1]).powi(2);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, o.position, o.facing));
            }
        }
        best.map(|(_, at, facing)| Placement::facing(at, facing))
    }

    /// The objects a cutscene's actors take when it starts, and where it will leave them;
    /// `line` for a conversation's cutscene (none for one a script loaded).
    pub fn bind_cutscene_actors(&self, tracks: &Tracks, line: Option<LineContext>) -> Vec<Bound> {
        let player = match line {
            Some(l) => Some(l.player),
            None => self.world.hero,
        };
        let owner = line.map(|l| l.owner);
        let mut out: Vec<Bound> = Vec::new();
        for (index, a) in tracks.actors.iter().enumerate() {
            let Some(name) = a.tag.as_deref() else {
                continue;
            };
            let Some(id) = self.cutscene_actor_object(name, owner, player) else {
                continue;
            };
            if out.iter().any(|b| b.object == id) {
                continue;
            }
            let Some(o) = self.world.get(id) else {
                continue;
            };
            let restore = match line {
                None => true,
                Some(l) => l.unstaged && Some(id) == player,
            } || o.kind == ObjectKind::Placeable;
            let end = line.map(|l| {
                let at = l.placement.point(a.end_position);
                let facing = facing_of(l.placement.turn(a.end_rotation)).unwrap_or(o.facing);
                (at, facing)
            });
            out.push(Bound {
                object: id,
                actor: index,
                position: o.position,
                facing: o.facing,
                end,
                restore,
                snaps: a.snaps,
            });
        }
        out
    }

    /// A cutscene ends: the objects it took are put back or left at their end poses (see
    /// the module docs).
    pub fn release_cutscene_actors(&mut self, bound: Vec<Bound>) {
        for b in bound {
            let Some(o) = self.world.get(b.object) else {
                continue;
            };
            let (area, kind) = (o.area, o.kind);
            let unmoved = o.position == b.position && o.facing == b.facing;
            let (mut at, facing) = if b.restore || kind == ObjectKind::Placeable {
                (b.position, b.facing)
            } else {
                match b.end {
                    Some(end) if unmoved => end,
                    _ => continue,
                }
            };
            if !b.restore && b.snaps {
                if let Some(z) = area.and_then(|a| self.floor_at(a, at)) {
                    at[2] = z;
                }
            }
            let moved = self.world.get_mut(b.object).is_some_and(|o| {
                let moved = o.position != at || o.facing != facing;
                o.position = at;
                o.facing = facing;
                moved
            });
            if moved {
                let t = self.world.time_ms;
                self.world.trace.push(crate::trace::TraceEntry::Log {
                    t,
                    channel: -1,
                    text: format!(
                        "cutscene leaves {} at [{:.2}, {:.2}, {:.2}] facing {:.1}",
                        b.object, at[0], at[1], at[2], facing
                    ),
                });
            }
        }
    }

    /// The pathing grid's floor under `p` in `area`, where it is walkable.
    pub(crate) fn floor_at(&mut self, area: ObjectId, p: [f32; 3]) -> Option<f32> {
        let g = self.nav_grid(area)?;
        let c = g.cell(p)?;
        crate::nav::walkable(&g, c).then(|| crate::nav::height(&g, c))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::cuttracks::Actor;

    fn actor(tag: &str, end: [f32; 3], turn: f32) -> Actor {
        let (s, w) = (turn.to_radians() * 0.5).sin_cos();
        Actor {
            id: 1,
            name: tag.into(),
            creature: None,
            tag: Some(tag.into()),
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            end_position: end,
            end_rotation: [0.0, 0.0, s, w],
            snaps: false,
            motion: Vec::new(),
            turn: Vec::new(),
            fov: Vec::new(),
            animations: Vec::new(),
            places: Vec::new(),
            camera_jumps: Vec::new(),
            placed_elsewhere: Vec::new(),
        }
    }

    fn sim() -> (Sim, ObjectId, ObjectId, ObjectId) {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world.area = Some(area);
        let mut put = |kind, tag: &str, at: [f32; 3]| {
            let id = s.world.spawn(kind, tag);
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = at;
            id
        };
        let hero = put(ObjectKind::Creature, "player", [0.0, 0.0, 0.0]);
        let hunter = put(ObjectKind::Creature, "hunter", [5.0, 5.0, 0.0]);
        put(ObjectKind::Stage, "talk", [100.0, 0.0, 0.0]);
        let near = put(ObjectKind::Stage, "talk", [10.0, 0.0, 0.0]);
        s.world.get_mut(near).unwrap().facing = 90.0;
        s.world.hero = Some(hero);
        (s, hero, hunter, near)
    }

    #[test]
    fn a_conversation_leaves_its_creatures_on_their_marks() {
        let (mut s, hero, hunter, _) = sim();
        let placement = s
            .conversation_placement(Some("talk"), false, hunter, hero)
            .unwrap();
        // The stage nearest the player, turned a quarter.
        assert_eq!(placement.position, [10.0, 0.0, 0.0]);
        let tracks = Tracks {
            actors: vec![
                actor("hunter", [0.0, 2.0, 0.0], 0.0),
                actor("PLAYER", [1.0, 0.0, 0.0], 90.0),
                actor("nobody", [0.0; 3], 0.0),
            ],
            ..Tracks::default()
        };
        let ctx = LineContext {
            owner: hunter,
            player: hero,
            unstaged: false,
            placement,
        };
        let bound = s.bind_cutscene_actors(&tracks, Some(ctx));
        assert_eq!(bound.len(), 2);
        s.release_cutscene_actors(bound);
        let h = s.world.get(hunter).unwrap();
        // Stage +Y is world -X.
        assert!((h.position[0] - 8.0).abs() < 1e-4 && h.position[1].abs() < 1e-4);
        assert!((h.facing - 90.0).abs() < 1e-3, "{}", h.facing);
        let p = s.world.get(hero).unwrap();
        assert!((p.position[0] - 10.0).abs() < 1e-4 && (p.position[1] - 1.0).abs() < 1e-4);
        assert!((p.facing - 180.0).abs() < 1e-3, "{}", p.facing);
    }

    #[test]
    fn moved_creatures_stay_and_scripted_cutscenes_put_back() {
        let (mut s, hero, hunter, _) = sim();
        let tracks = Tracks {
            actors: vec![actor("hunter", [0.0, 2.0, 0.0], 0.0)],
            ..Tracks::default()
        };
        let ctx = LineContext {
            owner: hunter,
            player: hero,
            unstaged: false,
            placement: Placement::NONE,
        };
        // Something moved it meanwhile: it stays where that put it.
        let bound = s.bind_cutscene_actors(&tracks, Some(ctx));
        s.world.get_mut(hunter).unwrap().position = [7.0, 7.0, 0.0];
        s.release_cutscene_actors(bound);
        assert_eq!(s.world.get(hunter).unwrap().position, [7.0, 7.0, 0.0]);
        // A script's cutscene puts it back where it stood.
        let bound = s.bind_cutscene_actors(&tracks, None);
        assert!(bound[0].restore && bound[0].end.is_none());
        s.world.get_mut(hunter).unwrap().position = [1.0, 1.0, 0.0];
        s.release_cutscene_actors(bound);
        assert_eq!(s.world.get(hunter).unwrap().position, [7.0, 7.0, 0.0]);
        // An unstaged line puts the player back and stages the others.
        let tracks = Tracks {
            actors: vec![actor("PLAYER", [3.0, 0.0, 0.0], 0.0)],
            ..Tracks::default()
        };
        let ctx = LineContext {
            unstaged: true,
            ..ctx
        };
        let bound = s.bind_cutscene_actors(&tracks, Some(ctx));
        assert!(bound[0].restore);
        s.release_cutscene_actors(bound);
        assert_eq!(s.world.get(hero).unwrap().position, [0.0; 3]);
    }

    #[test]
    fn an_unstaged_line_plays_at_its_speaker_facing_the_player() {
        let (s, hero, hunter, _) = sim();
        let p = s.conversation_placement(None, true, hunter, hero).unwrap();
        assert_eq!(p.position, [5.0, 5.0, 0.0]);
        // From (5, 5) towards the origin.
        let f = facing_of(p.rotation).unwrap();
        assert!((f - crate::world::facing_toward(-1.0, -1.0)).abs() < 1e-3);
        assert!(
            s.conversation_placement(Some("none"), false, hunter, hero)
                .is_none()
        );
    }
}
