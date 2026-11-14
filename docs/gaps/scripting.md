# Known gaps: script VM and engine functions

[Back to the index](../KNOWN_GAPS.md)

The NWScript-style VM and the engine functions, events, commands and effects that game scripts rely on.

## Engine functions measured against a real install

Gathered 2026-11-12 on main 1eb844a, from the release `thedas` binary and source. No game text.

### Method

- `thedas script stats <install> --report stats.json` on the GOG 1.05 install (DAODecomp/gamedata).
  It counts static call sites of each engine function across every distinct compiled script (`.ncs`).
  These are call sites in compiled code, not runtime calls.
- "Implemented" means the name has an arm in `crates/thedas-sim/src/engine.rs` `call()`, is in its
  `PRESENTATION` no-op list, or is a `Command*` constructor (generic `n if n.starts_with("Command")` arm).
  Anything else returns `None`: the sim counts it in `Trace::unimplemented` and returns the default for
  the return type (int 0, empty string, `OBJECT_INVALID`, ...).
- The arm list was extracted from the source by a script, not by hand, so a few names may be off. The
  unimplemented names below were spot-checked with grep (no match anywhere in `crates/`).

### Totals

| what | count |
| --- | --- |
| distinct compiled scripts | 4158 (0 failed to parse) |
| engine functions declared in `script.ldf` | 676 |
| declared functions that some script calls | 549 |
| function ids called that `script.ldf` does not declare | 7 (#859, #860, #861, #862, #10, #396, #397) |
| static call sites, all functions | 980,935 |
| declared functions the sim handles | 497 of 676 (474 match arms + 23 presentation no-ops) |
| called functions the sim handles | 479 of 549 |
| called functions the sim does not handle | 70 (2,061 static call sites, 0.2% of all) |
| functions the sim handles that no script calls | 18 |

So there are 70 unimplemented functions that scripts call, fewer than the 150 asked for. All are listed below, so there is no tail.

### Handled, but only as a no-op or a fixed value

- `PRESENTATION` (accepted as no-ops because they are presentation only, with sounds forwarded to the presentation). It has 25 names, 23 of them
  declared in script.ldf; SetCameraTarget and ApplyEffectVisualEffectOnLocation are not declared: ApplyEffectVisualEffect, ApplyEffectVisualEffectOnLocation, SetAtmosphere, SetAtmosphereRGB,
  FB_SetEffectResource, FB_SetEffectEnabled, PlaySound, PlaySoundSet, PlaySoundObject, PlayMusic,
  SetMusicIntensity, SetSoundSet, SetLoadHint, LinkAreaMiniMaps, SendGameEventRunDatabase, SendToRunDatabase,
  SetTrainingMode, SetPlaceableHealth, SetMapPinState, SetCameraTarget, SetLookAtEnabled, SetCreatureGoreLevel,
  Engine_SetModalAbilityGUI, ShowFloatyIcon, ShowSpecUnlockedNotification.
  (SetMapPinState and SetPlaceableHealth are listed here even though they arguably change game state. Whether that matters is unverified.)
- `STUBS` (fixed placeholder, counted in `Trace::stubbed`): GetPlotSummary (returns an empty string).
- Arms that return a constant or do nothing: GetGraphicsDetailLevel (1), GetMaxLevel (20),
  GetHasAchievement / GetHasAchievementByID (0), ClearAmbientDialogs (1), CreatePool (1), GetCreatureGoreLevel (0.0),
  GetShowSpecialMoveFloaties (0), ReadIniEntry (empty string), PreloadCharGen, SetDeathHint, PlayAdditiveAnimation, SignalDamage,
  EnableWeaponTrail (no-ops).
- Logged as "no effect in the simulation": DoAutoSave, TakeScreenshot. Also `Engine_ApplyEffectAtLocation` for any effect
  other than an area of effect or a visual effect (counted as stubbed).
- engine.rs has 46 comments marked "Provisional" (behaviour inferred, not verified against the exe).

### Ids missing from script.ldf

The GOG 1.05 engine has functions newer than the `script.ldf` it ships. `crates/thedas-script/src/inferred.rs`
adds inferred signatures for three of them, and the sim session loads them with `with_inferred`.

| id | static calls | sim status |
| --- | --- | --- |
| #862 (inferred name IsPackageLoaded) | 1645 | implemented |
| #10 | 24 | unknown id; the VM errors (2006 iteration-test scripts) |
| #861 | 13 | no inferred signature; the VM errors |
| #859 (Inferred859_GetIntArray) | 7 | signature only, no sim arm |
| #860 (Inferred860) | 2 | signature only, no sim arm |
| #396, #397 | 1 each | unknown id (test scripts addenemytoparty / removeenemyfromparty) |

### VM smoke run (do-nothing host)

4088 of 4158 scripts finish. The rest fail as follows: 58 exhaust the instruction budget of 2,000,000 (looping test/debug scripts such as zz_*), 4 call
unknown ids, 2 IsInTrigger with 0 args, 2 SetFacingPosition with 1 arg, 2 division by zero, 1 bad stack offset.

### Unimplemented functions scripts call (all 66, by static call count)

Static call sites / distinct scripts. The debug and console ones (DEBUG_ConsoleCommand, GetDebugHelpersEnabled,
SetCheatUsedFlag, Screenshot, SendOnlineTelemetry) matter less. InteractWithObject, ShowAsAllyOnMap,
SetPartyLeader, CloseQuestGroup, the trap-difficulty getters and the item functions affect gameplay.

| function | id | calls | scripts |
| --- | --- | --- | --- |
| DEBUG_ConsoleCommand | 704 | 649 | 57 |
| GetDebugHelpersEnabled | 331 | 421 | 421 |
| InteractWithObject | 587 | 108 | 108 |
| CloseQuestGroup | 846 | 65 | 5 |
| ShowAsAllyOnMap | 364 | 62 | 26 |
| GetPlaceablePopupText | 356 | 53 | 51 |
| SetMusicVolumeStateByTag | 488 | 52 | 36 |
| SetPhysicsController | 839 | 52 | 39 |
| SetCheatUsedFlag | 627 | 48 | 44 |
| GetEffectID | 614 | 38 | 13 |
| SetPartyLeader | 625 | 36 | 18 |
| SetItemIrremovable | 375 | 21 | 14 |
| MoveAllItems | 350 | 20 | 20 |
| ShowStartMenu | 362 | 16 | 11 |
| GetCreatureAttribute | 248 | 15 | 8 |
| GetTrapDisarmDifficulty | 413 | 15 | 15 |
| GetTlkTableString | 14 | 12 | 11 |
| SetItemDamaged | 645 | 11 | 11 |
| BeginTrainingStep | 843 | 11 | 1 |
| RemoveEffectsByCreator | 139 | 10 | 7 |
| HighlightGuiElement | 848 | 9 | 1 |
| IsItemStealable | 641 | 8 | 4 |
| SetPrimaryControlled | 815 | 7 | 7 |
| SetLocName | 307 | 6 | 3 |
| GetTrapDetectionDifficulty | 411 | 6 | 6 |
| BeginSlideshow | 632 | 6 | 6 |
| GetVectorMagnitude | 42 | 5 | 3 |
| GetAILevel | 221 | 5 | 5 |
| SetCanChangeEquipment | 391 | 5 | 4 |
| GetMeleeRingPosition | 606 | 5 | 5 |
| RemoveAllItemVisualEffects | 783 | 5 | 2 |
| SetFacingPosition | 23 | 4 | 4 |
| SetReactionOverride | 471 | 4 | 4 |
| StopSoundObject | 481 | 4 | 4 |
| OpenItemUpgradeGUI | 636 | 4 | 4 |
| GetMapPinState | 653 | 4 | 4 |
| BuyDownload | 831 | 4 | 4 |
| RemoveQuest | 850 | 4 | 1 |
| IsVectorEmpty | 40 | 3 | 3 |
| GetWeaponsUnsheathedStatus | 160 | 3 | 3 |
| SetItemHeraldry | 172 | 3 | 3 |
| PlayMovie | 173 | 3 | 3 |
| GetAOEFlags | 334 | 3 | 2 |
| GetPCSpeaker | 438 | 3 | 3 |
| GetVectorNormalize | 43 | 2 | 1 |
| IsFollowerLocked | 146 | 2 | 2 |
| IsDestroyable | 215 | 2 | 2 |
| GetCreatureMaxStamina | 240 | 2 | 2 |
| SetAOEFlags | 335 | 2 | 2 |
| ShowCraftingGUI | 355 | 2 | 2 |
| IsMoving | 388 | 2 | 2 |
| SetCreatureMoney | 409 | 2 | 2 |
| SendOnlineTelemetry | 630 | 2 | 1 |
| GetEffectsByCreator | 713 | 2 | 1 |
| GetRoamLocation | 826 | 2 | 2 |
| SaveGamePostCampaign | 832 | 2 | 2 |
| GetVisualEffectLocation | 186 | 1 | 1 |
| ToggleGamePause | 188 | 1 | 1 |
| SetCreatureStamina | 239 | 1 | 1 |
| ResetAtmosphere | 802 | 1 | 1 |
| Screenshot | 820 | 1 | 1 |
| GetPlayTime | 830 | 1 | 1 |
| SetGameCompletionPercentage | 844 | 1 | 1 |
| IsPRC | 857 | 1 | 1 |
| SetPartyPickerStage | 858 | 1 | 1 |

### Implemented but never called by any shipped script (18)

PrintFloat, PrintToLogWindow, tan, acos, asin, atan, log, GetLocalLocation, SetLocalLocation, LockQuickslotBar,
GetStoreMoney, SetStoreMoney, GetStoreMaxBuyPrice, SetStoreMaxBuyPrice, GetAreaFromLocation, CanUseConditionedAbility,
GetCommandFloat, SetWorldMapLocationLocName.

## Events, commands and effects measured against a real install

Gathered 2026-11-12 at main `1eb844a` by disassembling all 4,158 distinct compiled scripts and
following the values returned by `GetEventType`, `GetCommandType` and `GetEffectType` to the
constants they are compared with. The counts are static and may be slightly low: the tracking
gives up after calls to ids that `script.ldf` does not declare. They put numbers on `SCR-063`,
`SCR-055` and `SCR-045`.

### Event types scripts handle that the engine never raises

`script.ldf` names 111 event types; the engine raises or names 50. Scripts also define 73 event
values of their own (5,978 construction sites), which the simulation delivers generically.

| Event | Handler scripts comparing it | Scripts constructing it themselves |
| --- | --- | --- |
| `EVENT_TYPE_DAMAGED` | 180 | 149 |
| `EVENT_TYPE_TRAP_TRIGGERED` | 64 | 0 |
| `EVENT_TYPE_PLACEABLE_ONCLICK` | 64 | 0 |
| `EVENT_TYPE_PLACEABLE_COLLISION` | 56 | 0 |
| `EVENT_TYPE_UNLOCKED` | 35 | 0 |
| `EVENT_TYPE_AREALOADSAVE_PRELOADEXIT` | 10 | 0 |
| `EVENT_TYPE_AREALOADSAVE_POSTLOADEXIT` | 7 | 0 |
| `EVENT_TYPE_ITEM_ONHIT` | 6 | 1 |
| `EVENT_TYPE_USE_PLOTACTION` | 5 | 6 |
| `EVENT_TYPE_MODULE_PRESAVE` | 4 | 0 |
| `EVENT_TYPE_SPELLCASTAT` | 3 | 0 |
| `EVENT_TYPE_GIFT_ITEM` | 3 | 0 |
| `EVENT_TYPE_GUI_OPENED` | 2 | 0 |

One handler script each: `COMBAT_INITIATED`, `AREALOADSAVE_SPECIAL`, `CHARGEN_IMPORT_HERO`,
`MANA_STAM_DEPLETED`, `ITEM_ONTEST_USABLE`, `WORLD_MAP_CLOSED`, `LOAD_TACTICS_PRESET`,
`RUBBER_BAND`, `GIVE_UP`, `ON_SELECT`, `ON_ORDER_RECEIVED`, `PLAYER_COMMAND_ADDED`,
`USE_ABILITY_IMMEDIATE`, `TRAINING_BEGIN` and seven other `TRAINING_*` events.

Another 27 are named in `script.ldf`, never raised by the engine, and have no comparison the
analysis found (it may have lost them in a helper function): `MELEE_ATTACK_START`, `BLOCKED`,
`FAILTOOPEN`, `CLICK`, `TRAP_DISARMED`, `LISTENER`, `LOCKED`, `MODULE_GETCHARSTAGE`,
`GAMEOBJECTSLOADED`, `CHARGEN_SCREEN_ENTERED`, `CHARGEN_SELECT_LEVELUP_CLASS`,
`CHANTERS_DONATION`, `LEVEL_OF_THE_WEEK`, `ABILITY_ACQUIRED`, `REACHED_WAYPOINT`,
`AREALIST_POSTLOAD`, `INVENTORY_FULL`, `CREATURE_ENTERS_DIALOGUE`, `CHAR_RECORD_OPENED`,
`OPTIONS_CHANGED`, `PLOT_COMPLETED`, `PARTY_MONEY_CHANGED`, `CODEX_CHANGED`, `CRAFT_ITEM` and
three `TRAINING_*` events.

Traced since (testdata notes `combat-events-trace.md`, `placeable-area-events-trace.md`, from every
call to the exe's event constructor 0x69bac0):

- **Script-raised only, nothing to do:** `DAMAGED`, `SPELLCASTAT`, `COMBAT_INITIATED`, `UNLOCKED`,
  `LOCKED`, `FAILTOOPEN`, `BLOCKED`, `CLICK`, `TRAP_DISARMED` have no engine sender. The health
  change handler (0x6bc4b0) builds no event.
- **Now raised:** `MANA_STAM_DEPLETED` (0x6afb90), the `AREALOADSAVE_*` trio on loading a save
  (0x67f1c0, 0x6c90ab, 0x6c92b9), `MODULE_PRESAVE` before a save (0x5d6c40) and `GUI_OPENED` when a
  screen opens (0x60aeb0).
- **`ITEM_ONHIT`:** raised for a ranged hit's ammunition, to the shooter (0x6b6f60, 0x7e87c0). The
  melee ones go to the item, whose script is empty in every template, so they do nothing.
- **Engine-raised, still missing here:** `TRAP_TRIGGERED` (physics touch
  0x74f900, and a trapped placeable's use 0x7a7a96 instead of `USE`), `PLACEABLE_COLLISION`
  (0x74daa0, the controlled creature touching a placeable, once per placeable),
  `PLACEABLE_ONCLICK` (0x7f6550, with `ON_ORDER_RECEIVED`), `GIFT_ITEM` (0x718210),
  and `USE_PLOTACTION` (0x6fc480). The trapped-use path never fires with shipped data (no template
  sets `TrapFlag`), and which placeables get physics trigger shapes for the touch events is not
  traced yet. `USE` now carries int 0 = 1 when the user stands in front of the placeable.

### Commands that do nothing, by how often scripts build them

| Constructor | Call sites (scripts) | Note |
| --- | --- | --- |
| `CommandMoveAwayFromObject` | 1,010 (169) | now walks (provisional); type number invented (`SCR-057`) |
| `CommandPlayAnimation` | 829 (386) | |
| `CommandFly` | 353 (176) | DLC scripts pass a second integer |
| `CommandMoveToMultiLocations` | 323 (295) | now walks its list (provisional); type number invented |
| `CommandTurn` | 221 (214) | now turns (provisional); type number invented |
| `CommandSheatheWeapons` | 14 (14) | type number invented |
| `CommandDeathBlow` | 3 (3) | |
| `CommandUnsheatheWeapons` | 2 (1) | type number invented |

`CommandEquipItem` and `CommandUnequipItem` are declared and never called. Scripts compare
`GetCommandType` almost only with INVALID (5,042 sites), ATTACK (195) and WAIT (194).
`COMMAND_TYPE_DO_FUNCTION`, `DO_EVENT`, `INTERACT`, `DRIVE` and `RECOVER` have no constructor.

### Engine-native effects that are only stored

| Effect (id) | Construction sites |
| --- | --- |
| `EffectMovementSpeed` (9) | 236 (now applied) |
| `EffectMovementSpeedDebuff` (11) | 219 (now applied) |
| `EffectTransparency` (10) | 98 |
| `EffectScreenShake` (6) | 80 |
| `EffectParalyze` (8) | 48 |
| `EffectGeneric` (1) | 1 |

The engine gives behaviour of its own to area-of-effect (5), death (7), resurrection (1005),
upkeep (1016), damage over time (1017), modify-property (1012), damage (1013) and impact (1024),
and passes visual effects (4) to the presentation. The script-side types used most are
resurrection, recently-stunned, stun, death, modify mana and stamina, knockdown and damage over
time; the scripts handle those through APPLY_EFFECT and REMOVE_EFFECT. Scripts also construct
effect ids 20000 to 20002 and compare against 10000 and 2001, none of which is a row of the
effects table.

## Code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

Scope: `crates/thedas-script/src/*`, `crates/thedas-sim/src/engine.rs`, and the event queue, command queue and effect handling in `crates/thedas-sim/src/sim.rs`, `world.rs`, `combat.rs` (apply/remove/expire only), `aoe.rs` and `plot.rs` (flag access only). Line numbers are against `main` as read.

Dispatch summary: `engine::call` (`crates/thedas-sim/src/engine.rs:483`) matches the function's name from `script.ldf`. It has 460 named match arms, plus 25 names in the `PRESENTATION` list (`engine.rs:178`; one, `SetCreatureGoreLevel`, is in both), so 484 distinct named functions are answered, plus every `Command*` constructor whose return type is `command` through one generic arm (`engine.rs:2089`). Anything else falls to `_ => None` (`engine.rs:4289`); the host (`sim.rs:3604`) then records `TraceEntry::Unimplemented`, counts it in `Trace::unimplemented`, and the VM pushes the default value of the return type (`vm.rs:739`). Of the 484: 1 is on the explicit `STUBS` list, 12 return a fixed constant, about 33 do nothing or only log/count (19 pure presentation no-ops, 14 others), about 20 only store a value nothing else reads, and the rest have real behaviour, some of it partial (listed below).

### Engine-function dispatch and fallback

- **SCR-001** **Unknown engine functions silently return defaults.** Original: not traced. Ours: any function without a match arm returns the default of its return type (0, 0.0, "", OBJECT_INVALID, empty array, invalid engine handle) and the script keeps running; only the trace records it. Scripts that branch on such results take the "nothing there" path without error. Evidence: `crates/thedas-sim/src/engine.rs:4289`, `crates/thedas-sim/src/sim.rs:3604`, `crates/thedas-script/src/vm.rs:739`. Size: L. Depends: none. Kind: missing.
- **SCR-002** **No inventory of which script.ldf functions are unimplemented is kept in the repository.** Original: not applicable. Ours: the set of unanswered functions is known only by running `thedas script stats` / the trace against a real install; nothing in the tree lists the gap, so this document cannot enumerate the missing names. Evidence: `crates/thedas-sim/src/engine.rs:1-4`, `crates/thedas-sim/src/trace.rs:119`. Size: S. Depends: none. Kind: missing.
- **SCR-003** **Declared parameter defaults are never applied.** Original: not traced (compiled scripts may always push every argument). Ours: `Param::default` is parsed as text and never evaluated; the VM accepts `argc` down to the required count and the host reads missing arguments as 0 / "" / OBJECT_INVALID through `int()`, `string()`, `obj()`, not as the declared default. Only a few functions special-case `a.len()` (e.g. `FloatToString`, `GetObjectByTag`, `RemoveAllEffects`, `CreateItemOnObject`). Evidence: `crates/thedas-script/src/ldf.rs:74`, `crates/thedas-script/src/vm.rs:329`, `crates/thedas-sim/src/engine.rs:19-47`. Size: M. Depends: none. Kind: unverified.
- **SCR-004** **Argument type confusion is coerced rather than reported.** Original: not traced. Ours: `int()` truncates a float, `float()` widens an int, and a wrong-typed or missing argument becomes 0/""/OBJECT_INVALID with no trace entry, hiding script/host mismatches. Evidence: `crates/thedas-sim/src/engine.rs:19-47`. Size: S. Depends: none. Kind: provisional.
- **SCR-005** **Unreachable and copy-pasted arms.** Original: not applicable. Ours: `SetCreatureGoreLevel` is in `PRESENTATION` and also in the `DoAutoSave` arm, which can never run for it; the `GetPackage`/`GetCreatureRacialType`/`GetCombatantType` arm and the `GetItemUnique`/`GetItemMaterialProgression` arm carry a dead `hero_default` test for `name == "GetFollowerState"` copied from the follower arm. Evidence: `crates/thedas-sim/src/engine.rs:200`, `engine.rs:2823`, `engine.rs:2907-2913`, `engine.rs:3945-3951`. Size: S. Depends: none. Kind: bug.

### Stubs, constant answers and no-ops

- **SCR-006** **GetPlotSummary is a stub.** Original: not traced. Ours: returns "" and is counted in `Trace::stubbed`, the only entry of `STUBS`. Evidence: `crates/thedas-sim/src/engine.rs:212`, `engine.rs:2578`. Size: M. Depends: none. Kind: missing.
- **SCR-007** **Functions answered with a fixed constant.** Original: not traced for any of them. Ours: `GetGraphicsDetailLevel` = 1 (marked provisional), `GetMaxLevel` = 20, `ReadIniEntry` = "", `GetHasAchievement` = 0, `GetHasAchievementByID` = 0, `CreatePool` = 1 (exe builds a creature pool, 0x6598d0; skipped as a load optimisation), `GetCreatureGoreLevel` = 0.0, `GetShowSpecialMoveFloaties` = 0, `GetAbilitiesDueToAOEs` = empty `int[]`, `ClearAmbientDialogs` = 1 (exe 0x6510f0 stops queued ambient lines; none ever queue here). Evidence: `crates/thedas-sim/src/engine.rs:1548`, `engine.rs:1565`, `engine.rs:1567`, `engine.rs:1570`, `engine.rs:3074`, `engine.rs:3075`, `engine.rs:4209`, `engine.rs:3975`, `engine.rs:2713`. Size: M. Depends: SCR-090. Kind: missing.
- **SCR-008** **Presentation functions accepted as pure no-ops.** Original: not traced. Ours: return void and are only counted: `ApplyEffectVisualEffect`, `ApplyEffectVisualEffectOnLocation`, `SetAtmosphere`, `SetAtmosphereRGB`, `FB_SetEffectResource`, `FB_SetEffectEnabled`, `SetLoadHint`, `LinkAreaMiniMaps`, `SendGameEventRunDatabase`, `SendToRunDatabase`, `SetTrainingMode`, `SetPlaceableHealth`, `SetMapPinState`, `SetCameraTarget`, `SetLookAtEnabled`, `SetCreatureGoreLevel`, `Engine_SetModalAbilityGUI`, `ShowFloatyIcon` (exe 0x662db0), `ShowSpecUnlockedNotification` (0x663b80). Nothing is kept for rendering or the UI to read, so object visual effects, atmosphere, camera targets and the modal-ability indicator never show. Evidence: `crates/thedas-sim/src/engine.rs:178-207`, `engine.rs:519-527`. Size: M. Depends: none. Kind: missing.
- **SCR-009** **Some "presentation" no-ops hold simulation state.** Original: not traced. Ours: `SetPlaceableHealth`, `SetMapPinState` and `SetTrainingMode` are in the presentation list, so a placeable's health, a map pin's state and training mode are not stored, saved or readable. Evidence: `crates/thedas-sim/src/engine.rs:190-192`. Size: S. Depends: none. Kind: unverified.
- **SCR-010** **Other functions that do nothing.** Original: not traced. Ours: `PreloadCharGen`, `SetDeathHint`, `PlayAdditiveAnimation`, `SignalDamage`, `EnableWeaponTrail` return void with no record; `SetAbilityResult` is only counted; `DoAutoSave`, `TakeScreenshot`, `UnlockAchievement`, `UnlockAchievementByID`, `IncrementAchievementCountByID`, `SetStoryPlot` and `ShowAreaUnlockedNotification` only write a trace log line. Evidence: `crates/thedas-sim/src/engine.rs:2835`, `engine.rs:3682`, `engine.rs:4091`, `engine.rs:4288`, `engine.rs:4200`, `engine.rs:2823`, `engine.rs:3692`, `engine.rs:3452`, `engine.rs:2432`. Size: M. Depends: SCR-090, SCR-091. Kind: missing.
- **SCR-011** **HandleEvent with no script has no engine default handling.** Original: not traced (comment says the engine's default handling applies). Ours: returns without doing anything when the script name is empty. Evidence: `crates/thedas-sim/src/engine.rs:1248-1250`. Size: M. Depends: none. Kind: missing.

### Stored-only switches (written, never read)

- **SCR-012** **Creature switches stored but never acted on.** Original: `SetCreatureIsGhost` makes a ghost walk through other creatures (0x69f4a0, per comment); others not traced. Ours: `SetCreatureIsGhost` (field `Ghost`), `SetCreatureIsStatue` (`Statue`), `SetAILevel` (`AILevel`), `SetRoamRadius`/`SetRoamLocation` (locals `__roam_radius`/`__roam_location`), `SetPlotGiver` (`PlotGiver`) and `SetTrapDetected` (`TrapDetected`) write fields that no code outside engine.rs reads. Evidence: `crates/thedas-sim/src/engine.rs:4097-4121`, `engine.rs:3643`, `engine.rs:2364-2379`, `engine.rs:3423-3434`, `engine.rs:2358`. Size: M. Depends: none. Kind: missing.
- **SCR-013** **Quickslot bar selection and lock are not used.** Original: 0x719ae0 / 0x719ac0 set the bar shown and its lock (per comment). Ours: fields `QuickslotBar` and `QuickslotBarLocked` are written and never read by the quickbar or UI. Evidence: `crates/thedas-sim/src/engine.rs:4097-4103`. Size: S. Depends: none. Kind: missing.
- **SCR-014** **RevealCurrentMap has no reader.** Original: 0x665030 clears fog on the current area's map. Ours: sets the area's `MapRevealed` field, which nothing reads. Evidence: `crates/thedas-sim/src/engine.rs:2817-2822`. Size: S. Depends: none. Kind: missing.
- **SCR-015** **Plot actions control and story events are stored but unused.** Original: not traced (story events go to the save's "Story So Far", 0x666da0, 0x643bf0). Ours: `SetPlotActionSet/Enabled/State/Count` and `LogStoryEvent` write `World::plot_actions` and `World::story_events`, which no UI or save summary reads; `LogStoryEvent` keeps game time where the exe keeps play time and wall clock (provisional). Evidence: `crates/thedas-sim/src/engine.rs:3036-3069`, `engine.rs:3443-3451`. Size: S. Depends: none. Kind: missing.
- **SCR-016** **SetAutoLevelUp has no effect on levelling.** Original: not traced. Ours: stored in `AutoLevelUp` and only echoed by `GetAutoLevelUp`. Evidence: `crates/thedas-sim/src/engine.rs:3423-3434`, `engine.rs:3649`. Size: S. Depends: none. Kind: missing.

### Objects, locals, strings and maths

- **SCR-017** **SetPosition and SetLocation do not update triggers.** Original: 0x65b3b0 for SetLocation; trigger notification not traced. Ours: position and area are written directly; no ENTER/EXIT is sent (`move_object`/`update_triggers` are not called). Evidence: `crates/thedas-sim/src/engine.rs:895-918`. Size: S. Depends: none. Kind: provisional.
- **SCR-018** **DestroyObject leaves references behind and ignores its second argument.** Original: container removal on destruction not traced. Ours: removes the object (except the hero) and takes items off holders, but does not clear it from followers/escorts, perception lists, threat tables, `in_triggers`, command targets or AoE membership, raises no event, and ignores the delay/flag argument. Evidence: `crates/thedas-sim/src/engine.rs:1007-1037`. Size: M. Depends: none. Kind: provisional.
- **SCR-019** **CreateObject handles a fixed set of object types.** Original: not traced. Ours: maps 32/512/64/4096/128 to creature/placeable/item/waypoint/trigger and anything else to placeable; trims only `.utc`/`.utp`; a created trigger has no geometry; created triggers' and placeables' positions do not run trigger checks. Evidence: `crates/thedas-sim/src/engine.rs:1038-1077`. Size: S. Depends: none. Kind: provisional.
- **SCR-020** **Template local variables of some types are dropped.** Original: not traced. Ours: variable types 1 int, 2 float, 3 string, 12 resource are loaded; type 4 object always becomes OBJECT_INVALID; other types (e.g. location) are skipped. Evidence: `crates/thedas-sim/src/sim.rs:3446-3468`. Size: S. Depends: none. Kind: provisional.
- **SCR-021** **GetObjectByTag / UT_GetNearestObjectByTag search order is guessed.** Original: not traced. Ours: current area first, then area-less objects, then others, by id; `UT_GetNearestObjectByTag` falls back to any area. Evidence: `crates/thedas-sim/src/world.rs:824-839`, `crates/thedas-sim/src/engine.rs:835-844`. Size: S. Depends: none. Kind: provisional.
- **SCR-022** **GetNearestObject family: tie order and kind 0.** Original: GetNearestObjectToLocation 0x671510 (no living/perception/self test). Ours: equal distances order by id (provisional); a kind mask of 0 is treated as "any". Evidence: `crates/thedas-sim/src/engine.rs:826-833`, `engine.rs:2262-2289`. Size: S. Depends: none. Kind: provisional.
- **SCR-023** **GetObjectsInShape body radius and "in world" test are guessed.** Original: 0x670eb0, 0x4eb340, 0x4eb3b0. Ours: body radius from `Sim::body_radius` rather than the exe's virtual 0x9c; "in the world" (virtual 0x6c) taken as `active`. Evidence: `crates/thedas-sim/src/engine.rs:214-221`, `engine.rs:2290-2294`. Size: S. Depends: none. Kind: provisional.
- **SCR-024** **GetClusterCenter candidates and skipped creatures are guessed.** Original: 0x671980. Ours: candidates are the perception list (the exe's per-creature limit not traced); creatures the exe skips (virtual 0x1d0) taken as the dead. Evidence: `crates/thedas-sim/src/engine.rs:268-286`. Size: S. Depends: none. Kind: provisional.
- **SCR-026** **Guessed creature status answers.** Original: not traced. Ours: `GetCanDiePermanently` = not immortal; `HasDeathEffect` = dead flag; `IsAbilityAvailable` = has the ability (no cost, cooldown or condition test); `Deprecated_GetMaxHealth` reads template fields `MaxHealth`/`Health`, not the health property; `SetMaxHealth` resets current health to the maximum when it is 0 or above it. Evidence: `crates/thedas-sim/src/engine.rs:4052`, `engine.rs:3778`, `engine.rs:3435`, `engine.rs:3091-3100`, `engine.rs:2691-2703`. Size: S. Depends: none. Kind: unverified.
- **SCR-027** **GetTeam only sees the loaded area.** Original: not traced. Ours: filters objects to `world.area`. Evidence: `crates/thedas-sim/src/engine.rs:3101-3116`. Size: S. Depends: none. Kind: provisional.
- **SCR-028** **GetOrientation / location orientation is a z-rotation guess.** Original: not traced. Ours: returns `[0, 0, facing]`; `SetOrientation` takes only z. Evidence: `crates/thedas-sim/src/engine.rs:995-996`, `engine.rs:2787-2798`. Size: S. Depends: none. Kind: provisional.
- **SCR-029** **SetName with an empty name and SetAppearanceType -1 are guessed.** Original: not traced. Ours: empty name restores the template's; -1 restores the template's appearance. Evidence: `crates/thedas-sim/src/engine.rs:734-741`, `engine.rs:2936-2965`. Size: S. Depends: none. Kind: provisional.
- **SCR-031** **Number parsing and formatting are not matched to the exe.** Original: not traced. Ours: `StringToInt`/`StringToFloat` return 0 unless the whole trimmed string parses (no leading-digit parse); `ToString` on a float is assumed `%f` six decimals (provisional); objects print in decimal for `ToString`/`ObjectToString`; engine values print a Rust debug form. Evidence: `crates/thedas-sim/src/engine.rs:540-557`, `engine.rs:169-175`, `engine.rs:4037`. Size: S. Depends: none. Kind: unverified.
- **SCR-032** **Timers are simulation-time approximations.** Original: not traced. Ours: `GetTime` = whole simulation seconds; `GetLowResTimer` = simulation milliseconds truncated to i32 (wraps after about 24.8 days of play time). Evidence: `crates/thedas-sim/src/engine.rs:1566`, `engine.rs:3015`. Size: S. Depends: none. Kind: unverified.
- **SCR-033** **Random number generator is not the exe's.** Original: not traced. Ours: xorshift64* seeded per `Sim`; `Random(n)` is `next % n` (modulo bias), `RandomFloat` 24-bit. Deterministic and saved, but sequences cannot match the original. Evidence: `crates/thedas-sim/src/world.rs:426-453`, `crates/thedas-sim/src/engine.rs:607-608`. Size: S. Depends: none. Kind: unverified.
- **SCR-035** **Talk-table strings: English only, no tokens.** Original: not traced. Ours: `GetStringByStringId` reads only `*en-us.tlk` and returns the raw text; designer tokens such as `<FirstName/>` are filled only by the UI (`thedas-ui/src/protocol.rs:86`), and no female/alternate string variant is chosen. Evidence: `crates/thedas-sim/src/sim.rs:423-443`, `crates/thedas-sim/src/engine.rs:1581`. Size: M. Depends: none. Kind: missing.

### Items and inventory functions

- **SCR-098** **Script item moves raise no inventory events and MoveItem does not merge stacks.** Original: not traced. Ours: `RemoveItemsByTag`, `MoveItem` and `RemoveItem` take worn items off properly (UNEQUIP, effects end), but send no INVENTORY_REMOVED/INVENTORY_ADDED, and `MoveItem` appends the item without merging it into a matching stack. Evidence: `crates/thedas-sim/src/engine.rs` (the three handlers). Size: S. Depends: none. Kind: unverified.
- **SCR-040** **Weapon sets partly modelled.** Original: not traced. Ours: `EquipItem` comment says weapon sets are not modelled though code now routes inactive-set items; only the in-hand set takes effect; `SwitchWeaponSet` sends no equip events (provisional). Evidence: `crates/thedas-sim/src/engine.rs:1655-1705`, `engine.rs:3024-3034`. Size: S. Depends: none. Kind: provisional.
- **SCR-041** **Item properties lack sub-items and stat contributions.** Original: 0x665570, 0x7556d0, 0x674260. Ours: items hold no sub-items (runes), so `bIncludeSubItems` adds nothing; `GetItemStat` does not add item properties (material, enchantments). Evidence: `crates/thedas-sim/src/engine.rs:3813-3850`. Size: M. Depends: none. Kind: provisional.
- **SCR-042** **Party inventory and starting size are guessed.** Original: not traced. Ours: the party inventory is the hero's backpack; `DEFAULT_INVENTORY_SIZE` = 70. Evidence: `crates/thedas-sim/src/engine.rs:105-106`, `engine.rs:3250-3260`, `engine.rs:3369-3391`. Size: S. Depends: none. Kind: provisional.

### Party, followers and approval

- **SCR-043** **Approval is a bare counter.** Original: not traced. Ours: `AdjustFollowerApproval` adds to field `Approval` with no bound, no initial value from templates, no `SetFollowerApprovalEnabled` gate, and no event; `SetFollowerApprovalDescription` and `SetFollowerApprovalEnabled` are stored only. Evidence: `crates/thedas-sim/src/engine.rs:2657-2673`, `engine.rs:3423-3434`. Size: M. Depends: none. Kind: missing.
- **SCR-044** **Follower state handling is approximate.** Original: SetFollowerState 0x65e6e0 → 0x63b9a0 moves between pool and party, raises no PARTYMEMBER events. Ours: party pool is modelled only through state values (comment: provisional); the hero reads as FOLLOWER_STATE_ACTIVE without being set (provisional); joining sets the hero's group and moves the follower to the leader's area (provisional). Evidence: `crates/thedas-sim/src/engine.rs:2592-2656`. Size: M. Depends: none. Kind: provisional.

### Effects

- **SCR-045** **Most effect types have no engine behaviour.** Original: not traced per type. Ours: the engine itself acts only on EFFECT_TYPE_DEATH (7, kills), RESURRECTION (1005, revives), AOE (5, creates an AoE object), visual effect (4, kept at locations for rendering only), UPKEEP (1016, read for modal state), DOT (1017, damage attribution only) and the movement-speed pair (9, 11: their potencies multiply the creature's speed, as 0x7a0610/0x79f910 do). Every other type is passed to the target's APPLY_EFFECT script and then stored until it expires or is removed; the effects.gda `Flags` (disable movement, combat, talents) are only reported by `GetEffectsFlags` and never stop commands or movement. Evidence: `crates/thedas-sim/src/combat.rs:538-612`, `combat.rs:717-746`, `crates/thedas-sim/src/engine.rs:2072-2086`. Size: L. Depends: none. Kind: missing.
- **SCR-046** **Engine_ApplyEffectAtLocation of other types is a placeholder.** Original: 0x65c830 applies to the location's area. Ours: only types 4 and 5 do anything; anything else is counted in `Trace::stubbed` and logged "no effect in the simulation". Evidence: `crates/thedas-sim/src/engine.rs:2403-2486`. Size: M. Depends: SCR-045. Kind: missing.
- **SCR-047** **Visual effects on objects are dropped.** Original: not traced. Ours: `ApplyEffectVisualEffect` is a no-op; effects of type 4 applied to creatures go through APPLY_EFFECT like others but nothing exposes them for rendering (only location VFX and `AddItemVisualEffect` are kept). Evidence: `crates/thedas-sim/src/engine.rs:179`, `engine.rs:2016-2033`, `crates/thedas-sim/src/world.rs:350-371`. Size: M. Depends: none. Kind: missing.
- **SCR-048** **Effects have no ids.** Original: not traced. Ours: effects carry `id` 0 (only location VFX get one), `GetEffects`' effect-id filter is not applied, and `RemoveEffect` matches by full structural equality, so two identical effects cannot be told apart. Evidence: `crates/thedas-sim/src/engine.rs:49-56`, `crates/thedas-sim/src/combat.rs:669-676`. Size: M. Depends: none. Kind: provisional.
- **SCR-049** **Effect parameter layouts are inferred.** Original: not traced. Ours: `SetEffectEngineFloat` stores at float index i+32 (provisional layout); `EffectModifyProperty` packs ids into ints and changes into floats; `EffectDamage`/`EffectImpact` layouts are read from how rules scripts read them; `EffectModifyPropertyHostile` only sets a `hostile` flag whose use is unknown; engine ints/vectors are kept but never read. Evidence: `crates/thedas-sim/src/engine.rs:1851-1856`, `engine.rs:3663-3681`, `engine.rs:3779-3810`, `crates/thedas-sim/src/world.rs:338-347`. Size: S. Depends: none. Kind: provisional.
- **SCR-050** **ApplyEffectModifyProperty targets a guess.** Original: not traced. Ours: applies to the current effect's target, or the caller when no effect is being handled. Evidence: `crates/thedas-sim/src/engine.rs:3895-3921`. Size: S. Depends: none. Kind: provisional.
- **SCR-051** **Effect expiry has tick granularity.** Original: not traced. Ours: temporary effects are removed in `expire_effects` at the end of each tick, after all events of that tick, not at their due time between events. Evidence: `crates/thedas-sim/src/combat.rs:698-715`, `crates/thedas-sim/src/sim.rs:889`. Size: S. Depends: none. Kind: provisional.
- **SCR-052** **Resurrection by health is inferred.** Original: 0x6bc5fb-0x6bc631 clears CREATURE_SPAWN_DEAD. Ours: setting current health above 0 on a dead creature revives it (provisional, from module_core). Evidence: `crates/thedas-sim/src/engine.rs:1436-1454`. Size: S. Depends: none. Kind: provisional.
- **SCR-053** **SetCreatureMana clamp is assumed.** Original: 0x66cd00. Ours: clamps to 0..total as SetCreatureProperty does (provisional). Evidence: `crates/thedas-sim/src/engine.rs:1360-1389`. Size: S. Depends: none. Kind: provisional.
- **SCR-054** **AoE objects are approximations.** Original: CAreaOfEffectObject (0x7a0230, 0x7e4da0, 0x7e5950). Ours: membership updated once per tick, id order, rectangle axes, script column fallback, ENTER gate and conversation hold-back are all provisional. Evidence: `crates/thedas-sim/src/aoe.rs:23-29`. Size: M. Depends: none. Kind: provisional.

### Commands

- **SCR-055** **Commands accepted but not executed.** Original: not traced. Ours: only Attack, UseAbility, MoveToLocation, MoveToObject, MoveToMultiLocations, MoveAwayFromObject, JumpToLocation, JumpToObject, Turn, Wait, SwitchWeaponSet, StartConversation and UseObject do anything (MoveAway, MultiLocations and Turn are provisional: no line-of-sight search, straight legs and one pass for a loop, no turn rate). `CommandPlayAnimation`, `CommandSheatheWeapons`, `CommandUnsheatheWeapons`, `CommandDeathBlow`, `CommandFly` and any other constructor sit 500 ms in the queue and complete with success. Evidence: `crates/thedas-sim/src/sim.rs:1526-1560`, `sim.rs:2572-2663`, `crates/thedas-sim/src/world.rs:280-281`. Size: L. Depends: SCR-056. Kind: missing.
- **SCR-056** **Command constructors drop arguments.** Original: not traced. Ours: the generic constructor keeps the first object, the first float, all ints and strings, a location, and a non-zero vector only if no location; second objects, later floats and arrays (location lists) are discarded. Evidence: `crates/thedas-sim/src/engine.rs:2089-2139`. Size: M. Depends: none. Kind: provisional.
- **SCR-057** **Unnamed command types get invented numbers.** Original: not recovered. Ours: five constructors with no COMMAND_TYPE constant get 1000+index. Evidence: `crates/thedas-sim/src/engine.rs:461-479`. Size: S. Depends: none. Kind: provisional.
- **SCR-058** **Command accessors read only index 0.** Original: not traced. Ours: `SetCommandFloat`/`GetCommandFloat` and `GetCommandObject` handle only index 0; other indices are dropped or answer the default. Evidence: `crates/thedas-sim/src/engine.rs:2140-2192`. Size: S. Depends: SCR-056. Kind: provisional.
- **SCR-059** **AddCommand to the front leaves a stale timer on the displaced command.** Original: not traced. Ours: inserting at index 0 pushes the running command back with its `due`, `from` and `approach` still set; when it returns to the front it finishes at its old due time without starting again (an attack would skip COMMAND_PENDING), and it gets no completion event when displaced. Later AddCommand arguments are not read. Evidence: `crates/thedas-sim/src/engine.rs:2193-2216`, `crates/thedas-sim/src/sim.rs:2395-2398`. Size: S. Depends: none. Kind: bug.
- **SCR-060** **ClearAllCommands and SetCommandResult are partial.** Original: not traced. Ours: `ClearAllCommands` ignores arguments after the object, always returns 1 and reports COMMAND_RESULT_INVALID for the running command (provisional); `SetCommandResult` acts only on COMMAND_RESULT_FAILED, other results are stored in an unread `CommandResult` field. Evidence: `crates/thedas-sim/src/engine.rs:2539-2554`, `engine.rs:3763-3777`. Size: S. Depends: none. Kind: provisional.
- **SCR-061** **Command timing is a stand-in.** Original: not traced. Ours: non-move, non-attack commands take 500 ms; default attack 1500 ms; every command at least 1 ms; `IsConjuring` derived from the simulation's cast window (provisional). Evidence: `crates/thedas-sim/src/sim.rs:1526-1560`, `crates/thedas-sim/src/combat.rs:81`, `crates/thedas-sim/src/engine.rs:1555-1564`. Size: M. Depends: none. Kind: provisional.
- **SCR-062** **Commands are not stopped by disabling effects or death.** Original: not traced. Ours: no code checks effect flags or stun-type effects before starting or running a command (see SCR-045). Evidence: `crates/thedas-sim/src/sim.rs:2386-2569`. Size: M. Depends: SCR-045. Kind: missing.

### Events

- **SCR-063** **Engine-raised events still missing.** Original: traced (see "Event types scripts handle that the engine never raises" above). Ours: `TRAP_TRIGGERED`, `PLACEABLE_COLLISION`, `PLACEABLE_ONCLICK` (with `ON_ORDER_RECEIVED`), `GIFT_ITEM`, and `USE_PLOTACTION` are not raised. Evidence: testdata notes `combat-events-trace.md`, `placeable-area-events-trace.md`. Size: M. Depends: placeable physics shapes for the touch events. Kind: missing.
- **SCR-064** **Load and new-game event order is provisional.** Original: not traced. Ours: MODULE_LOAD then MODULE_START on a new game; SPAWN for each spawned creature/placeable, AREALOAD_SPECIAL, AREALOAD_PRELOADEXIT, ENTER for the hero, AREALOAD_POSTLOADEXIT on area load, all at delay 0. Evidence: `crates/thedas-sim/src/sim.rs:2732-2736`, `sim.rs:2956-2976`. Size: S. Depends: none. Kind: provisional.
- **SCR-065** **Event delivery to objects without a script is silent.** Original: not traced. Ours: `deliver` returns without a trace when the target has no script and none was given. Evidence: `crates/thedas-sim/src/sim.rs:566-568`. Size: S. Depends: SCR-011. Kind: unverified.
- **SCR-066** **Events and commands tie order is a guess.** Original: not traced. Ours: on a tie an event runs before a finishing command; events at the same time run in queue order. Evidence: `crates/thedas-sim/src/sim.rs:813-828`. Size: S. Depends: none. Kind: unverified.
- **SCR-067** **Events for objects in left areas are held.** Original: not traced. Ours: events to parked objects wait until the party returns (provisional). Evidence: `crates/thedas-sim/src/sim.rs:829-834`. Size: S. Depends: none. Kind: provisional.
- **SCR-068** **EndHeartbeat leaves one pending beat.** Original: not traced. Ours: `EndHeartbeat` removes the interval, but a HEARTBEAT2 already queued is still delivered once; `InitHeartbeat` enforces a 0.1 s minimum. Evidence: `crates/thedas-sim/src/engine.rs:2674-2690`, `crates/thedas-sim/src/sim.rs:1896-1909`. Size: S. Depends: none. Kind: unverified.
- **SCR-069** **Projectiles are timed events only.** Original: not traced. Ours: projectiles fly at a fixed 20 m/s (provisional), never collide, and only deliver the impact event if `SetProjectileImpactEvent` was called; a homing projectile's flight time is fixed at launch. Evidence: `crates/thedas-sim/src/engine.rs:3193-3241`. Size: M. Depends: none. Kind: provisional.
- **SCR-070** **Ability cast event integers left zero.** Original: not traced. Ours: ABILITY_CAST_START/IMPACT carry the ability and three integers set to 0 that ability_core reads (provisional guesses). Evidence: `crates/thedas-sim/src/combat.rs:1150-1165`. Size: S. Depends: none. Kind: provisional.

### VM

- **SCR-071** **STORE_STATE, STORE_STATE_ALL and opcode 0x2E are unsupported.** Original: not traced (0x2E meaning unknown; seen only after the two STORE_STATEs in test scripts). Ours: any of them aborts the script with `Unsupported`; there are no stored action closures. Evidence: `crates/thedas-script/src/vm.rs:588-593`, `crates/thedas-script/src/ncs.rs:17-18`. Size: M. Depends: none. Kind: missing.
- **SCR-072** **Arrays of multi-cell elements are unsupported.** Original: not traced. Ours: READARRAY/WRITEARRAY with an element larger than one cell (vector or struct arrays) abort. Evidence: `crates/thedas-script/src/vm.rs:532-540`. Size: S. Depends: none. Kind: missing.
- **SCR-073** **Comparison and arithmetic type coverage is partial.** Original: not traced. Ours: GT/GEQ/LT/LEQ only for int/int and float/float; ADD/SUB/MUL/DIV only for int, float, mixed int/float, string concatenation and some vector forms (no vector×vector, no float÷vector); MOD pops ints regardless of type code; anything else aborts. Evidence: `crates/thedas-script/src/vm.rs:353-372`, `vm.rs:409-429`, `vm.rs:634-703`. Size: S. Depends: none. Kind: unverified.
- **SCR-074** **Engine-structure equality compares handles.** Original: not traced. Ours: EQUAL/NEQUAL on two engine values compares type and handle id; every getter and setter makes a new handle, so two values holding the same effect, location or event compare unequal. Evidence: `crates/thedas-script/src/vm.rs:397`, `crates/thedas-sim/src/world.rs:813-818`. Size: S. Depends: none. Kind: bug.
- **SCR-075** **Inferred semantics of the stack machine.** Original: not traced. Ours: SAVEBP/RESTOREBP, divide-by-zero aborting, array value semantics and growth, `CONST object 0/1` as OBJECT_SELF/OBJECT_INVALID, OBJECT_INVALID = 0x7F000000 and the DA:O type codes are all marked provisional. Evidence: `crates/thedas-script/src/vm.rs:5-14`, `vm.rs:24-25`, `crates/thedas-script/src/ncs.rs:206`. Size: M. Depends: none. Kind: provisional.
- **SCR-076** **Instruction and nesting limits are invented.** Original: not traced. Ours: each script run gets 5,000,000 instructions (`Sim::script_budget`; the VM's own default is 10,000,000) and scripts nest at most 24 deep; exceeding either stops the script with a trace entry, with no rollback of side effects already made. Evidence: `crates/thedas-sim/src/sim.rs:141`, `sim.rs:392`, `crates/thedas-script/src/vm.rs:175`. Size: S. Depends: none. Kind: unverified.
- **SCR-077** **No save or restore of running scripts.** Original: not traced. Ours: scripts always run to completion inside one call; there is no VM state to serialise, and engine handles are cleared when the outermost script ends, so engine values cannot outlive a run except as copies in the world (queued events, commands, locals of type location). Evidence: `crates/thedas-sim/src/sim.rs:536-540`. Size: S. Depends: SCR-071. Kind: unverified.
- **SCR-078** **Only four engine structures are modelled.** Original: script.ldf names event, location, command, effect, itemproperty and player structures. Ours: `EngineData` holds Event, Location, Command and Effect only; any other structure value is an empty handle. Evidence: `crates/thedas-sim/src/world.rs:386-393`, `crates/thedas-sim/src/sim.rs:369-374`. Size: S. Depends: none. Kind: missing.
- **SCR-079** **Unknown action ids abort the script.** Original: not traced. Ours: an ACTION id absent from the table raises `UnknownAction` and stops the script. Evidence: `crates/thedas-script/src/vm.rs:324-327`. Size: S. Depends: SCR-080. Kind: unverified.

### Script table and inferred functions

- **SCR-080** **Functions missing from the shipped script.ldf are guessed.** Original: the GOG 1.05 engine has functions newer than its table. Ours: #859 `(object,int,int)->int[]`, #860 `(int,float,float,int,object,string)->void`, #862 `IsPackageLoaded(string)->int` and a second `CommandFly` int are inferred from call sites; #859 and #860 have no implementation and return defaults; #862 answers whether a package or addin layer is mounted. Evidence: `crates/thedas-script/src/inferred.rs:1-11`, `inferred.rs:24-56`, `crates/thedas-sim/src/engine.rs:1571-1579`. Size: M. Depends: none. Kind: provisional.

### Partially modelled engine features

- **SCR-081** **Placeable state machine moves only on the default action.** Original: not traced. Ours: `SetPlaceableActionResult` changes state only when the action is the state controller's default; a failed action does nothing; `GetPlaceableAction` falls back to transition/dialog/use guesses when no controller applies. Evidence: `crates/thedas-sim/src/engine.rs:4339-4409`, `engine.rs:3261-3297`. Size: M. Depends: none. Kind: provisional.
- **SCR-082** **Traps have no engine mechanics.** Original: not traced. Ours: no trap detection, arming, triggering or disarming code; `SetTrapDetected` stores a field; trap areas exist only as AoE objects. Evidence: `crates/thedas-sim/src/engine.rs:2358-2363`, `crates/thedas-sim/src/aoe.rs:1-2`. Size: L. Depends: SCR-063. Kind: missing.
- **SCR-100** **SetObjectActive ignores its animation and effect.** Original: with nAnimation non-zero, deactivating queues a PLAY_ANIMATION command (type 14) and hides the creature only when it ends; GetObjectActive reports inactive from the start; nVFX plays a temporary visual effect for the animation's length; activating plays the animation and blocks the command queue for about 0.8 of its length (0x6618b0, 0x744870, 0x746e80; testdata notes `setobjectactive-trace.md`). WR_SetObjectActive passes 648/649 only for appearances with `bHasAppearAnim`/`bHasDisappearAnim` (monsters). Ours: shows or hides at once. Size: S. Depends: animation lengths in the sim. Kind: missing.
- **SCR-083** **TriggerPerception skips the room-visibility check.** Original: 0x65a7b0 → 0x6b32a0 → 0x6a2bb0. Ours: same-area, alive, within 60 m; room visibility not modelled. Evidence: `crates/thedas-sim/src/engine.rs:3498-3523`. Size: S. Depends: none. Kind: provisional.
- **SCR-084** **Melee ring and cooldown keys are approximations.** Original: not traced. Ours: `GetCreaturesInMeleeRing` uses provisional geometry and keeps only ring 0; cooldowns are keyed by ability and item tag, shared by all items with that tag. Evidence: `crates/thedas-sim/src/engine.rs:3996-4036`, `engine.rs:4147-4167`. Size: S. Depends: none. Kind: provisional.
- **SCR-085** **World map travel is instant; the Fade map is the primary map.** Original: not traced. Ours: `WorldMapStartTravelling` transitions at once; `OpenFadeMap` opens the primary map. Evidence: `crates/thedas-sim/src/engine.rs:3166-3192`. Size: M. Depends: none. Kind: provisional.
- **SCR-087** **Tactic per-follower objects read from the UI's writes.** Original: not traced. Ours: `GetTacticTargetObject`/`GetTacticConditionObject` answer from the tactics screen's fields. Evidence: `crates/thedas-sim/src/engine.rs:4270-4283`. Size: S. Depends: none. Kind: provisional.
- **SCR-089** **Plot event layout and plot-script OBJECT_SELF are inferred.** Original: flag storage 0x75bc70, 0x75ea60, 0x75f7e0 traced. Ours: event layout (GUID string 0, flag int 1, new/old ints 2-3, owner object 0) and OBJECT_SELF = hero are provisional. Evidence: `crates/thedas-sim/src/plot.rs:5-9`, `plot.rs:250-257`. Size: S. Depends: none. Kind: provisional.

### Feature checklist: absent features

- **SCR-090** **Achievements.** Original: not traced. Ours: no achievement state; unlocks are logged, queries answer 0. Evidence: `crates/thedas-sim/src/engine.rs:1568-1570`, `engine.rs:3692-3699`. Size: M. Depends: none. Kind: missing.
- **SCR-091** **Save/autosave from scripts.** Original: not traced. Ours: `DoAutoSave` logs only; no script-triggered save path. Evidence: `crates/thedas-sim/src/engine.rs:2823-2830`. Size: M. Depends: none. Kind: missing.
- **SCR-092** **Camera, atmosphere and weather.** Original: not traced. Ours: `SetCameraTarget`, `SetAtmosphere`, `SetAtmosphereRGB` and `SetLookAtEnabled` are no-ops; no camera or weather state in the simulation. Evidence: `crates/thedas-sim/src/engine.rs:180-195`. Size: M. Depends: none. Kind: missing.
- **SCR-093** **Gifts.** Original: not traced. Ours: no code found. Evidence: none (grep for "gift" in `crates/thedas-sim/src` finds nothing). Size: M. Depends: SCR-043. Kind: missing.
- **SCR-094** **Crafting a recipe.** Original: not traced. Ours: recipe items are learned into the known-recipe list; no code found that crafts from a recipe. Evidence: `crates/thedas-sim/src/inventory.rs:625-680`, `crates/thedas-sim/src/inventory.rs:730`. Size: M. Depends: none. Kind: missing.
- **SCR-095** **Periodic engine heartbeat.** Original: not traced. Ours: only scripted HEARTBEAT2 (`InitHeartbeat`) and the first AOE_HEARTBEAT exist; no code sends a regular per-object heartbeat. Evidence: `crates/thedas-sim/src/engine.rs:2674-2690`, `crates/thedas-sim/src/aoe.rs:11-13`. Size: S. Depends: SCR-063. Kind: unverified.
- **SCR-096** **Debug functions.** Original: not traced. Ours: `DEBUG_PrintToScreen` and `DisplayStatusMessage` go to the trace log only; nothing reaches the screen. Evidence: `crates/thedas-sim/src/engine.rs:3016-3023`. Size: S. Depends: none. Kind: missing.
- **SCR-097** **Graphics setting and gore level.** Original: not traced. Ours: always medium detail and gore 0 (SCR-007). Evidence: `crates/thedas-sim/src/engine.rs:1546-1548`, `engine.rs:3075`. Size: S. Depends: none. Kind: provisional.

### Checked and looks complete

- Local variables on objects, modules and areas (int, float, string, object, resource, location get/set).
- Event construction and access (Event, Get/SetEvent* including locations, GetCurrentEvent, SignalEvent immediate and queued, DelayEvent with script override).
- 2DA access from scripts (GetM2DAInt/Float/String/Resource, GetHashedM2DAInt, GetM2DARows, GetM2DARowIdFromRowIndex), with missing tables and columns counted.
- Plot flags (GetPartyPlotFlag, SetPartyPlotFlag, defined flags answered by the plot script, GET_PLOT/SET_PLOT events, GetPlotGUID/ResRef/FlagName/EntryName/FlagRewardId).
- Creature properties (Get/Set/UpdateCreatureProperty by value type, attribute/simple/depletable/derived kinds, GetCreaturePropertyType, mana getters), traced against the exe.
- Effect constructors and accessors (Effect, Get/SetEffect*, GetHasEffects, GetEffects, RemoveEffect, RemoveAllEffects, RemoveEffectsByParameters, GetCurrentEffect, SetIsCurrentEffectValid, Engine_ApplyEffectOnObject, Engine_ApplyEffectOnParty).
- Location functions (Location, GetLocation, SetLocationPosition/Facing, Get*FromLocation, IsLocationValid, GetDistanceBetweenLocations, AngleToVector, VectorToAngle, VectorToString).
- Trigger enter/exit on movement and area entry.
- Perception events (appear/disappear, stealth flag) and perception queries.
- Area and module load events (with the order caveat in SCR-064).
- Timers via DelayEvent and InitHeartbeat/EndHeartbeat.
- Game mode functions (GetGameMode, SetGameMode with conversation hold and GAMEMODE_CHANGE).
- Threat table functions (Update/Clear/ClearEnemy, size, enemy, value by index or object).
- Stealth flag (Get/SetStealthEnabled) feeding perception.
- Abilities: Add/Remove/HasAbility, GetAbilityCount, GetConditionedAbilities, CanUseConditionedAbility, cooldowns, modal state, AoE objects and membership.
- Store functions (OpenStore, money, max buy price, mark-up, mark-down).
- World map functions (location status and name, player location, primary/secondary map, GUI status, open/close, random encounter completion), apart from SCR-085.
- Party list functions (GetPartyList, GetPartyPoolList, party picker, non-party followers, follower formation location).
- XP sharing to the pool on hero XP gain, SetCanLevelUp from unspent points.
- Codex, journal and tutorials via plot flags (plot.rs, journal.rs).
- Cutscene loading (LoadCutscene queue) and conversation start (BeginConversation).
- Sound and music requests (PlaySound, PlaySoundSet, PlaySoundObject, PlayMusic, SetMusicIntensity, SetSoundSet) handed to presentation.
- Location visual effects kept for rendering until expiry or removal.
- Body bags and corpse decay functions.
- DLC/addin check via the inferred IsPackageLoaded.
- String and math helpers (length, case, left/right/sub, sqrt, trig, log, pow, abs, conversions), apart from SCR-031.
- NCS parsing: all opcodes, DA:O resource and array type codes, bounds-checked jumps.
