# 01: Engine functions

Gathered 2026-11-12 on main 1eb844a, from the release `thedas` binary and source. No game text.

## Method

- `thedas script stats <install> --report stats.json` on the GOG 1.05 install (DAODecomp/gamedata).
  It counts static call sites of each engine function across every distinct compiled script (`.ncs`).
  These are call sites in compiled code, not runtime calls.
- "Implemented" means the name has an arm in `crates/thedas-sim/src/engine.rs` `call()`, is in its
  `PRESENTATION` no-op list, or is a `Command*` constructor (generic `n if n.starts_with("Command")` arm).
  Anything else returns `None`: the sim counts it in `Trace::unimplemented` and returns the default for
  the return type (int 0, empty string, `OBJECT_INVALID`, ...).
- The arm list was extracted from the source by a script, not by hand, so a few names may be off. The
  unimplemented names below were spot-checked with grep (no match anywhere in `crates/`).

## Totals

| what | count |
| --- | --- |
| distinct compiled scripts | 4158 (0 failed to parse) |
| engine functions declared in `script.ldf` | 676 |
| declared functions that some script calls | 549 |
| function ids called that `script.ldf` does not declare | 7 (#859, #860, #861, #862, #10, #396, #397) |
| static call sites, all functions | 980,935 |
| declared functions the sim handles | 496 of 676 (473 match arms + 23 presentation no-ops) |
| called functions the sim handles | 478 of 549 |
| called functions the sim does not handle | 71 (2,062 static call sites, 0.2% of all) |
| functions the sim handles that no script calls | 18 |

So there are 71 unimplemented functions that scripts call, fewer than the 150 asked for. All are listed below, so there is no tail.

## Handled, but only as a no-op or a fixed value

- `PRESENTATION` (accepted as no-ops because they are presentation only, with sounds forwarded to the presentation). It has 25 names, 23 of them
  declared in script.ldf; SetCameraTarget and ApplyEffectVisualEffectOnLocation are not declared: ApplyEffectVisualEffect, ApplyEffectVisualEffectOnLocation, SetAtmosphere, SetAtmosphereRGB,
  FB_SetEffectResource, FB_SetEffectEnabled, PlaySound, PlaySoundSet, PlaySoundObject, PlayMusic,
  SetMusicIntensity, SetSoundSet, SetLoadHint, LinkAreaMiniMaps, SendGameEventRunDatabase, SendToRunDatabase,
  SetTrainingMode, SetPlaceableHealth, SetMapPinState, SetCameraTarget, SetLookAtEnabled, SetCreatureGoreLevel,
  Engine_SetModalAbilityGUI, ShowFloatyIcon, ShowSpecUnlockedNotification.
  (SetMapPinState and SetPlaceableHealth are listed here even though they arguably change game state. Whether that matters is unverified.)
- `STUBS` (fixed placeholder, counted in `Trace::stubbed`): GetPlotSummary (returns an empty string).
- Arms that return a constant or do nothing: GetGraphicsDetailLevel (1), GetMaxLevel (20), GetPRCEnabled /
  GetHasAchievement / GetHasAchievementByID (0), ClearAmbientDialogs (1), CreatePool (1), GetCreatureGoreLevel (0.0),
  GetShowSpecialMoveFloaties (0), ReadIniEntry (empty string), PreloadCharGen, SetDeathHint, PlayAdditiveAnimation, SignalDamage,
  EnableWeaponTrail (no-ops).
- Logged as "no effect in the simulation": DoAutoSave, TakeScreenshot. Also `Engine_ApplyEffectAtLocation` for any effect
  other than an area of effect or a visual effect (counted as stubbed).
- engine.rs has 46 comments marked "Provisional" (behaviour inferred, not verified against the exe).

## Ids missing from script.ldf

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

## VM smoke run (do-nothing host)

4088 of 4158 scripts finish. The rest fail as follows: 58 exhaust the instruction budget of 2,000,000 (looping test/debug scripts such as zz_*), 4 call
unknown ids, 2 IsInTrigger with 0 args, 2 SetFacingPosition with 1 arg, 2 division by zero, 1 bad stack offset.

## Unimplemented functions scripts call (all 71, by static call count)

Static call sites / distinct scripts. The debug and console ones (DEBUG_ConsoleCommand, GetDebugHelpersEnabled,
SetCheatUsedFlag, Screenshot, SendOnlineTelemetry) matter less. InteractWithObject, ShowAsAllyOnMap, GetSafeLocation,
SetPartyLeader, CloseQuestGroup, SetCanUseItem, the trap-difficulty getters and the item functions affect gameplay.

| function | id | calls | scripts |
| --- | --- | --- | --- |
| DEBUG_ConsoleCommand | 704 | 649 | 57 |
| GetDebugHelpersEnabled | 331 | 421 | 421 |
| SetCanUseItem | 813 | 140 | 65 |
| InteractWithObject | 587 | 108 | 108 |
| CloseQuestGroup | 846 | 65 | 5 |
| ShowAsAllyOnMap | 364 | 62 | 26 |
| GetPlaceablePopupText | 356 | 53 | 51 |
| SetMusicVolumeStateByTag | 488 | 52 | 36 |
| SetPhysicsController | 839 | 52 | 39 |
| GetSafeLocation | 659 | 50 | 22 |
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
| SetEffectAnimation | 611 | 5 | 2 |
| RemoveAllItemVisualEffects | 783 | 5 | 2 |
| SetFacingPosition | 23 | 4 | 4 |
| SetReactionOverride | 471 | 4 | 4 |
| StopSoundObject | 481 | 4 | 4 |
| SetEffectFlags | 609 | 4 | 4 |
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
| ClearCurrentCommand | 540 | 1 | 1 |
| ResetAtmosphere | 802 | 1 | 1 |
| Screenshot | 820 | 1 | 1 |
| GetPlayTime | 830 | 1 | 1 |
| SetGameCompletionPercentage | 844 | 1 | 1 |
| SetZoomLevel | 849 | 1 | 1 |
| IsPRC | 857 | 1 | 1 |
| SetPartyPickerStage | 858 | 1 | 1 |

## Implemented but never called by any shipped script (18)

PrintFloat, PrintToLogWindow, tan, acos, asin, atan, log, GetLocalLocation, SetLocalLocation, LockQuickslotBar,
GetStoreMoney, SetStoreMoney, GetStoreMaxBuyPrice, SetStoreMaxBuyPrice, GetAreaFromLocation, CanUseConditionedAbility,
GetCommandFloat, SetWorldMapLocationLocName.
