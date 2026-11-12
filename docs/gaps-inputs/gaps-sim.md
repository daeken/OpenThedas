# Gaps: simulation, combat, AI, abilities, items, scripting, movement

Owner: sim/combat stream. Last updated 2026-11-12 16:15 UTC, at main after #317 (plus the open capsule-blocking branch m1-body-block).
Each entry gives what the original does, what we do, evidence, a rough size (S < half a day, M about a day, L several days, XL a week or more) and dependencies.
"Traced" means read from DAOrigins.exe; trace notes live in OpenThedas-testdata/notes/.

## A. Movement, collision, placement

1. **Creature bodies block each other** (in progress, branch m1-body-block).
   - Original: PhysX capsule controllers (radius max(0.6·PERSPACE, 0.1)); live creatures slide along each other; no push, no avoidance; dead ones stop blocking (creature-collision-trace.md).
   - Ours: on main creatures pass through each other. The branch adds 2D capsule sliding inside the timed-path mover.
   - Size M. Deps: none. Risk: stuck walkers in doorways, since the exe has no re-path either. A watchdog trace line is included.
2. **Movement is a timed path, not physics.**
   - Original: the controller moves per frame. It climbs steps (step offset 0.5), follows slopes (limit cos 60°) and stops on walls. A blocked head retries with step offset 0.
   - Ours: straight lines between nav points at appearance speed, with z from the path. No step offset, no slope limit, no wall sliding outside the navmesh.
   - Size L. Deps: collision.rs level geometry (exists, used for line of sight).
3. **Path follower details.** The exe's path follower (0xa63xxx–0xa64xxx) is not traced: goal handling, corner cutting, re-path on a moving target, timeouts. Ours uses provisional APPROACH_REPLAN 1 m and grid A*. Size M (trace) plus M (apply).
4. **Melee ring** (#293): slots sit at fixed world angles; the exe may turn them with the target's facing (untraced). Slots are held by "heading for or standing on" rather than an explicit reservation (0x7cb0e0). The ring offset column 0xed58b0c9 and the boss meleering1..9 rings (High Dragon, Archdemon, Spirit Apparatus) are not read. Size S–M.
5. **Attack start step's 60° facing test** (0x7a87c0). Ours turns attackers to face instantly. Turn rate and timing are not traced, nor whether the exe's turn costs time. Size S (trace) plus S.
6. **CommandJumpToLocation, CommandFly, SetFacingPosition, GetSafeLocation, IsMoving, ClearCurrentCommand, GetRoamLocation** are not implemented (static call counts 137, 353, 4, 50, 2, 1, 2). CommandFly is mostly in flyby/cutscene scripts. Size S each.
7. **SetPhysicsController** (52 calls) is not implemented. It turns a creature's controller on or off, which matters once blocking lands. Size S. Deps: A1.
8. **Personal space and overlap at spawn**: creatures placed overlapping are not pushed apart (the exe's capsule grows in 0.1 steps "while there is room"). Size S.

## B. Combat rules and timing

1. **Attack impacts come at the end of the attack, not at animation hit frames.**
   - Original: each hand's impact fires on its animation event (0x6c0c46, "Left" means the off hand).
   - Ours: both hands land when the attack's duration ends, main hand first.
   - Size M–L. Deps: animation event data in the sim (the sim has no animation clock).
2. **Projectiles land instantly.** No flight time, no misses from moving targets, no projectile speed table. Size M.
3. **Ability timing** is provisional (cast start to impact; Sim::ability_timing). Conjure and cast times come from tables, but the exe's own numbers for some are not recovered. Size M.
4. **Draw and sheathe timing** is not traced; weapons switch state instantly. Size S–M.
5. **AoE, friendly fire and AoE shapes.** aoe.rs exists; shape and flag details (GetAOEFlags/SetAOEFlags not implemented) and friendly fire by difficulty are not verified against the exe. Size M.
6. **Flanking and backstab geometry**: rules_core computes these from facing and positions, so they depend on facing accuracy (A5) and blocking (A1). Not verified. Size S (verify).
7. **Knockdown, stun, grab and paralysis as engine states.** Effects apply through scripts. The engine-side movement lock and animation states (e.g. knockdown stops movement and attacks for the animation's length) are only partly modelled. Size M.
8. **Death and resurrection, party knockout.** IsDead by health (the exe reads HP ≤ 0 plus the death action) is parked: a branch broke the gate chain. Ours uses a dead flag. Size M (study). Evidence: parked m1-isdead-healthless.
9. **Regeneration and fatigue** are implemented per traces; combat versus out-of-combat rates should be re-verified after #212's game-mode change. Size S.
10. **Threat** (threat.rs) is provisional on decay and on the weights read through an emulated x87 stack. Size M to verify.
11. **Combat music, combat mode and the end-of-fight check** are script-owned (verified). Nothing missing known.
12. **Stealth**: SetStealthEnabled and perception of stealthed creatures exist. The detection roll timing and stealth movement speed are not traced. Size M.
13. **Traps**: GetTrapDisarmDifficulty and GetTrapDetectionDifficulty are not implemented (15 and 6 calls). Trap detection by the Survival skill and disarm by rogues probably don't work. Size M.
14. **Stealing**: IsItemStealable is not implemented (8 calls); the steal flow is unverified. Size S–M.
15. **Dual-wield and off-hand rules** are as rules_core gives them. Off-hand impact timing is tied to B1.

## C. AI and tactics

1. **Tactics engine**: slots, conditions and target types are implemented. Coverage of every condition and command type against AI Tactics Conditions/Base Conditions (M2DA 137/139/140/141) has not been audited. Size M.
2. **Creature AI** runs from scripts (ai_main and friends). Engine functions the AI uses that we stub (GetAILevel, SetReactionOverride, ShowAsAllyOnMap…) are minor. Size S.
3. **Party AI while the player controls one member**: autopilot exists. The exe's rules for followers in combat when no tactics are set are not verified. Size M.
4. **SetPartyLeader and SetPrimaryControlled** are not implemented (36 and 7 calls): scripted control switches (e.g. playing as another character) won't happen. Size S–M.

## D. Abilities, effects, levelling

1. **Shapeshifting**: there are engine.rs references, but shapeshift appearance and ability swaps are unverified. Size M.
2. **Summons** (animate dead, wolves): not found in the sim; SpawnCreature or script paths are not audited. Size M.
3. **Sustained abilities and modal GUI**: implemented. Upkeep and fatigue verified only partly. Size S.
4. **Specialisations**: levelup.rs handles them. Unlocks via plot (trainers, manuals) are not audited. Size S.
5. **Effects**: SetEffectFlags, SetEffectAnimation, GetEffectID, GetEffectsByCreator, RemoveEffectsByCreator and GetVisualEffectLocation are not implemented (4, 5, 38, 2, 10, 1 calls). Size S each.
6. **Item properties as effects**: item properties apply through scripts. Material and enchantments on created items are not added (inventory.rs provisional). Size M.
7. **Runes and item upgrades**: OpenItemUpgradeGUI is not implemented (4 calls); the rune slot flow is not built. Size M. Deps: UI.
8. **Crafting** (poison, traps, potions): recipes are learned and inventory holds them; ShowCraftingGUI is not implemented and the craft flow is not built. Size M. Deps: UI.
9. **Injuries**: the engine has some support (the harness uses `injury remparty`); injury on knockout and the injury kit flow are not verified. Size M.
10. **Creature scaling to area level**: ScaleEquippedItems is implemented for levels above 0. With level ≤ 0 the exe reads the area level from table 0xe1 and skips already-scaled creatures; ours reads tier 0. Creature level scaling on spawn by area level is not audited. Size M. Evidence: inventory.rs provisional note.
11. **GetCreatureAttribute, GetCreatureMaxStamina, SetCreatureStamina, SetCreatureMoney** are not implemented (15, 2, 1, 2 calls). Size S.

## E. Items and inventory

1. **One shared party pack**: as the game shows it (provisional). Stored follower inventories exist.
2. **SetItemIrremovable, SetItemDamaged, SetCanUseItem, SetCanChangeEquipment, SetItemHeraldry, MoveAllItems, RemoveAllItemVisualEffects** are not implemented (21, 11, 140, 5, 3, 20, 5 calls). SetCanUseItem (140) matters most (quest items and cutscenes). Size S each.
3. **Body bags that die when empty** (DieWhenEmpty, coin pouch model) are not removed; pool creatures (CreatePool) are not modelled. Size S.
4. **Stores**: implemented. Buy and sell price rules have not been re-checked against the exe. Size S.
5. **Item stacking**: done per the exe (#237).

## F. Scripting engine and world rules

1. **Engine functions not named anywhere in the sim**: 182 of 673 declared, mostly debug, online, telemetry and DLC (DEBUG_ConsoleCommand 649 calls, GetDebugHelpersEnabled 421, BuyDownload, SendOnlineTelemetry, Screenshot…). The gameplay-relevant ones are listed in A–E. The full ranked list is in sim-work/stats.json plus unimpl.txt. Size: S per function.
2. **InteractWithObject** (108 calls, ambient and placeable interactions: sitting, leaning, using) is not implemented. Size M. Deps: animation and presentation.
3. **Quest groups**: CloseQuestGroup (65 calls) and RemoveQuest are not implemented; the journal may keep quests that should close. Size S.
4. **Placeable popup text** (GetPlaceablePopupText, 53 calls) is not implemented. Size S.
5. **Music volume states by tag** (SetMusicVolumeStateByTag, 52 calls) are not implemented. Audio dep. Size S.
6. **BeginSlideshow** (6 calls, ending slides) and **PlayMovie** (3) are not implemented. Size S–M. Deps: UI.
7. **Training mode and tutorial steps** (BeginTrainingStep, HighlightGuiElement) are not implemented; single-script use. Size S.
8. **Cheat flag, play time and completion %** (SetCheatUsedFlag 48, GetPlayTime, SetGameCompletionPercentage): trivial, but GetPlayTime matters for achievements. Size S.
9. **Time of day, weather and atmosphere**: ResetAtmosphere is not implemented, and nothing in the sim tracks time of day. Size S (as far as the sim goes).
10. **World map**: travel is instant. Random encounters exist in worldmap.rs, but the encounter chance and table rules are not verified. Size M.
11. **Camp and party picker**: implemented. Pool XP top-up on picker open is traced (xp-trace.md) and implemented.
12. **Approval and romance**: GetFollowerApproval exists in engine.rs. The approval ranges tables (M2DA 166/167/168) and gift handling are not audited. Size M.
13. **Persuade and intimidate checks** in dialog: conditions run through scripts, but nothing was found in the sim. Verify the skill check plumbing. Size S.
14. **Script VM edge cases**: 58 scripts exhaust the 2M instruction budget in the do-nothing smoke run (mostly zz_ test scripts). One bad stack offset (combateveryoneimmortal), two divisions by zero, and unknown engine functions #10, #396, #397, #862 are used by test or DLC scripts. Size S to triage.
15. **Area transitions, parked areas and held events**: work. Objects in parked areas don't run, which is provisional.
16. **Saves**: our own JSON format. Reading the original's .das saves is not built. Size XL. Deps: formats.

## G. Determinism and tooling

1. **Approach checks on a 100 ms grid**: provisional (the exe checks per frame). A 33 ms experiment barely changed results and was dropped.
2. **The gate covers the Human Noble chain only**, s01–s16. There are no gate chains for the other five origins past their openings. Size M (harness). Deps: QA saves.
3. **Coverage counters** (autoplay --coverage) over 8 saves show only SetBodybagDecayDelay unimplemented (fixed in #303). Wider coverage needs more chains (G2).
