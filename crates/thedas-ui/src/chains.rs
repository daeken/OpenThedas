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
//! The lists are built as DAOrigins.exe builds them (0x62e9c0, for the message's handlers
//! 0x718090 and 0x719450):
//!
//! - The abilities of a type (0x62e050): the rows of ABI_base (M2DA 1) by row position, those
//!   with an id above 0 and a use type, of that `AbilityType` (1 talents, 2 spells, 3 skills).
//! - Their prerequisite tree (0x62db40): an ability's children are the abilities whose
//!   `prereqability` it is, in row order; the tree's top holds those that need none. Each
//!   ability's rank is its depth in the tree, from 1, where a hidden ability (GUI type 12:
//!   `HIDDEN_WARRIOR`, `TRAIT_HUMANOID`, ...) adds no level.
//! - The creature's list (0x62dcd0), depth first, children in order. A child and everything
//!   under it is left out unless its screens mask (ABI_base's column hashed 0xece22f72) has
//!   the screen's display mode; a hidden ability is one the creature has; and its GUI type's
//!   row of guitypes (M2DA 111) lets it through: `Flags` bit 0 asks for a humanoid (the
//!   creature's appearance row names a `combat_humanoid` blend tree, 0x6a25a0), `Ability`
//!   names an ability the creature must have (a class's or specialization's), `Orphanage`
//!   lists the ability only when the creature has it. Hidden abilities are walked but not
//!   listed.
//! - An entry's display state: 1 the creature has it, else 2 when it could take it
//!   (0x71bef0: its level reaches `prereqlevel`, it has the prerequisite ability, its
//!   attribute reaches the one asked, and for a talent whose GUI type asks for combat
//!   training (guitypes' column hashed 0x9c01305e) its Combat Training rank reaches the
//!   talent's rank), else 3.
//!
//! Provisional: the creature's level is its level property and an attribute its total;
//! the exe's own readers (0x630460, 0x71a230) are not traced further.

use std::collections::BTreeMap;

use thedas_formats::gda::Cell;
use thedas_sim::Sim;
use thedas_sim::m2da::Table;

use crate::UiValue;

/// ABI_base (M2DA 1) and guitypes (111), as the movies' `_2DA` names them.
const ABILITIES: i32 = 1;
const GUI_TYPES: i32 = 111;

/// The level-up screen's display mode (`SharedLibrary.Ability.DISPLAYMODE_LEVELUP`).
pub const DISPLAYMODE_LEVELUP: i64 = 2;
/// The skills and spells/talents screen's (abilities.gfx, `DISPLAYMODE_SPELLBOOK`).
pub const DISPLAYMODE_SPELLBOOK: i64 = 4;

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
    /// The chain it is listed in: a parent's first listed child continues its chain, each
    /// later one starts the next.
    pub chain: i64,
    /// How many listed abilities stand above it in its tree.
    pub depth: i64,
}

/// The GUI type of abilities that hold a tree together but are never listed.
const GUI_HIDDEN: i64 = 12;
/// guitypes' column that says its talents need Combat Training (name not known).
const GUI_COMBAT_TRAINING: u32 = 0x9c01_305e;
/// The appearances table (M2DA 2) and its column naming the creature's combat blend tree.
const APPEARANCES: i32 = 2;
const APPEARANCE_COMBAT: u32 = 0xde80_b418;
/// Combat Training's four ranks (skills 100100 to 100103).
const COMBAT_TRAINING: i64 = 100_100;
/// The creature's level (`properties.gda`).
const PROPERTY_LEVEL: i32 = 15;
/// How deep a tree is followed: far past any real one, against tables that never end.
const MAX_DEPTH: i64 = 64;

/// The prerequisite tree of one ability type: each ability's children (0 holds the top's) in
/// row order, and each ability's rank.
#[derive(Default)]
pub struct Tree {
    children: BTreeMap<i64, Vec<i64>>,
    rank: BTreeMap<i64, i64>,
}

impl Tree {
    pub fn new(abi: &Table, ability_type: i64) -> Tree {
        let mut children: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
        for &id in &abi.order {
            if id > 0 && int(abi, "UseType", id) != 0 && int(abi, "AbilityType", id) == ability_type
            {
                children
                    .entry(int(abi, "prereqability", id))
                    .or_default()
                    .push(id);
            }
        }
        let mut rank = BTreeMap::new();
        let mut todo = vec![(0, 1)];
        while let Some((parent, depth)) = todo.pop() {
            for &c in children.get(&parent).into_iter().flatten() {
                if depth > MAX_DEPTH || rank.insert(c, depth).is_some() {
                    continue;
                }
                let hidden = int(abi, "GUIType", c) == GUI_HIDDEN;
                todo.push((c, if hidden { depth } else { depth + 1 }));
            }
        }
        Tree { children, rank }
    }

    /// An ability's rank: its depth in the tree from 1 (0 for one not in the tree).
    pub fn rank(&self, id: i64) -> i64 {
        self.rank.get(&id).copied().unwrap_or(0)
    }
}

/// Whether a talent of this GUI type needs Combat Training.
fn needs_combat_training(gui: &Table, ability_type: i64, gui_type: i64) -> bool {
    ability_type == 1 && int_hash(gui, GUI_COMBAT_TRAINING, gui_type) != 0
}

/// What listing for one creature reads.
struct Lister<'a> {
    sim: &'a Sim,
    who: u32,
    abi: &'a Table,
    gui: &'a Table,
    tree: &'a Tree,
    ability_type: i64,
    mode: i64,
    humanoid: bool,
    combat_training: i64,
}

impl Lister<'_> {
    fn has(&self, id: i64) -> bool {
        i32::try_from(id).is_ok_and(|id| {
            self.sim
                .world
                .get(self.who)
                .is_some_and(|o| o.abilities.contains(&id))
        })
    }

    /// Whether the creature could take the ability now (0x71bef0).
    fn can_take(&self, id: i64) -> bool {
        let level = self.sim.property_total(self.who, PROPERTY_LEVEL) as i64;
        let prereq = int(self.abi, "prereqability", id);
        let attribute = int(self.abi, "prereqattribute", id);
        let trained =
            !needs_combat_training(self.gui, self.ability_type, int(self.abi, "GUIType", id))
                || self.tree.rank(id) <= self.combat_training;
        level >= int(self.abi, "prereqlevel", id)
            && (prereq == 0 || self.has(prereq))
            && (attribute == 0
                || i32::try_from(attribute).is_ok_and(|a| {
                    f64::from(self.sim.property_total(self.who, a))
                        >= int_hash(self.abi, PREREQ_VALUE, id) as f64
                }))
            && trained
    }

    /// Whether a child of the tree, and what is under it, is listed for the creature.
    fn passes(&self, id: i64) -> bool {
        if int_hash(self.abi, SCREENS, id) & self.mode == 0 {
            return false;
        }
        let g = int(self.abi, "GUIType", id);
        if g == GUI_HIDDEN && !self.has(id) {
            return false;
        }
        if int(self.gui, "Flags", g) & 1 != 0 && !self.humanoid {
            return false;
        }
        let anchor = int(self.gui, "Ability", g);
        if anchor != 0 && !self.has(anchor) {
            return false;
        }
        int(self.gui, "Orphanage", g) == 0 || self.has(id)
    }

    /// Lists the children of `parent` (0 for the tree's top) at `depth`, each with what is
    /// under it.
    fn list(&self, parent: i64, depth: i64, chain: &mut i64, out: &mut Vec<Entry>) {
        if depth > MAX_DEPTH {
            return;
        }
        let mut first = true;
        for &c in self.tree.children.get(&parent).into_iter().flatten() {
            // A self-referring row is its own child: it is not walked again.
            if c == parent || !self.passes(c) {
                continue;
            }
            if !std::mem::take(&mut first) {
                *chain += 1;
            }
            let hidden = int(self.abi, "GUIType", c) == GUI_HIDDEN;
            if !hidden {
                let state = if self.has(c) {
                    DISPLAYSTATE_HAVE
                } else if self.can_take(c) {
                    DISPLAYSTATE_AVAILABLE
                } else {
                    DISPLAYSTATE_UNLOCKABLE
                };
                out.push(Entry {
                    id: c,
                    state,
                    chain: *chain,
                    depth,
                });
            }
            self.list(c, if hidden { depth } else { depth + 1 }, chain, out);
        }
    }
}

/// The abilities of `ability_type` the screen lists for `who`, chains in order.
pub fn chains(sim: &Sim, who: u32, ability_type: i64, mode: i64) -> Vec<Entry> {
    let tables = sim.world.tables();
    let Some(abi) = tables.get(ABILITIES, "") else {
        return Vec::new();
    };
    listed(
        sim,
        who,
        &abi,
        &Tree::new(&abi, ability_type),
        ability_type,
        mode,
    )
}

/// [`chains`] with the table and the type's tree already at hand.
fn listed(
    sim: &Sim,
    who: u32,
    abi: &Table,
    tree: &Tree,
    ability_type: i64,
    mode: i64,
) -> Vec<Entry> {
    let tables = sim.world.tables();
    let (Some(gui), Some(o)) = (tables.get(GUI_TYPES, ""), sim.world.get(who)) else {
        return Vec::new();
    };
    // A humanoid: the appearance's combat blend tree is the humanoids' (0x69f030).
    let appearance = o
        .fields
        .get("Appearance_Type")
        .or_else(|| o.fields.get("Appearance"))
        .copied()
        .unwrap_or(0) as i64;
    let humanoid = tables.get(APPEARANCES, "").is_some_and(|t| {
        matches!(t.cell_by_hash(APPEARANCE_COMBAT, appearance),
            Some(Cell::String(s)) if s.to_ascii_lowercase().contains("combat_humanoid"))
    });
    // The highest Combat Training rank had (0x71c024).
    let combat_training = (1..=4)
        .rev()
        .find(|r| i32::try_from(COMBAT_TRAINING + r - 1).is_ok_and(|id| o.abilities.contains(&id)))
        .unwrap_or(0);
    let lister = Lister {
        sim,
        who,
        abi,
        gui: &gui,
        tree,
        ability_type,
        mode,
        humanoid,
        combat_training,
    };
    let mut out = Vec::new();
    lister.list(0, 0, &mut 0, &mut out);
    out
}

/// The values the screens read for each listed ability, under `prefix`
/// (`CharGenCreature.Stats`, or `GUI.SelectedChar.Stats` for the skills and spells/talents
/// screen), listing the abilities shown in display `mode` (`DISPLAYMODE_*`).
pub fn bind(sim: &Sim, who: u32, prefix: &str, mode: i64, v: &mut BTreeMap<String, UiValue>) {
    let tables = sim.world.tables();
    let Some(abi) = tables.get(ABILITIES, "") else {
        return;
    };
    let gui = tables.get(GUI_TYPES, "");
    for (list, ty) in LISTS {
        let tree = Tree::new(&abi, ty);
        for (i, e) in listed(sim, who, &abi, &tree, ty, mode).iter().enumerate() {
            let k = format!("{prefix}.{list}.{i}");
            ability(sim, &abi, &k, e.id, ty, e.state, v);
            // The tooltip's Combat Training line: the talent's rank against the creature's.
            let training = gui
                .as_ref()
                .is_some_and(|g| needs_combat_training(g, ty, int(&abi, "GUIType", e.id)));
            let t = format!("{k}.AbilityTemplate");
            v.insert(
                format!("{t}.RequiresCombatTraining"),
                UiValue::Number(f64::from(u8::from(training))),
            );
            v.insert(format!("{t}.Rank"), UiValue::Number(tree.rank(e.id) as f64));
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

    /// A sim with a tiny ABI_base (the warrior's hidden class ability over a two-talent
    /// chain with a branch, the dog's, a dual-weapon tree needing nothing, a skill chain
    /// under the humanoid trait, a monster ability, a placeholder without a use type and a
    /// talent of a screen not shown), guitypes and two appearances. The creature is level 1.
    fn sim(humanoid: bool, abilities: &[i32]) -> (Sim, u32) {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let t = s.world.tables();
        let i = |v: i64| Cell::Int(v);
        let abi = |id, label: &str, ty, prereq, gui, screens, use_type, level| {
            (
                id,
                vec![
                    Cell::String(label.into()),
                    i(ty),
                    i(prereq),
                    i(gui),
                    Cell::String(format!("ico_{id}")),
                    i(screens),
                    i(use_type),
                    i(level),
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
                "prereqlevel",
            ],
            vec![
                abi(6, "DUAL_TRAINING", 1, 0, 9, 7, 3, 0),
                abi(7, "DUAL_FINESSE", 1, 6, 9, 7, 3, 0),
                abi(8, "DUAL_EXPERT", 1, 7, 9, 7, 3, 0),
                abi(14, "POWERFUL", 1, 4022, 101, 7, 4, 0),
                abi(15, "THREATEN", 1, 14, 101, 7, 4, 0),
                abi(16, "BRAVERY", 1, 15, 101, 7, 4, 12),
                abi(17, "PRECISE", 1, 14, 101, 7, 4, 0),
                abi(18, "SPELLBOOK_ONLY", 1, 4022, 101, 4, 4, 0),
                abi(4022, "HIDDEN_WARRIOR", 1, 0, 12, 7, 3, 0),
                abi(4034, "HIDDEN_DOG", 1, 0, 12, 7, 3, 0),
                abi(90017, "DOG_GROWL", 1, 4034, 117, 7, 4, 0),
                abi(90024, "MONSTER_CHARGE", 1, 0, 5, 7, 4, 0),
                abi(100000, "INVALID_SKILL", 3, 0, 13, 7, 0, 0),
                abi(4002, "TRAIT_HUMANOID", 3, 0, 12, 7, 3, 0),
                abi(100061, "HERBALISM", 3, 4002, 13, 7, 3, 0),
                abi(100062, "IMPROVED_HERBALISM", 3, 100061, 13, 7, 3, 0),
            ],
        );
        let gui = |id, label: &str, ability, orphan, flags, training| {
            (
                id,
                vec![
                    Cell::String(label.into()),
                    i(ability),
                    i(orphan),
                    i(flags),
                    i(training),
                ],
            )
        };
        t.insert(
            "#111",
            &["Label", "Ability", "Orphanage", "Flags", "#9c01305e"],
            vec![
                gui(5, "Monster", 0, 1, 0, 0),
                gui(9, "Dual Weapon", 0, 0, 1, 1),
                gui(12, "Hidden", 0, 0, 0, 0),
                gui(13, "Skill", 0, 0, 0, 0),
                gui(101, "Class: Warrior", 4022, 0, 0, 0),
                gui(117, "Class: Dog", 4034, 0, 0, 0),
            ],
        );
        t.insert(
            "#2",
            &["#de80b418"],
            vec![
                (1, vec![Cell::String("combat_humanoid".into())]),
                (2, vec![Cell::String("combat_dog".into())]),
            ],
        );
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let o = s.world.get_mut(c).unwrap();
        o.properties.insert(PROPERTY_LEVEL, (1.0, 0.0));
        o.fields
            .insert("Appearance_Type".into(), if humanoid { 1 } else { 2 });
        o.abilities = abilities.iter().copied().collect();
        (s, c)
    }

    fn ids(e: &[Entry]) -> Vec<(i64, i64)> {
        e.iter().map(|e| (e.id, e.state)).collect()
    }

    #[test]
    fn a_warrior_sees_the_trees_depth_first_in_row_order() {
        let (s, c) = sim(true, &[4022, 4002, 14, 100100]);
        let talents = chains(&s, c, 1, DISPLAYMODE_LEVELUP);
        assert_eq!(
            ids(&talents),
            vec![
                // The dual-weapon tree: one Combat Training rank reaches its first talent.
                (6, DISPLAYSTATE_AVAILABLE),
                (7, DISPLAYSTATE_UNLOCKABLE),
                (8, DISPLAYSTATE_UNLOCKABLE),
                // The warrior's, under its hidden class ability: the chain, then the branch.
                (14, DISPLAYSTATE_HAVE),
                (15, DISPLAYSTATE_AVAILABLE),
                (16, DISPLAYSTATE_UNLOCKABLE),
                (17, DISPLAYSTATE_AVAILABLE),
            ],
            "no hidden ability, no dog tree, no monster ability, no other screen's talent"
        );
        // A first child continues its parent's chain; each later one starts the next.
        assert_eq!(
            talents
                .iter()
                .map(|e| (e.chain, e.depth))
                .collect::<Vec<_>>(),
            vec![(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (2, 1)]
        );
        assert_eq!(
            ids(&chains(&s, c, 3, DISPLAYMODE_LEVELUP)),
            vec![
                (100061, DISPLAYSTATE_AVAILABLE),
                (100062, DISPLAYSTATE_UNLOCKABLE)
            ],
            "the row without a use type is left out"
        );
        // The spellbook's own talent shows there.
        assert!(
            chains(&s, c, 1, DISPLAYMODE_SPELLBOOK)
                .iter()
                .any(|e| e.id == 18)
        );
    }

    #[test]
    fn a_talents_rank_is_its_depth_and_combat_training_gates_it() {
        let (s, c) = sim(true, &[4022, 6]);
        let tree = Tree::new(&s.world.tables().get(ABILITIES, "").unwrap(), 1);
        // A hidden ability adds no level: the warrior's first talent is rank 1 too.
        assert_eq!(
            [6, 7, 8, 4022, 14, 15, 17, 99].map(|id| tree.rank(id)),
            [1, 2, 3, 1, 1, 2, 2, 0]
        );
        // No Combat Training: the next dual-weapon talent cannot be taken though its
        // prerequisite is had; the class talents do not ask for training.
        let state = |s: &Sim, id| {
            chains(s, c, 1, DISPLAYMODE_LEVELUP)
                .iter()
                .find(|e| e.id == id)
                .map(|e| e.state)
        };
        assert_eq!(state(&s, 7), Some(DISPLAYSTATE_UNLOCKABLE));
        assert_eq!(state(&s, 14), Some(DISPLAYSTATE_AVAILABLE));
        let (s, _) = sim(true, &[4022, 6, 100100, 100101]);
        assert_eq!(state(&s, 7), Some(DISPLAYSTATE_AVAILABLE));
        // A level asked: BRAVERY needs level 12.
        let (mut s, c) = sim(true, &[4022, 14, 15]);
        assert_eq!(state(&s, 16), Some(DISPLAYSTATE_UNLOCKABLE));
        s.world
            .get_mut(c)
            .unwrap()
            .properties
            .insert(PROPERTY_LEVEL, (12.0, 0.0));
        assert_eq!(state(&s, 16), Some(DISPLAYSTATE_AVAILABLE));
    }

    #[test]
    fn the_dog_sees_only_its_own_tree() {
        let (s, c) = sim(false, &[4034]);
        assert_eq!(
            ids(&chains(&s, c, 1, DISPLAYMODE_LEVELUP)),
            vec![(90017, DISPLAYSTATE_AVAILABLE)],
            "the weapon trees are for humanoids"
        );
        assert!(chains(&s, c, 3, DISPLAYMODE_LEVELUP).is_empty());
        // A monster ability is listed for the creature that has it.
        let (s, c) = sim(false, &[4034, 90024]);
        assert_eq!(
            ids(&chains(&s, c, 1, DISPLAYMODE_LEVELUP)),
            vec![(90017, DISPLAYSTATE_AVAILABLE), (90024, DISPLAYSTATE_HAVE)]
        );
    }

    #[test]
    fn bound_tokens_answer_get_ability_attributes() {
        let (s, c) = sim(true, &[4022]);
        let mut v = BTreeMap::new();
        bind(&s, c, "CharGenCreature.Stats", DISPLAYMODE_LEVELUP, &mut v);
        // The tooltip's Combat Training line reads the template's rank.
        let dual = "CharGenCreature.Stats.TalentChains.1.AbilityTemplate";
        assert_eq!(v[&format!("{dual}.Rank")], UiValue::Number(2.0));
        assert_eq!(
            v[&format!("{dual}.RequiresCombatTraining")],
            UiValue::Number(1.0)
        );
        let k = "CharGenCreature.Stats.TalentChains.4";
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
