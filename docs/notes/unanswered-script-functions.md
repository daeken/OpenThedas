# Unanswered Origins engine functions (SCR-002)

Snapshot against main `e67af2e3`, measured 2026-11-14 on the local Origins
1.05 install. This lists every declaration in the winning `script.ldf` whose
Origins dispatch falls through `engine::call` to `None`, including functions
not called by the installed scripts. It records API names and our measurements;
no game resource or declaration file is included.

Of **676 declarations**, **508 have a dispatch path** and **168 are unanswered**.
Of the unanswered functions, **63 are called at 1,871 static sites**, and
**105 have no static call site** in the measured install. A dispatch path can
still be a no-op, constant, stub or partial implementation; this inventory is
not a claim of gameplay coverage. The remaining scripting gaps describe those
limitations.

Measurement: `thedas script stats <install> --report <private-output>.json`
parsed **4,284 distinct compiled scripts**, with zero parse failures. There
are 556 called IDs: 549 declared functions and seven IDs absent from the shipped
table. Of the 549 declared functions called, 486 have a dispatch path; 22 more
answered declarations are never called. Call counts are static references,
not execution counts. These fresh counts supersede the older call-count
snapshot in the area inventory when choosing work. The older snapshot counted
4,158 scripts; this records the current reader/install combination.

The complete declaration set was compared with the name arms in
`crates/thedas-sim/src/engine.rs`, its `PRESENTATION` list and its generic
`Command*` constructor guard (command return type required). Guards were checked
against Origins declarations: the resource-taking `PlayCutscene` arm accepts
DA2's signature, while Origins declares zero arguments and still falls through.
All **168** listed declarations were then called with default typed arguments
through the baseline MCP's `call_engine_function`; each returned
`implemented: false`. This checks name/guard classification without executing
unimplemented gameplay. It does not validate native semantics.

To refresh, rerun the stats command, read the winning declaration table from
that install's resource stack, compare its complete name set with dispatch
(including guards), and confirm the unanswered set through
`call_engine_function`. Keep resource extracts and raw stats outside the
repository. The newer inferred IDs 859/860/862 are outside this declaration
inventory; their state is tracked separately in the scripting gap list.

## Called unanswered declarations

| function | id | calls | scripts |
| --- | --- | --- | --- |
| DEBUG_ConsoleCommand | 704 | 649 | 57 |
| GetDebugHelpersEnabled | 331 | 430 | 430 |
| InteractWithObject | 587 | 113 | 113 |
| CloseQuestGroup | 846 | 65 | 5 |
| ShowAsAllyOnMap | 364 | 62 | 26 |
| SetMusicVolumeStateByTag | 488 | 57 | 41 |
| GetPlaceablePopupText | 356 | 53 | 51 |
| SetPhysicsController | 839 | 52 | 39 |
| SetCheatUsedFlag | 627 | 50 | 46 |
| GetEffectID | 614 | 38 | 13 |
| SetPartyLeader | 625 | 36 | 18 |
| SetItemIrremovable | 375 | 21 | 14 |
| MoveAllItems | 350 | 20 | 20 |
| ShowStartMenu | 362 | 16 | 11 |
| GetCreatureAttribute | 248 | 15 | 8 |
| GetTrapDisarmDifficulty | 413 | 15 | 15 |
| GetTlkTableString | 14 | 12 | 11 |
| SetItemDamaged | 645 | 11 | 11 |
| RemoveEffectsByCreator | 139 | 10 | 7 |
| HighlightGuiElement | 848 | 9 | 1 |
| IsItemStealable | 641 | 8 | 4 |
| SetPrimaryControlled | 815 | 7 | 7 |
| GetAILevel | 221 | 6 | 6 |
| SetLocName | 307 | 6 | 3 |
| GetTrapDetectionDifficulty | 411 | 6 | 6 |
| BeginSlideshow | 632 | 6 | 6 |
| GetVectorMagnitude | 42 | 5 | 3 |
| SetCanChangeEquipment | 391 | 5 | 4 |
| GetMeleeRingPosition | 606 | 5 | 5 |
| RemoveAllItemVisualEffects | 783 | 5 | 2 |
| RemoveQuest | 850 | 5 | 2 |
| SetFacingPosition | 23 | 4 | 4 |
| SetReactionOverride | 471 | 4 | 4 |
| StopSoundObject | 481 | 4 | 4 |
| OpenItemUpgradeGUI | 636 | 4 | 4 |
| GetMapPinState | 653 | 4 | 4 |
| BuyDownload | 831 | 4 | 4 |
| GetWeaponsUnsheathedStatus | 160 | 3 | 3 |
| SetItemHeraldry | 172 | 3 | 3 |
| PlayMovie | 173 | 3 | 3 |
| GetCreatureMaxStamina | 240 | 3 | 3 |
| GetAOEFlags | 334 | 3 | 2 |
| GetPCSpeaker | 438 | 3 | 3 |
| GetVectorNormalize | 43 | 2 | 1 |
| IsFollowerLocked | 146 | 2 | 2 |
| IsDestroyable | 215 | 2 | 2 |
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

## Uncalled unanswered declarations

| function | id |
| --- | --- |
| AddGeneralScoreboardEntry | 791 |
| CharToInt | 59 |
| CheckLineOfSightVector | 47 |
| ClearCreatureProperties | 333 |
| DEBUG_SendRawInputEvent | 338 |
| DEBUG_SetCursorPosition | 339 |
| DEPRECATED_GetCreatureStrengthModifier | 700 |
| DebugSpawnScriptDebugger | 16 |
| Deprecated_ExecuteScript | 8 |
| ECSplitString | 383 |
| EnablevEvent | 123 |
| FeetToMeters | 61 |
| GetAchievementCountByID | 618 |
| GetAutoLevelFollowers | 341 |
| GetBackgroundDefaultsIndex | 637 |
| GetClassRank | 623 |
| GetCommandIsPlayerIssued | 628 |
| GetCommandPriority | 730 |
| GetCreatureCanBeTracked | 765 |
| GetCreatureStamina | 238 |
| GetEffectAnimation | 607 |
| GetEffectDurationType | 608 |
| GetEffectFlags | 605 |
| GetEffectString | 689 |
| GetFollowPartyLeader | 768 |
| GetGamePlotAssist | 602 |
| GetItemAttackRating | 134 |
| GetItemDamaged | 646 |
| GetItemHeraldry | 171 |
| GetItemSubItems | 821 |
| GetItemVisualEffectsIDs | 781 |
| GetLocalCommand | 87 |
| GetLocalEffect | 90 |
| GetLocalEvent | 84 |
| GetLocalItemProperty | 93 |
| GetLocalPlayer | 78 |
| GetMapPatchState | 650 |
| GetMaxItemStackSize | 299 |
| GetOnlineStatistic | 640 |
| GetPlatform | 694 |
| GetPlayerMapPatch | 651 |
| GetPlayerMapPin | 654 |
| GetWeaponArmorPenetration | 703 |
| GetWeaponAttackRating | 693 |
| GetWeaponBaseDamage | 699 |
| GetWeaponStrengthModifier | 701 |
| HandleEvent_String | 133 |
| InsertString | 101 |
| IntToChar | 60 |
| IsEventValid | 107 |
| IsItemIndestructible | 378 |
| IsItemIrremovable | 376 |
| IsLocationSafe | 456 |
| IsPRCAvailable | 836 |
| OpenJournalPRCTab | 852 |
| PlayCutscene | 648 |
| PrintObject | 3 |
| PrintResource | 733 |
| PrintToLogAndFlush | 130 |
| PrintVector | 4 |
| Probe | 680 |
| RegisterEventListener | 124 |
| RemoveCommand | 535 |
| RemoveCommandByIndex | 534 |
| RemoveItemVisualEffect | 782 |
| ResetReaction | 669 |
| SendOnlineScreenShot | 631 |
| SetChampionshipScoreboardEntry | 792 |
| SetClassRank | 325 |
| SetCommandInt | 545 |
| SetCommandObject | 551 |
| SetControllable | 814 |
| SetCreatureCanBeTracked | 764 |
| SetCreatureMaxMana | 245 |
| SetCreatureMaxStamina | 241 |
| SetDead | 729 |
| SetDestroyable | 216 |
| SetEffectEngineObject | 762 |
| SetEffectString | 690 |
| SetFollowerLocked | 147 |
| SetFollowerSubState | 144 |
| SetItemAbilityId | 191 |
| SetLoadImage | 361 |
| SetLocalCommand | 88 |
| SetLocalEffect | 91 |
| SetLocalEvent | 85 |
| SetLocalItemProperty | 94 |
| SetLocalPlayer | 79 |
| SetLocationArea | 462 |
| SetLocationOrientation | 465 |
| SetMapPatchState | 649 |
| SetObjectAlwaysVisible | 187 |
| SetTournamentScoreboardEntry | 793 |
| ShowAllCodexEntries | 619 |
| ShowChampionshipScoreboardGUI | 789 |
| ShowDeathScreen | 771 |
| ShowGeneralScoreboardGUI | 788 |
| ShowTournamentScoreboardGUI | 790 |
| ShowTutorial | 310 |
| StopMusic | 485 |
| StopSound | 478 |
| StringToVector | 56 |
| UnregisterEventListener | 125 |
| UpdateOnlineStatistic | 639 |
| YardsToMeters | 62 |

