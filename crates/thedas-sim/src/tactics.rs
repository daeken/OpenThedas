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

/// The tactics tables (M2DA ids, as DAOrigins.exe loads them) and their columns (by hash:
/// the names are not all known).
pub const TABLE_ABILITIES: i32 = 1;
pub const TABLE_CONDITIONS: i32 = 137;
pub const TABLE_BASE_CONDITIONS: i32 = 139;
pub const TABLE_TARGET_TYPES: i32 = 140;
pub const TABLE_COMMAND_TYPES: i32 = 141;
const HASH_SHOW: u32 = 0x2a1af533;
const HASH_PER_FOLLOWER: u32 = 0x1f36ea4e;
const HASH_TARGET_MASK: u32 = 0x12a02374;
const HASH_MASK: u32 = 0x77485db5;
const HASH_CONDITION_BASE: u32 = 0x56bb2ec7;
const HASH_SELF_ALLOWED: u32 = 0x0a7453a1;
const HASH_LIMITS_ABILITIES: u32 = 0x8a2e61df;
const HASH_USE_TYPE: u32 = 0xacefc4c2;
const HASH_USES_ITEM: u32 = 0x58d1de43;
const HASH_TARGETS: u32 = 0x0f642429;
const HASH_BASE_ALLOWED: u32 = 0x2eb7565c;
const HASH_FOR_TACTICS: u32 = 0x50a30845;
/// TacticsTargetTypes' Self and Hero rows.
const TARGET_SELF: i32 = 1;
const TARGET_HERO: i32 = 5;

/// A table cell as a number (0 when missing or not a number).
fn int(t: &crate::m2da::Table, hash: u32, row: i64) -> i64 {
    use thedas_formats::gda::Cell;
    match t.cell_by_hash(hash, row) {
        Some(Cell::Int(v)) => *v,
        Some(Cell::Float(v)) => *v as i64,
        Some(Cell::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

impl Sim {
    /// Whether a creature's tactics run: the screen's AI on/off and "Off" preset (PartyTactics'
    /// `Enabled`, which `GetNumTactics` checks, 0x66d580).
    pub fn tactics_enabled(&self, who: ObjectId) -> bool {
        self.world
            .get(who)
            .and_then(|o| o.fields.get(FIELD_ENABLED))
            .is_none_or(|&e| e != 0)
    }

    /// The target types the tactics screen offers `who` (`ListTacticTargets`): each shown
    /// row of TacticsTargetTypes (140) that some action can be aimed at, with -1, and for a
    /// per-follower type, the type once for each follower other than the hero and `who`,
    /// with the follower. The hero's own type (5) is not offered to the hero. As
    /// DAOrigins.exe's 0x7b0a80.
    pub fn tactic_targets(&self, who: ObjectId) -> (Vec<i32>, Vec<i64>) {
        let (mut types, mut ids) = (Vec::new(), Vec::new());
        let Some(t) = self.world.tables().get(TABLE_TARGET_TYPES, "") else {
            return (types, ids);
        };
        let hero = self.world.hero;
        for &row in &t.order {
            if int(&t, HASH_SHOW, row) == 0 {
                continue;
            }
            let ty = row as i32;
            if int(&t, HASH_PER_FOLLOWER, row) != 0 {
                for m in crate::engine::party_of(self) {
                    if Some(m) == hero || m == who {
                        continue;
                    }
                    if !self.tactic_actions(who, ty, 0).is_empty() {
                        types.push(ty);
                        ids.push(m as i64);
                    }
                }
            } else {
                if Some(who) == hero && ty == TARGET_HERO {
                    continue;
                }
                if !self.tactic_actions(who, ty, 0).is_empty() {
                    types.push(ty);
                    ids.push(-1);
                }
            }
        }
        (types, ids)
    }

    /// The condition groups (TacticsBaseConditions, 139) offered for a target type
    /// (`ListTacticBaseConditions`): shown rows whose target mask meets the type's and that
    /// hold some condition for it. As DAOrigins.exe's 0x7af570.
    pub fn tactic_base_conditions(&self, target_type: i32) -> Vec<i32> {
        let tables = self.world.tables();
        let (Some(base), Some(types)) = (
            tables.get(TABLE_BASE_CONDITIONS, ""),
            tables.get(TABLE_TARGET_TYPES, ""),
        ) else {
            return Vec::new();
        };
        let mask = int(&types, HASH_TARGET_MASK, target_type as i64);
        if mask == 0 {
            return Vec::new();
        }
        base.order
            .iter()
            .filter(|&&row| {
                int(&base, HASH_SHOW, row) != 0
                    && int(&base, HASH_MASK, row) & mask != 0
                    && !self.tactic_conditions(row as i32, target_type).0.is_empty()
            })
            .map(|&row| row as i32)
            .collect()
    }

    /// The conditions of a group (`ListTacticConditions`): shown rows of TacticsConditions
    /// (137) in the group, those not allowed for Self (target type 1) left out there, a
    /// per-follower condition once for each party member but the hero, with the member (else
    /// -1). As DAOrigins.exe's 0x7aeb50.
    pub fn tactic_conditions(&self, base: i32, target_type: i32) -> (Vec<i32>, Vec<i64>) {
        let (mut conds, mut ids) = (Vec::new(), Vec::new());
        let Some(t) = self.world.tables().get(TABLE_CONDITIONS, "") else {
            return (conds, ids);
        };
        let hero = self.world.hero;
        for &row in &t.order {
            if int(&t, HASH_SHOW, row) == 0 || int(&t, HASH_CONDITION_BASE, row) != base as i64 {
                continue;
            }
            if int(&t, HASH_SELF_ALLOWED, row) == 0 && target_type == TARGET_SELF {
                continue;
            }
            if int(&t, HASH_PER_FOLLOWER, row) != 0 {
                for m in crate::engine::party_of(self) {
                    if Some(m) != hero {
                        conds.push(row as i32);
                        ids.push(m as i64);
                    }
                }
            } else {
                conds.push(row as i32);
                ids.push(-1);
            }
        }
        (conds, ids)
    }

    /// The actions (CommandTypes, 141) `who` can take on a target type
    /// (`ListTacticActions`): shown rows whose target mask meets the type's, of use type 0, or
    /// 4 when `who` has an ability for it (or the party an item, for the use-item row), or 2
    /// when `who` has a mode to switch. As DAOrigins.exe's 0x7afe80.
    pub fn tactic_actions(&self, who: ObjectId, target_type: i32, base: i32) -> Vec<i32> {
        let tables = self.world.tables();
        let (Some(cmds), Some(types)) = (
            tables.get(TABLE_COMMAND_TYPES, ""),
            tables.get(TABLE_TARGET_TYPES, ""),
        ) else {
            return Vec::new();
        };
        let mask = int(&types, HASH_TARGET_MASK, target_type as i64);
        if mask == 0 {
            return Vec::new();
        }
        cmds.order
            .iter()
            .filter(|&&row| {
                if int(&cmds, HASH_SHOW, row) == 0 || int(&cmds, HASH_MASK, row) & mask == 0 {
                    return false;
                }
                match int(&cmds, HASH_USE_TYPE, row) {
                    0 => true,
                    4 if int(&cmds, HASH_USES_ITEM, row) != 0 => {
                        !self.tactic_items(target_type).is_empty()
                    }
                    4 => !self.tactic_abilities(who, target_type, base).is_empty(),
                    2 => !self.tactic_modal_abilities(who).is_empty(),
                    _ => false,
                }
            })
            .map(|&row| row as i32)
            .collect()
    }

    /// The abilities `who` knows that tactics can use (`ListTacticAbilities`): ABI_base rows
    /// of use type 4 marked for tactics, and with a condition group that limits them, only
    /// those it allows. As DAOrigins.exe's 0x7aed50. Provisional: the executable tests the
    /// target type's bits against 7 (always set), so the target type limits nothing; kept.
    pub fn tactic_abilities(&self, who: ObjectId, _target_type: i32, base: i32) -> Vec<i32> {
        let tables = self.world.tables();
        let Some(abi) = tables.get(TABLE_ABILITIES, "") else {
            return Vec::new();
        };
        let limited = base != 0
            && tables
                .get(TABLE_BASE_CONDITIONS, "")
                .is_some_and(|b| int(&b, HASH_LIMITS_ABILITIES, base as i64) != 0);
        self.known_abilities(who)
            .into_iter()
            .filter(|&id| {
                let row = id as i64;
                int(&abi, HASH_USE_TYPE, row) == 4
                    && int(&abi, HASH_FOR_TACTICS, row) != 0
                    && (!limited || int(&abi, HASH_BASE_ALLOWED, row) != 0)
            })
            .collect()
    }

    /// The modes `who` can switch (`ListTacticModalAbilities`): known abilities of use type 2
    /// marked for tactics. As DAOrigins.exe's 0x7aef50.
    pub fn tactic_modal_abilities(&self, who: ObjectId) -> Vec<i32> {
        let Some(abi) = self.world.tables().get(TABLE_ABILITIES, "") else {
            return Vec::new();
        };
        self.known_abilities(who)
            .into_iter()
            .filter(|&id| {
                int(&abi, HASH_USE_TYPE, id as i64) == 2
                    && int(&abi, HASH_FOR_TACTICS, id as i64) != 0
            })
            .collect()
    }

    /// The party's items a use-item command can use on a target type (`ListTacticItems`):
    /// those whose ability is marked for tactics and aims at the type (Self 1, Enemy 4, others
    /// 2, against the ability's target bits). As DAOrigins.exe's 0x7af6c0. Items of one tag
    /// are listed once (the executable keeps them as one stack; here each is its own object).
    pub fn tactic_items(&self, target_type: i32) -> Vec<ObjectId> {
        let Some(abi) = self.world.tables().get(TABLE_ABILITIES, "") else {
            return Vec::new();
        };
        let bit = match target_type {
            1 => 1,
            3 => 4,
            _ => 2,
        };
        let mut tags = std::collections::BTreeSet::new();
        self.party_backpack()
            .into_iter()
            .filter(|&item| {
                let Some(a) = self.world.get(item).and_then(|o| o.fields.get("AbilityID")) else {
                    return false;
                };
                let row = *a as i64;
                int(&abi, HASH_TARGETS, row) & bit != 0
                    && int(&abi, HASH_FOR_TACTICS, row) != 0
                    && self
                        .world
                        .get(item)
                        .is_some_and(|o| tags.insert(o.tag.clone()))
            })
            .collect()
    }

    fn known_abilities(&self, who: ObjectId) -> Vec<i32> {
        self.world
            .get(who)
            .map(|o| o.abilities.iter().copied().collect())
            .unwrap_or_default()
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
                // An empty value clears an object (`CommandItem` "" for a command that uses
                // no item); others need a number.
                let object = int(1)
                    .and_then(|v| u32::try_from(v).ok())
                    .filter(|&o| o > 0);
                if rest == "Enabled" {
                    let Some(value) = int(1) else {
                        return true;
                    };
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
                let value = match int(1) {
                    Some(v) => v,
                    None if *field == "CommandItem" => 0,
                    None => return true,
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
                    // The party member a per-follower target or condition names (-1 none),
                    // and the item a use-item command uses ("" none).
                    "TargetID" => t.target_object = object,
                    "ConditionID" => t.condition_object = object,
                    "CommandItem" => t.item = object,
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

    #[test]
    fn the_pickers_list_what_the_tables_and_the_party_allow() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let dog = s.world.spawn(ObjectKind::Creature, "dog");
        let ali = s.world.spawn(ObjectKind::Creature, "alistair");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers = vec![dog, ali];
        let t = s.world.tables();
        let c = |v: &[i64]| v.iter().map(|&x| Cell::Int(x)).collect::<Vec<_>>();
        let h = |x: u32| format!("#{x:08x}");
        // Target types: Self, Enemy, Hero, a per-follower type, and a hidden one.
        let (show, tmask, per) = (h(HASH_SHOW), h(HASH_TARGET_MASK), h(HASH_PER_FOLLOWER));
        t.insert(
            "#140",
            &[&show, &tmask, &per],
            vec![
                (1, c(&[1, 1, 0])),
                (3, c(&[1, 4, 0])),
                (4, c(&[0, 8, 0])),
                (5, c(&[1, 16, 0])),
                (6, c(&[1, 32, 1])),
            ],
        );
        // Commands: Attack (enemies only), use ability (4), activate mode (2), use item.
        let (mask, use_type, item) = (h(HASH_MASK), h(HASH_USE_TYPE), h(HASH_USES_ITEM));
        t.insert(
            "#141",
            &[&show, &mask, &use_type, &item],
            vec![
                (2, c(&[1, 4, 0, 0])),
                (4, c(&[1, 63, 4, 0])),
                (5, c(&[1, 63, 2, 0])),
                (7, c(&[1, 63, 4, 1])),
            ],
        );
        // Condition groups: HP (all types), Clustered (enemies, limits abilities).
        let limits = h(HASH_LIMITS_ABILITIES);
        t.insert(
            "#139",
            &[&show, &mask, &limits],
            vec![(2, c(&[1, 63, 0])), (4, c(&[1, 4, 1])), (9, c(&[1, 63, 0]))],
        );
        // Conditions: two HP ones (one not for Self), a clustered one, a per-follower one.
        let (base, self_ok) = (h(HASH_CONDITION_BASE), h(HASH_SELF_ALLOWED));
        t.insert(
            "#137",
            &[&show, &base, &self_ok, &per],
            vec![
                (1, c(&[1, 2, 1, 0])),
                (2, c(&[1, 2, 0, 0])),
                (25, c(&[1, 4, 1, 0])),
                (30, c(&[1, 9, 1, 1])),
            ],
        );
        // Abilities: a talent (allowed when clustered), a talent, a mode, a potion's ability.
        let (targets, allowed, tactics) =
            (h(HASH_TARGETS), h(HASH_BASE_ALLOWED), h(HASH_FOR_TACTICS));
        t.insert(
            "#1",
            &[&use_type, &targets, &allowed, &tactics],
            vec![
                (617, c(&[4, 4, 0, 1])),
                (700, c(&[4, 4, 1, 1])),
                (19, c(&[2, 1, 0, 1])),
                (90, c(&[4, 1, 0, 1])),
            ],
        );
        s.world.get_mut(hero).unwrap().abilities = [617, 700].into();
        // Two potions of one kind in the dog's pack.
        for _ in 0..2 {
            let p = s.world.spawn(ObjectKind::Item, "potion");
            s.world
                .get_mut(p)
                .unwrap()
                .fields
                .insert("AbilityID".into(), 90);
            s.world.get_mut(dog).unwrap().inventory.push(p);
        }

        // Self can be used (an item), not the hero's own type; the follower type once for
        // each follower but the hero and the one asking.
        let (types, ids) = s.tactic_targets(hero);
        assert_eq!(types, vec![1, 3, 6, 6]);
        assert_eq!(ids, vec![-1, -1, dog as i64, ali as i64]);
        // The dog has no ability: nothing to do to the hero or a follower.
        let (types, ids) = s.tactic_targets(dog);
        assert_eq!((types, ids), (vec![1, 3], vec![-1, -1]));
        s.world.get_mut(dog).unwrap().abilities.insert(617);
        let (types, ids) = s.tactic_targets(dog);
        assert_eq!(
            (types, ids),
            (vec![1, 3, 5, 6], vec![-1, -1, -1, ali as i64])
        );
        assert_eq!(s.tactic_actions(hero, 3, 0), vec![2, 4]);
        // Abilities are offered whatever the target type (see `tactic_abilities`).
        assert_eq!(s.tactic_actions(hero, 1, 0), vec![4, 7]);
        s.world.get_mut(hero).unwrap().abilities.insert(19);
        assert_eq!(s.tactic_actions(hero, 1, 0), vec![4, 5, 7]);
        assert_eq!(s.tactic_modal_abilities(hero), vec![19]);
        // Groups with conditions for the type; clustered limits the abilities.
        assert_eq!(s.tactic_base_conditions(3), vec![2, 4, 9]);
        assert_eq!(s.tactic_base_conditions(1), vec![2, 9]);
        assert_eq!(s.tactic_conditions(2, 1).0, vec![1], "not for Self");
        assert_eq!(s.tactic_conditions(2, 3).0, vec![1, 2]);
        let (conds, ids) = s.tactic_conditions(9, 3);
        assert_eq!((conds, ids), (vec![30, 30], vec![dog as i64, ali as i64]));
        assert_eq!(s.tactic_abilities(hero, 3, 0), vec![617, 700]);
        assert_eq!(s.tactic_abilities(hero, 3, 4), vec![700]);
        assert_eq!(s.tactic_items(1).len(), 1, "one stack");
        assert!(s.tactic_items(3).is_empty());

        // The screen's per-follower picks and items reach the slot.
        let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let set = |s: &mut Sim, k: &str, v: &str| {
            s.tactics_message(
                MSG_SET_ATTRIBUTE,
                &a(&[&format!("GUI.SelectedChar.Tactics.TacticsList.0.{k}"), v]),
            )
        };
        let potion = s.tactic_items(1)[0];
        set(&mut s, "TargetID", &dog.to_string());
        set(&mut s, "ConditionID", &ali.to_string());
        set(&mut s, "CommandItem", &potion.to_string());
        let slot = s.world.get(hero).unwrap().combat.tactics[1].clone();
        assert_eq!(
            (slot.target_object, slot.condition_object, slot.item),
            (Some(dog), Some(ali), Some(potion))
        );
        set(&mut s, "TargetID", "-1");
        set(&mut s, "CommandItem", "");
        let slot = s.world.get(hero).unwrap().combat.tactics[1].clone();
        assert_eq!((slot.target_object, slot.item), (None, None));
    }
}
