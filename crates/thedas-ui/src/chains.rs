//! The ability chains the character-generation and level-up screens list
//! (`CharGenCreature.Stats.TalentChains`, `SpellChains`, `SkillChains`), which the game's
//! engine builds when a screen sends `GameModeController.FindAbilityChains`.
//!
//! Each list is flat: a screen starts a new chain whenever an ability's prerequisite is not
//! the ability before it, so each chain's abilities follow its root. An entry's token
//! (`CharGenCreature.Stats.SkillChains.N`) carries the ability's `AbilityTemplate` fields and
//! `DisplayState`, and the attributes `SharedLibrary.Ability.LoadFromToken` asks for with
//! `GetAbilityAttributes` (`ID`, `UseType`, `Icon`, `GUIType`, `PrereqAbilityID`, see
//! [`ability_attributes`]).
//!
//! Provisional, read from ABI_base, guitypes and cla_data (the engine's own rule is not
//! recovered):
//! - a class or specialization tree (a guitypes row with an `Ability`) is listed when the
//!   creature has that ability (4022 Warrior, 4034 Dog, ...);
//! - a chain whose root needs an ability (Weapon and Shield needs 4022, skills need 4002
//!   TRAIT_HUMANOID) is listed when the creature has it;
//! - a chain whose root needs nothing (Archery, Dual Weapon, the spell schools) is listed for
//!   player classes (cla_data `PlayerClass`) only, which keeps them from the dog and Shale;
//! - ABI_base column 43 is taken to be the screens an ability shows on (a mask of the
//!   screens' display modes: 1 character generation, 2 level-up, 4 spellbook);
//! - rows labelled `INVALID_*` (the tables' placeholders) are left out;
//! - display state: 1 the creature has it, 2 its prerequisite ability is had (the screen
//!   checks level and attributes itself), 3 otherwise.

use std::collections::BTreeMap;

use thedas_formats::gda::Cell;
use thedas_sim::Sim;
use thedas_sim::m2da::Table;

use crate::UiValue;

/// ABI_base (M2DA 1), guitypes (111) and cla_data (3), as the movies' `_2DA` names them.
const ABILITIES: i32 = 1;
const GUI_TYPES: i32 = 111;
const CLASSES: i32 = 3;

/// The level-up screen's display mode (`SharedLibrary.Ability.DISPLAYMODE_LEVELUP`).
pub const DISPLAYMODE_LEVELUP: i64 = 2;

pub const DISPLAYSTATE_HAVE: i64 = 1;
pub const DISPLAYSTATE_AVAILABLE: i64 = 2;
pub const DISPLAYSTATE_UNLOCKABLE: i64 = 3;

/// The lists and the ability type each holds (ABI_base `AbilityType`).
pub const LISTS: [(&str, i64); 3] = [("TalentChains", 1), ("SpellChains", 2), ("SkillChains", 3)];

fn int(t: &Table, column: &str, row: i64) -> i64 {
    match t.cell(column, row) {
        Some(Cell::Int(v)) => *v,
        Some(Cell::Float(v)) => *v as i64,
        _ => 0,
    }
}

/// ABI_base columns whose names are not known, by hash: the screens an ability shows on
/// (column 43) and the attribute value it needs (column 17, beside `prereqattribute`).
const SCREENS: u32 = 0xece2_2f72;
const PREREQ_VALUE: u32 = 0x7cfe_85d1;

fn int_hash(t: &Table, h: u32, row: i64) -> i64 {
    match t.cell_by_hash(h, row) {
        Some(Cell::Int(v)) => *v,
        Some(Cell::Float(v)) => *v as i64,
        _ => 0,
    }
}

fn text(t: &Table, column: &str, row: i64) -> String {
    match t.cell(column, row) {
        Some(Cell::String(s)) => s.trim_end_matches('\0').trim().to_string(),
        _ => String::new(),
    }
}

/// One listed ability.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: i64,
    pub state: i64,
}

/// The abilities of `ability_type` the screen lists for `who`, chains in order.
pub fn chains(sim: &Sim, who: u32, ability_type: i64, mode: i64) -> Vec<Entry> {
    let tables = sim.world.tables();
    let (Some(abi), Some(gui)) = (tables.get(ABILITIES, ""), tables.get(GUI_TYPES, "")) else {
        return Vec::new();
    };
    let Some(o) = sim.world.get(who) else {
        return Vec::new();
    };
    let has = |id: i64| i32::try_from(id).is_ok_and(|id| o.abilities.contains(&id));
    let class = o.properties.get(&27).map_or(0, |v| v.0.max(v.1) as i64);
    let player_class = tables
        .get(CLASSES, "")
        .is_some_and(|t| int(&t, "PlayerClass", class) != 0);
    // The abilities of this type shown on this screen, in table order.
    let shown: Vec<i64> = abi
        .order
        .iter()
        .copied()
        .filter(|&id| {
            id > 0
                && !text(&abi, "Label", id).starts_with("INVALID")
                && int(&abi, "AbilityType", id) == ability_type
                && int_hash(&abi, SCREENS, id) & mode != 0
        })
        .collect();
    let gui_of = |id: i64| int(&abi, "GUIType", id);
    let prereq = |id: i64| int(&abi, "prereqability", id);
    // The tree a GUI type belongs to: listed, and how.
    let tree_listed = |g: i64| -> Option<bool> {
        if g <= 0 || !gui.order.contains(&g) {
            return None;
        }
        let label = text(&gui, "Label", g).to_ascii_lowercase();
        if int(&gui, "Orphanage", g) != 0 || label.contains("hidden") {
            return None;
        }
        let anchor = int(&gui, "Ability", g);
        Some(anchor == 0 || has(anchor))
    };
    let mut out = Vec::new();
    let mut listed = std::collections::BTreeSet::new();
    for &root in &shown {
        let g = gui_of(root);
        // A root: its prerequisite is not in the same tree.
        let p = prereq(root);
        if p > 0 && shown.contains(&p) && gui_of(p) == g {
            continue;
        }
        let Some(class_ok) = tree_listed(g) else {
            continue;
        };
        if !class_ok {
            continue;
        }
        let anchored = int(&gui, "Ability", g) != 0;
        let root_ok = if p > 0 {
            has(p)
        } else {
            anchored || player_class
        };
        if !root_ok {
            continue;
        }
        // The chain: the root, then each ability whose prerequisite is the last one.
        let mut chain = vec![root];
        let mut last = root;
        while let Some(&next) = shown
            .iter()
            .find(|&&a| prereq(a) == last && gui_of(a) == g && !chain.contains(&a))
        {
            chain.push(next);
            last = next;
        }
        for id in chain {
            if !listed.insert(id) {
                continue;
            }
            let p = prereq(id);
            let state = if has(id) {
                DISPLAYSTATE_HAVE
            } else if p <= 0 || has(p) {
                DISPLAYSTATE_AVAILABLE
            } else {
                DISPLAYSTATE_UNLOCKABLE
            };
            out.push(Entry { id, state });
        }
    }
    out
}

/// The values the screens read for each listed ability, under `prefix`
/// (`CharGenCreature.Stats`).
pub fn bind(sim: &Sim, who: u32, prefix: &str, v: &mut BTreeMap<String, UiValue>) {
    let tables = sim.world.tables();
    let Some(abi) = tables.get(ABILITIES, "") else {
        return;
    };
    for (list, ty) in LISTS {
        for (i, e) in chains(sim, who, ty, DISPLAYMODE_LEVELUP).iter().enumerate() {
            ability(
                sim,
                &abi,
                &format!("{prefix}.{list}.{i}"),
                e.id,
                ty,
                e.state,
                v,
            );
        }
    }
}

/// An ability's values under token `k` (`k` itself is the id; `.ID`, `.UseType`, `.Icon`,
/// ..., `.AbilityTemplate.*`), as the screens read an ability icon. `ty` is its ability type
/// and `state` its display state (`DISPLAYSTATE_*`).
pub fn ability(
    sim: &Sim,
    abi: &thedas_sim::m2da::Table,
    k: &str,
    id: i64,
    ty: i64,
    state: i64,
    v: &mut BTreeMap<String, UiValue>,
) {
    let num = |x: i64| UiValue::Number(x as f64);
    let name_strref = int(abi, "NameStrRef", id);
    let name = u32::try_from(name_strref)
        .ok()
        .and_then(|s| sim.string(s))
        .unwrap_or_default();
    v.insert(k.to_string(), num(id));
    for (key, value) in [
        ("ID", num(id)),
        ("UseType", num(int(abi, "UseType", id))),
        ("Icon", UiValue::String(text(abi, "Icon", id))),
        ("GUIType", num(int(abi, "GUIType", id))),
        ("PrereqAbilityID", num(int(abi, "prereqability", id))),
        ("DisplayState", num(state)),
        ("AbilityInstance", num(0)),
        ("IsError", num(0)),
        ("CooldownPercentage", num(0)),
    ] {
        v.insert(format!("{k}.{key}"), value);
    }
    let t = format!("{k}.AbilityTemplate");
    for (key, value) in [
        ("ID", num(id)),
        ("AbilityType", num(ty)),
        // The quickbar's tooltip names the use type (passive, activated, sustained) from
        // here.
        ("UseType", num(int(abi, "UseType", id))),
        ("Name", UiValue::String(name)),
        ("NameStrRef", num(name_strref)),
        ("DescStrRef", num(int(abi, "DescStrRef", id))),
        ("TooltipStrRef", num(int(abi, "TooltipStrRef", id))),
        ("Conditions", num(int(abi, "conditions", id))),
        ("ConditionMode", num(int(abi, "condition_mode", id))),
        ("Cooldown", num(int(abi, "cooldown", id))),
        ("Cost", num(int(abi, "cost", id))),
        ("CostUpkeep", num(int(abi, "costupkeep", id))),
        ("PrereqLevel", num(int(abi, "prereqlevel", id))),
        ("PrereqAttributeID", num(int(abi, "prereqattribute", id))),
        ("PrereqAttributeValue", num(int_hash(abi, PREREQ_VALUE, id))),
        ("Speed", num(int(abi, "Speed", id))),
        ("RequiresCombatTraining", num(0)),
        ("Rank", num(0)),
    ] {
        v.insert(format!("{t}.{key}"), value);
    }
}

/// `GetAbilityAttributes(token)`: what `SharedLibrary.Ability.LoadFromToken` reads as
/// `ReturnedAbilityAttributes` (id, use type, icon, GUI type, prerequisite, display state),
/// from the token's values.
pub fn ability_attributes(value: impl Fn(&str) -> Option<UiValue>, token: &str) -> UiValue {
    UiValue::List(
        [
            "ID",
            "UseType",
            "Icon",
            "GUIType",
            "PrereqAbilityID",
            "DisplayState",
        ]
        .iter()
        .map(|k| value(&format!("{token}.{k}")).unwrap_or(UiValue::Undefined))
        .collect(),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_sim::world::ObjectKind;
    use thedas_vfs::Vfs;

    use super::*;

    /// A sim with a tiny ABI_base (two warrior talents in a chain, a dog talent, an archery
    /// root needing nothing, a skill chain rooted on a trait, a placeholder and a
    /// monster-only ability), guitypes and cla_data.
    fn sim(class: f32, abilities: &[i32]) -> (Sim, u32) {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let t = s.world.tables();
        let i = |v: i64| Cell::Int(v);
        let abi = |id, label: &str, ty, prereq, gui, screens| {
            (
                id,
                vec![
                    Cell::String(label.into()),
                    i(ty),
                    i(prereq),
                    i(gui),
                    Cell::String(format!("ico_{id}")),
                    i(screens),
                    i(4),
                ],
            )
        };
        t.insert(
            "#1",
            &[
                "Label",
                "AbilityType",
                "prereqability",
                "GUIType",
                "Icon",
                "#ece22f72",
                "UseType",
            ],
            vec![
                abi(14, "POWERFUL", 1, 4022, 101, 7),
                abi(15, "THREATEN", 1, 14, 101, 7),
                abi(90017, "DOG_GROWL", 1, 4034, 117, 7),
                abi(34, "MELEE_ARCHER", 1, 0, 8, 7),
                abi(90024, "MONSTER_CHARGE", 1, 0, 5, 0),
                abi(100000, "INVALID_SKILL", 3, 0, 13, 7),
                abi(100061, "HERBALISM", 3, 4002, 13, 7),
                abi(100062, "IMPROVED_HERBALISM", 3, 100061, 13, 7),
            ],
        );
        let gui = |id, label: &str, ability, orphan| {
            (id, vec![Cell::String(label.into()), i(ability), i(orphan)])
        };
        t.insert(
            "#111",
            &["Label", "Ability", "Orphanage"],
            vec![
                gui(5, "Monster", 0, 1),
                gui(8, "Archery", 0, 0),
                gui(13, "Skill", 0, 0),
                gui(101, "Class: Warrior", 4022, 0),
                gui(117, "Class: Dog", 4034, 0),
            ],
        );
        t.insert(
            "#3",
            &["PlayerClass"],
            vec![(1, vec![i(1)]), (17, vec![i(0)])],
        );
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let o = s.world.get_mut(c).unwrap();
        o.properties.insert(27, (class, 0.0));
        o.abilities = abilities.iter().copied().collect();
        (s, c)
    }

    fn ids(e: &[Entry]) -> Vec<(i64, i64)> {
        e.iter().map(|e| (e.id, e.state)).collect()
    }

    #[test]
    fn a_warrior_sees_its_class_tree_and_open_trees_in_chain_order() {
        let (s, c) = sim(1.0, &[4022, 4002, 14]);
        assert_eq!(
            ids(&chains(&s, c, 1, DISPLAYMODE_LEVELUP)),
            vec![
                (14, DISPLAYSTATE_HAVE),
                (15, DISPLAYSTATE_AVAILABLE),
                (34, DISPLAYSTATE_AVAILABLE)
            ],
            "no dog tree, no monster ability"
        );
        assert_eq!(
            ids(&chains(&s, c, 3, DISPLAYMODE_LEVELUP)),
            vec![
                (100061, DISPLAYSTATE_AVAILABLE),
                (100062, DISPLAYSTATE_UNLOCKABLE)
            ],
            "the placeholder is left out"
        );
    }

    #[test]
    fn the_dog_sees_only_its_own_tree() {
        let (s, c) = sim(17.0, &[4034]);
        assert_eq!(
            ids(&chains(&s, c, 1, DISPLAYMODE_LEVELUP)),
            vec![(90017, DISPLAYSTATE_AVAILABLE)]
        );
        assert!(chains(&s, c, 3, DISPLAYMODE_LEVELUP).is_empty());
    }

    #[test]
    fn bound_tokens_answer_get_ability_attributes() {
        let (s, c) = sim(1.0, &[4022]);
        let mut v = BTreeMap::new();
        bind(&s, c, "CharGenCreature.Stats", &mut v);
        let k = "CharGenCreature.Stats.TalentChains.1";
        assert_eq!(
            v[&format!("{k}.AbilityTemplate.AbilityType")],
            UiValue::Number(1.0)
        );
        assert_eq!(
            ability_attributes(|n| v.get(n).cloned(), k),
            UiValue::List(vec![
                UiValue::Number(15.0),
                UiValue::Number(4.0),
                UiValue::String("ico_15".into()),
                UiValue::Number(101.0),
                UiValue::Number(14.0),
                UiValue::Number(DISPLAYSTATE_UNLOCKABLE as f64),
            ])
        );
    }
}
