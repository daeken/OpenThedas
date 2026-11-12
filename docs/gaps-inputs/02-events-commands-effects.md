# 02: Events, commands, effects

Gathered 2026-11-12 on main 1eb844a. No game text.

## Method (heuristic)

- Names and values: EVENT_TYPE_* and COMMAND_TYPE_* come from `script.ldf` (111 event types, 21 command types).
  `script.ldf` declares only EFFECT_TYPE_INVALID. Effect types are rows of `effects.gda`: 81 ids, named by
  their Label column. The scripts' own constant headers (.nss) are not in the install, so script-defined event
  types have no names here.
- Script references: all 4158 distinct compiled scripts were disassembled with `thedas script disasm`. A
  stack-tracking pass follows each value returned by GetEventType, GetEffectType and GetCommandType through
  locals and switch duplicates, and counts the EQUALII and NEQUALII comparisons against an integer constant.
  It also counts `CONSTI v` directly before `Event(` or `Effect(` as a construction site. These are static
  counts. The tracking gives up after 1688 calls to ids that script.ldf does not declare, so the counts may be slightly low.
- Sim handling: an event type counts as handled when `EVENT_TYPE_X` or the bare `"X"` string appears in
  crates/thedas-sim, thedas-app, thedas-ui or thedas-mcp. Commands and effects were checked by reading sim.rs, combat.rs, aoe.rs and engine.rs.

## Events

Script-defined events (values not in script.ldf, sent by scripts through SignalEvent or DelayEvent, which the sim implements):
73 distinct values at 5978 Event() sites, and 74 values compared at 1820 sites. The sim delivers them generically;
their handling lives in the scripts.

### Event types the scripts use that the sim never raises

| event | handler scripts comparing it | Event() sites in scripts |
| --- | --- | --- |
| EVENT_TYPE_DAMAGED | 180 | 149 |
| EVENT_TYPE_TRAP_TRIGGERED | 64 | 0 |
| EVENT_TYPE_PLACEABLE_ONCLICK | 64 | 0 |
| EVENT_TYPE_PLACEABLE_COLLISION | 56 | 0 |
| EVENT_TYPE_UNLOCKED | 35 | 0 |
| EVENT_TYPE_USE_PLOTACTION | 5 | 6 |
| EVENT_TYPE_AREALOADSAVE_PRELOADEXIT | 10 | 0 |
| EVENT_TYPE_AREALOADSAVE_POSTLOADEXIT | 7 | 0 |
| EVENT_TYPE_ITEM_ONHIT | 6 | 1 |
| EVENT_TYPE_TRAINING_BEGIN | 1 | 0 |
| EVENT_TYPE_MODULE_PRESAVE | 4 | 0 |
| EVENT_TYPE_SPELLCASTAT | 3 | 0 |
| EVENT_TYPE_GIFT_ITEM | 3 | 0 |
| EVENT_TYPE_GUI_OPENED | 2 | 0 |
| EVENT_TYPE_COMBAT_INITIATED | 1 | 0 |
| EVENT_TYPE_AREALOADSAVE_SPECIAL | 1 | 0 |
| EVENT_TYPE_CHARGEN_IMPORT_HERO | 1 | 0 |
| EVENT_TYPE_MANA_STAM_DEPLETED | 1 | 0 |
| EVENT_TYPE_ITEM_ONTEST_USABLE | 1 | 0 |
| EVENT_TYPE_WORLD_MAP_CLOSED | 1 | 0 |
| EVENT_TYPE_LOAD_TACTICS_PRESET | 1 | 0 |
| EVENT_TYPE_RUBBER_BAND | 1 | 0 |
| EVENT_TYPE_GIVE_UP | 1 | 0 |
| EVENT_TYPE_ON_SELECT | 1 | 0 |
| EVENT_TYPE_ON_ORDER_RECEIVED | 1 | 0 |
| EVENT_TYPE_PLAYER_COMMAND_ADDED | 1 | 0 |
| EVENT_TYPE_USE_ABILITY_IMMEDIATE | 1 | 0 |
| EVENT_TYPE_TRAINING_FOLLOWER_SELECTED | 1 | 0 |
| EVENT_TYPE_TRAINING_GUI_TACTICS_OPENED | 1 | 0 |
| EVENT_TYPE_TRAINING_GUI_LEVLUP_OPENED | 1 | 0 |
| EVENT_TYPE_TRAINING_GUI_INVENTORY_OPENED | 1 | 0 |
| EVENT_TYPE_TRAINING_MAXIMUM_ZOOM_IN | 1 | 0 |
| EVENT_TYPE_TRAINING_TACTIC_PRESET_SELECTED | 1 | 0 |
| EVENT_TYPE_TRAINING_MOVEMENT_COMMAND_ISSUED | 1 | 0 |

Named in script.ldf, never raised by the sim, and with no comparison or Event() site the analysis found (27). The analysis may have lost track of them (for example a comparison in a helper function), or the scripts may never use them: EVENT_TYPE_MELEE_ATTACK_START, EVENT_TYPE_BLOCKED, EVENT_TYPE_FAILTOOPEN, EVENT_TYPE_CLICK, EVENT_TYPE_TRAP_DISARMED, EVENT_TYPE_LISTENER, EVENT_TYPE_LOCKED, EVENT_TYPE_MODULE_GETCHARSTAGE, EVENT_TYPE_GAMEOBJECTSLOADED, EVENT_TYPE_CHARGEN_SCREEN_ENTERED, EVENT_TYPE_CHARGEN_SELECT_LEVELUP_CLASS, EVENT_TYPE_CHANTERS_DONATION, EVENT_TYPE_LEVEL_OF_THE_WEEK, EVENT_TYPE_ABILITY_ACQUIRED, EVENT_TYPE_REACHED_WAYPOINT, EVENT_TYPE_AREALIST_POSTLOAD, EVENT_TYPE_INVENTORY_FULL, EVENT_TYPE_CREATURE_ENTERS_DIALOGUE, EVENT_TYPE_CHAR_RECORD_OPENED, EVENT_TYPE_OPTIONS_CHANGED, EVENT_TYPE_PLOT_COMPLETED, EVENT_TYPE_PARTY_MONEY_CHANGED, EVENT_TYPE_CODEX_CHANGED, EVENT_TYPE_CRAFT_ITEM, EVENT_TYPE_TRAINING_QBAR_ITEM_EQUIPPED, EVENT_TYPE_TRAINING_WEAPON_EQUIPPED, EVENT_TYPE_TRAINING_DELAYED_NOACTION.

### Event types the sim raises or names (50)

EVENT_TYPE_SET_PLOT, EVENT_TYPE_GET_PLOT, EVENT_TYPE_DEATH, EVENT_TYPE_ENTER, EVENT_TYPE_USE, EVENT_TYPE_SPAWN, EVENT_TYPE_AREALOAD_PRELOADEXIT, EVENT_TYPE_AREALOAD_POSTLOADEXIT, EVENT_TYPE_EXIT, EVENT_TYPE_AREALOAD_SPECIAL, EVENT_TYPE_INVENTORY_REMOVED, EVENT_TYPE_COMMAND_COMPLETE, EVENT_TYPE_INVENTORY_ADDED, EVENT_TYPE_DIALOGUE, EVENT_TYPE_APPLY_EFFECT, EVENT_TYPE_REMOVE_EFFECT, EVENT_TYPE_PERCEPTION_APPEAR, EVENT_TYPE_PERCEPTION_DISAPPEAR, EVENT_TYPE_EQUIP, EVENT_TYPE_UNEQUIP, EVENT_TYPE_AOE_HEARTBEAT, EVENT_TYPE_ATTACK_IMPACT, EVENT_TYPE_COMMAND_PENDING, EVENT_TYPE_POPUP_RESULT, EVENT_TYPE_MODULE_START, EVENT_TYPE_INVALID, EVENT_TYPE_MODULE_LOAD, EVENT_TYPE_CHARGEN_END, EVENT_TYPE_ABILITY_CAST_START, EVENT_TYPE_BEGIN_TRAVEL, EVENT_TYPE_WORLDMAP_PRETRANSITION, EVENT_TYPE_PLAYERLEVELUP, EVENT_TYPE_ABILITY_CAST_IMPACT, EVENT_TYPE_CHARGEN_START, EVENT_TYPE_CHARGEN_SELECT_BACKGROUND, EVENT_TYPE_GAMEMODE_CHANGE, EVENT_TYPE_DEATH_RES_PARTY, EVENT_TYPE_HEARTBEAT2, EVENT_TYPE_WORLDMAP_POSTTRANSITION, EVENT_TYPE_CHARGEN_SELECT_RACE, EVENT_TYPE_CHARGEN_SELECT_CLASS, EVENT_TYPE_CHARGEN_SELECT_SOUNDSET, EVENT_TYPE_CHARGEN_SELECT_NAME, EVENT_TYPE_CHARGEN_ASSIGN_ATTRIBUTES, EVENT_TYPE_CHARGEN_ASSIGN_ABILITIES, EVENT_TYPE_CHARGEN_SELECT_GENDER, EVENT_TYPE_PARTYMEMBER_ADDED, EVENT_TYPE_PARTYMEMBER_DROPPED, EVENT_TYPE_PARTYPICKER_CLOSED, EVENT_TYPE_ABILITY_PROJECTILE_LAUNCHED.

EVENT_TYPE_DAMAGED: no engine-side raise in the sim, although the exe presumably raises it on damage (inferred). Some scripts
construct it themselves (149 sites).

## Commands

The sim executes 10 command kinds (sim.rs `command_duration` / completion): Attack, UseAbility, MoveToObject,
MoveToLocation, JumpToObject, JumpToLocation, Wait, StartConversation, SwitchWeaponSet, UseObject.

The rest are queued with a type and complete after the default 0.5 s, doing nothing (stored only; inferred from
`command_duration` falling through to "anything else takes half a second"):

| constructor | static call sites (scripts) | note |
| --- | --- | --- |
| CommandMoveAwayFromObject | 1010 (169) | type number invented (1000+), see engine.rs UNNAMED_COMMANDS |
| CommandPlayAnimation | 829 (386) | |
| CommandFly | 353 (176) | DLC scripts pass a second int |
| CommandMoveToMultiLocations | 323 (295) | type number invented; its name starts with "CommandMoveTo", but it carries a location[]; the sim probably treats it as a move to nowhere (inferred) |
| CommandTurn | 221 (214) | type number invented |
| CommandSheatheWeapons | 14 (14) | type number invented |
| CommandDeathBlow | 3 (3) | |
| CommandUnsheatheWeapons | 2 (1) | type number invented |
| CommandEquipItem, CommandUnequipItem | 0 | declared, never called |

COMMAND_TYPE_* comparisons against GetCommandType (sites / scripts): INVALID 5042/1197, ATTACK 195/193, WAIT 194/193,
USE_OBJECT 22/11, MOVE_TO_OBJECT 7/5, MOVE_TO_LOCATION 4/2, USE_ABILITY 2/1, DEATHBLOW 1/1, FLY 1/1. None of the other 12 are compared.
COMMAND_TYPE_DO_FUNCTION, DO_EVENT, INTERACT, DRIVE, RECOVER have no constructor in script.ldf.

## Effects

Script-side effect types (1001 and up) are applied by the scripts' own handlers through EVENT_TYPE_APPLY_EFFECT and
REMOVE_EFFECT, which the sim sends. The sim gives these engine-side behaviour of its own: AOE (5),
EFFECT_TYPE_DEATH (7), RESURRECTION (1005), DOT (1017), UPKEEP (1016), MODIFY_PROPERTY (1012), DAMAGE (1013) and
IMPACT (1024). EffectVisualEffect (4) is passed to the presentation. GetEffectsFlags reports effects.gda Flags to scripts.
The sim's movement, command and combat code never reads those flags (grep: no other use of `Flags`), so an
effect that disables movement or combat only works if a script checks it (inferred).

Engine-native types that the sim only stores:

| effect (id) | Effect() sites | compared |
| --- | --- | --- |
| EffectMovementSpeed (9) | 236 | 0 |
| EffectMovementSpeedDebuff (11) | 219 | 0 |
| EffectTransparency (10) | 98 | 0 |
| EffectScreenShake (6) | 80 | 0 |
| EffectParalyze (8) | 48 | 0 |
| EffectGeneric (1) | 1 | 0 |

Most-used script-side types (Effect() sites / comparison sites). The scripts handle them; the sim stores them and sends APPLY_EFFECT:
RESURRECTION 1006/12, RECENTLY_STUNNED 746/0, STUN 57/758, DEATH 379/12, VisualEffect 364/94, MODIFYMANASTAMINA 191/12,
KNOCKDOWN 76/13, DOT 68/12, DAZE 33/12, DISPEL_MAGIC 31/12, ROOT 27/12, EVASION 27/0, KNOCKBACK 27/0, UPKEEP 24/24,
HEAL 22/12, MODIFY_CRIT_CHANCE 21/12, MABARI_DOMINANCE 20/12. Comparison counts of 12 come from one shared rules function compiled into 12 scripts.

Effect ids constructed with values that are not in effects.gda: 20000, 20001, 20002 (5 sites).
Ids compared against GetEffectType that are not in effects.gda: 10000 and 2001 (12 sites each).
