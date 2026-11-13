# Known gaps: simulation and gameplay

[Back to the index](../KNOWN_GAPS.md)

Movement, combat, abilities, AI, party, items, progression, conversations, cutscene logic, world and saves.

## Part 1: the owning stream's list

Written by the stream that works on this area. Entries marked traced were read from the original executable; trace notes live outside the repository with the test data.

Owner: sim/combat stream. Last updated 2026-11-12 16:15 UTC, at main after #317 (plus the open capsule-blocking branch m1-body-block).
Each entry gives what the original does, what we do, evidence, a rough size (S < half a day, M about a day, L several days, XL a week or more) and dependencies.
"Traced" means read from DAOrigins.exe; trace notes live in OpenThedas-testdata/notes/.

### A. Movement, collision, placement

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

### B. Combat rules and timing

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

### C. AI and tactics

1. **Tactics engine**: slots, conditions and target types are implemented. Coverage of every condition and command type against AI Tactics Conditions/Base Conditions (M2DA 137/139/140/141) has not been audited. Size M.
2. **Creature AI** runs from scripts (ai_main and friends). Engine functions the AI uses that we stub (GetAILevel, SetReactionOverride, ShowAsAllyOnMap…) are minor. Size S.
3. **Party AI while the player controls one member**: autopilot exists. The exe's rules for followers in combat when no tactics are set are not verified. Size M.
4. **SetPartyLeader and SetPrimaryControlled** are not implemented (36 and 7 calls): scripted control switches (e.g. playing as another character) won't happen. Size S–M.

### D. Abilities, effects, levelling

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

### E. Items and inventory

1. **One shared party pack**: as the game shows it (provisional). Stored follower inventories exist.
2. **SetItemIrremovable, SetItemDamaged, SetCanUseItem, SetCanChangeEquipment, SetItemHeraldry, MoveAllItems, RemoveAllItemVisualEffects** are not implemented (21, 11, 140, 5, 3, 20, 5 calls). SetCanUseItem (140) matters most (quest items and cutscenes). Size S each.
3. **Body bags that die when empty** (DieWhenEmpty, coin pouch model) are not removed; pool creatures (CreatePool) are not modelled. Size S.
4. **Stores**: implemented. Buy and sell price rules have not been re-checked against the exe. Size S.
5. **Item stacking**: done per the exe (#237).

### F. Scripting engine and world rules

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

### G. Determinism and tooling

1. **Approach checks on a 100 ms grid**: provisional (the exe checks per frame). A 33 ms experiment barely changed results and was dropped.
2. **The gate covers the Human Noble chain only**, s01–s16. There are no gate chains for the other five origins past their openings. Size M (harness). Deps: QA saves.
3. **Coverage counters** (autoplay --coverage) over 8 saves show only SetBodybagDecayDelay unimplemented (fixed in #303). Wider coverage needs more chains (G2).

## Abilities measured against a real install

Gathered 2026-11-12 at main `1eb844a` from the ability table as the game resolves it (551 rows;
the DLC and Awakening worksheets are not counted). "Mentioned" means the ability's id appears
next to an ability word in any test, gate log, QA log, note or source file. It is an upper bound
on what has ever run, and no test asserts the outcome of a single ability.

| Family | Rows | Mentioned anywhere |
| --- | --- | --- |
| Talents | 138 | 62 |
| Spells | 93 | 22 |
| Skills (30 follower-specific) | 70 | 16 |
| Monster abilities | 147 | 9 |
| Item abilities | 102 | 47 |
| Total | 550 | 156 |

The family split is partly inferred (monster abilities by id range and label). Abilities are
exercised only incidentally, by the party AI in the sweep chain's fights. Families known to be
broken or untested: monster abilities (138 of 147 never touched; the broodmother, archdemon,
golem, dragon and werewolf rows are beyond any save the harness reaches), summoning,
shapeshifting, projectile flight time, area-of-effect flags and shapes, control effects (the
effect flags are not enforced, `SCR-045`), trap, stealing and crafting skills, runes and item
upgrades, and injuries.

## Part 2: code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

Scope: sim.rs, combat.rs, threat.rs, tactics.rs, orders.rs, aoe.rs, collision.rs, nav.rs, world.rs,
worldmap.rs, inventory.rs, container.rs, store.rs, quickbar.rs, levelup.rs, chargen.rs, derived.rs,
appearance.rs, decay.rs, dialog.rs, plot.rs, journal.rs, cutscene.rs, cuttracks.rs, cutactors.rs,
chanters.rs, save.rs, session.rs, m2da.rs, trace.rs. Line numbers are against `main` as read.
Much of the game's rules live in the original scripts (rules_core, ability_core, player_core and
the AI includes), which the simulation runs; "no code found" below means no code in these
modules, not that the scripts cannot do it.

### Movement and navigation

- **SIM-001** **Command executor completes commands at once.** Original: not traced. Ours: a command finishes as soon as the executor reaches it, except waits (their duration), moves (route length over speed), attacks and abilities (animation timings); everything else is then followed by `COMMAND_COMPLETE`. Evidence: `crates/thedas-sim/src/world.rs:280`, `crates/thedas-sim/src/sim.rs:7`. Size: L. Depends: none. Kind: provisional.
- **SIM-002** **Unhandled command kinds take a flat 500 ms and do nothing.** Original: not traced. Ours: `command_duration` gives every command other than wait, weapon-set switch, move and jump 500 ms; `finish_command` acts only on attack, use-ability, start-conversation, switch-weapon-set and use-object, so `CommandPlayAnimation`, `CommandDeathBlow`, `CommandTurn`, `CommandMoveAwayFromObject`, `CommandSheatheWeapons`, `CommandUnsheatheWeapons` and any other constructor complete after 500 ms with no effect on the world. Evidence: `crates/thedas-sim/src/sim.rs:1556`, `crates/thedas-sim/src/sim.rs:2621-2649`. Size: M. Depends: SIM-001. Kind: missing.
- **SIM-004** **Stale doc on `command_duration`.** Original: n/a. Ours: the comment says moves go at 5 m/s in a straight line with no pathfinding, while `next_command` routes moves on the pathing grid at the appearance's walk or run speed. Evidence: `crates/thedas-sim/src/sim.rs:1526-1530`, `crates/thedas-sim/src/sim.rs:2527-2551`. Size: S. Depends: none. Kind: bug.
- **SIM-005** **Locomotion speed model is partial.** Original: target speed has a multiplier and bonus (+0xd4, +0xdc), a second stance column 0x03186054 under a flag at +0xf8, and acceleration of 8 m/s per second (0xa635d0, 0xb1b7c8). Ours: none of these are modelled; creatures reach full speed at once. Evidence: `crates/thedas-sim/src/sim.rs:1581-1586`. Size: S. Depends: none. Kind: provisional.
- **SIM-006** **Default movement speed is a guess.** Original: not traced. Ours: 5.0 m/s for a creature whose appearance gives no speed. Evidence: `crates/thedas-sim/src/sim.rs:349-350`. Size: S. Depends: none. Kind: provisional.
- **SIM-007** **Movement is interpolated along straight legs, not stepped by the engine.** Original: not traced. Ours: a moving creature's position is computed on demand from its start point, start time and route at constant speed (`advance_movers`); `move_object` itself is a teleport that tests only the destination for triggers. Evidence: `crates/thedas-sim/src/sim.rs:1562-1572`, `crates/thedas-sim/src/sim.rs:1911-1914`, `crates/thedas-sim/src/sim.rs:1986`. Size: M. Depends: none. Kind: provisional.
- **SIM-008** **Trigger crossing during movement is sampled.** Original: the trigger test during locomotion is not traced. Ours: positions between ticks are sampled every 0.5 m (`TRIGGER_STEP`) for ENTER/EXIT. Evidence: `crates/thedas-sim/src/sim.rs:258-259`, `crates/thedas-sim/src/sim.rs:2019-2023`. Size: S. Depends: SIM-007. Kind: provisional.
- **SIM-009** **Triggers are tested in the XY plane only.** Original: not traced. Ours: trigger outlines ignore height, and the creature is taken as the event's creator and object 0. Evidence: `crates/thedas-sim/src/sim.rs:1858-1860`. Size: S. Depends: none. Kind: provisional.
- **SIM-010** **Pathing-grid semantics are inferred from data.** Original: not traced. Ours: a cell is walkable when bit 0 is set and bit 3 clear, height is the stored value in sixteenths of a metre, and steps up to `MAX_STEP` 0.6 m are allowed. Evidence: `crates/thedas-sim/src/nav.rs:3-8`, `crates/thedas-sim/src/nav.rs:18-27`. Size: M. Depends: none. Kind: provisional.
- **SIM-011** **Pathfinder is a plain grid A\*.** Original: the original pathfinder is not traced. Ours: 8-connected A\* over grid cells without corner cutting, routes are cell centres (no smoothing), search capped at 400,000 expanded cells. Evidence: `crates/thedas-sim/src/nav.rs:7-8`, `crates/thedas-sim/src/nav.rs:21-22`, `crates/thedas-sim/src/sim.rs:1936-1938`. Size: L. Depends: SIM-010. Kind: provisional.
- **SIM-012** **No route falls back to a straight line through walls.** Original: not traced (the original fails such moves with a path result per the comments on door blocking). Ours: with no grid, an end off the grid or no route, the mover walks a straight line to the goal, crossing walls. Evidence: `crates/thedas-sim/src/nav.rs:211`, `crates/thedas-sim/src/sim.rs:1915-1917`, `crates/thedas-sim/src/sim.rs:1648-1663`. Size: S. Depends: SIM-011. Kind: provisional.
- **SIM-013** **No creature-creature collision or dynamic obstacles.** Original: creature-creature collision and avoidance. Ours: routes treat only closed doors' doorway cells as blocked; creatures walk through each other and through placeables; the only spacing between creatures is the melee ring for attackers. Evidence: `crates/thedas-sim/src/sim.rs:1918-1938`, `crates/thedas-sim/src/nav.rs:100`. Size: L. Depends: SIM-011. Kind: missing.
- **SIM-014** **Doorway extent is an inferred radius.** Original: not traced. Ours: a closed door blocks walkable cells within 1.25 m of the door, from the observed ~2.5 m gaps in walls. Evidence: `crates/thedas-sim/src/nav.rs:197-199`. Size: S. Depends: none. Kind: provisional.
- **SIM-015** **Route choice between openable and locked doors.** Original: not traced. Ours: a route through openable doors is preferred over one through locked doors when both exist. Evidence: `crates/thedas-sim/src/sim.rs:1936-1938`. Size: S. Depends: SIM-011. Kind: provisional.
- **SIM-016** **Failed-command retry delay is a stand-in for one update.** Original: the executor runs once per update. Ours: a command failed as it starts (door, full melee ring) is reported 100 ms later (`APPROACH_CHECK_MS`). Evidence: `crates/thedas-sim/src/sim.rs:2334-2345`, `crates/thedas-sim/src/sim.rs:253-257`. Size: S. Depends: none. Kind: provisional.
- **SIM-017** **Approach checks and re-planning are periodic.** Original: the attack start step tests positions every update (0x7a87c0, 0x6a3dc0); the re-pathing rule is not traced. Ours: walkers closing on a creature re-check every 100 ms and re-plan when the goal drifts more than 1 m (`APPROACH_REPLAN`). Evidence: `crates/thedas-sim/src/sim.rs:253-257`, `crates/thedas-sim/src/sim.rs:293-296`, `crates/thedas-sim/src/sim.rs:2069-2074`. Size: S. Depends: none. Kind: provisional.
- **SIM-018** **Melee ring details.** Original: ring set up at 0x6ac2a0, slot reservation 0x7cb0e0, `COMMAND_FAILED_NO_SPACE_IN_MELEE_RING`, per-appearance ring offset (apr_base 0xed58b0c9) and extra rings `meleering1..9.gda`. Ours: slots at fixed world angles (rotation with the target's facing not traced), segment-against-circle test read as "nearer chord end inside", no explicit reservation (places read from where attackers head or stand), a full ring waits once then fails, ring offset and extra rings not read. Evidence: `crates/thedas-sim/src/sim.rs:261-265`, `crates/thedas-sim/src/sim.rs:276-280`, `crates/thedas-sim/src/sim.rs:2148-2153`. Size: M. Depends: none. Kind: provisional.
- **SIM-019** **Melee walk-up uses the slack only to skip a walk.** Original: not traced. Ours: a melee attacker under way walks until within the bodies' reach; the 1 m tolerance only spares the walk when already that close. Evidence: `crates/thedas-sim/src/sim.rs:2095-2099`. Size: S. Depends: SIM-018. Kind: provisional.
- **SIM-020** **Caster approach geometry.** Original: the personal-range check is 0x7a7525/0x6a3dc0; whether the engine walks the caster up is not traced. Ours: range measured from caster centre to target body edge; a cone aimed at a place is used from where the caster stands; personal-range talents walk the caster up (inferred from AI behaviour); radii taken as `PERSPACE`. Evidence: `crates/thedas-sim/src/sim.rs:2253-2267`. Size: S. Depends: none. Kind: provisional.
- **SIM-021** **Jumping to an object keeps the jumper's facing.** Original: not traced. Ours: a move or jump to an object ends at its position with the mover's facing unchanged. Evidence: `crates/thedas-sim/src/sim.rs:2582-2593`. Size: S. Depends: none. Kind: provisional.
- **SIM-023** **Party follow by command is only partly traced.** Original: start test 0x760980, follow update every 0.5 s (0x630f60, 0x760dc0), formation 0xc0d240. Ours: follow constants come from the exe but the comment states the overall follow (distances, formation, pathing) is not traced; follow bookkeeping (`following`, `next_follow_ms`) is not saved. Evidence: `crates/thedas-sim/src/sim.rs:1727-1735`, `crates/thedas-sim/src/sim.rs:95-98`. Size: M. Depends: none. Kind: provisional.
- **SIM-024** **Formation slots beyond the table.** Original: the exe picks a random point within 4 m. Ours: slots past the table take the last entry. Evidence: `crates/thedas-sim/src/sim.rs:198-201`. Size: S. Depends: none. Kind: provisional.
- **SIM-025** **Talk/use approach distance is a guess.** Original: not traced. Ours: walkers stop 0.6 m (`INTERACT_GAP`) beyond the two radii before talking or using. Evidence: `crates/thedas-sim/src/orders.rs:5`, `crates/thedas-sim/src/orders.rs:12-14`. Size: S. Depends: none. Kind: provisional.
- **SIM-026** **Area transition clears followers' queues.** Original: the exe's clearing of action queues on a transition is not traced. Ours: every party member's commands are cleared and they are placed in formation behind the hero. Evidence: `crates/thedas-sim/src/sim.rs:1429-1448`. Size: S. Depends: none. Kind: provisional.
- **SIM-027** **Ground-click raycast uses the pathing grid.** Original: not traced. Ours: a click on the ground lands on the first pathing-grid cell a ray meets; no level geometry is used. Evidence: `crates/thedas-sim/src/orders.rs:167-168`. Size: S. Depends: SIM-010. Kind: unverified.

### Combat

- **SIM-028** **Combat core is provisional as a whole.** Original: not traced beyond the addresses cited per item. Ours: event layouts read from how `rules_core` reads them; perception range and default attack time are guesses. Evidence: `crates/thedas-sim/src/combat.rs:1`, `crates/thedas-sim/src/combat.rs:37-38`. Size: L. Depends: none. Kind: provisional.
- **SIM-029** **Stale module doc on game-mode switching.** Original: the scripts change mode; the engine only applies `SetGameMode` (0x65e4e0 -> 0x6414a0). Ours: the module doc says the party seeing hostiles switches `GM_EXPLORE` to `GM_COMBAT`, but `update_game_mode` does not change the mode. Evidence: `crates/thedas-sim/src/combat.rs:15-17`, `crates/thedas-sim/src/combat.rs:358-369`. Size: S. Depends: none. Kind: bug.
- **SIM-030** **Perception range fallbacks.** Original: not traced. Ours: 30 m when ranges.gda gives nothing (`PERCEPTION_RANGE`); tracking out to 60 m (`VISUAL_RANGE`, marked provisional though 0xadef18 is cited); ranges.gda `SecondaryRange` unused. Evidence: `crates/thedas-sim/src/combat.rs:46-59`, `crates/thedas-sim/src/combat.rs:159-164`. Size: S. Depends: none. Kind: provisional.
- **SIM-031** **Perception runs only in the hero's area and only at fixed points.** Original: each creature's tick (0x6b89b0 from 0x6be7a4). Ours: `update_perception` covers creatures in the hero's area only, once at the start of each tick and after each completed command, not per creature update. Evidence: `crates/thedas-sim/src/combat.rs:217-233`, `crates/thedas-sim/src/sim.rs:761`, `crates/thedas-sim/src/sim.rs:2653`. Size: M. Depends: none. Kind: unverified.
- **SIM-032** **Sight lines approximate the exe's visibility.** Original: hostile sight is a PhysX ray from 1.8 m (0x67c530 -> 0x82deb0); room visibility applies. Ours: closed doors (their models' sight shapes, at any distance; a door without shapes falls back to the doorway test) stand in for room visibility for everyone; without level collision files a pathing-grid wall stands in for the ray, so furniture, rubble and some water block sight. Evidence: `crates/thedas-sim/src/combat.rs:262-268`, `crates/thedas-sim/src/nav.rs:231-238`, `crates/thedas-sim/src/nav.rs:384-392`. Size: M. Depends: SIM-033. Kind: provisional.
- **SIM-033** **Collision masks and back faces.** Original: not traced. Ours: placeable masks never change at runtime (opened, destroyed), triangles are two-sided. Evidence: `crates/thedas-sim/src/collision.rs:13-15`. Size: S. Depends: none. Kind: provisional.
- **SIM-034** **Hearing perception.** Original: perception (hearing). Ours: no code found. Evidence: `crates/thedas-sim/src/combat.rs:217`. Size: M. Depends: SIM-031. Kind: missing.
- **SIM-035** **Stealth visibility rules.** Original: not traced. Ours: allies and neutrals always see stealthed creatures; a hostile is told once (stealthed flag) and the notice is not repeated while in range. Evidence: `crates/thedas-sim/src/combat.rs:299-316`. Size: S. Depends: none. Kind: provisional.
- **SIM-036** **Hostility change re-announcement.** Original: not traced. Ours: a perceived creature whose hostility changes gets a new `PERCEPTION_APPEAR` with the old/new hostility flag. Evidence: `crates/thedas-sim/src/combat.rs:335-346`. Size: S. Depends: none. Kind: provisional.
- **SIM-037** **Hostility comes only from `SetGroupHostility` pairs.** Original: not traced. Ours: two creatures are hostile only when their groups were set hostile by a script; no default group relationship table is loaded. Evidence: `crates/thedas-sim/src/combat.rs:193-209`. Size: S. Depends: none. Kind: unverified.
- **SIM-038** **Default attack duration.** Original: not traced. Ours: 1500 ms when neither the script nor an animation gives a duration. Evidence: `crates/thedas-sim/src/combat.rs:78-81`. Size: S. Depends: none. Kind: provisional.
- **SIM-039** **Aim loop duration handling.** Original: the attack executor holds for the aim loop (0x7a244b). Ours: `SetAimLoopDuration` is added on top of the attack duration, marked provisional. Evidence: `crates/thedas-sim/src/combat.rs:103-104`. Size: S. Depends: none. Kind: provisional.
- **SIM-040** **Attackers turn to face their target instantly.** Original: 0x7a2410 accepts targets within 67.5° of facing; how the engine turns them is not confirmed. Ours: turn at once when the swing starts. Evidence: `crates/thedas-sim/src/combat.rs:1213-1221`. Size: S. Depends: none. Kind: provisional.
- **SIM-041** **Animation events are not played; both hands' impacts land at the end.** Original: each hit event of the attack animation sends one hand's impact (0x6c0c46 -> 0x6a6230), off hand on `Left` events. Ours: both hands' impacts arrive when the attack's time is up, main hand first. Evidence: `crates/thedas-sim/src/combat.rs:1252-1268`. Size: M. Depends: none. Kind: provisional.
- **SIM-042** **Per-hand targets.** Original: the exe keeps a target per hand slot (0x6a0160, 0x6a0170). Ours: both hands report the `CommandAttack` target. Evidence: `crates/thedas-sim/src/combat.rs:1279-1284`. Size: S. Depends: SIM-041. Kind: provisional.
- **SIM-043** **Melee and ranged duration composition.** Original: draw/aim/fire sequence 0x7a87c0, 0x7a4dd0, 0x7a4f51. Ours: melee durations taken to be whole swings and the whole attack. Evidence: `crates/thedas-sim/src/combat.rs:1339-1352`. Size: S. Depends: none. Kind: provisional.
- **SIM-044** **Weapon-style tree and attack variant choice.** Original: not traced. Ours: style read from weapon slots and kinds (`bitm_base.gda`), tree from its name, one of `attk_v1`-`attk_v6` picked by the simulation's dice. Evidence: `crates/thedas-sim/src/combat.rs:1400-1411`, `crates/thedas-sim/src/combat.rs:1712-1714`. Size: S. Depends: none. Kind: provisional.
- **SIM-045** **`GetWeaponStyle` is derived, not stored.** Original: the style lives in the creature's combat state (0x6a3c30); how it is set is not traced. Ours: computed from equipped weapons. Evidence: `crates/thedas-sim/src/combat.rs:1729-1733`. Size: S. Depends: none. Kind: provisional.
- **SIM-046** **Melee reach fallback.** Original: not traced. Ours: 2.0 m when neither side's size is known. Evidence: `crates/thedas-sim/src/combat.rs:1785-1787`. Size: S. Depends: none. Kind: provisional.
- **SIM-047** **Ranged weapon shots have no projectile.** Original: projectiles (the ability path launches one whose arrival sends the impact, 0x7e71c0). Ours: a ranged `CommandAttack`'s impact arrives at the end of draw, aim and fire; no projectile object is created for weapon shots. Evidence: `crates/thedas-sim/src/combat.rs:1339-1352`, `crates/thedas-sim/src/sim.rs:2483-2490`. Size: M. Depends: none. Kind: missing.
- **SIM-048** **Death from missing health only for placed-dead creatures.** Original: a creature with no health counts 0 and is dead (0x6ab4b0). Ours: no property table for creatures scripts never set up, so only `CREATURE_SPAWN_DEAD` creatures are corpses from creation; other creatures without health stay alive. Evidence: `crates/thedas-sim/src/combat.rs:848-863`. Size: S. Depends: none. Kind: provisional.
- **SIM-049** **Death screen and party revival flow.** Original: `EVENT_TYPE_DEATH_RES_PARTY` sent from the death screen (0x76e400). Ours: the whole party falling sets a `game_over` flag only; no code sends `DEATH_RES_PARTY` or offers reload. Evidence: `crates/thedas-sim/src/combat.rs:365-367`, `crates/thedas-sim/src/combat.rs:390-406`. Size: M. Depends: none. Kind: missing.
- **SIM-050** **Autopilot opens fights itself.** Original: `AI_PARTY_CLEAR_TO_ATTACK` is set by rules_core when the controlled character attacks. Ours: on autopilot the simulation sets it whenever the party is fighting, and an idle hero waits a second before its AI round (harness behaviour). Evidence: `crates/thedas-sim/src/combat.rs:380-388`, `crates/thedas-sim/src/combat.rs:1074-1077`. Size: S. Depends: none. Kind: provisional.
- **SIM-051** **Critical hits, backstabs and deathblows are only classified.** Original: criticals, backstabs, deathblows. Ours: the trace counts `COMBAT_RESULT_CRITICALHIT`, `_DEATHBLOW`, `_BACKSTAB` as hits; no engine code decides them (scripts do) and `CommandDeathBlow` does nothing (SIM-002). Evidence: `crates/thedas-sim/src/trace.rs:105-106`, `crates/thedas-sim/src/trace.rs:162`. Size: M. Depends: SIM-002, SIM-041. Kind: unverified.
- **SIM-052** **Flanking.** Original: flanking. Ours: no code found. Evidence: `crates/thedas-sim/src/combat.rs:1`. Size: M. Depends: SIM-040. Kind: missing.
- **SIM-053** **Armour penetration.** Original: armour penetration. Ours: no combat code found; `GetItemStat` scales an `ArmorPenetration` stat by material only. Evidence: `crates/thedas-sim/src/inventory.rs:789-795`. Size: S. Depends: none. Kind: unverified.
- **SIM-054** **Damage types and resistances.** Original: damage types and resistances. Ours: no damage-type code found; derived.rs adds the attribute part of `Resistance_Mind` and `Resistance_Physical`. Evidence: `crates/thedas-sim/src/derived.rs:19-20`. Size: M. Depends: none. Kind: unverified.
- **SIM-055** **Friendly fire by difficulty.** Original: friendly fire by difficulty. Ours: no code found. Evidence: `crates/thedas-sim/src/aoe.rs:14-18`. Size: M. Depends: SIM-057. Kind: missing.
- **SIM-056** **Knockdown, stun and similar engine effects.** Original: knockdown and other effects. Ours: no code found in these modules (effects run only through the target's `APPLY_EFFECT` handler). Evidence: `crates/thedas-sim/src/combat.rs:536-537`. Size: L. Depends: SIM-041. Kind: missing.
- **SIM-057** **Difficulty scaling.** Original: difficulty scaling. Ours: the difficulty is stored and read by `GetGameDifficulty`; no engine-side scaling found. Evidence: `crates/thedas-sim/src/world.rs:571-572`, `crates/thedas-sim/src/chargen.rs:78-81`. Size: M. Depends: none. Kind: unverified.
- **SIM-058** **Injuries.** Original: injuries. Ours: no code found in these modules. Evidence: `crates/thedas-sim/src/combat.rs:536`. Size: M. Depends: none. Kind: missing.
- **SIM-059** **Fatigue.** Original: fatigue. Ours: no code found beyond a save repair that zeroes Fatigue's base. Evidence: `crates/thedas-sim/src/save.rs:47-48`. Size: S. Depends: none. Kind: missing.
- **SIM-060** **Threat order ties and update rate.** Original: sort not traced; the update runs per rendered frame (0x6ab620). Ours: ties broken by object id, update once per simulation tick. Evidence: `crates/thedas-sim/src/threat.rs:20-21`. Size: S. Depends: none. Kind: provisional.
- **SIM-061** **Weapon-set switch sends no equip events.** Original: switch 0x6a8fa0 for `SwitchWeaponSet` (0x65ccc0) and the command (0x7a1ed0). Ours: slots swap with no `EQUIP`/`UNEQUIP` events. Evidence: `crates/thedas-sim/src/combat.rs:1020-1024`. Size: S. Depends: none. Kind: provisional.

### Abilities and effects

- **SIM-062** **Cast event layout.** Original: not traced. Ours: `COMMAND_PENDING`'s third object taken as the item; `ABILITY_CAST_START`/`IMPACT` carry three integers left 0. Evidence: `crates/thedas-sim/src/combat.rs:1123-1127`, `crates/thedas-sim/src/combat.rs:1150-1153`. Size: S. Depends: none. Kind: provisional.
- **SIM-063** **Ability projectiles land at once.** Original: the projectile flies and its arrival sends the impact (0x7e71c0). Ours: `ABILITY_PROJECTILE_LAUNCHED` then the impact immediately at release. Evidence: `crates/thedas-sim/src/combat.rs:1179-1183`, `crates/thedas-sim/src/combat.rs:1563-1564`. Size: M. Depends: none. Kind: provisional.
- **SIM-064** **Ability animation timing choices.** Original: conjure timing 0x7445cf-0x744665; tree selection not traced. Ours: trees searched weapon, weapon-style, combat; cast lengths scaled by speeds while conjure uses raw length; the `_end` animation is not counted. Evidence: `crates/thedas-sim/src/combat.rs:1558-1564`, `crates/thedas-sim/src/combat.rs:1612-1614`. Size: S. Depends: none. Kind: provisional.
- **SIM-065** **Animation loop flag with a component.** Original: with an animation component the exe asks the component (0xa5e940). Ours: only the table flag is read. Evidence: `crates/thedas-sim/src/combat.rs:1701-1704`. Size: S. Depends: none. Kind: provisional.
- **SIM-066** **Sustained (modal) ability detection.** Original: not traced. Ours: an ability is on when the creature carries an effect from it (such as its upkeep). Evidence: `crates/thedas-sim/src/combat.rs:717-719`. Size: S. Depends: none. Kind: provisional.
- **SIM-067** **Ability use conditions partly checked.** Original: 0x71c310 including bit 0x100 against appearance flags (0x71a210) and a two-handed test on two item fields (0xa664c0). Ours: "two-handed" is the simulation's weapon style; 0x100 is not checked. Evidence: `crates/thedas-sim/src/combat.rs:748-763`. Size: S. Depends: SIM-045. Kind: provisional.
- **SIM-068** **Area-of-effect membership details.** Original: updated as objects move; area list sorted by x; ENTER gate on appear (0x7a043d); conversation hold-back (0x6a2960). Ours: membership refreshed once per tick, creatures in id order, rectangle axes from corner offsets, row script column used when the effect names none, ENTER always on at appearance, ENTER not held in conversations. Evidence: `crates/thedas-sim/src/aoe.rs:23-29`. Size: M. Depends: none. Kind: provisional.
- **SIM-069** **Quickbar targeting bits and AoE shapes.** Original: not traced. Ours: `TargetType` bits read from table contents (1 self, 2 friend, 4 hostile, 32 place); `aoe_type` other than 1 and 2 covers no area; with no Abilities table any id above 0 is accepted. Evidence: `crates/thedas-sim/src/quickbar.rs:42-46`, `crates/thedas-sim/src/quickbar.rs:74-76`, `crates/thedas-sim/src/quickbar.rs:117-118`. Size: S. Depends: none. Kind: provisional.
- **SIM-070** **Item cooldowns keyed by tag.** Original: not traced. Ours: cooldowns stored by `ability:item tag`. Evidence: `crates/thedas-sim/src/quickbar.rs:164-166`, `crates/thedas-sim/src/combat.rs:110`. Size: S. Depends: none. Kind: provisional.
- **SIM-071** **Spell combinations.** Original: spell combinations. Ours: no code found. Evidence: `crates/thedas-sim/src/combat.rs:1`. Size: M. Depends: none. Kind: missing.
- **SIM-072** **Shapeshifting.** Original: shapeshifting. Ours: no code found in these modules (engine.rs restores the appearance for `-1`). Evidence: `crates/thedas-sim/src/engine.rs:2936`. Size: M. Depends: none. Kind: missing.
- **SIM-073** **Summons.** Original: summoned creatures. Ours: no code found. Evidence: `crates/thedas-sim/src/sim.rs:1109-1111`. Size: M. Depends: none. Kind: missing.
- **SIM-074** **Poisons and weapon coatings.** Original: poisons. Ours: no code found. Evidence: `crates/thedas-sim/src/inventory.rs:54-59`. Size: M. Depends: SIM-080. Kind: missing.

### AI and tactics

- **SIM-075** **Tactics preset application.** Original: not traced. Ours: a preset's `tacticsuserpresets.gda` row names an M2DA table whose rows fill slots from 1; followers without slots get their first added preset; the parameter column is taken as the fifth. Evidence: `crates/thedas-sim/src/combat.rs:926-958`, `crates/thedas-sim/src/combat.rs:996-997`. Size: S. Depends: none. Kind: provisional.
- **SIM-076** **Custom tactics presets are not kept.** Original: `LoadPreset` type 1 and `SavePreset` (custom presets). Ours: messages accepted and ignored. Evidence: `crates/thedas-sim/src/tactics.rs:11-12`, `crates/thedas-sim/src/tactics.rs:28-29`. Size: S. Depends: none. Kind: missing.
- **SIM-077** **Tactics slot writes are not traced in the exe.** Original: handlers `LoadPreset` 0x7ae4b0, `DirtyPreset` 0x7ad100, `SetBehaviour` 0x7ad120 are traced; slot writes are not. Ours: slot writes read from tactics.gfx. Evidence: `crates/thedas-sim/src/tactics.rs:11`. Size: S. Depends: none. Kind: provisional.
- **SIM-078** **`ListTacticAbilities` target-type test.** Original: 0x7aed50 tests the target type's bits against 7 (always set). Ours: the target type limits nothing, kept as observed. Evidence: `crates/thedas-sim/src/tactics.rs:231-234`. Size: S. Depends: none. Kind: provisional.
- **SIM-079** **Hostile AI packages and ambient behaviour.** Original: AI packages for hostiles, ambient animations, patrols, commoner walking. Ours: no engine code found; hostile AI runs only as scripts, ambient animation and patrol behaviour have no code (waypoint walks now run, see SCR-055). Evidence: `crates/thedas-sim/src/combat.rs:3-5`. Size: L. Depends: SIM-002. Kind: missing.

### Party

- **SIM-080** **Party pool membership.** Original: not traced. Ours: creatures with follower state ACTIVE, AVAILABLE, SUSPENDED or LOCKEDACTIVE form the pool and travel with the party, those out of it inactive. Evidence: `crates/thedas-sim/src/sim.rs:1109-1135`. Size: S. Depends: none. Kind: provisional.
- **SIM-081** **Party picker fill when nobody picks.** Original: `ApplyPartyPickerChanges` 0x636f50 is traced; the headless fill is not. Ours: the party as it was, free places filled from the pool in recruit order. Evidence: `crates/thedas-sim/src/sim.rs:1286-1300`. Size: S. Depends: SIM-080. Kind: provisional.
- **SIM-082** **Pool experience sharing is a non-original harness option.** Original: companions in camp get nothing during fights; module_core tops them up on `PARTYPICKER_INIT`. Ours: optional `pool_xp`, off by default. Evidence: `crates/thedas-sim/src/world.rs:580-582`, `crates/thedas-sim/src/levelup.rs:29-37`. Size: S. Depends: none. Kind: provisional.
- **SIM-083** **Party camp.** Original: party camp. Ours: no code found (only the party picker and travel scripts' transitions). Evidence: `crates/thedas-sim/src/worldmap.rs:389-393`. Size: M. Depends: none. Kind: missing.
- **SIM-084** **Gifts.** Original: follower gifts. Ours: no code found. Evidence: `crates/thedas-sim/src/inventory.rs:559-565`. Size: M. Depends: none. Kind: missing.
- **SIM-085** **Romance flags.** Original: romance flags. Ours: no code found (plot flags only). Evidence: `crates/thedas-sim/src/plot.rs:1-9`. Size: S. Depends: none. Kind: unverified.
- **SIM-086** **Approval.** Original: approval. Ours: no code in these modules; engine.rs keeps an `Approval` field. Evidence: `crates/thedas-sim/src/engine.rs:2657-2670`. Size: S. Depends: none. Kind: unverified.
- **SIM-087** **Mabari dog and dominance marks.** Original: Mabari dominance marks. Ours: no code found. Evidence: `crates/thedas-sim/src/sim.rs:1109`. Size: S. Depends: none. Kind: missing.

### Items, inventory and stores

- **SIM-088** **Inventory event layout.** Original: not traced. Ours: receiver gets `INVENTORY_ADDED` (creator previous owner, object 0 item), previous owner `INVENTORY_REMOVED`; `ITEM_SEND_ACQUIRED_EVENT` items tell the module; `UNEQUIP` integer 1 taken to mean the same as `EQUIP`'s. Evidence: `crates/thedas-sim/src/inventory.rs:3-7`, `crates/thedas-sim/src/inventory.rs:229-232`. Size: S. Depends: none. Kind: provisional.
- **SIM-089** **Shared party backpack.** Original: not traced. Ours: one pack for the party; looted items go to the hero; containers take nothing back. Evidence: `crates/thedas-sim/src/inventory.rs:275-277`, `crates/thedas-sim/src/container.rs:7-9`. Size: S. Depends: none. Kind: provisional.
- **SIM-090** **Worn item properties read from table contents; on-hit properties and restrictions not applied.** Original: not traced. Ours: kinds (1 worn, 2 on hit, 3 restriction, 4 other), power rule and all-attributes row inferred; only worn-kind properties become effects. Evidence: `crates/thedas-sim/src/inventory.rs:24-31`, `crates/thedas-sim/src/inventory.rs:54-59`. Size: M. Depends: none. Kind: provisional.
- **SIM-091** **Equipment restrictions are not checked.** Original: equipment restrictions (attribute and class requirements). Ours: `equip_item` checks only that the item fits the slot and the wearer is in the party; no requirement or two-handed/off-hand exclusion code found. Evidence: `crates/thedas-sim/src/inventory.rs:305-358`. Size: M. Depends: SIM-090. Kind: missing.
- **SIM-092** **Item stat modifiers ignore material while `GetItemStat` scales by it.** Original: not traced (script.ldf says GetItemStat includes the material modifier). Ours: property modifiers from equipped items are not scaled by material or quality, while `GetItemStat` applies `materialrules.gda`; the comment also says items' own property lists are "not applied yet", which save revision 1 contradicts. Evidence: `crates/thedas-sim/src/inventory.rs:757-760`, `crates/thedas-sim/src/inventory.rs:789-795`, `crates/thedas-sim/src/save.rs:21-22`. Size: M. Depends: none. Kind: bug.
- **SIM-093** **`ScaleEquippedItems` at level 0 or below.** Original: reads the area's level from table 0xe1 and skips already-scaled creatures. Ours: reads tier 0. Evidence: `crates/thedas-sim/src/inventory.rs:448-450`. Size: S. Depends: none. Kind: provisional.
- **SIM-094** **Recipe and coin receipt owner rules.** Original: 0x740a58 (recipes), 0x7409fa (coin). Ours: "the party" taken as hero and followers for recipes; coin compared against the hero; no inventory events sent for coin. Evidence: `crates/thedas-sim/src/inventory.rs:625-629`, `crates/thedas-sim/src/inventory.rs:657-661`. Size: S. Depends: none. Kind: provisional.
- **SIM-095** **Crafting.** Original: crafting (herbalism, poison-making, trap-making). Ours: recipes are learned into `known_recipes`; no code found that crafts an item from a recipe. Evidence: `crates/thedas-sim/src/world.rs:518-519`. Size: M. Depends: SIM-094. Kind: missing.
- **SIM-096** **`StoreFollowerInventory` behaviour.** Original: not traced. Ours: equipped items moved with their slots remembered; `equipped` false leaves them worn. Evidence: `crates/thedas-sim/src/inventory.rs:838-841`. Size: S. Depends: none. Kind: provisional.
- **SIM-097** **Body bag money.** Original: the bag refers back to the creature and is looted from its inventory. Ours: money moves into the bag like items. Evidence: `crates/thedas-sim/src/inventory.rs:1012-1018`. Size: S. Depends: none. Kind: provisional.
- **SIM-098** **Template equipment slot ids.** Original: 0x6bcc30 reads `Equip_ItemList`. Ours: each struct's type id taken as `INVENTORY_SLOT_*`. Evidence: `crates/thedas-sim/src/sim.rs:3237-3239`. Size: S. Depends: none. Kind: provisional.
- **SIM-099** **Store prices.** Original: not traced. Ours: value is template `Cost` times stack size; sell at `MarkUp`%, buy at `MarkDown`%, capped by `MaxBuyPrice`; no material, quality or property factor. Evidence: `crates/thedas-sim/src/store.rs:3-7`, `crates/thedas-sim/src/store.rs:74-104`. Size: M. Depends: none. Kind: provisional.
- **SIM-100** **Store trades whole stacks for the hero only.** Original: not traced. Ours: quantity argument unused; buyer/seller argument unused (buyer is always the hero, party money is the hero's). Evidence: `crates/thedas-sim/src/store.rs:198-215`, `crates/thedas-sim/src/store.rs:113`. Size: S. Depends: none. Kind: provisional.
- **SIM-101** **Stealing.** Original: stealing. Ours: no code found. Evidence: `crates/thedas-sim/src/store.rs:1`. Size: M. Depends: none. Kind: missing.
- **SIM-102** **Encumbrance and inventory size.** Original: inventory size limit and backpack upgrades. Ours: no code found. Evidence: `crates/thedas-sim/src/inventory.rs:559-565`. Size: S. Depends: none. Kind: missing.
- **SIM-103** **Set bonuses.** Original: item set bonuses. Ours: no code found. Evidence: `crates/thedas-sim/src/inventory.rs:54`. Size: S. Depends: SIM-090. Kind: missing.
- **SIM-104** **Runes and enchantment.** Original: runes/enchantment. Ours: no code found in these modules (engine.rs mentions sub-items in a property query). Evidence: `crates/thedas-sim/src/engine.rs:3818`. Size: M. Depends: SIM-090. Kind: missing.
- **SIM-105** **Lockpicking, keys and traps.** Original: doors, locks, keys, lockpicking; traps (detect, disarm, set). Ours: no code in these modules; engine.rs answers lock getters and stores `TrapDetected`; trap areas of effect run through aoe.rs. Evidence: `crates/thedas-sim/src/engine.rs:2358-2360`, `crates/thedas-sim/src/engine.rs:2862-2869`. Size: M. Depends: none. Kind: unverified.

### Progression

- **SIM-106** **Auto-level table reading.** Original: not traced. Ours: column 5 of rows 1-6 weighs attributes, columns 1-4 list abilities; tried while points remain. Evidence: `crates/thedas-sim/src/levelup.rs:76-81`. Size: S. Depends: none. Kind: provisional.
- **SIM-107** **Level-up and chargen event layouts.** Original: `GUICharGen` handlers not traced. Ours: layouts inferred from levelup.gfx, chargen.gfx and sys_chargen. Evidence: `crates/thedas-sim/src/levelup.rs:210-219`, `crates/thedas-sim/src/sim.rs:1494-1511`, `crates/thedas-sim/src/chargen.rs:104-113`. Size: S. Depends: none. Kind: provisional.
- **SIM-108** **Any unknown level-up message closes the screen.** Original: not traced. Ours: the catch-all arm sends `CHARGEN_END`. Evidence: `crates/thedas-sim/src/levelup.rs:272`. Size: S. Depends: none. Kind: unverified.
- **SIM-109** **Attribute requirements.** Original: attribute requirements for items and abilities. Ours: no code found. Evidence: `crates/thedas-sim/src/inventory.rs:305`. Size: M. Depends: SIM-091. Kind: missing.
- **SIM-110** **Headless chargen quick start.** Original: not traced. Ours: `CHARGEN_END` with quick start 0 picks a pre-made character; 3 starts in Lothering. Evidence: `crates/thedas-sim/src/sim.rs:313-316`. Size: S. Depends: none. Kind: provisional.
- **SIM-111** **Default face and unnamed hero.** Original: not traced. Ours: the face screen's first entry when none chosen; an unnamed hero takes the background-defaults name. Evidence: `crates/thedas-sim/src/sim.rs:707-710`, `crates/thedas-sim/src/chargen.rs:120-122`, `crates/thedas-sim/src/sim.rs:3337-3339`. Size: S. Depends: none. Kind: provisional.
- **SIM-112** **Hero import ignored.** Original: `GUICharGen.ImportHero`. Ours: treated as presentation-only, no effect. Evidence: `crates/thedas-sim/src/chargen.rs:25-36`. Size: M. Depends: SIM-160. Kind: missing.
- **SIM-113** **Tattoos not put on the face.** Original: tattoo sliders. Ours: kept but not applied. Evidence: `crates/thedas-sim/src/appearance.rs:10-11`. Size: S. Depends: none. Kind: missing.
- **SIM-114** **Face morph weight readings.** Original: tables 0x7fe650, 0x7fdd00; setter at 0x805800. Ours: some nose weights read through an emulated x87 stack; scale sides from write order; the normal-map pairing is not traced. Evidence: `crates/thedas-sim/src/appearance.rs:341-343`, `crates/thedas-sim/src/appearance.rs:477-479`, `crates/thedas-sim/src/appearance.rs:577-579`. Size: S. Depends: none. Kind: provisional.
- **SIM-115** **Player creature fallback.** Original: built from `default_player`. Ours: without that template, a bare creature tagged `player`. Evidence: `crates/thedas-sim/src/sim.rs:2743-2745`. Size: S. Depends: none. Kind: provisional.
- **SIM-116** **Difficulty storage location.** Original: kept with the client options, not in the save. Ours: kept in the world and saved. Evidence: `crates/thedas-sim/src/chargen.rs:78-81`. Size: S. Depends: none. Kind: provisional.

### Conversations and plots

- **SIM-117** **Conversation walking rule.** Original: not traced. Ours: first start node whose condition passes is spoken; replies are children whose conditions pass; ends when no replies or a reply leads nowhere. Evidence: `crates/thedas-sim/src/dialog.rs:10-13`. Size: M. Depends: none. Kind: provisional.
- **SIM-118** **Show-once field meaning.** Original: not traced. Ours: field 12206 taken as 0 always, 1 per conversation, 2 per game. Evidence: `crates/thedas-sim/src/dialog.rs:46-49`. Size: S. Depends: none. Kind: provisional.
- **SIM-119** **Most conversation node fields are ignored.** Original: not traced. Ours: only 12201, 12202, 12203, 12206, 12207, 12208, 12209, 12211 and 12400 are read; no code reads camera, animation or voice-timing data from nodes. Evidence: `crates/thedas-sim/src/dialog.rs:137-165`. Size: M. Depends: none. Kind: missing.
- **SIM-120** **Ambient conversations play out all at once.** Original: not traced. Ours: between creatures outside the party, played to the end in one go, first reply each time, stopping at a repeat. Evidence: `crates/thedas-sim/src/dialog.rs:518-519`, `crates/thedas-sim/src/dialog.rs:604-605`. Size: M. Depends: none. Kind: provisional.
- **SIM-121** **Game mode after a conversation.** Original: `SetGameMode(4)` at 0x697765; the mode returned to is not traced. Ours: the replaced mode or one a script asked for meanwhile. Evidence: `crates/thedas-sim/src/dialog.rs:555-559`. Size: S. Depends: none. Kind: provisional.
- **SIM-122** **Line end ordering on reply.** Original: not traced. Ours: the answered line ends before the reply's action runs. Evidence: `crates/thedas-sim/src/dialog.rs:637`. Size: S. Depends: none. Kind: provisional.
- **SIM-123** **Cutscene lines that say a conversation's lines.** Original: not traced. Ours: the k-th such line says the k-th line the conversation walks from the line that played it. Evidence: `crates/thedas-sim/src/cutscene.rs:9-13`, `crates/thedas-sim/src/dialog.rs:445-447`, `crates/thedas-sim/src/sim.rs:1025-1028`. Size: S. Depends: none. Kind: provisional.
- **SIM-124** **Party interjections.** Original: interjections by party members. Ours: no code found. Evidence: `crates/thedas-sim/src/dialog.rs:464`. Size: M. Depends: SIM-119. Kind: missing.
- **SIM-125** **Skill checks in conversation.** Original: skill checks (persuade, intimidate). Ours: no engine code found; only condition scripts. Evidence: `crates/thedas-sim/src/dialog.rs:246-247`. Size: S. Depends: none. Kind: unverified.
- **SIM-126** **Plot event layout and file fields inferred.** Original: flag storage 0x75bc70, 0x75ea60, 0x75f7e0 traced. Ours: event carries GUID as string 0, flag int 1, new/old ints 2/3; fields 13003 (reward), 13004 (journal text), 13022 (type), 13010 (priority), 13017 (parent), 13005 (ending flags) inferred from data. Evidence: `crates/thedas-sim/src/plot.rs:5-9`, `crates/thedas-sim/src/plot.rs:37-61`. Size: S. Depends: none. Kind: provisional.
- **SIM-127** **`OBJECT_SELF` for plot scripts.** Original: not traced. Ours: the hero. Evidence: `crates/thedas-sim/src/plot.rs:250-251`. Size: S. Depends: none. Kind: provisional.
- **SIM-128** **Journal content rules.** Original: the plot manager is not traced. Ours: quest/codex rules read from plot files and journal.gfx; script-set flags only; sub-quest parent listing; alerts one per type. Evidence: `crates/thedas-sim/src/journal.rs:19-20`, `crates/thedas-sim/src/journal.rs:236-238`, `crates/thedas-sim/src/journal.rs:288-290`, `crates/thedas-sim/src/journal.rs:365-369`. Size: M. Depends: SIM-126. Kind: provisional.
- **SIM-129** **Tutorials assume Show Tutorials on.** Original: TutorialControl 0x693390, 0x693f10. Ours: option taken as on; a flag with no text queues nothing. Evidence: `crates/thedas-sim/src/journal.rs:174-177`. Size: S. Depends: none. Kind: provisional.
- **SIM-130** **Job boards.** Original: `ShowChantersGUI` 0x662f40 -> 0x6f46a0. Ours: table and movie layouts inferred; the plot column's name unknown. Evidence: `crates/thedas-sim/src/chanters.rs:4-13`, `crates/thedas-sim/src/chanters.rs:33-34`. Size: S. Depends: none. Kind: provisional.
- **SIM-131** **Popup result layout.** Original: not traced. Ours: object 0 owner, integers kind and button (1 first). Evidence: `crates/thedas-sim/src/sim.rs:1137-1140`. Size: S. Depends: none. Kind: provisional.
- **SIM-132** **Achievements.** Original: achievements. Ours: no code in these modules; engine.rs answers that none is unlocked. Evidence: `crates/thedas-sim/src/engine.rs:1568-1570`. Size: S. Depends: none. Kind: missing.

### Cutscenes

- **SIM-133** **Cutscene file fields inferred.** Original: `EC::Cutscene` partly traced. Ours: 5000 length, 5200 actors, 5206 tracks, `ASLN` lines (22, 5301, 5302, 5562); field 5564 taken as voice-over mode; a sound track's actor taken as its source. Evidence: `crates/thedas-sim/src/cutscene.rs:5-9`, `crates/thedas-sim/src/cutscene.rs:56-59`, `crates/thedas-sim/src/cutscene.rs:325`. Size: S. Depends: none. Kind: provisional.
- **SIM-134** **Cutscene length capped.** Original: not traced. Ours: at most 600 s whatever the file says. Evidence: `crates/thedas-sim/src/cutscene.rs:40-41`. Size: S. Depends: none. Kind: unverified.
- **SIM-135** **Cutscene end locals inferred.** Original: not traced. Ours: `CUTSCENE_SET_PLOT`, `CUTSCENE_SET_PLOT_FLAG`, `CUTSCENE_TALK_SPEAKER` read from module locals; left alone for conversation cutscenes. Evidence: `crates/thedas-sim/src/sim.rs:918-921`, `crates/thedas-sim/src/sim.rs:940-942`. Size: S. Depends: none. Kind: provisional.
- **SIM-136** **Camera and track span assumptions.** Original: not traced. Ours: camera looks along local +Y, field of view taken as vertical, 45° default FOV, tracks hold their end value, no-motion actors stand at base, missing transition linear. Evidence: `crates/thedas-sim/src/cuttracks.rs:47-51`, `crates/thedas-sim/src/cuttracks.rs:105-107`, `crates/thedas-sim/src/cuttracks.rs:266-267`. Size: S. Depends: none. Kind: provisional.
- **SIM-137** **Animation track timing and blending.** Original: 0x7db400 stops 0xae7118 short of the span end; blend-out 0x7d9450, 0x7d92e0 under 5405 and 5411. Ours: neither followed. Evidence: `crates/thedas-sim/src/cuttracks.rs:295-297`, `crates/thedas-sim/src/cuttracks.rs:406-410`. Size: S. Depends: none. Kind: provisional.
- **SIM-138** **Camera jump flag 5571 and stage camera kinds.** Original: 0x7df369; kinds 1-3 aimed at 0x7de060, 0x7de220. Ours: 5571 not followed; kinds 1-3 taken as plain cameras; missing stage camera gives no pose. Evidence: `crates/thedas-sim/src/cuttracks.rs:330-334`. Size: S. Depends: none. Kind: provisional.
- **SIM-139** **Jump-to-place precedence.** Original: not traced. Ours: a playing jump wins over motion tracks; keyless motion keeps the last place. Evidence: `crates/thedas-sim/src/cuttracks.rs:430-436`. Size: S. Depends: none. Kind: provisional.
- **SIM-140** **`ActionLinkToActor` (kind 27) not followed.** Original: track kind 27. Ours: spans recorded; while one plays the camera gives no pose. Evidence: `crates/thedas-sim/src/cuttracks.rs:51-53`, `crates/thedas-sim/src/cuttracks.rs:704`. Size: M. Depends: none. Kind: missing.
- **SIM-141** **Other cutscene track kinds are dropped.** Original: VFX, fades, facial animation and lip sync, props, time dilation tracks. Ours: only kinds 4, 11, 13, 17, 18, 24, 25, 27, 38 plus `ASLN`, `ACSP` and `APBM` are read; every other kind falls into `_ => {}`; no code found for VFX, fade, facial/lip-sync, prop or time-dilation tracks. Evidence: `crates/thedas-sim/src/cuttracks.rs:678-745`. Size: L. Depends: none. Kind: missing.
- **SIM-142** **Cutscene actor binding gaps.** Original: `HENCH...` names take follower slots (0x7e3760); a missing stage places at the origin. Ours: `HENCHMAN` names take no object; a missing stage moves nobody; walkmesh snap at end on the grid floor; the end move tells no trigger. Evidence: `crates/thedas-sim/src/cutactors.rs:33-39`. Size: M. Depends: none. Kind: provisional.
- **SIM-143** **World frozen during a cutscene.** Original: not traced. Ours: a playing cutscene holds the world's clock; the remainder of the tick runs after it ends; headless runs skip cutscenes by policy. Evidence: `crates/thedas-sim/src/sim.rs:744-752`, `crates/thedas-sim/src/sim.rs:323-332`. Size: S. Depends: none. Kind: unverified.

### World and areas

- **SIM-144** **Load event order.** Original: not traced. Ours: `MODULE_LOAD`, then per area `SPAWN`, `AREALOAD_SPECIAL`, `AREALOAD_PRELOADEXIT`, hero `ENTER`, `AREALOAD_POSTLOADEXIT`. Evidence: `crates/thedas-sim/src/sim.rs:7-11`, `crates/thedas-sim/src/sim.rs:2732`, `crates/thedas-sim/src/sim.rs:2957-2975`. Size: S. Depends: none. Kind: provisional.
- **SIM-145** **Area lists read.** Original: not traced. Ours: only `CreatureList`, `PlaceableList`, `TriggerList`, `WaypointList`, `StoreList`, `SoundList` and `StageList` are read from `.are`; any other list is ignored. Evidence: `crates/thedas-sim/src/sim.rs:2913-2931`. Size: S. Depends: none. Kind: unverified.
- **SIM-146** **Unloaded areas freeze.** Original: not traced. Ours: objects in an area the party left are parked and their events held until return. Evidence: `crates/thedas-sim/src/sim.rs:832-837`, `crates/thedas-sim/src/sim.rs:1409-1417`. Size: S. Depends: none. Kind: provisional.
- **SIM-147** **Stale doc about areas not reused.** Original: n/a. Ours: an orphan comment says the left area is not reused on return, while `enter_area` reuses known areas. Evidence: `crates/thedas-sim/src/sim.rs:1107-1108`, `crates/thedas-sim/src/sim.rs:2863-2890`. Size: S. Depends: none. Kind: bug.
- **SIM-148** **Module start position parsed but unused.** Original: not traced. Ours: CIF field 17102 is stored in `ModuleInfo::start_position` and never read; field meanings inferred. Evidence: `crates/thedas-sim/src/sim.rs:3497-3512`, `crates/thedas-sim/src/sim.rs:50`. Size: S. Depends: none. Kind: bug.
- **SIM-149** **`VarTable` types.** Original: not traced. Ours: 1 int, 2 float, 3 string, 4 object, 12 resource. Evidence: `crates/thedas-sim/src/sim.rs:3437`. Size: S. Depends: none. Kind: provisional.
- **SIM-150** **`GetObjectByTag` order.** Original: not traced. Ours: current area first, then area-less objects, then the rest. Evidence: `crates/thedas-sim/src/world.rs:824-827`. Size: S. Depends: none. Kind: provisional.
- **SIM-151** **Map pin object type.** Original: not traced. Ours: pins take the "map patch" object type 65536. Evidence: `crates/thedas-sim/src/world.rs:56-57`. Size: S. Depends: none. Kind: provisional.
- **SIM-152** **World map travel flow.** Original: not traced. Ours: from `sp_module_travel` and worldmap.gfx: pin click message layout, `BEGIN_TRAVEL` integers (integer 0 unread), party location kept by the engine, random encounter areas not ending the journey. Evidence: `crates/thedas-sim/src/worldmap.rs:10-15`, `crates/thedas-sim/src/worldmap.rs:170-173`, `crates/thedas-sim/src/worldmap.rs:303-305`, `crates/thedas-sim/src/worldmap.rs:364-367`, `crates/thedas-sim/src/worldmap.rs:389-393`. Size: M. Depends: none. Kind: provisional.
- **SIM-153** **Corpse decay simplifications.** Original: clock waits for the death animation; cap depends on detail level; bag model swap; empty bags removed. Ours: clock from death; cap 20; model swap left to presentation; empty bags kept; pool creatures (`CreatePool`) not modelled. Evidence: `crates/thedas-sim/src/decay.rs:21-23`, `crates/thedas-sim/src/decay.rs:73-76`. Size: S. Depends: none. Kind: provisional.
- **SIM-154** **Time of day and area lighting state.** Original: time of day and area lighting state. Ours: no code found. Evidence: `crates/thedas-sim/src/world.rs:3-6`. Size: M. Depends: none. Kind: missing.
- **SIM-155** **Placeable state machines other than doors.** Original: placeable state machines. Ours: nav reads only door states (`statecnt_door.gda` 0 and 1 closed); no other state controller code in these modules. Evidence: `crates/thedas-sim/src/nav.rs:242-247`. Size: M. Depends: none. Kind: unverified.
- **SIM-156** **The Fade's shapeshift forms and attribute fonts.** Original: Fade shapeshift forms, attribute fonts. Ours: no code found. Evidence: `crates/thedas-sim/src/sim.rs:2582-2583`. Size: M. Depends: SIM-072. Kind: missing.
- **SIM-157** **Landsmeet, final battle and army mechanics.** Original: Landsmeet, final battle, armies. Ours: no code found. Evidence: `crates/thedas-sim/src/sim.rs:1`. Size: L. Depends: SIM-079. Kind: missing.
- **SIM-158** **DLC and Awakening content.** Original: DLC and Awakening. Ours: M2DA merges every worksheet with a table's prefix (e.g. `abi_dlc`); DLC archives are now readable and their `core` folders mounted, and the add-ins' `PRCSCR_*.gda` scripts run when an area list loads (`crates/thedas-sim/src/prcscr.rs`, traced), but they are not run again when add-ins are refreshed while playing, the `_ep1` add-ins' tables are read in the base campaign too (their scripts are missing there), and no expansion module loads (see `FMT-047`). Evidence: `crates/thedas-sim/src/m2da.rs:3-7`, `crates/thedas-sim/src/prcscr.rs:1-16`. Size: L. Depends: none. Kind: missing.
- **SIM-159** **Dragon Age II.** Original: Dragon Age II. Ours: no code found. Evidence: `crates/thedas-sim/src/lib.rs:1`. Size: L. Depends: none. Kind: missing.

### Saves

- **SIM-160** **Original save import.** Original: `.das` saves. Ours: JSON world dump only; no `.das` read or write. Evidence: `crates/thedas-sim/src/save.rs:3-6`. Size: L. Depends: none. Kind: missing.
- **SIM-161** **Autosave and quicksave.** Original: autosave, quicksave. Ours: `DoAutoSave` logs and does nothing; no quicksave code found. Evidence: `crates/thedas-sim/src/engine.rs:2823-2829`. Size: S. Depends: SIM-160. Kind: missing.
- **SIM-162** **Tutorial queue saved.** Original: whether saved is not traced. Ours: saved; a held pause with an empty queue is released on load. Evidence: `crates/thedas-sim/src/save.rs:333-335`. Size: S. Depends: none. Kind: provisional.
- **SIM-163** **Music intensity saved.** Original: not traced. Ours: saved. Evidence: `crates/thedas-sim/src/world.rs:566-568`. Size: S. Depends: none. Kind: provisional.
- **SIM-164** **Sim-level state outside the world is not saved.** Original: not traced. Ours: `Sim` fields such as `following`, `next_follow_ms`, `walk_blocked_by`, `auto_pause_combat` and the policies are not in `SaveGame`. Evidence: `crates/thedas-sim/src/sim.rs:73-139`, `crates/thedas-sim/src/save.rs:87-96`. Size: S. Depends: none. Kind: unverified.
- **SIM-165** **Old own saves may carry lost property bases.** Original: n/a. Ours: pre-revision-3 saves are repaired only for Fatigue and Damage_Shield_points; other damaged bases are reported, not fixed. Evidence: `crates/thedas-sim/src/save.rs:47-57`. Size: S. Depends: none. Kind: provisional.

### Other

- **SIM-166** **Unimplemented engine calls return defaults silently.** Original: n/a. Ours: an engine function `engine::call` does not implement returns the default for its type and is traced as `Unimplemented`. Evidence: `crates/thedas-sim/src/sim.rs:3605-3612`, `crates/thedas-sim/src/trace.rs:44-46`. Size: L. Depends: none. Kind: missing.
- **SIM-167** **English talk tables only.** Original: localised talk tables. Ours: only `*en-us.tlk` is merged. Evidence: `crates/thedas-sim/src/sim.rs:423-431`. Size: S. Depends: none. Kind: missing.
- **SIM-168** **M2DA worksheet precedence.** Original: not traced. Ours: for duplicate row ids the later-sorting worksheet wins. Evidence: `crates/thedas-sim/src/m2da.rs:5-7`. Size: S. Depends: none. Kind: provisional.
- **SIM-169** **Hidden abilities and bar placement.** Original: placement rule not traced. Ours: abilities with icon `hidden` stay off the bar; usable abilities fill free slots lowest first, then quick items; a set ability is not set again elsewhere; quick items grouped by tag. Evidence: `crates/thedas-sim/src/quickbar.rs:6-11`, `crates/thedas-sim/src/quickbar.rs:60-63`, `crates/thedas-sim/src/quickbar.rs:192-194`, `crates/thedas-sim/src/quickbar.rs:238-246`. Size: S. Depends: none. Kind: provisional.

### Checked and looks complete

- Derived attribute contributions to property totals (ten derivation functions, 0x79c760): `crates/thedas-sim/src/derived.rs:1-23`.
- Health and mana/stamina regeneration by combat state and period (0x71b0e0): `crates/thedas-sim/src/combat.rs:409-421`.
- Threat decay, proximity term and table size of five (0x6ab620, 0x741d80), apart from tie order (SIM-060): `crates/thedas-sim/src/threat.rs:1-18`.
- Item stacking rules and stack merge on add (0x755a70, 0x755d10): `crates/thedas-sim/src/inventory.rs:532-565`.
- Plot flag storage, initial values and defined flags above 127 (0x75bc70, 0x75ea60, 0x75f7e0): `crates/thedas-sim/src/plot.rs:21-48`.
- Party picker events `PARTYMEMBER_ADDED`/`_DROPPED`/`PARTYPICKER_CLOSED` (0x636f50): `crates/thedas-sim/src/sim.rs:1160-1166`, `crates/thedas-sim/src/sim.rs:1286-1289`.
- Tactics screen option lists (targets 0x7b0a80, conditions 0x7aeb50, actions 0x7afe80, modes 0x7aef50, items 0x7af6c0): `crates/thedas-sim/src/tactics.rs:98-273`.
- Cutscene curve evaluation (Bézier, linear, hold; 0x7d2440, 0x7ccdf0): `crates/thedas-sim/src/cuttracks.rs:9-17`.
- Cutscene actor binding and end poses (0x7e36b0, 0x7e30b0, 0x7e10a0), apart from SIM-142: `crates/thedas-sim/src/cutactors.rs:3-31`.
- Movie (`APBM`) and sound (`ACSP`) track listing for the presentation: `crates/thedas-sim/src/cutscene.rs:15-23`, `crates/thedas-sim/src/cutscene.rs:316-330`.
- Melee reach constants and body radius from `PERSPACE` (0x6a3dc0, 0x748a40): `crates/thedas-sim/src/combat.rs:1789-1807`.
- Run, combat-run and walk speeds from `apr_base.gda` (0xa63120), apart from SIM-005: `crates/thedas-sim/src/sim.rs:1574-1606`.
- Ability conjure and cast timing from animation tables (0x7445cf-0x744665), apart from SIM-064: `crates/thedas-sim/src/combat.rs:1542-1557`.
- Ranged fire animation timing (0x7a4dd0, 0x7a4f51): `crates/thedas-sim/src/combat.rs:1339-1351`.
- Conditioned modal abilities turned off on equip change (0x6743a0): `crates/thedas-sim/src/combat.rs:838-840`.
- Corpses placed dead (`CREATURE_SPAWN_DEAD`, 0x6a3ad0, 0x6bc4b0): `crates/thedas-sim/src/combat.rs:848-858`.
- Body bag creation and reuse (0x6a90d0, 0x6a92b9): `crates/thedas-sim/src/inventory.rs:1012-1022`.
- `AddItemProperty` / `RemoveItemProperty` with wearer effects (0x665290, 0x665370): `crates/thedas-sim/src/inventory.rs:86-118`.
- Equip event from every equip path (0x6c16e0): `crates/thedas-sim/src/inventory.rs:208-216`.
- Recipe items learned on receipt (0x740a58): `crates/thedas-sim/src/inventory.rs:625-628`.
- Tutorial popup queue and pause (0x693630, 0x693f10): `crates/thedas-sim/src/journal.rs:57-69`.
- Death of a creature and `EVENT_TYPE_DEATH` with killer: `crates/thedas-sim/src/combat.rs:886-913`.
- Effect expiry for temporary effects and `REMOVE_EFFECT` delivery: `crates/thedas-sim/src/combat.rs:666-716`.
- Area-of-effect creation, heartbeat, ENTER/EXIT and removal (0x7a0230, 0x7e4da0, 0x7e5950, 0x7a0520), apart from SIM-068: `crates/thedas-sim/src/aoe.rs:5-21`.
- Area revisit keeps objects and state: `crates/thedas-sim/src/sim.rs:2863-2890`.
- Area transition EXIT/ENTER events for the party: `crates/thedas-sim/src/sim.rs:1398-1448`.
- Closed doors fail moves with `COMMAND_FAILED_PATH_ACTION_REQUIRED` and the door as object 2: `crates/thedas-sim/src/sim.rs:2346-2348`.
- Codex groups and Story So Far log: `crates/thedas-sim/src/journal.rs:15-17`, `crates/thedas-sim/src/world.rs:597-598`.
- Trace and coverage counters: `crates/thedas-sim/src/trace.rs:114-195`.
