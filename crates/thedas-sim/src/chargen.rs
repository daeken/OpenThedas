//! Character creation's screen (chargen.gfx) as player input: its choices become the
//! character-generation events the module's `sys_chargen` script handles, for the creature
//! in character creation (`StartCharGen` mode 0, [`World::chargen_mode`]). The name and
//! the shared attribute and ability messages go through [`Sim::level_up_message`].
//!
//! [`World::chargen_mode`]: crate::world::World::chargen_mode

use thedas_script::vm::ObjectId;

use crate::sim::Sim;

/// The screen opened for the creature.
pub const MSG_START: &str = "GUICharGen.CharGenStart";
/// Gender, race, class and background: the creature, the old choice, the new one and
/// whether the creature's looks follow it.
pub const MSG_GENDER: &str = "GUICharGen.CharGenSelectGender";
pub const MSG_RACE: &str = "GUICharGen.CharGenSelectRace";
pub const MSG_CLASS: &str = "GUICharGen.CharGenSelectClass";
pub const MSG_BACKGROUND: &str = "GUICharGen.CharGenSelectBackground";
/// The voice: the creature, 0 and the sound set.
pub const MSG_SOUNDSET: &str = "GUICharGen.CharGenSelectAppearanceAndSoundSet";
/// The screen closed: the creature, 0 and how it was left (`CHARGEN_EXIT_*`: 0 default,
/// 1 basic, 2 advanced, 3 level of the week, 4 import).
pub const MSG_END: &str = "GUICharGen.CharGenEnd";
/// Messages about the screen's own presentation (the appearance scene's camera and the
/// head being edited, the telemetry it uploads, starting over, importing a hero), which
/// change nothing in the game here.
pub const PRESENTATION: &[&str] = &[
    "GUICharGen.CharGenResetCreature",
    "GUICharGen.CharGenStartAppearanceScene",
    "GUICharGen.UpdateHeroAppearance",
    "GUICharGen.CharGenUploadInfo",
    "GUICharGen.CharGenSaveInfo",
    "GUICharGen.Reset",
    "GUICharGen.ImportHero",
];

/// A client option changed: the option and its value. Character creation ends by asking
/// for the difficulty (`GameOptions.DifficultyLevel`); the options menu sets it too.
pub const MSG_CLIENT_OPTION: &str = "ClientOptions.SetClientOptionMessage";
/// The client options were committed (written to the player's settings): nothing to do here.
pub const MSG_COMMIT_OPTIONS: &str = "ClientOptions.CommitOptions";

/// `background_defaults`' M2DA id (chargen.gfx's `TABLE_BACKGROUND_DEFAULTS`) and its
/// default-name columns' hashes (`HASH_DEFAULTNAMEMALE`, `HASH_DEFAULTNAMEFEMALE`).
const TABLE_BACKGROUND_DEFAULTS: i32 = 1001;
const HASH_DEFAULT_NAME_MALE: u32 = 2_407_608_778;
const HASH_DEFAULT_NAME_FEMALE: u32 = 79_362_247;

impl Sim {
    /// The first name character creation offers a creature (the name entry starts with it):
    /// the background defaults table's row race × 1000 + class × 100 + background (chargen.gfx's
    /// `CreatureUtils.BackgroundDefaultsIndex`), its male or female default name (a talk-table
    /// string). `None` when the row or the string is missing.
    pub fn default_first_name(&self, id: ObjectId) -> Option<String> {
        let o = self.world.get(id)?;
        let race = o.fields.get("Race").copied().unwrap_or(0) as i64;
        let class = self.property_total(id, 27) as i64;
        let background = self.property_total(id, 37) as i64;
        let column = if o.fields.get("Gender").copied() == Some(2) {
            HASH_DEFAULT_NAME_FEMALE
        } else {
            HASH_DEFAULT_NAME_MALE
        };
        let row = race * 1000 + class * 100 + background;
        let strref = match self
            .world
            .tables()
            .get(TABLE_BACKGROUND_DEFAULTS, "")?
            .cell_by_hash(column, row)?
        {
            thedas_formats::gda::Cell::Int(v) if *v > 0 => *v as u32,
            _ => return None,
        };
        self.string(strref).filter(|s| !s.is_empty())
    }

    /// The client options the game keeps in its world: the difficulty (0 casual to 3
    /// nightmare), which `GetGameDifficulty` reads. The player's auto-pause and tutorial
    /// settings are read by the simulation too ([`Sim::auto_pause_combat`],
    /// [`Sim::show_tutorials`]) but left for the interface to keep. Other options are not
    /// taken. Provisional: the executable keeps the difficulty
    /// with the client options; the save does not record it there (here the world does).
    pub fn client_option_message(&mut self, name: &str, args: &[String]) -> bool {
        match name {
            MSG_CLIENT_OPTION
                if args.first().map(|a| a.trim()) == Some("GameOptions.DifficultyLevel") =>
            {
                match args
                    .get(1)
                    .and_then(|a| a.trim().parse::<f64>().ok())
                    .filter(|v| v.fract() == 0.0 && (0.0..=3.0).contains(v))
                {
                    Some(v) => {
                        self.world.difficulty = v as i32;
                        true
                    }
                    None => false,
                }
            }
            // The player's settings the simulation reads; the interface keeps them too (so
            // the message is not taken).
            MSG_CLIENT_OPTION => {
                let on = args
                    .get(1)
                    .and_then(|a| a.trim().parse::<f64>().ok())
                    .map(|v| v != 0.0);
                match (args.first().map(|a| a.trim()), on) {
                    (Some("GameOptions.AutoPauseCombat"), Some(on)) => self.auto_pause_combat = on,
                    (Some("GameOptions.ShowTutorials"), Some(on)) => self.show_tutorials = on,
                    _ => {}
                }
                false
            }
            // The options screen's Restore Defaults: both back to how the game's options start.
            "ClientOptions.RestoreDefaults" => {
                self.auto_pause_combat = false;
                self.show_tutorials = true;
                false
            }
            MSG_COMMIT_OPTIONS => true,
            _ => false,
        }
    }

    /// The creation screen's messages, while a creature is in character creation. Each choice
    /// becomes the event sys_chargen handles, with the choice and the screen's
    /// update-appearance flag as its integers (`CHARGEN_SELECT_GENDER` 54, `_RACE` 46,
    /// `_CLASS` 47, `_BACKGROUND` 55: sys_chargen reads integer 0 as the choice, and applies a
    /// class only with integer 1 set), the voice as `CHARGEN_SELECT_SOUNDSET` (48; its
    /// string 0 goes to `SetSoundSet`), and closing as `CHARGEN_END` (mode 0 and the exit,
    /// which sys_chargen reads as its "quick start": 0 picks a pre-made character, 3 starts
    /// in Lothering). Provisional: the executable's message handlers (`GUICharGen`, for
    /// `CharGenSelectRaceMessage` and the rest) are not traced; the layout is read from
    /// chargen.gfx's arguments and sys_chargen's handlers.
    pub(crate) fn chargen_message(&mut self, name: &str, args: &[String]) -> bool {
        if self.world.chargen.is_none() || self.world.chargen_mode != 0 {
            return false;
        }
        // The screen announces itself (its creature may not have reached it yet: it learns
        // it from `CharGenCreature.ID`), and its presentation messages change nothing here.
        // The face screen opens on its first head for the creature's race and gender when it
        // has no face yet, so the preview shows what its sliders say. Provisional: the
        // exe's own choice of starting head is not traced.
        if name == "GUICharGen.CharGenStartAppearanceScene" {
            if let Some(c) = self.world.chargen {
                if self.world.get(c).is_some_and(|o| o.head_morph.is_none()) {
                    let first = self.face_screen_default(c);
                    if let Some(o) = self.world.get_mut(c) {
                        o.head_morph = first;
                    }
                }
            }
            return true;
        }
        if name == MSG_START || PRESENTATION.contains(&name) {
            return true;
        }
        let int = |i: usize| {
            args.get(i)
                .and_then(|a| a.trim().parse::<f64>().ok())
                .filter(|v| v.fract() == 0.0 && v.abs() < i32::MAX as f64)
                .map(|v| v as i32)
        };
        let who = int(0).filter(|&w| w >= 0).map(|w| w as ObjectId);
        if who.is_none() || who != self.world.chargen {
            return false;
        }
        let choice = match name {
            MSG_GENDER => "CHARGEN_SELECT_GENDER",
            MSG_RACE => "CHARGEN_SELECT_RACE",
            MSG_CLASS => "CHARGEN_SELECT_CLASS",
            MSG_BACKGROUND => "CHARGEN_SELECT_BACKGROUND",
            MSG_SOUNDSET => {
                return match args.get(2).map(|s| s.trim()) {
                    Some(set) if !set.is_empty() => self.chargen_event(
                        "CHARGEN_SELECT_SOUNDSET",
                        Vec::new(),
                        vec![set.to_string()],
                    ),
                    _ => true,
                };
            }
            MSG_END => {
                return self.chargen_event("CHARGEN_END", vec![0, int(2).unwrap_or(0)], Vec::new());
            }
            _ => return false,
        };
        // A new race or gender takes the face made for the old one off (its presets and
        // hairs are the old one's): the face screen starts over.
        if matches!(name, MSG_GENDER | MSG_RACE) {
            if let Some(o) = who.and_then(|w| self.world.get_mut(w)) {
                o.head_morph = None;
                o.face_choices.clear();
            }
        }
        match int(2) {
            Some(new) => self.chargen_event(choice, vec![new, int(3).unwrap_or(1)], Vec::new()),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::world::ObjectKind;

    const LDF: &str = "int EVENT_TYPE_CHARGEN_START = 44;\n\
        int EVENT_TYPE_CHARGEN_SELECT_RACE = 46;\n\
        int EVENT_TYPE_CHARGEN_SELECT_CLASS = 47;\n\
        int EVENT_TYPE_CHARGEN_SELECT_SOUNDSET = 48;\n\
        int EVENT_TYPE_CHARGEN_SELECT_GENDER = 54;\n\
        int EVENT_TYPE_CHARGEN_SELECT_BACKGROUND = 55;\n\
        int EVENT_TYPE_CHARGEN_END = 59;\n";

    fn queued(s: &Sim) -> Vec<(i32, Vec<i32>, Vec<String>)> {
        s.world
            .queue
            .values()
            .map(|q| (q.event.ty, q.event.ints.clone(), q.event.strings.clone()))
            .collect()
    }

    #[test]
    fn the_creation_screen_choices_become_chargen_events() {
        let table = EngineTable::parse(LDF.as_bytes()).unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        let m = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(m);
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.chargen_policy = crate::sim::ChargenPolicy::Manual;
        let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // Nobody in character creation: not the screen's.
        assert!(!s.chargen_message(MSG_RACE, &a(&["1", "0", "2", "1"])));
        // Level-up (mode 1) is levelup.gfx's.
        s.start_chargen(hero, 1, 0);
        assert!(!s.chargen_message(MSG_RACE, &a(&[&hero.to_string(), "0", "2", "1"])));
        s.chargen_event("CHARGEN_END", vec![1, 0], Vec::new());
        s.world.queue.clear();

        s.start_chargen(hero, 0, 0);
        assert_eq!(s.world.chargen_mode, 0);
        s.world.queue.clear();
        let h = hero.to_string();
        assert!(s.chargen_message(MSG_START, &a(&[&h])));
        assert!(s.chargen_message(MSG_GENDER, &a(&[&h, "1", "2", "1"])));
        assert!(s.chargen_message(MSG_RACE, &a(&[&h, "1", "2", "1"])));
        assert!(s.chargen_message(MSG_CLASS, &a(&[&h, "0", "3", "0"])));
        assert!(s.chargen_message(MSG_BACKGROUND, &a(&[&h, "0", "7", "1"])));
        assert!(s.chargen_message(MSG_SOUNDSET, &a(&[&h, "0", "pc_f_cocky"])));
        assert!(s.chargen_message("GUICharGen.CharGenResetCreature", &[]));
        // Another creature's choice is not taken.
        assert!(!s.chargen_message(MSG_RACE, &a(&["99999", "1", "2", "1"])));
        assert_eq!(
            queued(&s),
            vec![
                (54, vec![2, 1], vec![]),
                (46, vec![2, 1], vec![]),
                (47, vec![3, 0], vec![]),
                (55, vec![7, 1], vec![]),
                (48, vec![], vec!["pc_f_cocky".to_string()]),
            ]
        );
        assert!(s.chargen_message(MSG_END, &a(&[&h, "0", "1"])));
        // The difficulty prompt that ends it.
        assert_eq!(s.world.difficulty, 1);
        assert!(
            s.client_option_message(MSG_CLIENT_OPTION, &a(&["GameOptions.DifficultyLevel", "2"]))
        );
        assert_eq!(s.world.difficulty, 2);
        assert!(!s.client_option_message(
            MSG_CLIENT_OPTION,
            &a(&["GameOptions.DifficultyLevel", "NaN"])
        ));
        assert_eq!(s.world.difficulty, 2);
        // The player's settings the simulation reads are left for the interface to keep.
        assert!(
            !s.client_option_message(MSG_CLIENT_OPTION, &a(&["GameOptions.AutoPauseCombat", "1"]))
        );
        assert!(s.auto_pause_combat);
        assert!(
            !s.client_option_message(MSG_CLIENT_OPTION, &a(&["GameOptions.ShowTutorials", "0"]))
        );
        assert!(!s.show_tutorials);
        assert!(!s.client_option_message("ClientOptions.RestoreDefaults", &[]));
        assert!(!s.auto_pause_combat && s.show_tutorials);
        assert_eq!(queued(&s).last(), Some(&(59, vec![0, 1], vec![])));
        assert_eq!(s.world.chargen, None);
    }

    #[test]
    fn an_unnamed_hero_goes_by_the_name_creation_offers() {
        use thedas_formats::gda::Cell;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        let other = s.world.spawn(ObjectKind::Creature, "npc");
        s.world.hero = Some(hero);
        // Human (3) warrior (class 1) noble (background 5): row 3105.
        let o = s.world.get_mut(hero).unwrap();
        o.fields.insert("Race".into(), 3);
        o.fields.insert("Gender".into(), 1);
        o.properties.insert(27, (1.0, 1.0));
        o.properties.insert(37, (5.0, 5.0));
        s.world.tables().insert(
            "#1001",
            &["#8f8131ca", "#04baf8c7"],
            vec![(3105, vec![Cell::Int(9001), Cell::Int(9002)])],
        );
        let _ = s.strings.set(
            [(9001, "Aedan".to_string()), (9002, "Elissa".to_string())]
                .into_iter()
                .collect(),
        );
        assert_eq!(s.default_first_name(hero).as_deref(), Some("Aedan"));
        assert_eq!(s.display_name(hero), "Aedan");
        s.world
            .get_mut(hero)
            .unwrap()
            .fields
            .insert("Gender".into(), 2);
        assert_eq!(s.display_name(hero), "Elissa");
        // A name chosen wins; others keep their own names (here, the tag).
        s.world.get_mut(hero).unwrap().name = Some("Morrigan's Friend".into());
        assert_eq!(s.display_name(hero), "Morrigan's Friend");
        assert_eq!(s.display_name(other), "npc");
    }
}
