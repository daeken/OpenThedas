//! Engine values the movies read, computed from the simulation. The interface only reads the
//! simulation; nothing here writes it.
//!
//! Provisional: key names come from the movies' bytecode, and their meanings are inferred from
//! how the movies use them (`Party.Members.N.ID` is a creature id, `Attributes.P.Current` and
//! `.Total` are a property's current and base (maximum) values, as in `properties.gda`).

use std::collections::BTreeMap;

use thedas_sim::Sim;
use thedas_sim::world::ObjectKind;

use crate::UiValue;

fn num(v: impl Into<f64>) -> UiValue {
    UiValue::Number(v.into())
}

/// The party, hero first.
pub fn party(sim: &Sim) -> Vec<u32> {
    let hero = sim.world.hero;
    let mut v: Vec<u32> = hero.into_iter().collect();
    if let Some(h) = hero.and_then(|h| sim.world.get(h)) {
        v.extend(h.followers.iter().copied());
    }
    v
}

fn name(sim: &Sim, id: u32) -> String {
    sim.world
        .get(id)
        .and_then(|o| {
            o.name_strref
                .and_then(|s| sim.string(s))
                .or(Some(o.tag.clone()))
        })
        .unwrap_or_default()
}

/// The conversation on screen: one in progress, or one that ended on a line the player has
/// not yet continued past (conversation.gfx shows it and sends `Conversation.SelectResponse
/// 0` from its continue action).
pub fn conversation(sim: &Sim) -> Option<thedas_sim::dialog::DialogState> {
    sim.dialog_state()
        .filter(|d| !d.ended || !d.line.is_empty())
}

/// The text the simulation gives a reply with no text of its own (a plain "continue").
const CONTINUE_REPLY: &str = "[continue]";

/// Whether the conversation on screen waits for the player to continue rather than pick a
/// reply: no replies, or one with no text. conversation.gfx shows no reply buttons then and
/// its continue action sends `Conversation.SelectResponse 0` (`HasResponses`).
pub fn awaits_continue(sim: &Sim) -> bool {
    conversation(sim).is_some_and(|d| match d.replies.as_slice() {
        [] => true,
        [(_, t)] => t == CONTINUE_REPLY || t.is_empty(),
        _ => false,
    })
}

/// `GM_DIALOG` (4) while a conversation is on screen, the world's game mode otherwise.
/// Provisional: the simulation does not switch game modes for conversations itself.
pub fn game_mode(sim: &Sim) -> i32 {
    if conversation(sim).is_some() {
        4
    } else {
        sim.world.game_mode
    }
}

/// Every value this module can answer for the current simulation state.
pub fn sim_values(sim: &Sim) -> BTreeMap<String, UiValue> {
    sim_values_for(sim, &Presentation::default())
}

/// What the interface shows that is not simulation state: a screen's own choices the engine
/// keeps for it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Presentation {
    /// The inventory screen's item category tab (`GUIInventory.SetItemListFilter`, see
    /// [`ItemCategory`]); 0 shows every item.
    pub item_filter: i32,
}

/// The inventory screen's item category tabs (inventory.gfx's `CATEGORY_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemCategory {
    Weapons = 1,
    Armor = 2,
    Accessories = 3,
    Other = 4,
    Crafting = 5,
    Plot = 6,
}

/// The inventory tab an item falls under, from its base type's `Type` in `bitm_base` (2
/// melee, 5 staff, 6 ranged, 7 ammunition: weapons; 3 shield, 4 armour: armour; 1 other: an
/// accessory when it can be worn) and, for crafting materials and recipes, the base type's
/// label. Provisional: the game's own grouping is not traced.
pub fn item_category(sim: &Sim, item: u32) -> ItemCategory {
    let Some(o) = sim.world.get(item) else {
        return ItemCategory::Other;
    };
    if o.plot {
        return ItemCategory::Plot;
    }
    let base = o.fields.get("BaseItem").copied().unwrap_or(0) as i64;
    let table = sim.world.tables().get(0, "bitm_base");
    let int = |c: &str| match table.as_ref().and_then(|t| t.cell(c, base).cloned()) {
        Some(thedas_formats::gda::Cell::Int(v)) => v,
        _ => 0,
    };
    let label = match table.as_ref().and_then(|t| t.cell("Label", base).cloned()) {
        Some(thedas_formats::gda::Cell::String(s)) => s.to_ascii_lowercase(),
        _ => String::new(),
    };
    match int("Type") {
        2 | 5 | 6 | 7 => ItemCategory::Weapons,
        3 | 4 => ItemCategory::Armor,
        _ if !sim.item_slots(item).is_empty() => ItemCategory::Accessories,
        _ if ["herbalism", "making item", "recipe"]
            .iter()
            .any(|k| label.contains(k)) =>
        {
            ItemCategory::Crafting
        }
        _ => ItemCategory::Other,
    }
}

/// [`sim_values`] with the interface's own choices.
pub fn sim_values_for(sim: &Sim, shown: &Presentation) -> BTreeMap<String, UiValue> {
    let mut v = BTreeMap::new();
    let mut set = |k: String, x: UiValue| {
        v.insert(k, x);
    };
    let members = party(sim);
    let hero = members.first().copied();
    set("Party.Hero.ID".into(), num(hero.unwrap_or(0)));
    if let Some(h) = hero {
        set(
            crate::protocol::HERO_NAME.into(),
            UiValue::String(name(sim, h)),
        );
    }
    set(
        "Party.SelectableMemberCount".into(),
        num(members.len() as f64),
    );
    set("Party.SelectedCount".into(), num(hero.is_some() as u8));
    set("Party.QuickbarLocked".into(), num(0));
    // The member the player controls is the selected one (the hero until another is chosen).
    let controlled = sim.world.controlled();
    for i in 0..4 {
        set(
            format!("Party.MemberSelected.{i}"),
            num((controlled.is_some() && members.get(i).copied() == controlled) as u8),
        );
    }
    let mut creature = |prefix: &str, id: u32| {
        let Some(o) = sim.world.get(id) else { return };
        v.insert(prefix.to_string(), num(id));
        v.insert(format!("{prefix}.ID"), num(id));
        // No combat or status simulation yet: nothing is poisoned, paralysed or busy.
        for k in [
            "Poisoned",
            "Paralyzed",
            "AbilityUpkeepCost",
            "CurrentAction",
            "PendingAction",
        ] {
            v.insert(format!("{prefix}.{k}"), num(0));
        }
        v.insert(
            format!("{prefix}.UserSelected"),
            num((Some(id) == controlled) as u8),
        );
        let mana = o.properties.contains_key(&8);
        v.insert(format!("{prefix}.Stats.ClassHasMana"), num(mana as u8));
        v.insert(format!("{prefix}.Name"), UiValue::String(name(sim, id)));
        v.insert(format!("{prefix}.Dead"), num(o.dead as u8));
        // The simulation keeps (base, current); for depletable properties (health, mana)
        // the base is the maximum the bars show as Total.
        // Health and mana bars show their maximum with what gear and effects add.
        let depletable = [
            thedas_sim::sim::constant(&sim.table, "PROPERTY_DEPLETABLE_HEALTH"),
            thedas_sim::sim::constant(&sim.table, "PROPERTY_DEPLETABLE_MANA_STAMINA"),
        ];
        for (p, (total, cur)) in &o.properties {
            let total = if depletable.contains(p) && *p != 0 {
                sim.depletable_max(id, *p)
            } else {
                *total
            };
            v.insert(format!("{prefix}.Attributes.{p}.Current"), num(*cur));
            v.insert(format!("{prefix}.Attributes.{p}.Total"), num(total));
        }
    };
    for (i, id) in members.iter().enumerate() {
        creature(&format!("Party.Members.{i}"), *id);
    }
    if let Some(c) = controlled.or(hero) {
        creature("GUI.SelectedChar", c);
    }
    // The creature in character generation or level-up, as levelup.gfx reads it: its
    // attributes and points (34 attribute, 35 skill, 36 talent, 38 specialization), with
    // what equipment adds as each attribute's Modifier. The screen spends the points itself
    // unless the auto-level option (`Stats.AutoLevelUp`) is on; it is off. Provisional.
    if let Some(c) = sim.world.chargen {
        creature("CharGenCreature", c);
        if let Some(o) = sim.world.get(c) {
            for p in o.properties.keys() {
                v.insert(
                    format!("CharGenCreature.Attributes.{p}.Modifier"),
                    num(sim.equipment_bonus(c, *p)),
                );
            }
        }
        v.insert("CharGenCreature.Stats.AutoLevelUp".into(), num(0));
        crate::chains::bind(sim, c, "CharGenCreature.Stats", &mut v);
    }
    // Empty slots read as no creature (0); `GetNumMembers` does not count them.
    for i in members.len()..4 {
        v.insert(format!("Party.Members.{i}"), num(0));
        v.insert(format!("Party.Summons.{i}"), num(0));
    }
    let mut set = |k: String, x: UiValue| {
        v.insert(k, x);
    };
    if let Some(a) = sim.world.area.and_then(|a| sim.world.get(a)) {
        set("Area.ResRef".into(), UiValue::String(a.tag.clone()));
        set("Area.Name".into(), UiValue::String(name(sim, a.id)));
    }
    set(
        "GameModeController.CurrentGameMode".into(),
        num(game_mode(sim)),
    );
    // The conversation in progress, as conversation.gfx reads it: the NPC line, the
    // responses (`Responses.N.Text`, counted with GetNumMembers) and a ready flag. The movie
    // shows the responses when the flag turns 1; here the line has always played (a player
    // that times lines, as thedas-app does, holds it at 0 while each line plays).
    if let Some(d) = conversation(sim) {
        set(
            "Conversation.NPCLine".into(),
            UiValue::String(d.line.clone()),
        );
        set("Conversation.Responses".into(), num(d.replies.len() as f64));
        for (i, (_, text)) in d.replies.iter().enumerate() {
            // A reply without text reaches the movie empty, as the game's would.
            let text = if text == CONTINUE_REPLY { "" } else { text };
            set(
                format!("Conversation.Responses.{i}.Text"),
                UiValue::String(text.to_string()),
            );
            set(format!("Conversation.Responses.{i}.Icon"), num(0));
        }
        set("Conversation.ResponsesReady".into(), num(1));
        set("Conversation.HasVO".into(), num(0));
    }
    // The open job board, as chanters.gfx reads it (see thedas_sim::chanters).
    if let Some(b) = &sim.world.chanter_board {
        let text =
            |s: Option<u32>| UiValue::String(s.and_then(|s| sim.string(s)).unwrap_or_default());
        set("Journal.ChanterBoard.Header".into(), text(b.header_strref));
        for (i, j) in b.jobs.iter().enumerate() {
            let k = format!("Journal.ChanterBoard.ChanterEntries.{i}");
            set(format!("{k}.PlotName"), text(j.name_strref));
            set(format!("{k}.JournalText"), text(j.text_strref));
            set(format!("{k}.PlotResRef"), UiValue::String(j.plot.clone()));
            set(format!("{k}.Accepted"), num(j.accepted as u8));
        }
    }
    // The world map, as worldmap.gfx reads it: the primary and secondary maps (their
    // worldmaps.gda row as `Type`, the row's image as `ResourceName`), where the party is,
    // and each map's pins. Provisional: the pin states are taken to be the scripts'
    // WM_LOCATION_* values.
    let wm = &sim.world.world_map;
    set("WorldMap.GUIStatus".into(), num(wm.gui_status));
    set(
        "WorldMap.GUITravelStatus".into(),
        num(wm.travelling_to.is_some() as u8),
    );
    let player_area = wm
        .player_pin
        .and_then(|p| sim.world.get(p))
        .and_then(|p| p.locals.strings.get("WM_AREA").cloned())
        .unwrap_or_default();
    let tables = sim.world.tables();
    let maps = tables.get(0, "worldmaps");
    let (primary, secondary) = sim.shown_maps();
    // Whether there is a second map: the screen disables its map toggle when this is 0.
    // Provisional: the screen only compares it with 0.
    set(
        "WorldMap.SecondaryMap".into(),
        num(secondary.is_some() as u8),
    );
    for (key, map) in [("PrimaryMap", primary), ("SecondaryMap", secondary)] {
        let Some(m) = map
            .and_then(|m| sim.world.get(m))
            .filter(|m| m.kind == ObjectKind::Map)
        else {
            continue;
        };
        let k = format!("WorldMap.{key}");
        let id = m.fields.get("MapId").copied().unwrap_or(0);
        set(format!("{k}.Type"), num(id));
        let image = maps
            .as_ref()
            .and_then(|t| match t.cell_by_hash(0xa90d_54b7, id as i64) {
                Some(thedas_formats::gda::Cell::String(s)) => {
                    Some(s.trim_end_matches('\0').trim().to_string())
                }
                _ => None,
            })
            .unwrap_or_default();
        set(format!("{k}.ResourceName"), UiValue::String(image));
        set(
            format!("{k}.PlayerLocation.AreaTag"),
            UiValue::String(player_area.clone()),
        );
        let pins = sim.map_pins(m.id);
        for (i, p) in pins.iter().filter_map(|&p| sim.world.get(p)).enumerate() {
            let pk = format!("{k}.Locations.{i}");
            let name = p
                .name_strref
                .and_then(|s| sim.string(s))
                .unwrap_or_default();
            set(format!("{pk}.Tag"), UiValue::String(p.tag.clone()));
            set(format!("{pk}.Name"), UiValue::String(name.clone()));
            set(format!("{pk}.Tooltip"), UiValue::String(name));
            set(
                format!("{pk}.AreaTag"),
                UiValue::String(p.locals.strings.get("WM_AREA").cloned().unwrap_or_default()),
            );
            set(
                format!("{pk}.State"),
                num(p.fields.get("WMStatus").copied().unwrap_or(0)),
            );
            set(
                format!("{pk}.Type"),
                num(p.fields.get("WMIcon").copied().unwrap_or(0)),
            );
            set(format!("{pk}.RecentlyActivated"), num(0));
            // The pin's place on the map image, as the map file gives it.
            set(format!("{pk}.X"), num(p.position[0]));
            set(format!("{pk}.Y"), num(p.position[1]));
        }
    }
    let creatures = sim
        .world
        .objects
        .values()
        .filter(|o| o.kind == ObjectKind::Creature && o.active)
        .count();
    set("Area.CreatureCount".into(), num(creatures as f64));
    inventory(sim, controlled.or(hero), shown.item_filter, &mut v);
    if let Some(who) = controlled.or(hero) {
        quickbar(sim, who, &mut v);
    }
    v
}

/// The quickbar of the party member the player controls (`GUI.SelectedChar.Stats.
/// QuickSlots.N`, 0 when empty): each slot's ability as an ability icon reads it, whether it
/// is on (`Status` 1, a sustained ability running), how much cooldown is left, and the item
/// it comes from.
fn quickbar(sim: &Sim, who: u32, v: &mut BTreeMap<String, UiValue>) {
    let tables = sim.world.tables();
    let abi = tables.get(0, "ABI_base");
    let bar = sim.quickbar(who);
    v.insert("GUI.SelectedChar.Stats.QuickbarLocked".into(), num(0));
    // The quickbar shows its first row only. Provisional.
    v.insert("Party.QuickbarExpandValue".into(), num(0));
    // The keys shown on the first ten slots: the game's default bindings, 1 to 0.
    for n in 1..=10 {
        v.insert(
            format!("InputMappings.Mappings.ActivateQuickslot{n}.PrimaryMappedKey"),
            UiValue::String((n % 10).to_string()),
        );
    }
    for slot in 0..thedas_sim::quickbar::QUICKSLOTS {
        let k = format!("GUI.SelectedChar.Stats.QuickSlots.{slot}");
        let Some((_, q)) = bar.iter().find(|(s, _)| *s == slot) else {
            v.insert(k, num(0));
            continue;
        };
        if let Some(abi) = &abi {
            let ty = match abi.cell("AbilityType", q.ability as i64) {
                Some(thedas_formats::gda::Cell::Int(t)) => *t,
                _ => 0,
            };
            crate::chains::ability(
                sim,
                abi,
                &k,
                q.ability as i64,
                ty,
                crate::chains::DISPLAYSTATE_HAVE,
                v,
            );
        } else {
            v.insert(k.clone(), num(q.ability));
        }
        let item = sim
            .party_backpack()
            .into_iter()
            .chain(
                sim.world
                    .get(who)
                    .map(|o| o.inventory.clone())
                    .unwrap_or_default(),
            )
            .find(|&i| {
                !q.item.is_empty()
                    && sim
                        .world
                        .get(i)
                        .is_some_and(|o| o.tag.eq_ignore_ascii_case(&q.item))
            });
        v.insert(
            format!("{k}.Status"),
            num(sim.modal_active(who, q.ability) as u8),
        );
        v.insert(
            format!("{k}.CooldownPercentage"),
            num(sim.cooldown_left(who, q.ability, &q.item)),
        );
        v.insert(format!("{k}.PendingActivation"), num(0));
        // The slot's picture (quickbar.gfx draws only this): the item's icon for an
        // item's slot, else the ability's. Provisional: inferred from the movie.
        let icon = match item {
            Some(i) => sim.item_look(i).icon,
            None => abi
                .as_ref()
                .and_then(|t| match t.cell("Icon", q.ability as i64) {
                    Some(thedas_formats::gda::Cell::String(s)) => {
                        Some(s.trim_end_matches('\0').trim().to_string())
                    }
                    _ => None,
                })
                .unwrap_or_default(),
        };
        v.insert(format!("{k}.ItemIcon"), UiValue::String(icon));
        v.insert(format!("{k}.SourceItem.ID"), num(item.unwrap_or(0)));
        v.insert(
            format!("{k}.SourceItemTag"),
            UiValue::String(q.item.clone()),
        );
        v.insert(
            format!("{k}.RefTemplate"),
            UiValue::String(
                item.and_then(|i| sim.world.get(i))
                    .map(|o| o.template.clone())
                    .unwrap_or_default(),
            ),
        );
        // How many of the item are left; no text for an ability's slot.
        let stack = match item.and_then(|i| sim.world.get(i)) {
            Some(o) => num(o.fields.get("StackSize").copied().unwrap_or(1)),
            None => UiValue::String(String::new()),
        };
        v.insert(format!("{k}.SourceItem.StackSize"), stack);
    }
}

/// An item as the inventory screens read it (`<prefix>.ID`, `.Name`, `.Icon`, ...).
/// Provisional: the fields come from inventory.gfx's reads; flags the simulation does not
/// keep (damaged, newly acquired) are 0.
fn item(sim: &Sim, prefix: &str, id: u32, v: &mut BTreeMap<String, UiValue>) {
    let Some(o) = sim.world.get(id) else { return };
    let look = sim.item_look(id);
    let field = |k: &str, d: i32| num(o.fields.get(k).copied().unwrap_or(d));
    let mask = sim.item_slots(id).iter().fold(0u32, |m, s| m | (1 << s));
    let text = |s: Option<u32>| UiValue::String(s.and_then(|s| sim.string(s)).unwrap_or_default());
    let mut set = |k: &str, x: UiValue| {
        v.insert(format!("{prefix}.{k}"), x);
    };
    set("ID", num(id));
    set("Tag", UiValue::String(o.tag.clone()));
    set("Name", UiValue::String(name(sim, id)));
    set("Description", text(look.description));
    set("Icon", UiValue::String(look.icon));
    set("BaseItemID", field("BaseItem", 0));
    // A row of the material types table; the movie names it from the tables itself.
    set("MaterialType", field("Material", 0));
    set("EquippableSlots", num(mask));
    set("StackSize", field("StackSize", 1));
    set("Cost", num(sim.item_value(id)));
    set("AbilityID", field("AbilityID", -1));
    set("RecipeID", field("CraftingRecipeID", 0));
    set("Unique", field("Unique", 0));
    set("Irremovable", num(o.plot as u8));
    for k in [
        "Damaged",
        "Indestructible",
        "InfiniteStock",
        "NewlyAcquired",
        "StoreFlag",
        "RuneSlotMask",
    ] {
        set(k, num(0));
    }
}

/// The inventory screen: the party's backpack (`GUIInventory.PartyBackpackView.N.Item`, the
/// items of the chosen category tab, `filter`) with plot items listed apart
/// (`PartyPlotItemsView`), the money, and what the selected party member (`who`) wears
/// (`GUI.SelectedChar.Equipment.<INVENTORY_SLOT_*>`, 0 for an empty slot).
fn inventory(sim: &Sim, who: Option<u32>, filter: i32, v: &mut BTreeMap<String, UiValue>) {
    let pack = sim.party_backpack();
    let (plot, gear): (Vec<u32>, Vec<u32>) = pack
        .iter()
        .partition(|&&i| sim.world.get(i).is_some_and(|o| o.plot));
    let shown: Vec<u32> = gear
        .iter()
        .copied()
        .filter(|&i| filter <= 0 || item_category(sim, i) as i32 == filter)
        .collect();
    for (view, items) in [("PartyBackpackView", &shown), ("PartyPlotItemsView", &plot)] {
        // The view's own token, which its list watches: its items, so a change to them
        // makes the list read them again.
        let ids: Vec<String> = items.iter().map(|i| i.to_string()).collect();
        v.insert(
            format!("GUIInventory.{view}"),
            UiValue::String(ids.join(",")),
        );
        for (i, id) in items.iter().enumerate() {
            let k = format!("GUIInventory.{view}.{i}");
            v.insert(k.clone(), num(*id));
            v.insert(format!("{k}.GroupSize"), num(1));
            item(sim, &format!("{k}.Item"), *id, v);
        }
    }
    // No container is open (looting goes through the simulation's own commands for now).
    v.insert("GUIInventory.ShowContainer".into(), num(0));
    v.insert("GUIInventory.ContainerHasPlotItems".into(), num(0));
    v.insert("GUIInventory.CanAddItemsToContainer".into(), num(0));
    v.insert("Party.BackpackSize".into(), num(gear.len() as f64));
    let hero = sim.world.hero.and_then(|h| sim.world.get(h));
    v.insert(
        "Party.BackpackMaxSize".into(),
        num(hero
            .and_then(|h| h.fields.get("MaxInventorySize").copied())
            .unwrap_or(thedas_sim::engine::DEFAULT_INVENTORY_SIZE)),
    );
    v.insert("Party.Money.Total".into(), num(hero.map_or(0, |h| h.money)));
    let Some(o) = who.and_then(|w| sim.world.get(w)) else {
        return;
    };
    v.insert("GUI.SelectedChar.CanChangeEquipment".into(), num(1));
    v.insert("GUI.SelectedChar.CanSwitchWeaponSets".into(), num(1));
    v.insert(
        "GUI.SelectedChar.ActiveWeaponSet".into(),
        num(o.fields.get("ActiveWeaponSet").copied().unwrap_or(0)),
    );
    // Provisional: the slots the screen shows are taken to be INVENTORY_SLOT_* 0 to 19.
    for slot in 0..20 {
        let k = format!("GUI.SelectedChar.Equipment.{slot}");
        match o.equipment.get(&slot) {
            Some(id) => {
                v.insert(k.clone(), num(*id));
                item(sim, &k, *id, v);
            }
            None => {
                v.insert(k.clone(), num(0));
                v.insert(format!("{k}.ID"), num(0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_world_map_shows_its_pins_and_where_the_party_is() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let map = sim.world.spawn(ObjectKind::Map, "wide_open_world");
        sim.world
            .get_mut(map)
            .unwrap()
            .fields
            .insert("MapId".into(), 1);
        let pin = sim.world.spawn(ObjectKind::MapPin, "wml_wow_lothering");
        {
            let p = sim.world.get_mut(pin).unwrap();
            p.locals.objects.insert("WM_MAP".into(), map);
            p.locals
                .strings
                .insert("WM_AREA".into(), "lot100ar_lothering".into());
            p.fields
                .extend([("WMStatus".to_string(), 2), ("WMIcon".to_string(), 7)]);
            p.position = [0.25, 0.75, 0.0];
        }
        sim.world.world_map.primary = Some(map);
        sim.world.world_map.player_pin = Some(pin);
        let v = sim_values(&sim);
        let k = "WorldMap.PrimaryMap";
        assert_eq!(v.get(&format!("{k}.Type")), Some(&UiValue::Number(1.0)));
        assert_eq!(
            v.get(&format!("{k}.PlayerLocation.AreaTag")),
            Some(&UiValue::String("lot100ar_lothering".into()))
        );
        let l = format!("{k}.Locations.0");
        assert_eq!(
            v.get(&format!("{l}.Tag")),
            Some(&UiValue::String("wml_wow_lothering".into()))
        );
        assert_eq!(v.get(&format!("{l}.State")), Some(&UiValue::Number(2.0)));
        assert_eq!(v.get(&format!("{l}.Type")), Some(&UiValue::Number(7.0)));
        assert_eq!(v.get(&format!("{l}.X")), Some(&UiValue::Number(0.25)));
        assert_eq!(v.get(&format!("{l}.Y")), Some(&UiValue::Number(0.75)));
        assert!(!v.contains_key("WorldMap.SecondaryMap.Type"));
        // No second map: the screen's map toggle disables itself on 0.
        assert_eq!(v["WorldMap.SecondaryMap"], UiValue::Number(0.0));
    }

    #[test]
    fn the_inventory_lists_the_backpack_by_tab_and_plot_items_apart() {
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        let pelt = sim.world.spawn(ObjectKind::Item, "pelt");
        let key = sim.world.spawn(ObjectKind::Item, "key");
        sim.world.get_mut(key).unwrap().plot = true;
        sim.world.get_mut(hero).unwrap().inventory = vec![pelt, key];
        let all = sim_values(&sim);
        let k = "GUIInventory.PartyBackpackView";
        assert_eq!(all[&format!("{k}.0.Item.ID")], UiValue::Number(pelt as f64));
        assert_eq!(
            all[&format!("{k}.0.Item.Tag")],
            UiValue::String("pelt".into())
        );
        assert!(!all.contains_key(&format!("{k}.1")));
        assert_eq!(all[k], UiValue::String(pelt.to_string()));
        assert_eq!(
            all["GUIInventory.PartyPlotItemsView.0.Item.ID"],
            UiValue::Number(key as f64)
        );
        assert_eq!(all["Party.BackpackSize"], UiValue::Number(1.0));
        // An empty slot reads as no item.
        assert_eq!(all["GUI.SelectedChar.Equipment.0"], UiValue::Number(0.0));
        let weapons = sim_values_for(
            &sim,
            &Presentation {
                item_filter: ItemCategory::Weapons as i32,
            },
        );
        assert!(!weapons.contains_key(&format!("{k}.0")));
        let other = sim_values_for(
            &sim,
            &Presentation {
                item_filter: ItemCategory::Other as i32,
            },
        );
        assert!(other.contains_key(&format!("{k}.0")));
    }

    #[test]
    fn the_quickbar_shows_the_selected_members_abilities() {
        use thedas_formats::gda::Cell;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        sim.world.tables().insert(
            "ABI_base",
            &["AbilityType", "UseType", "TargetType", "Icon"],
            vec![(
                3024,
                vec![
                    Cell::Int(1),
                    Cell::Int(4),
                    Cell::Int(4),
                    Cell::String("tlt_ico_overrun".into()),
                ],
            )],
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        sim.world.get_mut(hero).unwrap().abilities.insert(3024);
        let v = sim_values(&sim);
        let k = "GUI.SelectedChar.Stats.QuickSlots";
        assert_eq!(v[&format!("{k}.0")], UiValue::Number(3024.0));
        assert_eq!(
            v[&format!("{k}.0.ItemIcon")],
            UiValue::String("tlt_ico_overrun".into())
        );
        assert_eq!(v[&format!("{k}.0.Status")], UiValue::Number(0.0));
        assert_eq!(
            v[&format!("{k}.0.CooldownPercentage")],
            UiValue::Number(0.0)
        );
        assert_eq!(v[&format!("{k}.1")], UiValue::Number(0.0));
        assert_eq!(
            v["InputMappings.Mappings.ActivateQuickslot10.PrimaryMappedKey"],
            UiValue::String("0".into())
        );
    }

    #[test]
    fn an_empty_world_has_no_party() {
        let sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let v = sim_values(&sim);
        assert_eq!(v.get("Party.Hero.ID"), Some(&UiValue::Number(0.0)));
        assert_eq!(v.get("Party.MemberSelected.0"), Some(&UiValue::Number(0.0)));
        assert!(!v.contains_key("Party.Members.0.ID"));
        assert!(!v.contains_key("GUI.SelectedChar.ID"));
    }

    #[test]
    fn the_open_board_lists_its_jobs() {
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        sim.world.chanter_board = Some(thedas_sim::chanters::ChanterBoard {
            board: 3,
            header_strref: None,
            jobs: vec![thedas_sim::chanters::BoardJob {
                plot: "lot100pt_bears".into(),
                name_strref: None,
                text_strref: None,
                accepted_flag: 2,
                accepted: true,
            }],
        });
        let v = sim_values(&sim);
        let k = "Journal.ChanterBoard.ChanterEntries.0";
        assert_eq!(
            v.get(&format!("{k}.PlotResRef")),
            Some(&UiValue::String("lot100pt_bears".into()))
        );
        assert_eq!(v.get(&format!("{k}.Accepted")), Some(&UiValue::Number(1.0)));
        assert!(!v.contains_key("Journal.ChanterBoard.ChanterEntries.1.PlotName"));
    }
}
