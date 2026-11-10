//! Which of the game's interface screens are up while playing, from the simulation: the HUD
//! while exploring, the conversation screen while someone talks, the cutscene screen (its
//! subtitles) while a cutscene plays, and the screens scripts open (world map, store, job
//! board, level-up) or the player opens (inventory).
//!
//! A screen opens when it becomes wanted and closes when it stops being wanted. A screen
//! that closes itself (its close button) stays closed until it is wanted afresh, so a
//! screen the simulation still asks for is not reopened over and over.
//!
//! Provisional: which movies make up the HUD, that every other screen hides it, and that
//! the world stops while a full-screen menu is up (the game's `GM_GUI`) are inferred, not
//! traced from the executable.

use std::collections::BTreeSet;

use thedas_sim::Sim;
use thedas_ui::stage::Stage;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Screen {
    Hud,
    Cutscene,
    Conversation,
    WorldMap,
    Store,
    Chanters,
    LevelUp,
    Inventory,
}

impl Screen {
    pub fn movies(self) -> &'static [&'static str] {
        match self {
            Screen::Hud => &["portraits.gfx", "quickbar.gfx", "minimap.gfx"],
            Screen::Cutscene => &["cutscene.gfx"],
            Screen::Conversation => &["conversation.gfx"],
            Screen::WorldMap => &["worldmap.gfx"],
            Screen::Store => &["store.gfx"],
            Screen::Chanters => &["chanters.gfx"],
            Screen::LevelUp => &["levelup.gfx"],
            Screen::Inventory => &["inventory.gfx"],
        }
    }

    /// A menu the world waits behind (everything but the HUD and conversations; a cutscene
    /// holds the world while it plays).
    pub fn stops_the_world(self) -> bool {
        !matches!(self, Screen::Hud | Screen::Conversation)
    }
}

/// The screen the simulation's state calls for; `inventory` is the player's own toggle. One
/// screen is up at a time: a cutscene first (what it hands on to waits for it), then a
/// conversation (a store or job board it opens waits for it to end), then the screens
/// scripts open, then the inventory, else the HUD.
pub fn wanted(sim: &Sim, inventory: bool) -> BTreeSet<Screen> {
    let w = &sim.world;
    let screen = if w.cutscene.is_some() {
        Screen::Cutscene
    } else if thedas_ui::binding::conversation(sim).is_some() {
        Screen::Conversation
    } else if w.chargen.is_some() {
        Screen::LevelUp
    } else if w.store.is_some() {
        Screen::Store
    } else if w.chanter_board.is_some() {
        Screen::Chanters
    } else if w.world_map.open {
        Screen::WorldMap
    } else if inventory {
        Screen::Inventory
    } else {
        Screen::Hud
    };
    BTreeSet::from([screen])
}

#[derive(Default)]
pub struct Screens {
    /// Up now.
    shown: BTreeSet<Screen>,
    /// Closed by their own movie while still wanted.
    dismissed: BTreeSet<Screen>,
    /// The player's inventory toggle (I).
    pub inventory: bool,
}

/// What [`Screens::update`] changed.
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub opened: Vec<Screen>,
    pub closed: Vec<Screen>,
}

impl Screens {
    pub fn shown(&self) -> &BTreeSet<Screen> {
        &self.shown
    }

    /// Whether a menu the world waits behind is up.
    pub fn world_stopped(&self) -> bool {
        self.shown.iter().any(|s| s.stops_the_world())
    }

    /// Whether a cutscene is playing (a click skips it).
    pub fn in_cutscene(&self) -> bool {
        self.shown.contains(&Screen::Cutscene)
    }

    /// Whether only the HUD is up: clicks that miss it reach the world.
    pub fn exploring(&self) -> bool {
        self.shown.iter().all(|&s| s == Screen::Hud)
    }

    /// Notes screens whose movies closed themselves (given which movies are open).
    pub fn note_open(&mut self, open: &dyn Fn(&str) -> bool) {
        let gone: Vec<Screen> = self
            .shown
            .iter()
            .copied()
            .filter(|s| !s.movies().iter().any(|m| open(m)))
            .collect();
        for s in gone {
            self.shown.remove(&s);
            if s == Screen::Inventory {
                self.inventory = false;
            } else {
                self.dismissed.insert(s);
            }
        }
    }

    /// Brings the shown screens to `want`: returns what to open and close.
    pub fn update(&mut self, want: BTreeSet<Screen>) -> Changes {
        self.dismissed.retain(|s| want.contains(s));
        let want: BTreeSet<Screen> = want.difference(&self.dismissed).copied().collect();
        let changes = Changes {
            opened: want.difference(&self.shown).copied().collect(),
            closed: self.shown.difference(&want).copied().collect(),
        };
        self.shown = want;
        changes
    }

    /// [`Screens::update`] (to [`wanted`]) applied to `stage`: movies of closed screens are dropped, those of
    /// opened screens loaded (failures logged).
    pub fn sync(&mut self, want: BTreeSet<Screen>, stage: &mut Stage) -> Changes {
        self.note_open(&|m| stage.movies.iter().any(|x| x.name == m));
        let changes = self.update(want);
        for s in &changes.closed {
            for m in s.movies() {
                stage.close(m);
            }
        }
        for s in &changes.opened {
            let started = std::time::Instant::now();
            for m in s.movies() {
                if let Err(e) = stage.open(m) {
                    log::warn!("ui: {m}: {e}");
                }
            }
            log::info!("ui: {s:?} up in {:.2}s", started.elapsed().as_secs_f64());
        }
        changes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(s: &[Screen]) -> BTreeSet<Screen> {
        s.iter().copied().collect()
    }

    #[test]
    fn opens_and_closes_on_change() {
        let mut s = Screens::default();
        let c = s.update(set(&[Screen::Hud]));
        assert_eq!(c.opened, vec![Screen::Hud]);
        let c = s.update(set(&[Screen::Conversation]));
        assert_eq!(c.opened, vec![Screen::Conversation]);
        assert_eq!(c.closed, vec![Screen::Hud]);
        assert!(!s.exploring() && !s.world_stopped());
        assert_eq!(s.update(set(&[Screen::Conversation])), Changes::default());
    }

    #[test]
    fn a_screen_that_closed_itself_stays_closed_until_wanted_again() {
        let mut s = Screens::default();
        s.update(set(&[Screen::WorldMap]));
        assert!(s.world_stopped());
        // The map's close button: its movie is gone though the simulation still wants it.
        s.note_open(&|_| false);
        assert_eq!(s.update(set(&[Screen::WorldMap])), Changes::default());
        // The simulation lets go and asks again: it opens.
        s.update(set(&[Screen::Hud]));
        let c = s.update(set(&[Screen::WorldMap]));
        assert_eq!(c.opened, vec![Screen::WorldMap]);
    }

    #[test]
    fn closing_the_inventory_clears_the_toggle() {
        let mut s = Screens {
            inventory: true,
            ..Default::default()
        };
        s.update(set(&[Screen::Inventory]));
        s.note_open(&|m| m != "inventory.gfx");
        assert!(!s.inventory);
    }

    #[test]
    fn the_hud_shows_when_nothing_else_does() {
        let sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        assert_eq!(wanted(&sim, false), set(&[Screen::Hud]));
        assert_eq!(wanted(&sim, true), set(&[Screen::Inventory]));
    }

    #[test]
    fn one_screen_at_a_time() {
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        sim.world.world_map.open = true;
        sim.world.store = Some(1);
        assert_eq!(wanted(&sim, true), set(&[Screen::Store]));
        sim.world.store = None;
        assert_eq!(wanted(&sim, true), set(&[Screen::WorldMap]));
        sim.world.cutscene = Some(thedas_sim::cutscene::Cutscene {
            name: "intro.cut".into(),
            seconds: 1.0,
            lines: Vec::new(),
        });
        assert_eq!(wanted(&sim, true), set(&[Screen::Cutscene]));
        assert!(Screen::Cutscene.stops_the_world());
    }
}
