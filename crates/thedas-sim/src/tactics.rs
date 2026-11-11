//! The tactics screen (tactics.gfx) as player input, for the party member it shows (the one
//! the player controls). The screen edits slots by writing their attributes
//! (`GUI.SelectedChar.Tactics.TacticsList.N.Enabled`, `.Target`, `.Condition`, `.Command`,
//! `.CommandParam`; the interface passes these writes on as [`MSG_SET_ATTRIBUTE`]) and sends
//! messages for the rest: a preset chosen (`LoadPreset`: its place in `PresetIdList` and its
//! type), the AI behaviour (`SetBehaviour`), and an edit made (`DirtyPreset`).
//!
//! List row N is tactics slot N + 1: the scripts number slots from 1 (see
//! [`Sim::tactics_of`]). The messages are DAOrigins.exe's PartyTactics handlers: `LoadPreset`
//! 0x7ae4b0, `DirtyPreset` 0x7ad100 and `SetBehaviour` 0x7ad120 (registered at 0x7ae850,
//! 0x7ad190 and 0x7ad210). Provisional: the slot writes are read from tactics.gfx, not traced
//! in the executable, and custom presets (`LoadPreset` type 1, `SavePreset`) are not kept.

use thedas_script::vm::ObjectId;

use crate::sim::Sim;

/// The tactics screen's values and messages (tactics.gfx's `TACTICS_TOKEN`).
pub const PREFIX: &str = "GUI.SelectedChar.Tactics.";
/// A movie wrote one of the tactics attributes (the attribute and the value).
pub const MSG_SET_ATTRIBUTE: &str = "Tactics.SetAttribute";
/// A preset chosen: its place in the creature's list and its type ([`PRESET_PREDEFINED`]).
pub const MSG_LOAD_PRESET: &str = "GUI.SelectedChar.Tactics.LoadPreset";
/// The AI behaviour chosen (a row of `aibehaviors`, the movie's `TABLE_AI_BEHAVIORS`).
pub const MSG_SET_BEHAVIOUR: &str = "GUI.SelectedChar.Tactics.SetBehaviour";
/// A slot was edited: the slots no longer match the preset.
pub const MSG_DIRTY_PRESET: &str = "GUI.SelectedChar.Tactics.DirtyPreset";
/// Messages that change nothing here: the screen asks for its data again, checks an action
/// it set, or saves the slots as a custom preset (custom presets are not kept).
pub const QUIET: &[&str] = &[
    "GUI.SelectedChar.Tactics.RefreshTacticsData",
    "GUI.SelectedChar.Tactics.ValidateAction",
    "GUI.SelectedChar.Tactics.SavePreset",
];

/// tactics.gfx's `PRESET_TYPE_NONE`: the preset picker's "Off", tactics switched off.
pub const PRESET_NONE: i32 = 0;
/// tactics.gfx's `PRESET_TYPE_PREDEFINED` (a preset from `tacticsuserpresets`).
pub const PRESET_PREDEFINED: i32 = 2;
/// The creature property the behaviour picker shows and sets (tactics.gfx's
/// `CONST_PROPERTY_AIBEHAVIOUR`; DAOrigins.exe's `SetBehaviour` handler 0x7ad120 sets property
/// 0x12 of the selected creature and recalculates it). script.ldf names property 18
/// `PROPERTY_SIMPLE_AGE`.
pub const PROPERTY_AI_BEHAVIOUR: i32 = 18;
/// The creature field marking its slots as changed since their preset (the screen's `Dirty`).
pub const FIELD_DIRTY: &str = "TacticsDirty";
/// The creature field for whether its tactics run (the screen's AI on/off button and the
/// preset picker's "Off", `GUI.SelectedChar.Tactics.Enabled`); unset is on.
pub const FIELD_ENABLED: &str = "TacticsEnabled";
/// The creature field for the kind of preset in use ([`PRESET_NONE`] or
/// [`PRESET_PREDEFINED`]); unset is predefined when it has a preset.
pub const FIELD_PRESET_TYPE: &str = "TacticPresetType";

impl Sim {
    /// Whether a creature's tactics run: the screen's AI on/off and "Off" preset (PartyTactics'
    /// `Enabled`, which `GetNumTactics` checks, 0x66d580).
    pub fn tactics_enabled(&self, who: ObjectId) -> bool {
        self.world
            .get(who)
            .and_then(|o| o.fields.get(FIELD_ENABLED))
            .is_none_or(|&e| e != 0)
    }

    /// The party member the tactics screen shows.
    pub fn tactics_subject(&self) -> Option<ObjectId> {
        self.world.controlled()
    }

    /// The creature's predefined presets (its `TacticPreset.N`, `TacticPresetCount` of them),
    /// as the screen's `PresetIdList`, each once: the executable's `AddTacticPresetID` adds a
    /// preset only once (0x7af4e0), and saves from before it did may repeat them.
    pub fn tactic_presets(&self, who: ObjectId) -> Vec<i32> {
        let Some(o) = self.world.get(who) else {
            return Vec::new();
        };
        let n = o
            .fields
            .get("TacticPresetCount")
            .copied()
            .unwrap_or(0)
            .max(0);
        let mut list = Vec::new();
        for p in (0..n).filter_map(|i| o.fields.get(&format!("TacticPreset.{i}")).copied()) {
            if !list.contains(&p) {
                list.push(p);
            }
        }
        list
    }

    /// The tactics screen's messages and attribute writes. Returns whether the message was
    /// one of them.
    pub(crate) fn tactics_message(&mut self, name: &str, args: &[String]) -> bool {
        if QUIET.contains(&name) {
            return true;
        }
        let Some(who) = self.tactics_subject() else {
            return false;
        };
        let int = |i: usize| {
            args.get(i).and_then(|a| match a.trim() {
                "true" => Some(1),
                "false" => Some(0),
                t => t
                    .parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .map(|v| v as i32),
            })
        };
        match name {
            MSG_SET_ATTRIBUTE => {
                let key = args.first().map(|a| a.trim()).unwrap_or_default();
                let Some(rest) = key.strip_prefix(PREFIX) else {
                    return false;
                };
                let Some(value) = int(1) else {
                    return true;
                };
                if rest == "Enabled" {
                    if let Some(o) = self.world.get_mut(who) {
                        o.fields.insert(FIELD_ENABLED.into(), (value != 0) as i32);
                    }
                    return true;
                }
                let parts: Vec<&str> = rest.split('.').collect();
                let ["TacticsList", row, field] = parts.as_slice() else {
                    return true;
                };
                let Some(slot) = row.parse::<usize>().ok().map(|r| r + 1).filter(|&s| s < 64)
                else {
                    return true;
                };
                // The slots as they stand (a preset not yet applied is applied first).
                self.tactics_of(who);
                let Some(o) = self.world.get_mut(who) else {
                    return true;
                };
                if o.combat.tactics.len() <= slot {
                    o.combat.tactics.resize(slot + 1, Default::default());
                }
                let t = &mut o.combat.tactics[slot];
                match *field {
                    "Enabled" => t.enabled = value != 0,
                    "Target" => t.target = value,
                    "Condition" => t.condition = value,
                    "Command" => t.command = value,
                    "CommandParam" => t.param = value,
                    // `TargetID` (a particular party member), `ConditionID` and `CommandItem`
                    // are not kept. Provisional.
                    _ => {}
                }
                true
            }
            // As the executable's handler (0x7ae4b0): the slots are no longer edited; a
            // predefined preset turns tactics on and fills the slots; "Off" turns them off and
            // switches every slot off.
            MSG_LOAD_PRESET => {
                let (Some(index), Some(kind)) = (int(0), int(1)) else {
                    return true;
                };
                if let Some(o) = self.world.get_mut(who) {
                    o.fields.remove(FIELD_DIRTY);
                }
                match kind {
                    PRESET_PREDEFINED => {
                        let preset = usize::try_from(index)
                            .ok()
                            .and_then(|i| self.tactic_presets(who).get(i).copied());
                        if let Some(p) = preset {
                            if self.apply_tactic_preset(who, p) {
                                if let Some(o) = self.world.get_mut(who) {
                                    o.fields.insert(FIELD_ENABLED.into(), 1);
                                    o.fields.insert(FIELD_PRESET_TYPE.into(), PRESET_PREDEFINED);
                                }
                            }
                        }
                    }
                    PRESET_NONE => {
                        self.tactics_of(who);
                        if let Some(o) = self.world.get_mut(who) {
                            o.fields.insert(FIELD_ENABLED.into(), 0);
                            o.fields.insert(FIELD_PRESET_TYPE.into(), PRESET_NONE);
                            for t in &mut o.combat.tactics {
                                t.enabled = false;
                            }
                        }
                    }
                    _ => {}
                }
                true
            }
            MSG_SET_BEHAVIOUR => {
                if let (Some(b), Some(o)) = (int(0), self.world.get_mut(who)) {
                    // Base and current alike: no effect modifies this property.
                    o.properties
                        .insert(PROPERTY_AI_BEHAVIOUR, (b as f32, b as f32));
                }
                true
            }
            MSG_DIRTY_PRESET => {
                if let Some(o) = self.world.get_mut(who) {
                    o.fields.insert(FIELD_DIRTY.into(), 1);
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_formats::gda::Cell;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::world::ObjectKind;

    #[test]
    fn the_tactics_screen_edits_slots_loads_presets_and_sets_the_behaviour() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        // Two presets: user preset 1 (Tank) is table 2001 with two slots.
        s.world.tables().insert(
            "tacticsuserpresets",
            &["TacticsTable"],
            vec![(1, vec![Cell::Int(2001)]), (2, vec![Cell::Int(2002)])],
        );
        s.world.tables().insert(
            "#2001",
            &["ID", "TargetType", "Condition", "Command", "#20804179"],
            vec![
                (
                    1,
                    vec![
                        Cell::Int(1),
                        Cell::Int(1),
                        Cell::Int(74),
                        Cell::Int(5),
                        Cell::Int(808),
                    ],
                ),
                (
                    2,
                    vec![
                        Cell::Int(2),
                        Cell::Int(3),
                        Cell::Int(8),
                        Cell::Int(4),
                        Cell::Int(617),
                    ],
                ),
            ],
        );
        let o = s.world.get_mut(hero).unwrap();
        for (k, v) in [
            ("TacticPresetCount", 2),
            ("TacticPreset.0", 2),
            ("TacticPreset.1", 1),
        ] {
            o.fields.insert(k.into(), v);
        }
        let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // The second preset in the list (Tank).
        assert!(s.tactics_message(MSG_LOAD_PRESET, &a(&["1", "2"])));
        let t = s.world.get(hero).unwrap().combat.tactics.clone();
        assert_eq!(t.len(), 3, "slot 0 is unused");
        assert_eq!((t[1].condition, t[2].param), (74, 617));
        // Row 1 is slot 2: its condition and command change; it is switched off.
        let set = |s: &mut Sim, k: &str, v: &str| {
            s.tactics_message(
                MSG_SET_ATTRIBUTE,
                &a(&[&format!("GUI.SelectedChar.Tactics.TacticsList.{k}"), v]),
            )
        };
        assert!(set(&mut s, "1.Condition", "12"));
        assert!(set(&mut s, "1.Command", "5"));
        assert!(set(&mut s, "1.CommandParam", "3041"));
        assert!(set(&mut s, "1.Enabled", "false"));
        assert!(set(&mut s, "1.TargetID", "-1"), "taken, not kept");
        assert!(s.tactics_message(MSG_DIRTY_PRESET, &[]));
        let o = s.world.get(hero).unwrap();
        let t = &o.combat.tactics[2];
        assert_eq!(
            (t.enabled, t.condition, t.command, t.param),
            (false, 12, 5, 3041)
        );
        assert_eq!(o.fields.get(FIELD_DIRTY), Some(&1));
        // A new slot past the preset's.
        assert!(set(&mut s, "4.Target", "2"));
        assert_eq!(s.world.get(hero).unwrap().combat.tactics[5].target, 2);
        // Disable tactics, and the behaviour.
        assert!(s.tactics_message(
            MSG_SET_ATTRIBUTE,
            &a(&["GUI.SelectedChar.Tactics.Enabled", "0"])
        ));
        assert!(s.tactics_message(MSG_SET_BEHAVIOUR, &a(&["2"])));
        let o = s.world.get(hero).unwrap();
        assert_eq!(o.fields.get(FIELD_ENABLED), Some(&0));
        assert_eq!(o.properties.get(&PROPERTY_AI_BEHAVIOUR), Some(&(2.0, 2.0)));
        // Loading a preset again clears the edits.
        assert!(s.tactics_message(MSG_LOAD_PRESET, &a(&["1", "2"])));
        let o = s.world.get(hero).unwrap();
        assert_eq!(o.combat.tactics[2].condition, 8);
        assert!(!o.fields.contains_key(FIELD_DIRTY));
        // "Off": tactics stop and every slot is switched off.
        assert!(s.tactics_message(MSG_LOAD_PRESET, &a(&["0", "0"])));
        let o = s.world.get(hero).unwrap();
        assert!(o.combat.tactics.iter().all(|t| !t.enabled));
        assert_eq!(o.fields.get(FIELD_PRESET_TYPE), Some(&PRESET_NONE));
        assert!(!s.tactics_enabled(hero));
        // A preset whose table is missing changes nothing.
        assert!(s.tactics_message(MSG_LOAD_PRESET, &a(&["0", "2"])));
        assert!(!s.tactics_enabled(hero));
        assert!(s.tactics_message(MSG_LOAD_PRESET, &a(&["1", "2"])));
        assert!(s.tactics_enabled(hero));
        let o = s.world.get(hero).unwrap();
        assert!(o.combat.tactics[1].enabled);
        assert_eq!(o.fields.get(FIELD_PRESET_TYPE), Some(&PRESET_PREDEFINED));
        assert!(!s.tactics_message("GUI.Other", &[]));
    }
}
