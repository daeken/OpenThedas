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
    sim.display_name(id)
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
    /// The current level's minimap, read from its resources.
    pub minimap: Option<crate::minimap::MinimapInfo>,
    /// Party members' portraits: the image the stage serves for each (by creature id).
    pub portraits: BTreeMap<u32, String>,
    /// The player's saved games, newest first (the save, load and death screens list them).
    pub saves: Vec<SaveListing>,
    /// The save and load screen's mode: 1 saving, 2 loading (saveload.gfx's `SAVE_MODE`,
    /// `LOAD_MODE`).
    pub save_mode: i32,
    /// The slot a new save would take.
    pub open_slot: i32,
    /// The journal, while its screen is up (the app builds it: [`thedas_sim::Sim::journal`]).
    pub journal: Option<thedas_sim::journal::JournalView>,
    /// The entry the journal opens on (a HUD notification clicked: `GUI.GoToQuest`,
    /// `GUI.GoToCodex`).
    pub journal_focus: Option<JournalFocus>,
    /// The player's settings changed from their defaults (see [`crate::options`]).
    pub options: BTreeMap<String, f64>,
    /// The size the game draws at, the options screen's resolution.
    pub resolution: (u32, u32),
    /// How far the player has turned the paperdoll from facing them (radians; the
    /// inventory's and character record's rotate buttons, see `crate::input::PAPERDOLL_TURN`).
    pub paperdoll_yaw: f32,
}

/// The journal entry a notification opens the journal on, as journal.gfx reads it
/// (`GUIJournal.Notification*`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JournalFocus {
    /// A codex entry rather than a quest.
    pub codex: bool,
    /// The plot resource.
    pub resref: String,
    /// The quest's group (its resource) or the codex entry's group (its plot type).
    pub group: String,
    /// The quest is done (it is under Completed Quests).
    pub completed: bool,
}

/// A saved game as the save and load screens list it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SaveListing {
    pub slot: i32,
    pub name: String,
    /// Where the party was.
    pub area: String,
    /// How long the game had been played, seconds.
    pub time_played_s: u64,
    /// When it was written, as text.
    pub last_played: String,
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
        // The character record shows each attribute as its base and what gear and effects
        // add (`Base`, `Modifier`); `Total` is both, as the game's PROPERTY_VALUE_TOTAL.
        for (p, (base, cur)) in &o.properties {
            let total = if depletable.contains(p) && *p != 0 {
                sim.depletable_max(id, *p)
            } else {
                sim.property_total(id, *p)
            };
            v.insert(format!("{prefix}.Attributes.{p}.Current"), num(*cur));
            v.insert(format!("{prefix}.Attributes.{p}.Total"), num(total));
            v.insert(format!("{prefix}.Attributes.{p}.Base"), num(*base));
            v.insert(
                format!("{prefix}.Attributes.{p}.Modifier"),
                num(sim.property_modifier(id, *p)),
            );
        }
        v.insert(
            format!("{prefix}.Stats.Race"),
            num(o.fields.get("Race").copied().unwrap_or(0)),
        );
        // The auto-level option is off (the player spends the points).
        v.insert(format!("{prefix}.Stats.AutoLevelUp"), num(0));
        v.insert(format!("{prefix}.InCombat"), num(o.combat.in_combat as u8));
        // Properties the creature has no value of (resistances, mostly) read as their
        // total of nothing: what derivation, gear and effects give. Provisional: the
        // character record's rows are taken to be properties 1 to 60.
        for p in 1..=60 {
            if o.properties.contains_key(&p) {
                continue;
            }
            v.insert(
                format!("{prefix}.Attributes.{p}.Total"),
                num(sim.property_total(id, p)),
            );
            v.insert(format!("{prefix}.Attributes.{p}.Base"), num(0));
            v.insert(
                format!("{prefix}.Attributes.{p}.Modifier"),
                num(sim.property_modifier(id, p)),
            );
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
            for (p, (base, _)) in &o.properties {
                v.insert(
                    format!("CharGenCreature.Attributes.{p}.Modifier"),
                    num(sim.equipment_bonus(c, *p)),
                );
                // The points being spent go on the base, which the screen shows as Total
                // (depletables keep their maximum).
                let depletable = [
                    "PROPERTY_DEPLETABLE_HEALTH",
                    "PROPERTY_DEPLETABLE_MANA_STAMINA",
                ]
                .iter()
                .any(|k| thedas_sim::sim::constant(&sim.table, k) == *p);
                if !depletable {
                    v.insert(format!("CharGenCreature.Attributes.{p}.Total"), num(*base));
                }
            }
        }
        v.insert("CharGenCreature.Stats.AutoLevelUp".into(), num(0));
        crate::chains::bind(sim, c, "CharGenCreature.Stats", &mut v);
    }
    // The portrait the engine drew of each member's head (portraits.gfx loads it).
    for (i, id) in members.iter().enumerate() {
        if let Some(p) = shown.portraits.get(id) {
            v.insert(
                format!("Party.Members.{i}.Portrait"),
                UiValue::String(p.clone()),
            );
        }
    }
    // Who may level up now (the character record's button): a party member the game marked
    // (`SetCanLevelUp`, or experience reaching the next level) out of combat.
    for id in &members {
        let can = sim.world.get(*id).is_some_and(|o| {
            o.fields.get("CanLevelUp").copied().unwrap_or(0) != 0 && !o.combat.in_combat
        });
        v.insert(
            format!("{}.{id}", crate::protocol::CAN_LEVEL_UP),
            num(can as u8),
        );
    }
    // Empty slots read as no creature: null, which portraits.gfx's portraits hide on
    // (`SharedLibrary.Portrait.SetCreatureToken`'s observer: `GetValue(token) == null` →
    // `HideMovie`, else `ShowMovie`; a 0 showed an empty slot as a dead member's skull).
    // `GetNumMembers` does not count them.
    for i in members.len()..4 {
        v.insert(format!("Party.Members.{i}"), UiValue::Null);
    }
    // Nobody has a summoned creature (no summoning yet): every summon slot is empty, also
    // beside the members there are (portraits.gfx reads all four).
    for i in 0..4 {
        v.insert(format!("Party.Summons.{i}"), UiValue::Null);
    }
    let mut set = |k: String, x: UiValue| {
        v.insert(k, x);
    };
    if let Some(a) = sim.world.area.and_then(|a| sim.world.get(a)) {
        set("Area.ResRef".into(), UiValue::String(a.tag.clone()));
        set("Area.Name".into(), UiValue::String(name(sim, a.id)));
    }
    // Where the camera looks (world metres): the area map (map.gfx) opens centred there
    // (`SetMapFocus` from `CameraPivotPosition`). Provisional: the camera follows the party
    // member the player controls, so its pivot is taken to be that member's place.
    if let Some(o) = sim
        .world
        .controlled()
        .or(sim.world.hero)
        .and_then(|id| sim.world.get(id))
    {
        set(
            "GameModeExplore.CameraPivotPositionX".into(),
            num(o.position[0]),
        );
        set(
            "GameModeExplore.CameraPivotPositionY".into(),
            num(o.position[1]),
        );
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
    store(sim, &mut v);
    party_picker(sim, shown, &mut v);
    saves(sim, shown, &mut v);
    journal(sim, shown, &mut v);
    alerts(sim, &mut v);
    crate::options::values(&shown.options, shown.resolution, &mut v);
    // The difficulty, as character creation's difficulty prompt starts from it.
    v.insert(
        "ClientOptions.GameOptions.DifficultyLevel".into(),
        num(sim.world.difficulty),
    );
    chargen_screen(sim, &mut v);
    // No tutorials are queued: popuplayer.gfx hides its tutorial panel when it sees the empty
    // list (`Tutorials.TutorialList`, observed). Provisional: the game's tutorial queue is
    // not modelled.
    v.insert(
        "Tutorials.TutorialList".into(),
        UiValue::String(String::new()),
    );
    v.insert("Tutorials.Enabled".into(), num(0));
    v.insert("Tutorials.TrainingStep".into(), num(0));
    if let Some(who) = controlled.or(hero) {
        tactics(sim, who, &mut v);
    }
    // The minimap, as minimap.gfx reads it: the picture, the world corners it spans
    // (`Left`/`Bottom` the lower, `Right`/`Top` the upper) and the share of the picture
    // they cover. Provisional: north is taken to be up (0).
    if let Some(m) = &shown.minimap {
        let s = |k: &str, x: UiValue| (format!("Area.{k}"), x);
        v.extend([
            s("Minimap", UiValue::String(m.texture.clone())),
            s("MinimapLeft", num(m.min[0])),
            s("MinimapBottom", num(m.min[1])),
            s("MinimapRight", num(m.max[0])),
            s("MinimapTop", num(m.max[1])),
            s("MinimapWidth", num(m.used[0])),
            s("MinimapHeight", num(m.used[1])),
            s("North", num(0)),
        ]);
        v.insert("GUI.MinimapOpen".into(), num(1));
    }
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
        // The item the slot comes from: one of the party's, which shares its items.
        let item = (!q.item.is_empty())
            .then(|| sim.party_item(who, &q.item))
            .flatten();
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
        // How many of the item the party has left (0, shown red, when they ran out); no
        // text for an ability's slot.
        let stack = if q.item.is_empty() {
            UiValue::String(String::new())
        } else {
            num(sim.item_count(&q.item))
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

/// The open store, as store.gfx reads it: its name, its mark-up and mark-down (percent; the
/// movie prices items from their `Cost` with them), its stock (`GUIStore.StoreView.N.Item`)
/// and what the party could sell (`GUIStore.PartyBackpackView`, the backpack without plot
/// items). Provisional: the lists' layout is taken from inventory.gfx's, which store.gfx
/// shares.
fn store(sim: &Sim, v: &mut BTreeMap<String, UiValue>) {
    let Some(st) = sim.world.store.and_then(|s| sim.world.get(s)) else {
        return;
    };
    let field = |k: &str, d: i32| st.fields.get(k).copied().unwrap_or(d);
    v.insert("Store.Name".into(), UiValue::String(name(sim, st.id)));
    v.insert("Store.MarkUp".into(), num(field("MarkUp", 100)));
    v.insert("Store.MarkDown".into(), num(field("MarkDown", 100)));
    v.insert("Party.BackpackSort".into(), num(0));
    let stock = st.inventory.clone();
    let sellable: Vec<u32> = sim
        .party_backpack()
        .into_iter()
        .filter(|&i| !sim.world.get(i).is_some_and(|o| o.plot))
        .collect();
    for (view, items) in [("StoreView", &stock), ("PartyBackpackView", &sellable)] {
        let ids: Vec<String> = items.iter().map(|i| i.to_string()).collect();
        v.insert(format!("GUIStore.{view}"), UiValue::String(ids.join(",")));
        for (i, id) in items.iter().enumerate() {
            let k = format!("GUIStore.{view}.{i}");
            v.insert(k.clone(), num(*id));
            v.insert(format!("{k}.GroupSize"), num(1));
            item(sim, &format!("{k}.Item"), *id, v);
            if view == "StoreView" {
                let infinite = sim
                    .world
                    .get(*id)
                    .is_some_and(|o| o.fields.get("Infinite").copied().unwrap_or(0) != 0);
                v.insert(format!("{k}.Item.InfiniteStock"), num(infinite as u8));
                v.insert(format!("{k}.Item.StoreFlag"), num(1));
            }
        }
    }
}

/// The journal, as journal.gfx reads it: the quests in progress (`Journal.PlotList.N`) and
/// done (`Journal.CompletedPlots.N`), each with its plot resource, name, latest entry
/// (`Description`), quest group and parent quest (resources); the quest groups
/// (`Journal.PlotGroups.N`: resource, name, priority, shown expanded); the codex
/// (`Journal.CodexGroups.N`: the plot type as `CodexGroup`, its name and `CodexList.M`, each
/// entry's resource, name, `Locked`, `Index` and `Descriptions.K`); and the quest marked
/// active (`Journal.ActivePlot.ResRef`). Each list's own value lists its entries, so the
/// movie's list observers fire. New entries are marked (`Updated`, `CodexUpdated`) until read,
/// and there is no conversation history (`Journal.Conversations`). Provisional: the layout is read from
/// journal.gfx; the attribute names are the executable's (see [`thedas_sim::journal`]).
fn journal(sim: &Sim, shown: &Presentation, v: &mut BTreeMap<String, UiValue>) {
    let Some(j) = &shown.journal else {
        return;
    };
    let text = |s: Option<u32>| UiValue::String(s.and_then(|s| sim.string(s)).unwrap_or_default());
    let s = |x: &str| UiValue::String(x.to_string());
    let joined = |it: &mut dyn Iterator<Item = &str>| s(&it.collect::<Vec<_>>().join(","));
    for (list, quests) in [("PlotList", &j.active), ("CompletedPlots", &j.completed)] {
        v.insert(
            format!("Journal.{list}"),
            joined(&mut quests.iter().map(|q| q.resref.as_str())),
        );
        for (i, q) in quests.iter().enumerate() {
            let k = format!("Journal.{list}.{i}");
            v.insert(k.clone(), s(&q.resref));
            v.insert(format!("{k}.ResRef"), s(&q.resref));
            v.insert(format!("{k}.Name"), text(q.name));
            v.insert(format!("{k}.Description"), text(q.description));
            v.insert(format!("{k}.Completed"), num(q.completed as u8));
            v.insert(format!("{k}.Updated"), num(q.updated as u8));
            v.insert(format!("{k}.PlotGroup"), s(&q.group));
            v.insert(
                format!("{k}.ParentPlot"),
                s(q.parent.as_deref().unwrap_or_default()),
            );
        }
    }
    v.insert(
        "Journal.PlotGroups".into(),
        joined(&mut j.groups.iter().map(|g| g.resref.as_str())),
    );
    for (i, g) in j.groups.iter().enumerate() {
        let k = format!("Journal.PlotGroups.{i}");
        v.insert(k.clone(), s(&g.resref));
        v.insert(format!("{k}.ResRef"), s(&g.resref));
        v.insert(format!("{k}.Name"), text(g.name));
        v.insert(format!("{k}.Priority"), num(g.priority));
        v.insert(format!("{k}.ExpandedInCurrentQuests"), num(1));
        v.insert(format!("{k}.ExpandedInCompletedQuests"), num(1));
    }
    v.insert(
        "Journal.CodexGroups".into(),
        s(&j.codex
            .iter()
            .map(|g| g.kind.to_string())
            .collect::<Vec<_>>()
            .join(",")),
    );
    for (i, g) in j.codex.iter().enumerate() {
        let k = format!("Journal.CodexGroups.{i}");
        v.insert(k.clone(), num(g.kind));
        v.insert(format!("{k}.CodexGroup"), num(g.kind));
        v.insert(format!("{k}.Name"), text(g.name));
        v.insert(
            format!("{k}.CodexList"),
            joined(&mut g.entries.iter().map(|e| e.resref.as_str())),
        );
        for (n, e) in g.entries.iter().enumerate() {
            let c = format!("{k}.CodexList.{n}");
            v.insert(c.clone(), s(&e.resref));
            v.insert(format!("{c}.ResRef"), s(&e.resref));
            v.insert(format!("{c}.Name"), text(e.name));
            v.insert(format!("{c}.Locked"), num(e.texts.is_empty() as u8));
            v.insert(
                format!("{c}.Updated"),
                num((e.updated && !e.texts.is_empty()) as u8),
            );
            v.insert(format!("{c}.Index"), s(&(n + 1).to_string()));
            v.insert(format!("{c}.Descriptions"), num(e.texts.len() as f64));
            for (d, t) in e.texts.iter().enumerate() {
                v.insert(format!("{c}.Descriptions.{d}"), text(Some(*t)));
            }
        }
    }
    // The entry a clicked notification opens the journal on, else none (the screen opens on
    // its quests; an unset `NotificationOpenPRCTab` reads as "open the downloadable content
    // tab").
    let focus = shown.journal_focus.clone().unwrap_or_default();
    let (quest, codex) = match &shown.journal_focus {
        Some(f) if f.codex => (JournalFocus::default(), focus),
        Some(_) => (focus, JournalFocus::default()),
        None => Default::default(),
    };
    v.insert("GUIJournal.NotificationQuest".into(), s(&quest.resref));
    v.insert("GUIJournal.NotificationQuestGroup".into(), s(&quest.group));
    v.insert(
        "GUIJournal.NotificationQuestIsCompleted".into(),
        num(quest.completed as u8),
    );
    v.insert("GUIJournal.NotificationCodex".into(), s(&codex.resref));
    v.insert(
        "GUIJournal.NotificationCodexGroup".into(),
        match codex.group.parse::<f64>() {
            Ok(n) => num(n),
            Err(_) => s(""),
        },
    );
    v.insert("GUIJournal.NotificationContentID".into(), s(""));
    v.insert("GUIJournal.NotificationOpenPRCTab".into(), num(0));
    v.insert("Journal.CodexUpdated".into(), num(j.codex_updated as u8));
    v.insert("Journal.Conversations".into(), s(""));
    v.insert(
        "Journal.ActivePlot.ResRef".into(),
        s(j.active_quest.as_deref().unwrap_or_default()),
    );
}

/// The HUD's notifications, as notifications.gfx reads them (`Party.Alerts.N`: the alert
/// type, 1 a quest's new journal entry and 2 a codex entry, new, the plot's resource and
/// name and whether the quest is done), with the list's own value listing them so the
/// movie's observer fires; no large ("uber") alerts. Provisional: every alert shows as new,
/// and which events raise one is inferred (see [`thedas_sim::journal::Alert`]).
fn alerts(sim: &Sim, v: &mut BTreeMap<String, UiValue>) {
    let alerts = &sim.world.alerts;
    v.insert(
        "Party.Alerts".into(),
        UiValue::String(
            alerts
                .iter()
                .map(|a| format!("{}:{}:{}", a.kind, a.resref, a.completed as u8))
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    for (i, a) in alerts.iter().enumerate() {
        let k = format!("Party.Alerts.{i}");
        v.insert(k.clone(), num(a.kind));
        v.insert(format!("{k}.Type"), num(a.kind));
        v.insert(format!("{k}.New"), num(1));
        v.insert(format!("{k}.WasUber"), num(0));
        v.insert(
            format!("{k}.QuestResRef"),
            UiValue::String(a.resref.clone()),
        );
        v.insert(
            format!("{k}.QuestName"),
            UiValue::String(a.name.and_then(|t| sim.string(t)).unwrap_or_default()),
        );
        v.insert(format!("{k}.QuestCompleted"), num(a.completed as u8));
        v.insert(format!("{k}.IntegerArg"), num(0));
        v.insert(format!("{k}.CustomText"), UiValue::String(String::new()));
        v.insert(format!("{k}.CustomIcon"), UiValue::String(String::new()));
    }
    v.insert("Party.UberAlerts".into(), UiValue::String(String::new()));
}

/// Character creation's own values for chargen.gfx: it is the game's creation screen, not the
/// toolset's character creator (`GUICharGen.CharCreatorRunning` 0), and importing a hero
/// from another module is off (`ImportEnabled` 0: there are no other modules' saves).
/// Provisional: read from the movie.
fn chargen_screen(sim: &Sim, v: &mut BTreeMap<String, UiValue>) {
    if sim.world.chargen.is_none() || sim.world.chargen_mode != 0 {
        return;
    }
    v.insert("GUICharGen.CharCreatorRunning".into(), num(0));
    v.insert("GUICharGen.ImportEnabled".into(), num(0));
}

/// The saved games, as saveload.gfx and deathscreen.gfx read them: the screen's mode, the
/// slot a new save takes, the one hero whose saves they are (`SaveLoad.HeroList.0`,
/// `SaveLoad.CurrentHero`) and each save (`SaveLoad.SaveFileList.N`: name, slot, area,
/// kind, time played, when). The death screen offers loading only when there is a save
/// (it counts the list). Provisional: no screenshots, every save of kind 0; a party is never
/// captured on death (`Party.CapturedOnDeath` 0) and the death screen has no hint.
fn saves(sim: &Sim, shown: &Presentation, v: &mut BTreeMap<String, UiValue>) {
    v.insert("SaveLoad.Mode".into(), num(shown.save_mode));
    v.insert("SaveLoad.OpenSlot".into(), num(shown.open_slot));
    v.insert("SaveLoad.CurrentHero".into(), num(0));
    if let Some(h) = sim.world.hero {
        let k = "SaveLoad.HeroList.0";
        v.insert(k.into(), num(h));
        v.insert(format!("{k}.Name"), UiValue::String(name(sim, h)));
        let last = shown.saves.first();
        v.insert(
            format!("{k}.TimePlayed"),
            UiValue::String(played(
                last.map_or(sim.world.time_ms / 1000, |s| s.time_played_s),
            )),
        );
        v.insert(
            format!("{k}.LastPlayed"),
            UiValue::String(last.map(|s| s.last_played.clone()).unwrap_or_default()),
        );
        v.insert(
            format!("{k}.LastLocation"),
            UiValue::String(last.map(|s| s.area.clone()).unwrap_or_default()),
        );
        v.insert(format!("{k}.Screenshot"), UiValue::String(String::new()));
    }
    // Saving lists a new save first (slot -1: the movie names it after the area and saves it
    // to the open slot), then the saves it may overwrite.
    let new_save = SaveListing {
        slot: -1,
        area: sim.world.area.map(|a| name(sim, a)).unwrap_or_default(),
        time_played_s: sim.world.time_ms / 1000,
        ..Default::default()
    };
    let rows: Vec<&SaveListing> = (shown.save_mode == 1)
        .then_some(&new_save)
        .into_iter()
        .chain(&shown.saves)
        .collect();
    // The list tokens the movies observe (the rows are rebuilt when they change).
    v.insert(
        "SaveLoad.SaveFileList".into(),
        UiValue::String(
            rows.iter()
                .map(|s| s.slot.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    if let Some(h) = sim.world.hero {
        v.insert("SaveLoad.HeroList".into(), UiValue::String(h.to_string()));
    }
    for (i, s) in rows.into_iter().enumerate() {
        let k = format!("SaveLoad.SaveFileList.{i}");
        v.insert(k.clone(), num(s.slot));
        v.insert(format!("{k}.Name"), UiValue::String(s.name.clone()));
        v.insert(format!("{k}.Slot"), num(s.slot));
        v.insert(format!("{k}.Area"), UiValue::String(s.area.clone()));
        v.insert(format!("{k}.SaveType"), num(0));
        v.insert(
            format!("{k}.TimePlayed"),
            UiValue::String(played(s.time_played_s)),
        );
        v.insert(
            format!("{k}.LastPlayed"),
            UiValue::String(s.last_played.clone()),
        );
        v.insert(format!("{k}.Story"), UiValue::String(String::new()));
        v.insert(format!("{k}.Screenshot"), UiValue::String(String::new()));
        v.insert(format!("{k}.ClassID"), num(0));
    }
    v.insert("Party.CapturedOnDeath".into(), num(0));
    v.insert("GUIDeath.DeathHint".into(), UiValue::String(String::new()));
}

/// Time played as the screens show it, hours and minutes.
fn played(secs: u64) -> String {
    format!("{}:{:02}", secs / 3600, secs % 3600 / 60)
}

/// The open party picker, as partypicker.gfx reads it: the scripts' picker status
/// (`Party.PartyPickerGUIStatus`, `SetPartyPickerGUIStatus`), the hero and the followers
/// picked so far (`PartyPicker.SelectedPartyMember.N.ID`, `.Portrait`: 0 the hero, then up
/// to three) and those that cannot be left out (`PartyPicker.LockedPartyMembers.N`). The
/// player picks followers in the world (they stand in camp) and the movie's buttons send
/// the rest. Provisional: no hover text (`PartyPicker.MouseOverName`) yet.
fn party_picker(sim: &Sim, shown: &Presentation, v: &mut BTreeMap<String, UiValue>) {
    let status = sim
        .world
        .module
        .and_then(|m| sim.world.get(m))
        .and_then(|m| m.fields.get("PartyPickerGUIStatus").copied())
        .unwrap_or(0);
    v.insert("Party.PartyPickerGUIStatus".into(), num(status));
    let Some(p) = &sim.world.party_picker else {
        return;
    };
    let members = sim.world.hero.into_iter().chain(p.selected.iter().copied());
    for i in 0..4 {
        v.insert(format!("PartyPicker.SelectedPartyMember.{i}"), num(0));
    }
    for (i, id) in members.enumerate().take(4) {
        let k = format!("PartyPicker.SelectedPartyMember.{i}");
        v.insert(k.clone(), num(id));
        v.insert(format!("{k}.ID"), num(id));
        v.insert(format!("{k}.Name"), UiValue::String(name(sim, id)));
        v.insert(
            format!("{k}.Portrait"),
            UiValue::String(shown.portraits.get(&id).cloned().unwrap_or_default()),
        );
        v.insert(format!("{k}.PortraitProperties.PortraitTint"), num(0));
    }
    for (i, id) in sim.locked_followers().iter().enumerate() {
        v.insert(format!("PartyPicker.LockedPartyMembers.{i}"), num(*id));
    }
    v.insert(
        "PartyPicker.MouseOverName".into(),
        UiValue::String(String::new()),
    );
    v.insert(
        "PartyPicker.MouseOverDescription".into(),
        UiValue::String(String::new()),
    );
}

/// The tactics of the party member the player controls, as tactics.gfx reads them
/// (`GUI.SelectedChar.Tactics.TacticsList.N`: whether the slot is on, its target type,
/// condition, command and the command's parameter; row N is slot N + 1, as the scripts number
/// slots from 1), with the presets it can pick (`PresetIdList.N`, its `AddTacticPresetID`
/// presets), the one in use (`PresetType` 2 and its place in the list as `PresetIndex`, or
/// type 0, "Off"), whether its slots were edited since (`Dirty`) and whether its tactics are
/// on (`Enabled`). DAOrigins.exe's PartyTactics keeps these tokens (set up at 0x7b11xx). The
/// list's own token changes with its rows, so tactics.gfx's list observer reads them again
/// after a preset is loaded. A slot's `TargetID` and `ConditionID` are the party members its
/// per-follower target and condition name (-1 none), `CommandItem` the item it uses.
/// Provisional: a creature with `NumTactics` set shows that many rows, and there are no
/// custom presets (`NumCustomSlots` 0).
fn tactics(sim: &Sim, who: u32, v: &mut BTreeMap<String, UiValue>) {
    use thedas_sim::tactics::{FIELD_DIRTY, FIELD_ENABLED, FIELD_PRESET_TYPE, PRESET_PREDEFINED};
    let Some(o) = sim.world.get(who) else { return };
    let k = "GUI.SelectedChar.Tactics";
    let field = |f: &str| o.fields.get(f).copied();
    v.insert(
        format!("{k}.Enabled"),
        num((field(FIELD_ENABLED).unwrap_or(1) != 0) as u8),
    );
    v.insert(format!("{k}.Dirty"), num(field(FIELD_DIRTY).unwrap_or(0)));
    v.insert(format!("{k}.DirtyPreset"), num(0));
    let presets = sim.tactic_presets(who);
    v.insert(format!("{k}.PresetIdList"), num(presets.len() as f64));
    for (i, p) in presets.iter().enumerate() {
        v.insert(format!("{k}.PresetIdList.{i}"), num(*p));
    }
    let current = field("TacticPreset").or_else(|| presets.first().copied());
    let index = current.and_then(|c| presets.iter().position(|&p| p == c));
    let kind = match (field(FIELD_PRESET_TYPE), index) {
        (Some(t), _) => t,
        (None, Some(_)) => PRESET_PREDEFINED,
        (None, None) => 0,
    };
    v.insert(format!("{k}.PresetType"), num(kind));
    v.insert(
        format!("{k}.PresetIndex"),
        num(index.filter(|_| kind == PRESET_PREDEFINED).unwrap_or(0) as f64),
    );
    v.insert(format!("{k}.NumCustomSlots"), num(0));
    v.insert("Party.InCamp".into(), num(0));
    let slots = sim.tactics_shown(who);
    let rows = match field("NumTactics") {
        Some(n) if n > 0 => n as usize,
        _ => slots.len().saturating_sub(1),
    };
    let mut token = Vec::with_capacity(rows);
    for i in 0..rows {
        let t = slots.get(i + 1).cloned().unwrap_or_default();
        let r = format!("{k}.TacticsList.{i}");
        v.insert(format!("{r}.Enabled"), num(t.enabled as u8));
        v.insert(format!("{r}.Target"), num(t.target));
        let object = |o: Option<u32>| num(o.map_or(-1.0, f64::from));
        v.insert(format!("{r}.TargetID"), object(t.target_object));
        v.insert(format!("{r}.Condition"), num(t.condition));
        v.insert(format!("{r}.ConditionID"), object(t.condition_object));
        v.insert(format!("{r}.Command"), num(t.command));
        v.insert(format!("{r}.CommandParam"), num(t.param));
        v.insert(format!("{r}.CommandItem"), num(t.item.unwrap_or(0)));
        v.insert(
            format!("{r}.ItemName"),
            UiValue::String(t.item.map(|i| name(sim, i)).unwrap_or_default()),
        );
        v.insert(
            format!("{r}.ItemIcon"),
            UiValue::String(t.item.map(|i| sim.item_look(i).icon).unwrap_or_default()),
        );
        token.push(format!(
            "{}:{}:{}:{}:{}",
            t.enabled as u8, t.target, t.condition, t.command, t.param
        ));
    }
    v.insert(format!("{k}.TacticsList"), UiValue::String(token.join(","))); // The use-item picker names the party's items by `Objects.<id>.Name`.
    for item in sim
        .tactic_items(0)
        .into_iter()
        .chain(sim.tactic_items(1))
        .chain(sim.tactic_items(3))
    {
        v.insert(
            format!("Objects.{item}.Name"),
            UiValue::String(name(sim, item)),
        );
    }
}

/// The inventory screen: the party's backpack/// The inventory screen: the party's backpack (`GUIInventory.PartyBackpackView.N.Item`, the
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
    fn empty_party_slots_read_as_no_creature() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "player");
        let dog = sim.world.spawn(ObjectKind::Creature, "dog");
        sim.world.hero = Some(hero);
        sim.world.get_mut(hero).unwrap().followers.push(dog);
        let v = sim_values(&sim);
        assert_eq!(v.get("Party.Members.0"), Some(&num(hero)));
        assert_eq!(v.get("Party.Members.1"), Some(&num(dog)));
        // portraits.gfx hides a portrait whose token reads null (a 0 drew a dead member).
        // How far the quickbar is open is the movie's to keep (seeded once by the stage).
        assert!(!v.contains_key(crate::stage::QUICKBAR_EXPAND_VALUE));
        for i in 2..4 {
            assert_eq!(v.get(&format!("Party.Members.{i}")), Some(&UiValue::Null));
        }
        for i in 0..4 {
            assert_eq!(v.get(&format!("Party.Summons.{i}")), Some(&UiValue::Null));
        }
    }

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
    fn members_show_the_portraits_drawn_of_them() {
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        let shown = Presentation {
            portraits: [(hero, "thedas_portrait_1_1.png".to_string())].into(),
            ..Default::default()
        };
        let v = sim_values_for(&sim, &shown);
        assert_eq!(
            v["Party.Members.0.Portrait"],
            UiValue::String("thedas_portrait_1_1.png".into())
        );
        assert!(!sim_values(&sim).contains_key("Party.Members.0.Portrait"));
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
                ..Default::default()
            },
        );
        assert!(!weapons.contains_key(&format!("{k}.0")));
        let other = sim_values_for(
            &sim,
            &Presentation {
                item_filter: ItemCategory::Other as i32,
                ..Default::default()
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
    fn the_minimap_spans_its_levels_corners() {
        let sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        assert!(!sim_values(&sim).contains_key("Area.Minimap"));
        let shown = Presentation {
            minimap: Some(crate::minimap::MinimapInfo {
                texture: "hrt001d_mmap_ph.dds".into(),
                min: [-70.0, -50.0],
                max: [25.0, 50.0],
                used: [0.5, 0.6],
                size: [512.0, 512.0],
            }),
            ..Default::default()
        };
        let v = sim_values_for(&sim, &shown);
        let n = |k: &str| v[k].clone();
        assert_eq!(
            n("Area.Minimap"),
            UiValue::String("hrt001d_mmap_ph.dds".into())
        );
        assert_eq!(
            [
                n("Area.MinimapLeft"),
                n("Area.MinimapBottom"),
                n("Area.MinimapRight"),
                n("Area.MinimapTop")
            ],
            [-70.0, -50.0, 25.0, 50.0].map(UiValue::Number)
        );
        assert_eq!(n("Area.MinimapWidth"), UiValue::Number(0.5));
        assert_eq!(n("GUI.MinimapOpen"), UiValue::Number(1.0));
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

    #[test]
    fn the_store_its_stock_and_what_the_party_could_sell() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        let store = sim.world.spawn(ObjectKind::Store, "store_gorim");
        let helm = sim.world.spawn(ObjectKind::Item, "helm");
        let pelt = sim.world.spawn(ObjectKind::Item, "pelt");
        let key = sim.world.spawn(ObjectKind::Item, "key");
        sim.world.get_mut(key).unwrap().plot = true;
        {
            let s = sim.world.get_mut(store).unwrap();
            s.inventory.push(helm);
            s.fields.insert("MarkUp".into(), 150);
            s.fields.insert("MarkDown".into(), 25);
        }
        sim.world
            .get_mut(helm)
            .unwrap()
            .fields
            .insert("Infinite".into(), 1);
        sim.world
            .get_mut(hero)
            .unwrap()
            .inventory
            .extend([pelt, key]);
        assert!(!sim_values(&sim).contains_key("GUIStore.StoreView"));
        sim.world.store = Some(store);
        let v = sim_values(&sim);
        assert_eq!(v["Store.MarkUp"], UiValue::Number(150.0));
        assert_eq!(v["Store.MarkDown"], UiValue::Number(25.0));
        assert_eq!(v["GUIStore.StoreView.0.Item.ID"], num(helm));
        assert_eq!(v["GUIStore.StoreView.0.Item.InfiniteStock"], num(1));
        assert_eq!(v["GUIStore.PartyBackpackView.0.Item.ID"], num(pelt));
        assert!(
            !v.contains_key("GUIStore.PartyBackpackView.1"),
            "no plot items"
        );
    }

    #[test]
    fn the_character_record_and_tactics_of_the_selected_member() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        {
            let o = sim.world.get_mut(hero).unwrap();
            o.properties.insert(1, (20.0, 0.0));
            o.modifiers.insert(1, 3.0);
            o.fields.insert("CanLevelUp".into(), 1);
            o.fields.insert("Race".into(), 2);
            // Slot 0 is unused: the scripts number slots from 1.
            o.combat.tactics.push(Default::default());
            o.combat.tactics.push(thedas_sim::combat::Tactic {
                enabled: true,
                target: 2,
                condition: 9,
                command: 4,
                param: 3000,
                ..Default::default()
            });
            for (k, v) in [
                ("TacticPresetCount", 2),
                ("TacticPreset.0", 5),
                ("TacticPreset.1", 7),
                ("TacticPreset", 7),
            ] {
                o.fields.insert(k.into(), v);
            }
        }
        let v = sim_values(&sim);
        let a = "GUI.SelectedChar.Attributes.1";
        assert_eq!(v[&format!("{a}.Base")], UiValue::Number(20.0));
        assert_eq!(v[&format!("{a}.Modifier")], UiValue::Number(3.0));
        assert_eq!(v[&format!("{a}.Total")], UiValue::Number(23.0));
        // A property it has no value of reads as nothing added.
        assert_eq!(
            v["GUI.SelectedChar.Attributes.42.Total"],
            UiValue::Number(0.0)
        );
        assert_eq!(v["GUI.SelectedChar.Stats.Race"], UiValue::Number(2.0));
        assert_eq!(
            v[&format!("{}.{hero}", crate::protocol::CAN_LEVEL_UP)],
            UiValue::Number(1.0)
        );
        let t = "GUI.SelectedChar.Tactics.TacticsList.0";
        assert_eq!(v[&format!("{t}.Condition")], UiValue::Number(9.0));
        assert_eq!(v[&format!("{t}.CommandParam")], UiValue::Number(3000.0));
        assert_eq!(v[&format!("{t}.Enabled")], UiValue::Number(1.0));
        assert_eq!(
            v[&format!("{t}.TargetID")],
            UiValue::Number(-1.0),
            "no follower"
        );
        assert_eq!(v[&format!("{t}.CommandItem")], UiValue::Number(0.0));
        let k = "GUI.SelectedChar.Tactics";
        assert!(!v.contains_key(&format!("{k}.TacticsList.1.Condition")));
        assert_eq!(
            v[&format!("{k}.TacticsList")],
            UiValue::String("1:2:9:4:3000".into())
        );
        assert_eq!(v[&format!("{k}.PresetIdList.1")], UiValue::Number(7.0));
        assert_eq!(v[&format!("{k}.PresetType")], UiValue::Number(2.0));
        assert_eq!(v[&format!("{k}.PresetIndex")], UiValue::Number(1.0));
        assert_eq!(v[&format!("{k}.Enabled")], UiValue::Number(1.0));
        // Three slots to fill: the rows past its own are empty.
        sim.world
            .get_mut(hero)
            .unwrap()
            .fields
            .insert("NumTactics".into(), 3);
        let v = sim_values(&sim);
        assert_eq!(
            v[&format!("{k}.TacticsList.2.Condition")],
            UiValue::Number(0.0)
        );
        assert!(!v.contains_key(&format!("{k}.TacticsList.3.Condition")));
        sim.world.get_mut(hero).unwrap().combat.in_combat = true;
        let v = sim_values(&sim);
        assert_eq!(
            v[&format!("{}.{hero}", crate::protocol::CAN_LEVEL_UP)],
            UiValue::Number(0.0),
            "not in combat"
        );
    }

    #[test]
    fn the_open_party_picker_shows_the_hero_and_the_followers_picked() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        let dog = sim.world.spawn(ObjectKind::Creature, "dog");
        sim.world.hero = Some(hero);
        assert!(!sim_values(&sim).contains_key("PartyPicker.SelectedPartyMember.0"));
        sim.world.party_picker = Some(thedas_sim::world::PartyPicker {
            selected: vec![dog],
        });
        let v = sim_values(&sim);
        assert_eq!(v["PartyPicker.SelectedPartyMember.0.ID"], num(hero));
        assert_eq!(v["PartyPicker.SelectedPartyMember.1.ID"], num(dog));
        assert_eq!(v["PartyPicker.SelectedPartyMember.2"], num(0));
        assert_eq!(v["Party.PartyPickerGUIStatus"], num(0));
    }

    #[test]
    fn the_saves_are_listed_for_the_save_load_and_death_screens() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        let shown = Presentation {
            saves: vec![SaveListing {
                slot: 3,
                name: "Before the tower".into(),
                area: "Ostagar".into(),
                time_played_s: 3725,
                last_played: "2026-11-11 10:00".into(),
            }],
            save_mode: 2,
            open_slot: 4,
            ..Default::default()
        };
        let v = sim_values_for(&sim, &shown);
        assert_eq!(v["SaveLoad.Mode"], num(2));
        assert_eq!(v["SaveLoad.OpenSlot"], num(4));
        assert_eq!(v["SaveLoad.SaveFileList.0.Slot"], num(3));
        assert_eq!(
            v["SaveLoad.SaveFileList.0.TimePlayed"],
            UiValue::String("1:02".into())
        );
        assert_eq!(v["SaveLoad.HeroList.0"], num(hero));
        assert_eq!(v["Party.CapturedOnDeath"], num(0));
        assert_eq!(v["SaveLoad.SaveFileList"], UiValue::String("3".into()));
        assert!(!sim_values(&sim).contains_key("SaveLoad.SaveFileList.0"));
        // Saving lists a new save first.
        let saving = Presentation {
            save_mode: 1,
            ..shown
        };
        let v = sim_values_for(&sim, &saving);
        assert_eq!(v["SaveLoad.SaveFileList.0.Slot"], num(-1));
        assert_eq!(v["SaveLoad.SaveFileList.1.Slot"], num(3));
    }

    #[test]
    fn the_journal_lists_quests_groups_and_the_codex() {
        use thedas_sim::journal::*;
        let sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let quest = |r: &str, parent: Option<&str>| QuestView {
            resref: r.into(),
            name: None,
            description: None,
            completed: false,
            group: "plt_pre_plot_group".into(),
            parent: parent.map(str::to_string),
            seq: 0,
            updated: r == "plt_tower_sub",
        };
        let shown = Presentation {
            journal: Some(JournalView {
                groups: vec![QuestGroupView {
                    resref: "plt_pre_plot_group".into(),
                    name: None,
                    priority: 2,
                }],
                active: vec![
                    quest("plt_tower", None),
                    quest("plt_tower_sub", Some("plt_tower")),
                ],
                completed: vec![quest("plt_joining", None)],
                codex: vec![CodexGroupView {
                    kind: 7,
                    name: None,
                    entries: vec![
                        CodexEntryView {
                            resref: "plt_cod_cha_duncan".into(),
                            name: None,
                            texts: vec![1, 2],
                            updated: true,
                        },
                        CodexEntryView {
                            resref: "plt_cod_cha_cailan".into(),
                            name: None,
                            texts: vec![],
                            updated: false,
                        },
                    ],
                }],
                active_quest: Some("plt_tower".into()),
                codex_updated: true,
            }),
            journal_focus: Some(JournalFocus {
                codex: false,
                resref: "plt_tower_sub".into(),
                group: "plt_pre_plot_group".into(),
                completed: false,
            }),
            ..Default::default()
        };
        let v = sim_values_for(&sim, &shown);
        // New entries are marked; a clicked notification's quest is the one to open on.
        assert_eq!(v["Journal.PlotList.1.Updated"], num(1));
        assert_eq!(v["Journal.PlotList.0.Updated"], num(0));
        assert_eq!(v["Journal.CodexGroups.0.CodexList.0.Updated"], num(1));
        assert_eq!(v["Journal.CodexUpdated"], num(1));
        assert_eq!(
            v["GUIJournal.NotificationQuest"],
            UiValue::String("plt_tower_sub".into())
        );
        assert_eq!(
            v["GUIJournal.NotificationQuestGroup"],
            UiValue::String("plt_pre_plot_group".into())
        );
        assert_eq!(
            v["GUIJournal.NotificationCodex"],
            UiValue::String(String::new())
        );
        let s = |x: &str| UiValue::String(x.into());
        assert_eq!(v["Journal.PlotList"], s("plt_tower,plt_tower_sub"));
        assert_eq!(v["Journal.PlotList.1.ParentPlot"], s("plt_tower"));
        assert_eq!(v["Journal.PlotList.0.ParentPlot"], s(""));
        assert_eq!(v["Journal.PlotList.0.PlotGroup"], s("plt_pre_plot_group"));
        assert_eq!(v["Journal.CompletedPlots.0.ResRef"], s("plt_joining"));
        assert_eq!(v["Journal.PlotGroups.0.ResRef"], s("plt_pre_plot_group"));
        assert_eq!(v["Journal.PlotGroups.0.Priority"], num(2));
        assert_eq!(v["Journal.CodexGroups.0.CodexGroup"], num(7));
        assert_eq!(v["Journal.CodexGroups.0.CodexList.0.Locked"], num(0));
        assert_eq!(v["Journal.CodexGroups.0.CodexList.1.Locked"], num(1));
        assert_eq!(v["Journal.CodexGroups.0.CodexList.1.Index"], s("2"));
        assert!(v.contains_key("Journal.CodexGroups.0.CodexList.0.Descriptions.1"));
        assert_eq!(v["Journal.ActivePlot.ResRef"], s("plt_tower"));
        assert_eq!(v["GUIJournal.NotificationOpenPRCTab"], num(0));
        // No journal up: nothing bound.
        assert!(!sim_values(&sim).contains_key("Journal.PlotList"));
    }

    #[test]
    fn the_hud_lists_the_journal_and_codex_notifications() {
        use thedas_sim::journal::{ALERT_CODEX, ALERT_QUEST, Alert};
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        assert_eq!(
            sim_values(&sim)["Party.Alerts"],
            UiValue::String(String::new())
        );
        sim.world.alerts = vec![
            Alert {
                kind: ALERT_QUEST,
                resref: "plt_bhn100pt_father".into(),
                name: None,
                completed: true,
            },
            Alert {
                kind: ALERT_CODEX,
                resref: "plt_cod_cha_duncan".into(),
                name: None,
                completed: false,
            },
        ];
        let v = sim_values(&sim);
        assert_eq!(
            v["Party.Alerts"],
            UiValue::String("1:plt_bhn100pt_father:1,2:plt_cod_cha_duncan:0".into())
        );
        assert_eq!(v["Party.Alerts.0.Type"], num(ALERT_QUEST));
        assert_eq!(v["Party.Alerts.0.QuestCompleted"], num(1));
        assert_eq!(v["Party.Alerts.0.New"], num(1));
        assert_eq!(
            v["Party.Alerts.1.QuestResRef"],
            UiValue::String("plt_cod_cha_duncan".into())
        );
        assert_eq!(v["Party.Alerts.1.Type"], num(ALERT_CODEX));
    }

    #[test]
    fn the_area_map_centres_on_the_party_member_the_camera_follows() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        assert!(!sim_values(&sim).contains_key("GameModeExplore.CameraPivotPositionX"));
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        sim.world.get_mut(hero).unwrap().position = [12.5, -3.0, 1.0];
        let v = sim_values(&sim);
        assert_eq!(v["GameModeExplore.CameraPivotPositionX"], num(12.5));
        assert_eq!(v["GameModeExplore.CameraPivotPositionY"], num(-3.0));
    }
}
