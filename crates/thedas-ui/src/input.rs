//! What the interface's messages do to the simulation: the player's input through the game's
//! own screens (picking a conversation reply, accepting a job, travelling on the world map).
//! Movies only send messages; the simulation acts on them here, as input, the way it acts on
//! a click in the world.

use std::sync::{Arc, Mutex};

use thedas_sim::Sim;

use crate::UiValue;
use crate::binding::Presentation;
use crate::protocol::{GameMessage, MessageHandler};

/// conversation.gfx's reply pick: the reply's index.
pub const SELECT_RESPONSE: &str = "Conversation.SelectResponse";
/// characterrecord.gfx's and portraits.gfx's level-up button: the creature and 0.
pub const GO_TO_LEVEL_UP: &str = "GUI.GoToLevelUp";
/// quickbar.gfx's slot use: the ability, the party member and the item it comes from.
pub const PERFORM_ABILITY: &str = "GameModeController.PerformAbility";

/// An ability waiting for its target (a quickbar ability that is not used on its user).
#[derive(Clone, Debug, PartialEq)]
pub struct Aim {
    pub caster: u32,
    pub ability: i32,
    /// The tag of the item it comes from, if any.
    pub item: String,
}

/// `who` uses quickbar `slot` (its key was pressed): an ability used on its user goes at
/// once; one that needs a target is returned to be aimed. Nothing in the slot, nothing.
pub fn use_quickslot(sim: &mut Sim, who: u32, slot: i32) -> Option<Aim> {
    let (_, q) = sim.quickbar(who).into_iter().find(|(s, _)| *s == slot)?;
    if sim.ability_on_self(q.ability) {
        sim.input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::OrderAbility {
                who,
                ability: q.ability,
                target: who,
                item: q.item.to_string(),
            },
        );
        return None;
    }
    Some(Aim {
        caster: who,
        ability: q.ability,
        item: q.item,
    })
}

/// The ability a [`PERFORM_ABILITY`] message asks a party member to use.
pub fn aim(sim: &Sim, name: &str, args: &[String]) -> Option<Aim> {
    if name != PERFORM_ABILITY {
        return None;
    }
    let int = |i: usize| {
        args.get(i)
            .and_then(|a| a.trim().parse::<f64>().ok())
            .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v < u32::MAX as f64)
            .map(|v| v as u32)
    };
    let (ability, caster) = (int(0)?, int(1)?);
    if ability == 0 || !thedas_sim::engine::party_of(sim).contains(&caster) {
        return None;
    }
    let item = int(2)
        .and_then(|i| sim.world.get(i))
        .map(|o| o.tag.clone())
        .unwrap_or_default();
    Some(Aim {
        caster,
        ability: ability as i32,
        item,
    })
}

/// The player picks reply `n` of the conversation on screen; continuing past the last line
/// of one that ended closes it (the conversation screen goes with it). Returns whether the
/// conversation closed.
pub fn reply(sim: &mut Sim, n: usize) -> bool {
    if sim.dialog_state().is_some_and(|d| d.ended) {
        sim.input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::EndConversation,
        );
        return true;
    }
    sim.input(
        thedas_sim::commands::CommandSource::Player,
        thedas_sim::commands::SimCommand::ChooseReply { choice: n },
    );
    false
}

/// The inventory's and the character record's gift action (the item's radial menu): the
/// item and the party member it goes to.
pub const GIFT_ITEM: &str = "GameModeController.GiftItem";

/// An item given to a follower (`GameModeController.GiftItem`): the follower hears
/// `EVENT_TYPE_GIFT_ITEM`, object 0 the item, from the member the player controls, as
/// DAOrigins.exe's GiftItemMessage handler sends it (0x718210: target the message's first
/// object, object 0 its second). The follower's scripts take the item and change the
/// approval. Provisional: the arguments are told apart by kind (the creature and the item),
/// since the movie's argument order is not traced.
fn gift_item(sim: &mut Sim, args: &[String]) -> bool {
    use thedas_sim::world::ObjectKind;
    let ids: Vec<u32> = args
        .iter()
        .filter_map(|a| a.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .map(|v| v as u32)
        .collect();
    let of = |kind: ObjectKind| {
        ids.iter()
            .copied()
            .find(|&id| sim.world.get(id).is_some_and(|o| o.kind == kind))
    };
    let (Some(to), Some(item)) = (of(ObjectKind::Creature), of(ObjectKind::Item)) else {
        return false;
    };
    let from = sim.world.controlled().or(sim.world.hero);
    let mut ev = sim.new_event("GIFT_ITEM", from);
    ev.objects = vec![item];
    sim.world.queue_event(0, to, ev, None);
    true
}

/// The message a script message box's buttons send (DAOrigins.exe's callback format at
/// 0xb07fd8, `GUIPopupLayer.ScriptDialogResult %d %d %d`).
pub const SCRIPT_DIALOG_RESULT: &str = "GUIPopupLayer.ScriptDialogResult";

/// Acts on a message a movie sent: a conversation reply is picked (continuing past the last
/// line of one that ended closes it), anything else goes to the simulation's interface
/// handlers. Returns whether the simulation acted on it.
pub fn apply(sim: &mut Sim, name: &str, args: &[String]) -> bool {
    // A script's message box answered: its button's callback (`ScriptDialogResult <popup
    // type> <button, from 1> <owner>`, see `binding::script_popup`), to which the app adds
    // the text typed into the box (argument 3).
    if name == SCRIPT_DIALOG_RESULT {
        let button = args
            .get(1)
            .and_then(|a| a.trim().parse::<f64>().ok())
            .map_or(1, |b| b as i32);
        let text = args.get(3).map(String::as_str).unwrap_or("");
        let answered = sim.input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::AnswerPopup {
                button,
                text: text.to_string(),
            },
        );
        sim.input(
            thedas_sim::commands::CommandSource::App,
            thedas_sim::commands::SimCommand::Tick { ms: 0 },
        );
        return answered;
    }
    if name == GIFT_ITEM {
        return gift_item(sim, args);
    }
    if name == SELECT_RESPONSE {
        let n = args
            .first()
            .and_then(|a| a.parse::<usize>().ok())
            .unwrap_or(0);
        reply(sim, n);
        sim.input(
            thedas_sim::commands::CommandSource::App,
            thedas_sim::commands::SimCommand::Tick { ms: 0 },
        );
        return true;
    }
    // An ability used on its user goes at once; one that needs a target waits for the
    // player to pick it (the message is passed back).
    if let Some(a) = aim(sim, name, args) {
        if !sim.ability_on_self(a.ability) {
            return false;
        }
        sim.input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::OrderAbility {
                who: a.caster,
                ability: a.ability,
                target: a.caster,
                item: a.item.to_string(),
            },
        );
        return true;
    }
    // The level-up button (character record, a portrait's level-up arrow): the creature and
    // 0. The game asks the module to open level-up for it (`EVENT_TYPE_PLAYERLEVELUP`).
    if name == GO_TO_LEVEL_UP {
        let who = args
            .first()
            .and_then(|a| a.trim().parse::<f64>().ok())
            .filter(|v| v.fract() == 0.0 && *v > 0.0 && *v < u32::MAX as f64)
            .map(|v| v as u32)
            .filter(|w| thedas_sim::engine::party_of(sim).contains(w));
        let Some(who) = who else { return false };
        sim.input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::RequestLevelUp { creature: who },
        );
        sim.input(
            thedas_sim::commands::CommandSource::App,
            thedas_sim::commands::SimCommand::Tick { ms: 0 },
        );
        return true;
    }
    // An appearance slider moved (`GUICharGen.AppearanceSliders.N.Value`, value): the
    // face of the creature being created.
    if name == thedas_sim::appearance::MSG_SET_SLIDER {
        let index = args.first().and_then(|k| {
            k.strip_prefix("GUICharGen.AppearanceSliders.")?
                .strip_suffix(".Value")?
                .parse::<usize>()
                .ok()
        });
        let value = args.get(1).and_then(|v| v.trim().parse::<f32>().ok());
        let (Some(index), Some(value), Some(who)) = (index, value, sim.world.chargen) else {
            return false;
        };
        let Some(slider) = crate::appearance::slider_at(sim, index) else {
            return false;
        };
        sim.input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::SetFaceChoice {
                creature: who,
                slider,
                value,
            },
        );
        return true;
    }
    if sim.input(
        thedas_sim::commands::CommandSource::Player,
        thedas_sim::commands::SimCommand::GuiMessage {
            name: name.to_string(),
            args: args.to_vec(),
        },
    ) {
        // (Loading a tactics preset raises `EVENT_TYPE_TRAINING_TACTIC_PRESET_SELECTED` in
        // the simulation, where the preset is applied: `Sim::gui_message`'s LoadPreset.)
        sim.input(
            thedas_sim::commands::CommandSource::App,
            thedas_sim::commands::SimCommand::Tick { ms: 0 },
        );
        return true;
    }
    false
}

/// Acts on the messages a tick returned that nothing handled yet; returns the ones the
/// simulation did not take (name and arguments).
pub fn apply_all(sim: &mut Sim, messages: &[GameMessage]) -> Vec<(String, Vec<String>)> {
    let mut ignored = Vec::new();
    for m in messages {
        if let GameMessage::Send(name, args) = m {
            if !apply(sim, name, args) {
                ignored.push((name.clone(), args.clone()));
            }
        }
    }
    ignored
}

/// inventory.gfx's category tab: the engine lists only that category's items.
pub const SET_ITEM_FILTER: &str = "GUIInventory.SetItemListFilter";

/// A screen's inspect action (inventory.gfx's `ItemUtils.ExamineItem`): the item's token,
/// which side to show on and whether to check the requirements.
pub const SHOW_ITEM_EXAMINE: &str = "GUIItemExamine.ShowItemExamine";
/// A movie closing a screen of its own by name (itemexamine.gfx: `ItemExamine`).
pub const HIDE_GUI_SCREEN: &str = "GUI.HideGUIScreen";
pub const ITEM_EXAMINE_SCREEN: &str = "ItemExamine";
/// The inventory's and character record's paperdoll turn buttons (DAOrigins.exe's
/// `RotatePaperdollLeftMessage` and `RotatePaperdollRightMessage`, handled by `GUIPaperdoll`).
pub const PAPERDOLL_LEFT: &str = "Paperdoll.RotatePaperdollLeft";
pub const PAPERDOLL_RIGHT: &str = "Paperdoll.RotatePaperdollRight";
/// The paperdoll dragged with the mouse: inventory.gfx sends the drag's horizontal and
/// vertical movement times -0.8 (`RotatePaperdollCameraMessage`). Provisional: taken as
/// degrees about the member; the vertical part (the camera's pitch) is not used.
pub const PAPERDOLL_DRAG: &str = "Paperdoll.RotatePaperdollCamera";
/// The paperdoll's camera preset chosen (chargen.gfx: 1 on the appearance page), and the
/// portrait asked to be drawn again (see [`crate::binding::Presentation`]).
pub const PAPERDOLL_PRESET: &str = "Paperdoll.SetCameraPreset";
pub const PAPERDOLL_PORTRAIT: &str = "Paperdoll.UpdatePortrait";
/// Character creation opening (`GUICharGen.CharGenStart`): the paperdoll starts on camera
/// preset 0 (0x7bbc4d). The simulation takes the message too.
pub const CHARGEN_START: &str = "GUICharGen.CharGenStart";
/// How far one press turns the paperdoll (radians). Provisional: the game's step is not
/// traced.
pub const PAPERDOLL_TURN: f32 = std::f32::consts::PI / 8.0;

/// Takes the interface's own messages (not simulation input) into `shown`; returns whether
/// `name` was one.
pub fn present(shown: &mut Presentation, name: &str, args: &[String]) -> bool {
    if crate::options::option_message(&mut shown.options, name, args) {
        return true;
    }
    match name {
        SHOW_ITEM_EXAMINE => {
            let int = |i: usize| {
                args.get(i)
                    .and_then(|a| a.trim().parse::<f64>().ok())
                    .map_or(0, |v| v as i32)
            };
            shown.examine = args
                .first()
                .map(|t| t.trim())
                .filter(|t| !t.is_empty())
                .map(|t| crate::binding::Examine {
                    token: t.to_string(),
                    right_side: int(1),
                    check_requirements: int(2),
                });
            true
        }
        HIDE_GUI_SCREEN if args.first().map(|a| a.trim()) == Some(ITEM_EXAMINE_SCREEN) => {
            shown.examine = None;
            true
        }
        PAPERDOLL_LEFT | PAPERDOLL_RIGHT => {
            let turn = if name == PAPERDOLL_LEFT {
                -PAPERDOLL_TURN
            } else {
                PAPERDOLL_TURN
            };
            shown.paperdoll_yaw = (shown.paperdoll_yaw + turn) % std::f32::consts::TAU;
            true
        }
        PAPERDOLL_DRAG => {
            let degrees = args
                .first()
                .and_then(|a| a.trim().parse::<f32>().ok())
                .filter(|d| d.is_finite())
                .unwrap_or(0.0);
            shown.paperdoll_yaw =
                (shown.paperdoll_yaw + degrees.to_radians()) % std::f32::consts::TAU;
            true
        }
        PAPERDOLL_PRESET => {
            shown.paperdoll_preset = args
                .first()
                .and_then(|a| a.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .map_or(0, |v| v as i32);
            true
        }
        PAPERDOLL_PORTRAIT => {
            shown.portrait_requests = shown.portrait_requests.wrapping_add(1);
            true
        }
        CHARGEN_START => {
            shown.paperdoll_preset = 0;
            false
        }
        SET_ITEM_FILTER => {
            shown.item_filter = args
                .first()
                .and_then(|a| a.trim().parse::<f64>().ok())
                .map_or(0, |v| v as i32);
            true
        }
        _ => false,
    }
}

/// Answers the movies' questions about `sim` (see [`crate::protocol::QueryHandler`]):
/// tactics.gfx's pickers (`DAExternalCommands.ListTactic*`, from
/// [`Sim::tactic_targets`] and its neighbours) and the names it shows for party members
/// (`GetObjectNameByID`). Unanswered while the simulation is busy elsewhere.
pub fn query_for(sim: Arc<Mutex<Sim>>) -> crate::protocol::QueryHandler {
    Arc::new(move |name: &str, args: &[String]| {
        let sim = sim.try_lock().ok()?;
        let answer = query(&sim, name, args);
        log::debug!("query {name} {args:?}: {answer:?}");
        Some(answer)
    })
}

/// [`query_for`]'s answers.
pub fn query(sim: &Sim, name: &str, args: &[String]) -> UiValue {
    let int = |i: usize| {
        args.get(i)
            .and_then(|a| a.trim().parse::<f64>().ok())
            .filter(|v| v.is_finite())
            .map_or(0, |v| v as i64)
    };
    let list =
        |v: Vec<i64>| UiValue::List(v.into_iter().map(|x| UiValue::Number(x as f64)).collect());
    let ints = |v: Vec<i32>| list(v.into_iter().map(i64::from).collect());
    let who = u32::try_from(int(0)).unwrap_or(0);
    let pair = |a: &str, x: Vec<i32>, ids: Vec<i64>| {
        UiValue::Object(
            [(a.to_string(), ints(x)), ("IDs".to_string(), list(ids))]
                .into_iter()
                .collect(),
        )
    };
    match name {
        "ListTacticTargets" => {
            let (t, ids) = sim.tactic_targets(who);
            pair("Targets", t, ids)
        }
        "ListTacticBaseConditions" => ints(sim.tactic_base_conditions(int(0) as i32)),
        "ListTacticConditions" => {
            let (c, ids) = sim.tactic_conditions(int(0) as i32, int(1) as i32);
            pair("Conditions", c, ids)
        }
        "ListTacticActions" => ints(sim.tactic_actions(who, int(1) as i32, int(2) as i32)),
        "ListTacticAbilities" => ints(sim.tactic_abilities(who, int(1) as i32, int(2) as i32)),
        "ListTacticModalAbilities" => ints(sim.tactic_modal_abilities(who)),
        "ListTacticItems" => list(
            sim.tactic_items(int(0) as i32)
                .into_iter()
                .map(i64::from)
                .collect(),
        ),
        "GetObjectNameByID" => UiValue::String(sim.display_name(who)),
        // `GetGender(creature)`: its gender (`Gender`, a `gender.gda` row); the character
        // record picks the race icon's column by it.
        "GetGender" => UiValue::Number(
            sim.world
                .get(who)
                .and_then(|o| o.fields.get("Gender").copied())
                .unwrap_or(0)
                .into(),
        ),
        // `HasAbility(creature, ability)`: whether the creature has learned the ability
        // (characterrecord.gfx shows a class's rank icon when the creature has the class's
        // starting ability).
        // No ability (0) is never had: the character record asks for every class row,
        // and the rows with none (the invalid class, the monster classes) showed as
        // ranks the hero held (a column of dog icons running off the page).
        "HasAbility" => UiValue::Bool(
            int(1) != 0
                && sim
                    .world
                    .get(who)
                    .is_some_and(|o| o.abilities.contains(&(int(1) as i32))),
        ),
        // `CreatureCanUseItem(item, creature)`: whether the creature may use the item;
        // inventory.gfx dims the rows of items it cannot (`ItemEquippable_mc`). The exe's
        // test (0x6ae8b0) checks the item's requirements against the creature. Provisional:
        // requirements are not modelled yet, so any item and creature that exist pass.
        "CreatureCanUseItem" => UiValue::Bool(
            sim.world.get(who).is_some()
                && u32::try_from(int(1))
                    .ok()
                    .and_then(|c| sim.world.get(c))
                    .is_some(),
        ),
        _ => UiValue::Undefined,
    }
}

/// A bridge handler acting on `sim` as a movie sends a message, before the movie goes on (a
/// movie reads values right after some messages, as chanters.gfx does after accepting a
/// job). When the simulation is busy elsewhere the message waits for [`apply_all`].
pub fn handler(sim: Arc<Mutex<Sim>>) -> MessageHandler {
    handler_for(sim, Arc::new(Mutex::new(Presentation::default())))
}

/// [`handler`] binding values with the interface's own choices (`shown`), which it keeps up
/// to date from the movies' messages.
pub fn handler_for(sim: Arc<Mutex<Sim>>, shown: Arc<Mutex<Presentation>>) -> MessageHandler {
    Arc::new(move |name: &str, args: &[String]| {
        let mut p = shown.lock().ok()?;
        if present(&mut p, name, args) {
            let sim = sim.try_lock().ok()?;
            return Some(
                crate::binding::sim_values_for(&sim, &p)
                    .into_iter()
                    .collect(),
            );
        }
        let mut sim = sim.try_lock().ok()?;
        // Replies are picked after the tick, as the MCP does: the movie does not read the
        // next line back in the same call. Dismissed notifications are dropped after the
        // frame too: notifications.gfx names its alerts by their place in `Party.Alerts`,
        // and an alert gone mid-frame shifted the list under a click being handled (a quest
        // alert's click read the codex alert's plot). The tactics screen's messages wait for
        // the slot edits it wrote just before them (those reach the game after the frame):
        // binding now would put the old slots back under the movie.
        if name == SELECT_RESPONSE
            || name == thedas_sim::journal::MSG_HIDE_ALERT
            || name.starts_with(thedas_sim::tactics::PREFIX)
            || !sim.input(
                thedas_sim::commands::CommandSource::Player,
                thedas_sim::commands::SimCommand::GuiMessage {
                    name: name.to_string(),
                    args: args.to_vec(),
                },
            )
        {
            return None;
        }
        sim.input(
            thedas_sim::commands::CommandSource::App,
            thedas_sim::commands::SimCommand::Tick { ms: 0 },
        );
        Some(
            crate::binding::sim_values_for(&sim, &p)
                .into_iter()
                .collect(),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gift_goes_to_the_follower_as_gift_item() {
        use thedas_sim::world::ObjectKind;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        let alistair = sim.world.spawn(ObjectKind::Creature, "alistair");
        let item = sim.world.spawn(ObjectKind::Item, "gift");
        // The order the movie sends them in does not matter.
        assert!(apply(
            &mut sim,
            GIFT_ITEM,
            &[item.to_string(), alistair.to_string()]
        ));
        let queued: Vec<_> = sim
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.objects.clone(), q.event.creator))
            .collect();
        assert!(queued.contains(&(alistair, vec![item], Some(hero))));
        assert!(!apply(&mut sim, GIFT_ITEM, &[item.to_string()]));
    }

    #[test]
    fn a_script_message_box_shows_and_its_button_answers_it() {
        use thedas_formats::gda::Cell;
        let mut sim = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        sim.world.tables().insert(
            "#207",
            &["#335f4422", "#187217e1", "#016926a0", "#4e28b067"],
            vec![(
                1,
                vec![Cell::Int(10), Cell::Int(11), Cell::Int(0), Cell::Int(0)],
            )],
        );
        let owner = sim
            .world
            .spawn(thedas_sim::world::ObjectKind::Creature, "asker");
        sim.world.popup = Some(thedas_sim::world::Popup {
            strref: 5,
            kind: 1,
            owner,
            input: false,
        });
        // Two labels (no talk table here, so they read as nothing to show): the buttons
        // follow the labels the table names.
        let v = crate::binding::sim_values(&sim);
        assert_eq!(
            v["GUIPopupLayer.PickerList"],
            UiValue::String(String::new())
        );
        assert!(v.contains_key("GUIPopupLayer.NumButtons"));
        let callback = format!("{SCRIPT_DIALOG_RESULT} 1 2 {owner}");
        let mut words = callback.split(' ').map(str::to_string);
        let name = words.next().unwrap();
        assert!(apply(&mut sim, &name, &words.collect::<Vec<_>>()));
        assert!(sim.world.popup.is_none());
        assert!(!crate::binding::sim_values(&sim).contains_key("GUIPopupLayer.NumButtons"));
    }

    #[test]
    fn the_examine_screen_opens_on_an_item_and_closes_itself() {
        let mut p = Presentation::default();
        let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let token = "GUIInventory.PartyBackpackView.2.Item";
        assert!(present(&mut p, SHOW_ITEM_EXAMINE, &a(&[token, "1", "0"])));
        assert_eq!(
            p.examine,
            Some(crate::binding::Examine {
                token: token.into(),
                right_side: 1,
                check_requirements: 0,
            })
        );
        assert!(!present(&mut p, HIDE_GUI_SCREEN, &a(&["Journal"])));
        assert!(p.examine.is_some());
        assert!(present(&mut p, HIDE_GUI_SCREEN, &a(&["ItemExamine"])));
        assert!(p.examine.is_none());
    }

    #[test]
    fn the_paperdoll_turns_with_its_buttons_and_drags() {
        let mut p = Presentation::default();
        let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(present(&mut p, PAPERDOLL_RIGHT, &[]));
        assert_eq!(p.paperdoll_yaw, PAPERDOLL_TURN);
        assert!(present(&mut p, PAPERDOLL_LEFT, &[]));
        assert!(present(&mut p, PAPERDOLL_DRAG, &a(&["-8", "4"])));
        assert!((p.paperdoll_yaw + 8f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn the_paperdoll_camera_preset_and_portrait_requests_are_kept() {
        let mut p = Presentation::default();
        let a = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(present(&mut p, PAPERDOLL_PRESET, &a(&["1"])));
        assert_eq!(p.paperdoll_preset, 1);
        assert!(present(&mut p, PAPERDOLL_PORTRAIT, &[]));
        assert_eq!(p.portrait_requests, 1);
        // Character creation opening starts on the body preset, and still reaches the
        // simulation.
        assert!(!present(&mut p, CHARGEN_START, &a(&["0", "0"])));
        assert_eq!(p.paperdoll_preset, 0);
    }

    fn sim() -> Sim {
        Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::default()),
            1,
        )
    }

    #[test]
    fn continuing_past_the_last_line_closes_the_conversation() {
        use thedas_sim::dialog::{Conversation, Dlg};
        let mut s = sim();
        let hero = s
            .world
            .spawn(thedas_sim::world::ObjectKind::Creature, "hero");
        let talk = |ended| Conversation {
            dlg: Dlg {
                name: "test".into(),
                starts: Vec::new(),
                nodes: Vec::new(),
            },
            owner: hero,
            player: hero,
            line: None,
            replies: Vec::new(),
            ended,
            shown: Vec::new(),
            chain: Vec::new(),
            cut_from: None,
            cut_lines_left: 0,
            staged: Vec::new(),
            staged_at: None,
            ambient_run: false,
        };
        // Still going with nothing to pick: a reply leaves it be.
        let world = s.world.clone();
        s.restore((world.clone(), Some(talk(false))));
        assert!(!reply(&mut s, 0));
        assert!(s.dialog_state().is_some());
        // Its last line shown: any reply, as the continue click, closes it.
        s.restore((world, Some(talk(true))));
        assert!(reply(&mut s, 3));
        assert!(s.dialog_state().is_none());
    }

    #[test]
    fn the_level_up_button_asks_the_module_for_a_party_members_level_up() {
        let table =
            thedas_script::EngineTable::parse(b"int EVENT_TYPE_PLAYERLEVELUP = 64;\n").unwrap();
        let mut s = Sim::new(Arc::new(thedas_vfs::Vfs::new()), Arc::new(table), 1);
        let module = s
            .world
            .spawn(thedas_sim::world::ObjectKind::Module, "module");
        s.world.module = Some(module);
        let hero = s
            .world
            .spawn(thedas_sim::world::ObjectKind::Creature, "hero");
        let stranger = s
            .world
            .spawn(thedas_sim::world::ObjectKind::Creature, "stranger");
        s.world.hero = Some(hero);
        s.world.trace.keep = true;
        assert!(!apply(
            &mut s,
            GO_TO_LEVEL_UP,
            &[stranger.to_string(), "0".into()]
        ));
        assert!(apply(
            &mut s,
            GO_TO_LEVEL_UP,
            &[hero.to_string(), "0".into()]
        ));
        // The module was told (its chargen script would open the screen).
        assert!(s.world.trace.entries.iter().any(|e| matches!(e,
            thedas_sim::trace::TraceEntry::EventQueued { target, ty: 64, .. } if *target == module)));
    }

    #[test]
    fn unknown_messages_are_passed_back() {
        let mut s = sim();
        let left = apply_all(
            &mut s,
            &[
                GameMessage::Send("Nobody.Listens".into(), vec!["1".into()]),
                GameMessage::Handled("Already.Done".into(), vec![]),
            ],
        );
        assert_eq!(left, vec![("Nobody.Listens".to_string(), vec!["1".into()])]);
    }

    #[test]
    fn quickbar_abilities_go_at_once_on_self_and_wait_for_a_target_otherwise() {
        use thedas_formats::gda::Cell;
        let mut s = sim();
        s.world.tables().insert(
            "ABI_base",
            &["TargetType"],
            vec![(700, vec![Cell::Int(1)]), (3024, vec![Cell::Int(4)])],
        );
        let hero = s
            .world
            .spawn(thedas_sim::world::ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let args = |a: i32| vec![a.to_string(), hero.to_string(), "0".to_string()];
        // On its user: ordered now.
        assert!(apply(&mut s, PERFORM_ABILITY, &args(700)));
        let q = &s.world.get(hero).unwrap().commands;
        assert_eq!((q[0].ints.clone(), q[0].target), (vec![700], Some(hero)));
        // On a foe: passed back to be aimed.
        assert!(!apply(&mut s, PERFORM_ABILITY, &args(3024)));
        assert_eq!(
            aim(&s, PERFORM_ABILITY, &args(3024)),
            Some(Aim {
                caster: hero,
                ability: 3024,
                item: String::new()
            })
        );
        // Not a party member: nothing.
        assert_eq!(
            aim(&s, PERFORM_ABILITY, &["3024".into(), "999".into()]),
            None
        );
    }

    #[test]
    fn the_item_tab_is_the_interfaces_own() {
        let mut shown = Presentation::default();
        assert!(present(&mut shown, SET_ITEM_FILTER, &["3".into()]));
        assert_eq!(shown.item_filter, 3);
        assert!(!present(&mut shown, SELECT_RESPONSE, &["0".into()]));
    }

    #[test]
    fn the_handler_leaves_replies_for_after_the_tick() {
        let h = handler(Arc::new(Mutex::new(sim())));
        assert!(h(SELECT_RESPONSE, &["0".into()]).is_none());
    }
}
