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
    /// DA2 named-stage release rules, saved with the prepared pose.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub da2_stage: bool,
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
    /// DA2 performance bindings, activated when this line is taken. Full DA2
    /// performance placement is separate from this world-state side effect.
    pub activate: Vec<String>,
    /// DA2's own stage and end poses; separate from Origins' presentation tracks.
    pub da2_stage: Option<Da2Stage>,
}

/// A DA2 line performance placed on a named area stage (CUT field 5024).
#[derive(Clone, Debug, PartialEq)]
pub struct Da2Stage {
    pub tag: String,
    pub leave_at_end: bool,
    pub tracks: Arc<Tracks>,
}

impl Da2Stage {
    fn from_performance(g: &Gff4) -> Option<Self> {
        let cut = crate::cutscene::cut_root(g)?;
        let tag = g.field(cut, 5024)?.as_str()?.trim_end_matches('\0').trim();
        // Relative placement has its own native context. Do not substitute the
        // authoring-stage filename (5017) for that context or for an area tag.
        if tag.is_empty() || matches!(tag, "[Owner Relative]" | "[Speaker Relative]") {
            return None;
        }
        let mut tracks = Tracks::from_cut(g, cut);
        for (actor, id) in tracks
            .actors
            .iter_mut()
            .zip(crate::cutscene::structs(g, cut, 5200))
        {
            actor.snaps = g
                .field(id, 5235)
                .and_then(Value::as_i64)
                .is_some_and(|n| n != 0);
        }
        Some(Self {
            tag: tag.to_string(),
            leave_at_end: g
                .field(cut, 5028)
                .and_then(Value::as_i64)
                .is_none_or(|n| n != 0),
            tracks: Arc::new(tracks),
        })
    }
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
                activate: Vec::new(),
                da2_stage: None,
            }
        })
        .collect()
}

impl Sim {
    /// The party members a playing cutscene does not cast, which presentation should not
    /// draw while it plays (empty when no full cutscene plays). Traced in Origins:
    /// GameModeCutscene enter 0x692980 -> 0x7cd0d0 enables playback; after actor binding
    /// (0x7e36b0 -> 0x7e30b0), update 0x7e3f76 -> 0x67c980 hides the moving party
    /// (party list +0x234, queried by 0x631830). Then 0x7e4020..0x7e40a7 makes every
    /// bound object visible again. Exit 0x7e1784 -> 0x67cad0 restores the party's normal
    /// visibility. Thus the unbound hero stays hidden at the Dwarf Commoner's brazier
    /// waypoint during bdccs_intro, without changing his position. See
    /// `docs/notes/cutscene-party-visibility.md` for the render-model visibility chain.
    /// Provisional: DA2, the native tool/editor override and the separate inline-dialogue
    /// presentation path have not been covered by this rule's Origins trace.
    pub fn cutscene_hidden_party(&self) -> Vec<ObjectId> {
        let Some(c) = &self.world.cutscene else {
            return Vec::new();
        };
        crate::engine::party_of(self)
            .into_iter()
            .filter(|m| !c.bound.iter().any(|b| b.object == *m))
            .collect()
    }

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
            .or_else(|| {
                if self.game != thedas_vfs::Game::DragonAge2 {
                    return None;
                }
                let (data, _) = self.vfs.read(&format!("{key}.cnv")).ok()?;
                let g = Gff4::parse(&data).ok()?;
                if !crate::cnv::is_cnv(&g) {
                    return None;
                }
                let performances = self.conversation_performances(&key);
                Some(
                    crate::cnv::line_performance_ids(&g)
                        .into_iter()
                        .map(|line| {
                            let performance = line.and_then(|id| performances.get(&id));
                            let activate = performance
                                .map(crate::cnv::performance_actor_tags)
                                .unwrap_or_default();
                            LineCut {
                                activate,
                                da2_stage: performance.and_then(Da2Stage::from_performance),
                                ..Default::default()
                            }
                        })
                        .collect(),
                )
            })
            .unwrap_or_default();
        let cuts = Arc::new(cuts);
        self.line_cuts.insert(key, cuts.clone());
        cuts
    }

    /// Prepare the release poses when DA2 takes a named-stage performance
    /// (0x5a36dc: take -> place -> 0x587db0). World positions stay put while it plays.
    pub(crate) fn bind_da2_stage(
        &mut self,
        stage: &Da2Stage,
        owner: ObjectId,
        player: ObjectId,
    ) -> Vec<Bound> {
        let Some(placement) = self.conversation_placement(Some(&stage.tag), false, owner, player)
        else {
            return Vec::new();
        };
        let mut bound = self.bind_cutscene_actors(
            &stage.tracks,
            Some(LineContext {
                owner,
                player,
                unstaged: false,
                placement,
            }),
        );
        for b in &mut bound {
            // DA2 5028 controls all taken objects, including the player and props.
            b.restore = !stage.leave_at_end;
            b.da2_stage = true;
            if stage.leave_at_end
                && b.snaps
                && let Some((at, _)) = &mut b.end
                && let Some(z) = self
                    .world
                    .get(b.object)
                    .and_then(|o| o.area)
                    .and_then(|area| self.floor_at(area, *at))
            {
                at[2] = z;
            }
        }
        bound
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
        let mut bindings = std::collections::HashMap::new();
        for (index, a) in tracks.actors.iter().enumerate() {
            let Some(name) = a.tag.as_deref() else {
                continue;
            };
            let id = if self.game == thedas_vfs::Game::DragonAge2 {
                // The native take path caches a binding name before claiming
                // its object. A repeated FOLLOWER label must not consume another
                // companion when a second resource actor uses that label.
                *bindings
                    .entry(name)
                    .or_insert_with(|| self.bound_actor_object(name, owner, player, &out))
            } else {
                self.cutscene_actor_object(name, owner, player)
            };
            let Some(id) = id else {
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
                da2_stage: false,
            });
        }
        out
    }

    /// DA2 0x59bd80 resolves FOLLOWER aliases against the active party and the
    /// objects already claimed by this performance. The label's number is not
    /// a party index: each plain alias takes the first unclaimed non-hero.
    fn bound_actor_object(
        &self,
        name: &str,
        owner: Option<ObjectId>,
        player: Option<ObjectId>,
        claimed: &[Bound],
    ) -> Option<ObjectId> {
        let name = name.trim();
        if self.game == thedas_vfs::Game::DragonAge2
            && name.starts_with("FOLLOWER")
            && name.encode_utf16().count() >= 10
        {
            let mut party = crate::engine::party_of(self)
                .into_iter()
                .filter(|id| Some(*id) != self.world.hero);
            return if let Some(tag) = name.strip_prefix("FOLLOWER:") {
                party.find(|id| self.world.get(*id).is_some_and(|o| o.tag == tag))
            } else {
                party.find(|id| !claimed.iter().any(|b| b.object == *id))
            };
        }
        self.cutscene_actor_object(name, owner, player)
    }

    /// A cutscene ends: the objects it took are put back or left at their end poses (see
    /// the module docs).
    pub fn release_cutscene_actors(&mut self, bound: Vec<Bound>) {
        for b in bound {
            let Some(o) = self.world.get(b.object) else {
                continue;
            };
            let (area, kind) = (o.area, o.kind);
            let da2_stage = self.game == thedas_vfs::Game::DragonAge2 && b.da2_stage;
            let unmoved = o.position == b.position && o.facing == b.facing;
            let (mut at, facing) = if b.restore || kind == ObjectKind::Placeable && !da2_stage {
                (b.position, b.facing)
            } else {
                match b.end {
                    Some(end) if unmoved || da2_stage => end,
                    _ => continue,
                }
            };
            if !b.restore && b.snaps && !da2_stage {
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

    #[test]
    fn party_members_a_cutscene_does_not_cast_are_hidden_while_it_plays() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        let cast = s.world.spawn(ObjectKind::Creature, "leske");
        let follower = s.world.spawn(ObjectKind::Creature, "follower");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers = vec![cast, follower];
        assert!(s.cutscene_hidden_party().is_empty(), "no cutscene");
        s.world.cutscene = Some(crate::cutscene::Cutscene {
            name: "bdccs_intro.cut".into(),
            seconds: 1.0,
            lines: Vec::new(),
            elapsed_ms: 0,
            from_dialog: false,
            bound: vec![Bound {
                object: cast,
                actor: 0,
                position: [0.0; 3],
                facing: 0.0,
                end: None,
                restore: false,
                snaps: false,
                da2_stage: false,
            }],
            placement: None,
        });
        assert_eq!(s.cutscene_hidden_party(), vec![hero, follower]);
    }

    fn actor(tag: &str, end: [f32; 3], turn: f32) -> Actor {
        let (s, w) = crate::fmath::DetMath::dsin_cos(turn.to_radians() * 0.5);
        Actor {
            id: 1,
            name: tag.into(),
            creature: None,
            model: None,
            tag: Some(tag.into()),
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            end_position: end,
            end_rotation: [0.0, 0.0, s, w],
            snaps: false,
            pose: None,
            motion: Vec::new(),
            turn: Vec::new(),
            fov: Vec::new(),
            animations: Vec::new(),
            places: Vec::new(),
            camera_jumps: Vec::new(),
            links: Vec::new(),
            shakes: Vec::new(),
            head_tracks: Vec::new(),
            visibility: Vec::new(),
            weapon_draws: Vec::new(),
            light: None,
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
    fn da2_stage_end_poses_are_prepared_once_and_survive_reload() {
        for leave_at_end in [false, true] {
            let (mut s, hero, hunter, _) = sim();
            s.game = thedas_vfs::Game::DragonAge2;
            let prop = s.world.spawn(ObjectKind::Placeable, "prop");
            let area = s.world.area;
            s.world.get_mut(prop).unwrap().area = area;
            let stage = Da2Stage {
                tag: "talk".into(),
                leave_at_end,
                tracks: Arc::new(Tracks {
                    actors: vec![
                        actor("hunter", [0.0, 2.0, 0.0], 0.0),
                        actor("PLAYER", [1.0, 0.0, 0.0], 90.0),
                        actor("prop", [2.0, 0.0, 0.0], 0.0),
                        actor("hunter", [99.0; 3], 0.0),
                        actor("missing", [99.0; 3], 0.0),
                    ],
                    ..Tracks::default()
                }),
            };
            let bound = s.bind_da2_stage(&stage, hunter, hero);
            assert_eq!(bound.len(), 3, "duplicate and missing bindings are ignored");
            assert_eq!(
                s.world.get(hero).unwrap().position,
                [0.0; 3],
                "taking does not teleport the world actor"
            );
            let before = bound
                .iter()
                .map(|b| (b.object, (b.position, b.facing)))
                .collect::<Vec<_>>();
            let expected = bound
                .iter()
                .map(|b| (b.object, b.end.unwrap()))
                .collect::<Vec<_>>();
            let saved = serde_json::to_string(&bound).unwrap();
            let restored: Vec<Bound> = serde_json::from_str(&saved).unwrap();
            assert_eq!(bound, restored);
            // Release uses the prepared record even if something moves the actor
            // or the hero crosses to a different nearest stage while it plays.
            for b in &bound {
                s.world.get_mut(b.object).unwrap().position = [900.0; 3];
            }
            s.release_cutscene_actors(restored);
            for (id, (at, facing)) in if leave_at_end { expected } else { before } {
                let o = s.world.get(id).unwrap();
                assert_eq!(o.position, at);
                assert_eq!(o.facing, facing);
            }
            if leave_at_end {
                let p = s.world.get(hero).unwrap();
                assert!((p.position[0] - 10.0).abs() < 1e-4 && (p.position[1] - 1.0).abs() < 1e-4);
                assert!((p.facing - 180.0).abs() < 1e-3);
                assert!(
                    (s.world.get(prop).unwrap().position[1] - 2.0).abs() < 1e-4,
                    "DA2 props use their end pose too"
                );
            }
            let missing = Da2Stage {
                tag: "absent".into(),
                ..stage
            };
            assert!(s.bind_da2_stage(&missing, hunter, hero).is_empty());
        }
    }

    #[test]
    fn old_bound_records_keep_origins_release_rules() {
        let (mut s, hero, hunter, _) = sim();
        let tracks = Tracks {
            actors: vec![actor("hunter", [2.0; 3], 0.0)],
            ..Tracks::default()
        };
        let bound = s.bind_cutscene_actors(
            &tracks,
            Some(LineContext {
                owner: hunter,
                player: hero,
                unstaged: false,
                placement: Placement::NONE,
            }),
        );
        let json = serde_json::to_string(&bound).unwrap();
        assert!(!json.contains("da2_stage"));
        let old: Vec<Bound> = serde_json::from_str(&json).unwrap();
        assert!(!old[0].da2_stage);
        s.world.get_mut(hunter).unwrap().position = [50.0; 3];
        s.release_cutscene_actors(old);
        assert_eq!(s.world.get(hunter).unwrap().position, [50.0; 3]);
    }

    #[test]
    fn da2_stage_reads_the_runtime_tag_and_release_switch() {
        use thedas_formats::gff4::{FieldDef, FourCC, List, StructData, StructTemplate, Version};
        let template = |name: &[u8; 4], labels: &[u32]| StructTemplate {
            label: FourCC::new(name),
            size: 0,
            fields: labels
                .iter()
                .map(|&label| FieldDef {
                    label,
                    type_code: 0,
                    flags: 0,
                    offset: 0,
                })
                .collect(),
        };
        let mut g = Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"LINE"),
            file_version: FourCC::new(b"V1.0"),
            templates: vec![
                template(b"LINE", &[31100]),
                template(b"CUT ", &[5017, 5024, 5028, 5200]),
                template(b"ACTR", &[5216, 5221, 5222, 5235]),
            ],
            structs: vec![
                StructData {
                    template: 0,
                    values: vec![Value::Struct(Some(StructId(1)))],
                },
                StructData {
                    template: 1,
                    values: vec![
                        Value::String(Some("authoring.stg\0".into())),
                        Value::String(Some("runtime_tag\0".into())),
                        Value::U8(0),
                        Value::List(Some(List::Struct(vec![Some(StructId(2))]))),
                    ],
                },
                StructData {
                    template: 2,
                    values: vec![
                        Value::String(Some("OWNER\0".into())),
                        Value::Vec3([1.0, 2.0, 3.0]),
                        Value::Quat([0.0, 0.0, 0.0, 1.0]),
                        Value::U8(255),
                    ],
                },
            ],
            root: StructId(0),
            shared_strings: None,
        };
        let stage = Da2Stage::from_performance(&g).unwrap();
        assert_eq!(stage.tag, "runtime_tag");
        assert!(!stage.leave_at_end);
        assert_eq!(stage.tracks.stage_file.as_deref(), Some("authoring.stg"));
        assert_eq!(stage.tracks.actors[0].end_position, [1.0, 2.0, 3.0]);
        assert!(stage.tracks.actors[0].snaps);
        g.structs[1].values[2] = Value::U8(255);
        assert!(Da2Stage::from_performance(&g).unwrap().leave_at_end);
        for unsupported in ["", "[Owner Relative]", "[Speaker Relative]"] {
            g.structs[1].values[1] = Value::String(Some(unsupported.into()));
            assert!(Da2Stage::from_performance(&g).is_none());
        }
    }

    #[test]
    fn a_da2_conversation_releases_its_stage_after_reload_and_on_advance() {
        use crate::dialog::{Dlg, Node, PlotRef};
        for game in [thedas_vfs::Game::Origins, thedas_vfs::Game::DragonAge2] {
            let (mut s, hero, owner, _) = sim();
            s.game = game;
            let line = |text: &str, children: Vec<usize>| Node {
                reply: false,
                strref: None,
                text: Some(text.into()),
                speaker: "OWNER".into(),
                listener: String::new(),
                condition: PlotRef::default(),
                action: PlotRef::default(),
                children,
                show_once: 0,
                cnv_flags: Some(1),
                ambient: false,
                cutscene: None,
            };
            let mut reply = line("Continue.", vec![2]);
            reply.reply = true;
            let dlg = Dlg {
                name: "named_stage".into(),
                starts: vec![0],
                nodes: vec![line("First.", vec![1]), reply, line("Last.", vec![])],
            };
            s.line_cuts.insert(
                dlg.name.clone(),
                Arc::new(vec![
                    LineCut {
                        da2_stage: Some(Da2Stage {
                            tag: "talk".into(),
                            leave_at_end: true,
                            tracks: Arc::new(Tracks {
                                actors: vec![actor("OWNER", [0.0, 2.0, 0.0], 0.0)],
                                ..Tracks::default()
                            }),
                        }),
                        ..LineCut::default()
                    },
                    LineCut::default(),
                ]),
            );
            let before = s.world.get(owner).unwrap().position;
            assert!(s.open_conversation(hero, owner, dlg, None));
            assert_eq!(s.world.get(owner).unwrap().position, before);
            let snapshot = serde_json::to_string(&s.snapshot()).unwrap();
            s.restore(serde_json::from_str(&snapshot).unwrap());
            s.choose_reply(0);
            let at = s.world.get(owner).unwrap().position;
            if game == thedas_vfs::Game::DragonAge2 {
                assert!((at[0] - 8.0).abs() < 1e-4 && at[1].abs() < 1e-4);
            } else {
                assert_eq!(at, before);
            }
            s.end_conversation();
            assert_eq!(s.world.get(owner).unwrap().position, at);
        }
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
    #[test]
    fn da2_follower_aliases_take_unclaimed_active_party_members_in_order() {
        for game in [thedas_vfs::Game::Origins, thedas_vfs::Game::DragonAge2] {
            let (mut s, hero, first, _) = sim();
            s.game = game;
            let second = s.world.spawn(ObjectKind::Creature, "second");
            let third = s.world.spawn(ObjectKind::Creature, "third");
            let reserve = s.world.spawn(ObjectKind::Creature, "reserve");
            let area = s.world.area;
            for id in [second, third, reserve] {
                s.world.get_mut(id).unwrap().area = area;
            }
            s.world.get_mut(hero).unwrap().followers = vec![first, second, third];
            let tracks = Tracks {
                actors: vec![
                    actor("second", [0.0; 3], 0.0),
                    actor("PLAYER", [0.0; 3], 0.0),
                    actor("FOLLOWER 99", [1.0; 3], 0.0),
                    actor("FOLLOWER 99", [9.0; 3], 0.0),
                    actor("FOLLOWER 1", [2.0; 3], 0.0),
                    actor("FOLLOWER 2", [3.0; 3], 0.0),
                    actor("FOLLOWER:second", [4.0; 3], 0.0),
                    actor("FOLLOWER:reserve", [5.0; 3], 0.0),
                ],
                ..Tracks::default()
            };
            let bound = s.bind_cutscene_actors(
                &tracks,
                Some(LineContext {
                    owner: first,
                    player: hero,
                    unstaged: false,
                    placement: Placement::NONE,
                }),
            );
            let expected = if game == thedas_vfs::Game::DragonAge2 {
                vec![second, hero, first, third]
            } else {
                vec![second, hero]
            };
            assert_eq!(bound.iter().map(|b| b.object).collect::<Vec<_>>(), expected);
            if game == thedas_vfs::Game::DragonAge2 {
                assert_eq!(bound[2].actor, 2);
                assert_eq!(bound[3].actor, 4);
                assert_eq!(bound[2].end.unwrap().0, [1.0; 3]);
                assert_eq!(bound[3].end.unwrap().0, [2.0; 3]);
            }
            let explicit = Tracks {
                actors: vec![actor("FOLLOWER:third", [0.0; 3], 0.0)],
                ..Tracks::default()
            };
            let explicit_bound = s.bind_cutscene_actors(&explicit, None);
            assert_eq!(
                explicit_bound.iter().map(|b| b.object).collect::<Vec<_>>(),
                if game == thedas_vfs::Game::DragonAge2 {
                    vec![third]
                } else {
                    vec![]
                }
            );
            s.world.get_mut(hero).unwrap().followers.clear();
            assert!(s.bind_cutscene_actors(&explicit, None).is_empty());
        }
    }
}
