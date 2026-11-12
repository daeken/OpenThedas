//! Which of the game's interface screens are up while playing, from the simulation: the HUD
//! while exploring, the conversation screen while someone talks, the cutscene screen (its
//! subtitles) while a cutscene plays, and the screens scripts open (world map, store, job
//! board, level-up) or the player opens (inventory, character record, tactics).
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
    CharacterRecord,
    Tactics,
    AreaMap,
    PartyPicker,
    Death,
    SaveLoad,
    CharGen,
    Journal,
    Options,
    ItemExamine,
    /// The skills screen (K) and the spells/talents screen (P): one movie, abilities.gfx,
    /// told which by `GUI.AbilityScene`.
    Skills,
    SpellsTalents,
    /// The start menu (startmenu2.gfx), before a game: New Game, Load, Options, Exit.
    StartMenu,
    /// The game menu over a running game (mainmenu.gfx, Escape or Start): save, load,
    /// options, back to the start menu.
    GameMenu,
}

impl Screen {
    pub fn movies(self) -> &'static [&'static str] {
        match self {
            // notifications.gfx: the journal's and codex's notifications (`Party.Alerts`).
            // floatylayer.gfx: the pause border and the text and bars over creatures,
            // under the panels.
            Screen::Hud => &[
                "floatylayer.gfx",
                "portraits.gfx",
                "quickbar.gfx",
                "minimap.gfx",
                "notifications.gfx",
                "tooltips.gfx",
            ],
            Screen::Cutscene => &["cutscene.gfx"],
            Screen::Conversation => &["conversation.gfx"],
            Screen::WorldMap => &["worldmap.gfx"],
            Screen::Store => &["store.gfx", "tooltips.gfx"],
            Screen::Chanters => &["chanters.gfx", "tooltips.gfx"],
            Screen::LevelUp => &["levelup.gfx", "tooltips.gfx"],
            Screen::Inventory => &["inventory.gfx", "navbar.gfx", "tooltips.gfx"],
            Screen::CharacterRecord => &["characterrecord.gfx", "navbar.gfx", "tooltips.gfx"],
            Screen::Tactics => &["tactics.gfx", "navbar.gfx", "tooltips.gfx"],
            Screen::AreaMap => &["map.gfx", "navbar.gfx", "tooltips.gfx"],
            Screen::PartyPicker => &["partypicker.gfx"],
            Screen::Death => &["deathscreen.gfx"],
            // Its save names and confirmations are dialog boxes on the popup layer.
            Screen::SaveLoad => &["saveload.gfx", "popuplayer.gfx"],
            // Quick Play and finishing ask for confirmation on the popup layer.
            Screen::CharGen => &["chargen.gfx", "popuplayer.gfx"],
            Screen::Journal => &["journal.gfx", "navbar.gfx", "tooltips.gfx"],
            Screen::Skills | Screen::SpellsTalents => {
                &["abilities.gfx", "navbar.gfx", "tooltips.gfx"]
            }
            // Settings that need confirming (a new resolution) ask on the popup layer.
            Screen::Options => &["optionsmenu.gfx", "popuplayer.gfx"],
            // Over the screen it was opened from (see `with_examine`).
            Screen::ItemExamine => &["itemexamine.gfx"],
            // Its confirmations (quitting) are dialog boxes on the popup layer.
            Screen::StartMenu => &["startmenu2.gfx", "popuplayer.gfx"],
            Screen::GameMenu => &["mainmenu.gfx", "popuplayer.gfx"],
        }
    }

    /// The book page navbar.gfx shows selected for this screen (`GUI.SelectedBookPage`, the
    /// screens' `GUISystem.GUI_*` ids), for the screens drawn in the book with its page tabs.
    pub fn book_page(self) -> Option<i32> {
        Some(match self {
            Screen::CharacterRecord => 0,
            Screen::Inventory => 8,
            Screen::Journal => 9,
            Screen::SpellsTalents => 10,
            Screen::Skills => 13,
            Screen::AreaMap => 15,
            Screen::Tactics => 23,
            _ => return None,
        })
    }

    /// A menu the world waits behind (everything but the HUD, conversations and cutscenes:
    /// a cutscene runs on the simulation's ticks, which hold the world still meanwhile).
    pub fn stops_the_world(self) -> bool {
        !matches!(self, Screen::Hud | Screen::Conversation | Screen::Cutscene)
    }
}

/// The screen the simulation's state calls for; `menu` is the one the player opened
/// (inventory, character record, tactics). One screen is up at a time: a cutscene first
/// (what it hands on to waits for it), then a conversation (a store or job board it opens
/// waits for it to end), then the screens scripts open, then the player's, else the HUD.
pub fn wanted(sim: &Sim, menu: Option<Screen>) -> BTreeSet<Screen> {
    let w = &sim.world;
    // The whole party fell: the death screen, or the load screen it opens.
    let screen = if w.game_over {
        if menu == Some(Screen::SaveLoad) {
            Screen::SaveLoad
        } else {
            Screen::Death
        }
    } else if w.cutscene.is_some() {
        Screen::Cutscene
    } else if thedas_ui::binding::conversation(sim).is_some() {
        Screen::Conversation
    } else if w.party_picker.is_some() {
        Screen::PartyPicker
    } else if w.chargen.is_some() {
        // StartCharGen's mode: 0 character creation, else level-up.
        if w.chargen_mode == 0 {
            Screen::CharGen
        } else {
            Screen::LevelUp
        }
    } else if w.store.is_some() {
        Screen::Store
    } else if w.chanter_board.is_some() {
        Screen::Chanters
    } else if w.world_map.open {
        Screen::WorldMap
    } else if let Some(m) = menu {
        m
    } else {
        Screen::Hud
    };
    BTreeSet::from([screen])
}

/// `want` with the examine screen over it, while an item is being examined from a screen
/// that shows items (the inventory, the character record, a store). DAOrigins.exe opens it
/// over the screen as the game's `GUI_ITEM_EXAMINE` (58) screen. Provisional: which screens
/// it may stand over is taken from where the movies offer an inspect action.
pub fn with_examine(mut want: BTreeSet<Screen>, examining: bool) -> BTreeSet<Screen> {
    let from = want.iter().any(|s| {
        matches!(
            s,
            Screen::Inventory | Screen::CharacterRecord | Screen::Store
        )
    });
    if examining && from {
        want.insert(Screen::ItemExamine);
    }
    want
}

#[derive(Default)]
pub struct Screens {
    /// Up now.
    shown: BTreeSet<Screen>,
    /// Closed by their own movie while still wanted.
    dismissed: BTreeSet<Screen>,
    /// The screen the player opened (I the inventory, C the character record, T tactics, M
    /// the area map).
    pub menu: Option<Screen>,
    /// Screens whose movies closed themselves since the last [`Screens::sync`].
    closed_by_player: Vec<Screen>,
    /// No game is running yet: the start menu shows when no other screen is up.
    pub front_end: bool,
}

/// What [`Screens::update`] changed.
#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub opened: Vec<Screen>,
    pub closed: Vec<Screen>,
    /// Closed by their own movie (a close button): the store's must be closed in the
    /// simulation too.
    pub dismissed: Vec<Screen>,
}

impl Screens {
    /// No screen up yet; `front_end` before a game (the start menu shows).
    pub fn new(front_end: bool) -> Self {
        Self {
            front_end,
            ..Default::default()
        }
    }

    /// The player opens or closes one of their screens (a key). Not before a game: the
    /// start menu opens its own.
    pub fn toggle(&mut self, s: Screen) {
        if self.front_end {
            return;
        }
        self.menu = if self.menu == Some(s) { None } else { Some(s) };
    }
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

    /// Whether the party picker is up: the player picks followers by clicking them in the
    /// world, around the picker's panels.
    pub fn picking_party(&self) -> bool {
        self.shown.contains(&Screen::PartyPicker)
    }

    /// Notes screens whose movies closed themselves (given which movies are open).
    pub fn note_open(&mut self, open: &dyn Fn(&str) -> bool) {
        let gone: Vec<Screen> = self
            .shown
            .iter()
            .copied()
            // A screen's own movie is its first; those after it (the popup layer, the book's
            // page tabs, tooltips) stay when it closes itself.
            .filter(|s| s.movies().first().is_some_and(|m| !open(m)))
            .collect();
        for s in gone {
            self.shown.remove(&s);
            self.closed_by_player.push(s);
            if self.menu == Some(s) {
                self.menu = None;
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
            dismissed: Vec::new(),
        };
        self.shown = want;
        changes
    }

    /// [`Screens::update`] (to [`wanted`]) applied to `stage`: movies of closed screens are dropped, those of
    /// opened screens loaded (failures logged).
    pub fn sync(&mut self, want: BTreeSet<Screen>, stage: &mut Stage) -> Changes {
        self.note_open(&|m| stage.movies.iter().any(|x| x.name == m));
        let mut changes = self.update(want);
        changes.dismissed = std::mem::take(&mut self.closed_by_player);
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
    fn closing_the_players_screen_clears_the_toggle() {
        let mut s = Screens::default();
        s.toggle(Screen::CharacterRecord);
        s.update(set(&[Screen::CharacterRecord]));
        s.note_open(&|m| m != "characterrecord.gfx");
        assert_eq!(s.menu, None);
        s.toggle(Screen::Tactics);
        s.toggle(Screen::Inventory);
        assert_eq!(s.menu, Some(Screen::Inventory));
        s.toggle(Screen::Inventory);
        assert_eq!(s.menu, None);
    }

    #[test]
    fn the_hud_shows_when_nothing_else_does() {
        let sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        assert_eq!(wanted(&sim, None), set(&[Screen::Hud]));
        assert_eq!(
            wanted(&sim, Some(Screen::Inventory)),
            set(&[Screen::Inventory])
        );
        assert_eq!(wanted(&sim, Some(Screen::Tactics)), set(&[Screen::Tactics]));
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
        assert_eq!(wanted(&sim, Some(Screen::Inventory)), set(&[Screen::Store]));
        sim.world.store = None;
        assert_eq!(
            wanted(&sim, Some(Screen::Inventory)),
            set(&[Screen::WorldMap])
        );
        sim.world.cutscene = Some(thedas_sim::cutscene::Cutscene {
            name: "intro.cut".into(),
            seconds: 1.0,
            lines: Vec::new(),
            elapsed_ms: 0,
            from_dialog: false,
        });
        assert_eq!(
            wanted(&sim, Some(Screen::Inventory)),
            set(&[Screen::Cutscene])
        );
        // The simulation holds the world itself while its cutscene's clock runs.
        assert!(!Screen::Cutscene.stops_the_world());
    }

    #[test]
    fn game_over_shows_the_death_screen_and_the_load_screen_it_opens() {
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        assert_eq!(
            wanted(&sim, Some(Screen::SaveLoad)),
            set(&[Screen::SaveLoad])
        );
        sim.world.game_over = true;
        sim.world.store = Some(1);
        assert_eq!(wanted(&sim, None), set(&[Screen::Death]));
        assert_eq!(wanted(&sim, Some(Screen::Inventory)), set(&[Screen::Death]));
        assert_eq!(
            wanted(&sim, Some(Screen::SaveLoad)),
            set(&[Screen::SaveLoad])
        );
        assert!(Screen::Death.stops_the_world());
    }

    #[test]
    fn character_creation_and_level_up_have_their_own_screens() {
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        sim.world.chargen = Some(1);
        sim.world.chargen_mode = 0;
        assert_eq!(wanted(&sim, None), set(&[Screen::CharGen]));
        assert!(Screen::CharGen.stops_the_world());
        sim.world.chargen_mode = 1;
        assert_eq!(wanted(&sim, None), set(&[Screen::LevelUp]));
    }
}
