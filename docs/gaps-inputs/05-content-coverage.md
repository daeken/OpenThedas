# 05: Content coverage and open items

Gathered 2026-11-12 against main 1eb844a. Sources: OpenThedas-testdata/README.md (sweep chain), gate/, qa/SCORECARD.md,
qa/run*/grades.md, notes/*.md. Each was checked against `git log --oneline` (535 commits). Paths are under
OpenThedas-testdata/ unless noted. No game text.

## How far each origin and the main quest play

| content | last step that passes | first thing that fails or is untested | evidence |
| --- | --- | --- | --- |
| Human Noble and main quest (sweep chain, sim only, thedas-mcp) | s01 Cousland → s04 siege → s07 Ostagar arrival → s08 Wilds → s11 Joining → s12 Tower → s13 beacon/Flemeth's hut → s14 Lothering → s15 Chanter's Board → s16 first camp (cam104ar_camp_arch1) | Needs a survival aid: the developer `healplayer` script runs whenever a member drops under 35% health, 63 times in all (32 in the Tower alone). Without it the party wipes in the Wilds. Past camp, nothing plays end to end: the Redcliffe night and Orzammar ambush scenarios wipe with and without `--prepare`, and so does tower3 (the Tower without the mage, a harness edit) | README.md; logs/s01–s16_*.log; notes/handoff-sim.md §5 |
| Regression gate | s04_night, s07_father, s11_joining, s14_lothering, s15_board: both binaries reach the same end areas (last run on file 2026-11-12 09:55 local) | Covers the Human Noble chain only. No gate chain exists for the other five origins | gate/A.sum, gate/B.sum, gate/run_A/logs |
| Dwarf Commoner | home → Dust Town → Commons → tavern → Beraht → Proving → jail → hideout → Duncan → Ostagar → Wilds wolves (dc_01–dc_22) | Nothing past the Wilds wolves | notes/gaps-qa.md §3; qa/saves/dc_* |
| City Elf | house → alienage → Soris → Vaughan → wedding → raid → Valendrian/Duncan → Cyrion (ce_01–ce_07) | Palace raid untested; Vaughan scene facing (C1) | gaps-qa.md §3 |
| Dalish Elf | clearing → Tamlen → wolves → ruins → mirror → camp (sim recipe); rendered up to the Bereskarn | Post-mirror untested by QA | gaps-qa.md §3; notes/dalish-ruins-diagnosis.md |
| Mage | Harrowing (Mouse, Sloth, the demon) → tower (mg_*, mg2_*) | Jowan/tower plot after waking untested | gaps-qa.md §3; qa/saves/mg2_* |
| Dwarf Noble | palace → Gorim joins → feast conversations (dn_*) | Proving/Trian/Bhelen plot untested | gaps-qa.md §3; qa/saves/dn_* |
| Companions, world map, camp beyond the prelude | Human Noble chain only (s14–s16) | Not graded by QA | gaps-qa.md §3 |

## Open or provisional items not fixed on main (one line each)

### QA findings (qa/; from notes/gaps-qa.md, rebuilt against 1eb844a)
- R1 characters lit by every room's character-rig sun (hair grey, hands paler than faces, rim glow): qa/run10/hn_o30.png, run5/dnoble_*, run11/dalish_o45.png
- R2 Human Noble starting shield wrong colours (probably R1): qa/run10/hn_o30.png
- R3 alienage reads as dusk, Ostagar ground dark: qa/run22/alienage.png, run33/ostagar.png; notes/handoff-render.md (camera.w = 1, exe value untraced)
- R4/R5 water not drawn (black region in the Fade, water missing everywhere): qa/run14/mage_walk.png
- R6 a guard draws semi-transparent in the Dust Town opening pan: qa/run9/dc_cut.png
- R7 selection rings ignore depth and float; the hostile ring is yellow, not red: qa/run19/duel_late.png, cell.png
- R8 grass and bushes missing; trees are interim billboards (#311); SpeedTree .spt parsed but not drawn: qa/run34/wilds_trees.png; notes/speedtree.md
- R9 the head close-up doesn't fade (provisional capsule): sim-work/ruins/after_bear.json
- R10 green placeholder-like emissives in Tapster's: qa/run17/tavern.png
- R11 see-through triangle on a bodice: qa/run5/celf_o30.png
- C1 Vaughan faces away at about 15 s (provisional): qa/run35/vaughan_15000.png
- C2 exploration camera after the Valendrian scene starts level against a wall: qa/run22/valendrian.png. Related, from notes/camera-start-and-fade-shape.md: the follow camera is never reset after a load, cutscene or movie, and zoom isn't kept across loads (follow.rs has no reset)
- C3 no camera roll or cut blending in cutscenes: #209 notes
- C4 cutscene actors only partly animate (#273)
- U1 inventory rows show no stack count: qa/run13/inv_main.png
- U2 store coin icons left-aligned (Ruffle inline images, parked): qa/run18/store.png
- U3 save thumbnails are a placeholder: qa/run30/f5.png
- U4 in-game Options shows a "Log In" bar (no reference to check against): qa/run21/opts.png
- U5 item names low-contrast in lists: qa/run13/inv_main.png
- S1 corpse decay built (#303/#316) but not tested by QA
- S2 hero present in the Rica coaching scene (low confidence): qa/run5/dcommon_skip.png
- S3 chargen attribute picks unverified in game: qa/run1 logs; qa/SCORECARD.md
- A1 reverb ungraded against the original: qa/run28/rev.wav
- A2 bodyfall sounds untested
- A3 footstep surface type ungraded: qa/run22/steps.wav
- Loading a save from the start menu headless leaves screens on {StartMenu} with no HUD (believed to be a harness quirk): qa/SCORECARD.md 14:45Z
- Harness: no original reference captures beyond the menus, no audio recording of the original, and no won-fight saves for testing corpse decay: notes/gaps-qa.md; reference/original/notes.txt
- (fixed since: save list "Level 0" by #298; regen and ability timing branches merged as #178/#179; engine grunts #241; equip events #223; da_fade #229)

### Simulation (notes/handoff-sim.md and trace notes)
- Living non-combatants have no health, so if a script turns one hostile it can't be killed (#165 covers only CREATURE_SPAWN_DEAD): notes/handoff-sim.md §7.1
- Regen details still open: whether paused time counts, whether only loaded creatures update, order within the update: handoff-sim.md §7.2
- Ability timing untraced: tree search order, the 1.5 s cast with no animation, projectile flight, `_end` animation, .evt field 4017: handoff-sim.md §7.3
- Pathing goes through locked doors when a longer unlocked route exists: handoff-sim.md §7.4
- No perception through closed doors (Tower seed 8): handoff-sim.md §7.4
- Melee-ring-full and door-blocked retries every 100 ms (provisional, #150): handoff-sim.md §7.4
- The dog trails the hero by a level; `--pool-xp` is provisional: handoff-sim.md §7.4; notes/xp-trace.md
- The harness doesn't kite or focus-fire, so the Redcliffe and Orzammar scenarios are lost: handoff-sim.md §5
- Creature bodies don't block each other on main (branch m1-body-block): notes/gaps-sim.md A1; notes/creature-collision-trace.md
- Combat state change while a conversation is pending: the GUI side is untraced: notes/conversation-in-combat-trace.md
- Combat mode end check: uncertain or untraced points listed in notes/combat-mode-trace.md ("Uncertain / not traced")
- Equip events: what fills +0x28c and the UNEQUIP on destroy path are untraced: notes/equip-events-trace.md "Uncertainties (open)"
- Stacking: loot pickup, store buying, GroupSize/InfiniteStock and GUI split messages untraced; INVENTORY_ADDED details provisional: notes/item-stacking-trace.md
- Body bag ageing in areas the player isn't in is untraced: notes/bodybag-decay-trace.md
- Follow formation: the consumer of +0xb0c and the catch-up formula are untraced: notes/follow-formation-trace.md
- Perception: StealthRadius/StealthMode untraced: notes/perception-trace.md
- GetMaxLevel's module field and GetPlotFlagRewardId untraced: notes/xp-trace.md
- .phy: double-sided rays, runtime mask changes (open doors) and other open points: notes/phy-format.md "Open points"
- Full sim gap list (movement physics, projectiles, traps, stealing, summons, shapeshift, crafting, runes, injuries, approval, persuasion): notes/gaps-sim.md

### Rendering (notes/handoff-render.md, character-lights.md, moving-lights.md)
- Moving creatures keep the lights chosen when they were built: handoff-render.md §1; notes/moving-lights.md
- The crowd view's object pass is GPU-bound: handoff-render.md §2
- No Umbra occlusion culling (deferred): handoff-render.md §3
- Provisional shadows: SUN_REACH 60 m, partition overlap, elevation clamp not applied, half-texel bias, casters a frame late: handoff-render.md §4
- Culling is frustum only; the skinned sphere is our own; sky and clouds are never culled: handoff-render.md §4
- The paperdoll is lit by the level, not by paperdoll_lightrig/lightprobe: handoff-render.md §4
- Level light rule traced but not applied; one room per object; per-object light index writer untraced: notes/character-lights.md
- Light influence function (vfunc+0x84) provisional: notes/moving-lights.md
- Fade shapes use the rest pose, not the animated skeleton: notes/camera-start-and-fade-shape.md

### UI and input (notes/handoff-ui.md, movie-controller-mode.md)
- Chargen: tattoos (slots 6–8, mml_tTattooMask), title swirl too bright, Dual Weapon header bar, Expression list: handoff-ui.md "overnight"
- LB+RB whole-party selection: its effect is unverified, and no HUD shows the selection: handoff-ui.md
- The movies' own console focus mode was left out; screens use our own focus navigation: handoff-ui.md; notes/movie-controller-mode.md
- Typing a save name with a pad uses D-pad letters (#284); Elite 2 paddles are ignored (gilrs): handoff-ui.md
- Floaty stats: which creatures get one and when, plus alpha/hover rules, untraced: handoff-ui.md
- Inventory console focus: no first focus at open; NavigateRight from SlotWeapon1 doesn't move: handoff-ui.md
- Unanswered engine values per screen: see 03-gui.md

### Audio (notes/handoff-audio.md)
- PlayMusic is only logged; music switching on markers and the theme system are untraced
- Music level vs the original unverified; Options→category mapping and pan law untraced
- Reverb: algorithm, trigger overlap and secondary instances provisional; 0.15 trigger weight open
- Footsteps: gate 0x7c4a80 traced; vegetation rustles done (#283); the Metal row missing
