//! Cutscene actors as drawn: while a cutscene plays, or a conversation line plays its own
//! cutscene, the creatures that play its actors are drawn where its tracks put them and
//! with its animations, and actors no creature of the world plays are drawn as the
//! cutscene's own creatures (see [`thedas_sim::cutactors`]).
//!
//! Traced in DAOrigins.exe: a cutscene's tracks move the model of the object playing an
//! actor, not the object (0x7cf450); an actor no object plays is a creature the cutscene
//! builds from its template (5208, 0x7e390f -> 0x6a4f00) that only it shows; a jump to a
//! place (`AJSP`, 0x7dc720) puts the actor on a place of the cutscene's stage file each
//! update; animations (`AANI`, 0x7db400) run at their speed from their offset (see
//! [`thedas_sim::cuttracks::AnimationTrack::time_at`]). Where the simulation leaves the
//! creatures afterwards is its own business ([`thedas_sim::cutactors`]).
//!
//! Presentation only: reads the simulation, the cutscene and stage files.
//!
//! Provisional: a creature that snaps to the walkmesh (track kind 38) is set on the level's
//! surface under it; animations are laid over each other by the bones they move, not
//! blended by weight; which animations play when, see
//! [`thedas_sim::cuttracks::Actor::animations_at`]; one loops when the creature's own
//! trees loop it (4027 0), else when it spans the whole cutscene; the cutscene's own
//! creatures stand in their template's look (equipment from its items).

use std::collections::HashMap;
use std::sync::Arc;

use thedas_formats::stage::Stage;
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::cutactors::{Bound, LineCut};
use thedas_sim::cuttracks::{Actor, Placement, Tracks, facing_of};
use thedas_sim::world::ObjectKind;
use thedas_ui::models::ObjectModel;

/// The first id given to a cutscene's own creatures (far above the world's).
pub const OWN_CREATURE_IDS: ObjectId = 0x6000_0000;

/// An animation of the cutscene an actor plays.
#[derive(Clone, Debug, PartialEq)]
pub struct CutAnimation {
    /// The animation tree (`41249.cub`) and its node.
    pub tree: String,
    pub node: String,
    /// Seconds into the animation.
    pub time: f32,
    /// The track spans the whole cutscene.
    pub unbounded: bool,
}

/// Where a cutscene draws an actor's creature.
#[derive(Clone, Debug, PartialEq)]
pub struct ActorShow {
    pub position: [f32; 3],
    pub facing: f32,
    /// Kept to the ground (see the module docs).
    pub snaps: bool,
    /// Its animations, the later over the earlier.
    pub animations: Vec<CutAnimation>,
}

/// What the cutscenes playing show: the world's creatures (by id) and the cutscene's own,
/// and animations creatures keep from cutscenes that ended (see [`CutsceneActors::shown`]).
#[derive(Default)]
pub struct Shown {
    pub actors: HashMap<ObjectId, ActorShow>,
    pub own: Vec<(ObjectModel, ActorShow)>,
    pub kept: HashMap<ObjectId, Vec<CutAnimation>>,
}

#[derive(Default)]
pub struct CutsceneActors {
    lines: HashMap<String, Option<Arc<Vec<LineCut>>>>,
    stages: HashMap<String, Option<Arc<Stage>>>,
    /// What is playing (a cutscene's name or a conversation line) and when it came up on
    /// the clock.
    source: Option<(String, f64)>,
    clock_ms: f64,
    /// The cutscene whose own creatures were built, and each one's actor and model.
    own: Option<(String, Vec<(usize, ObjectModel)>)>,
    /// The animations of what is playing that run past its end, by creature, as they end.
    ending: HashMap<ObjectId, Vec<CutAnimation>>,
    /// Animations creatures keep from the cutscenes and lines of the open conversation
    /// that have ended (a creature that sat down stays seated).
    kept: HashMap<ObjectId, Vec<CutAnimation>>,
}

impl CutsceneActors {
    /// Advances the presentation clock (conversation lines run on it).
    pub fn advance(&mut self, dt_ms: f64) {
        if dt_ms.is_finite() && dt_ms > 0.0 {
            self.clock_ms += dt_ms;
        }
    }

    /// What is playing now changed to `key` (none: nothing): what the last left running is
    /// kept, while the conversation stays open.
    fn switch(&mut self, key: Option<String>, open: bool) {
        if self.source.as_ref().map(|(k, _)| k) == key.as_ref() {
            return;
        }
        for (id, anims) in self.ending.drain() {
            self.kept.insert(id, anims);
        }
        if !open {
            self.kept.clear();
        }
        self.source = key.map(|k| (k, self.clock_ms));
    }

    /// What the playing cutscene (`tracks`, its tracks) or the shown conversation line's own
    /// cutscene shows now. Provisional: a creature keeps the animations a cutscene or line
    /// of the open conversation left running at its end (the executable leaves the model's
    /// animation as the cutscene set it) until the conversation closes.
    pub fn shown(&mut self, sim: &Sim, tracks: Option<Arc<Tracks>>) -> Shown {
        let open = sim.conversation_line().is_some();
        let mut out = Shown::default();
        if let (Some(c), Some(tracks)) = (sim.world.cutscene.as_ref(), tracks) {
            self.switch(Some(format!("cut:{}", c.name)), open || c.from_dialog);
            let t = c.elapsed_ms as f32 / 1000.0;
            let placement = c.placement.unwrap_or(Placement::NONE);
            let stage = self.stage(sim, tracks.stage_file.as_deref());
            self.pose_bound(&tracks, &c.bound, t, placement, stage.as_deref(), &mut out);
            // Actors no object plays: the cutscene's own creatures.
            let name = c.name.clone();
            if self.own.as_ref().is_none_or(|(n, _)| *n != name) {
                self.own = Some((name, own_creatures(sim, &tracks, &c.bound)));
            }
            if let Some((_, own)) = &self.own {
                for (i, model) in own {
                    if let Some(a) = tracks.actors.get(*i) {
                        let show = show(a, t, tracks.seconds, placement, stage.as_deref());
                        out.own.push((model.clone(), show));
                    }
                }
            }
        } else {
            self.own = None;
            match sim.line_staging() {
                Some((dlg, line, placement, bound)) => {
                    let dlg = dlg.to_ascii_lowercase();
                    self.switch(Some(format!("line:{dlg}:{line}")), true);
                    let since = self.source.as_ref().map_or(self.clock_ms, |(_, s)| *s);
                    let t = ((self.clock_ms - since) / 1000.0) as f32;
                    if let Some(tracks) = self
                        .line_cuts(sim, &dlg)
                        .and_then(|l| l.get(line).and_then(|c| c.tracks.clone()))
                    {
                        let stage = self.stage(sim, tracks.stage_file.as_deref());
                        self.pose_bound(&tracks, bound, t, placement, stage.as_deref(), &mut out);
                    }
                }
                None => self.switch(None, open),
            }
        }
        out.kept = self.kept.clone();
        out
    }

    fn pose_bound(
        &mut self,
        tracks: &Tracks,
        bound: &[Bound],
        t: f32,
        placement: Placement,
        stage: Option<&Stage>,
        out: &mut Shown,
    ) {
        for b in bound {
            let Some(a) = tracks.actors.get(b.actor) else {
                continue;
            };
            out.actors
                .insert(b.object, show(a, t, tracks.seconds, placement, stage));
            // What it will be left playing when this ends.
            let end = tracks.seconds;
            let left: Vec<CutAnimation> = a
                .animations_at(end, end)
                .into_iter()
                .filter(|(l, _)| l.end >= end && l.end < 1e9)
                .map(|(l, time)| CutAnimation {
                    tree: l.tree.clone(),
                    node: l.node.clone(),
                    time,
                    unbounded: false,
                })
                .collect();
            if left.is_empty() {
                self.ending.remove(&b.object);
            } else {
                self.ending.insert(b.object, left);
            }
        }
    }

    fn line_cuts(&mut self, sim: &Sim, dlg: &str) -> Option<Arc<Vec<LineCut>>> {
        self.lines
            .entry(dlg.to_string())
            .or_insert_with(|| {
                let (data, _) = sim.vfs.read(&format!("{dlg}.dlg")).ok()?;
                let g = thedas_formats::Gff4::parse(&data).ok()?;
                Some(Arc::new(thedas_sim::cutactors::line_cuts(&g)))
            })
            .clone()
    }

    fn stage(&mut self, sim: &Sim, file: Option<&str>) -> Option<Arc<Stage>> {
        let file = file?.to_ascii_lowercase();
        let file = if file.ends_with(".stg") {
            file
        } else {
            format!("{file}.stg")
        };
        self.stages
            .entry(file.clone())
            .or_insert_with(|| {
                let (data, _) = sim.vfs.read(&file).ok()?;
                Stage::parse(&data).ok().map(Arc::new)
            })
            .clone()
    }
}

/// An actor `t` seconds into its cutscene (`length` seconds long), placed as the cutscene
/// is.
fn show(a: &Actor, t: f32, length: f32, placement: Placement, stage: Option<&Stage>) -> ActorShow {
    let place = |name: &str| {
        stage?
            .places
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .map(|p| (p.position, p.rotation))
    };
    let pose = placement.pose(a.stage_pose_at(t, &place));
    let animations = a
        .animations_at(t, length)
        .into_iter()
        .map(|(l, time)| CutAnimation {
            tree: l.tree.clone(),
            node: l.node.clone(),
            time,
            unbounded: l.end >= 1e9,
        })
        .collect();
    ActorShow {
        position: pose.position,
        facing: facing_of(pose.rotation).unwrap_or(0.0),
        snaps: a.snaps,
        animations,
    }
}

/// The cutscene's own creatures: each actor no object took that names a creature template,
/// built (in a scratch simulation, as area loading builds creatures) for its look.
fn own_creatures(sim: &Sim, tracks: &Tracks, bound: &[Bound]) -> Vec<(usize, ObjectModel)> {
    let mut scratch: Option<Sim> = None;
    let mut out = Vec::new();
    for (i, a) in tracks.actors.iter().enumerate() {
        let Some(template) = a.creature.as_deref() else {
            continue;
        };
        if bound.iter().any(|b| b.actor == i) {
            continue;
        }
        let s = scratch.get_or_insert_with(|| Sim::new(sim.vfs.clone(), sim.table.clone(), 0));
        let id = s.create_from_template(ObjectKind::Creature, template, None, a.position, 0.0);
        let Some(o) = s.world.get(id) else {
            continue;
        };
        let d = thedas_ui::models::object_models(s, o);
        if d.models.is_empty() {
            continue;
        }
        out.push((
            i,
            ObjectModel {
                id: OWN_CREATURE_IDS + i as ObjectId,
                tag: a.name.clone(),
                kind: ObjectKind::Creature,
                models: d.models,
                attached: d.attached,
                tints: d.tints,
                morph: d.morph,
                hide_hair: d.hide_hair,
                motion: thedas_ui::models::motion(s, o),
                position: a.position,
                facing: 0.0,
            },
        ));
    }
    if !out.is_empty() {
        log::info!(
            "cutscene's own creatures: {:?}",
            out.iter().map(|(_, m)| m.tag.as_str()).collect::<Vec<_>>()
        );
    }
    out
}
