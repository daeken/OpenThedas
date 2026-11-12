# OpenThedas gaps: the player's side (QA thread)

Maintained by the QA/critic thread. Last updated 2026-11-12 16:15 UTC against Sera's build **1eb844a**.
Status tags: **confirmed** = seen in our build and checked against data, the exe or an original reference;
**suspected** = seen in our build, but the original's behaviour is from memory or the cause is unclear;
**provisional** = an owner has accepted it and is holding it open on purpose.
Paths are under `OpenThedas-testdata/qa/` unless they start otherwise. Saves are in `qa/saves/` and the top
of `OpenThedas-testdata/`. Captures are headless `thedas-app --save X --capture Y --steps ...` (recipes are
in each run folder's `.log` files).

## 1. Open findings (not fixed)

### Rendering / lighting
| # | What the original does | What we do | Evidence | Status |
|---|---|---|---|---|
| R1 | Characters lit by their room's lights (warm hall light in Highever); NPC hair takes its tint colour; hands match the face | Characters pick up every room's character-rig sun: NPC hair silver-grey (Bryce should be blond-brown, Howe dark), hands paler than faces, orange rim glow on armour (Dwarf Noble), dark Dalish hunters against the sky | run10/hn_o30.png vs wine shots cg7/d3; run5/dnoble_*; run11/dalish_o45.png | confirmed; rendering tracing per-object light selection |
| R2 | Human Noble starting shield: wooden planks, gold rim | Dark teal disc with silver boss | run10/hn_o30.png vs cg7 | confirmed (likely part of R1) |
| R3 | Bright wedding-morning alienage; daylight Ostagar on arrival | Alienage reads as dusk (olive sky, near-black shadows); Ostagar ground dark | run22/alienage.png, run33/ostagar.png | suspected; waiting on Sera's original references |
| R4 | Fade has haze, the Black City, no void | A flat black region at screen right in the Fade (undrawn water) | run14/mage_walk.png | confirmed (water not drawn) |
| R5 | Water surfaces everywhere | No water rendering | general | confirmed |
| R6 | Orzammar guard solid | One guard semi-transparent in the Dust Town opening pan | run9/dc_cut.png | suspected; queued in rendering |
| R7 | Selection/target rings sit on the ground; hostile ring red | Rings ignore depth, float at chest or hip height, draw through characters; hostile ring yellow | run19/duel_late.png, cell.png | suspected |
| R8 | Grass and bushes | Trees now draw as billboards (#311); grass and bushes are missing | run34/wilds_trees.png | confirmed |
| R9 | Head close-up fades out when the camera is inside it | Torso fade works (#314); the head close-up doesn't fade (provisional capsule) | sim-work/ruins/after_bear.json | provisional |
| R10 | Green placeholder-like emissives absent | Green dashes on Tapster's pillars, a green glyph by the bar | run17/tavern.png | suspected |
| R11 | Shianni's bodice solid | Flat or see-through triangle on her bodice | run5/celf_o30.png | suspected |

### Cameras, cutscenes, conversations
| # | Original | Ours | Evidence | Status |
|---|---|---|---|---|
| C1 | Vaughan faces the camera on his line | He faces away in the shot at about 15 s (scene otherwise staged since #319) | run35/vaughan_15000.png | provisional (held for the original's frames) |
| C2 | Exploration camera after the Valendrian scene looks down from height | Starts nearly level (pitch -5°) facing a dark wall | run22/valendrian.png (ce_06_valendrian.json) | suspected; triage trace pending |
| C3 | Camera roll and cut blending in cutscenes | None | #209 known gaps | confirmed (known) |
| C4 | Cutscene creature actors animate and walk | Partly (#273); some shots lack actors or poses | various | partial |

### UI
| # | Original | Ours | Evidence | Status |
|---|---|---|---|---|
| U1 | Inventory screen shows a stack count on the item | Count shown in the store, not on the inventory row | run13/inv_main.png | confirmed |
| U2 | Store prices right-aligned per coin | Coins float left (Ruffle inline images) | run18/store.png | parked (known cosmetic) |
| U3 | Save thumbnails | Sunburst placeholder (the original shows the same for some saves: reference/original/load_game_screen.png) | run30/f5.png | open item, partly parity |
| U4 | In-game Options: "Log In" bar shown? | We show it | run21/opts.png | question; no reference |
| U5 | Item names readable | Item names low-contrast grey in lists | run13/inv_main.png | suspected |

### Simulation / gameplay
| # | Original | Ours | Evidence | Status |
|---|---|---|---|---|
| S1 | Corpses lie 60 s, fade, bone pile or pouch | Built (#303/#316); untested by QA (fight saves wipe without input) | — | untested |
| S2 | Dwarf Commoner: is the hero present in the Rica coaching scene? | Armoured hero stands in it | run5/dcommon_skip.png | suspected; low confidence |
| S3 | Chargen attribute picks land on the hero | Skills and talents confirmed; attribute picks unverified in game | run1 logs | untested |

### Audio
| # | Original | Ours | Evidence | Status |
|---|---|---|---|---|
| A1 | Reverb character per area | Present (#279/#283); ungraded without an original recording | run28/rev.wav | untested vs original |
| A2 | Bodyfall sounds | Built (#290/#297); not counted yet | — | untested |
| A3 | Surface footsteps (stone/grass) | Steps present (#267); surface type ungraded | run22/steps.wav | untested vs original |

### Harness and test gaps (no game bug, but they block coverage)
- No original reference captures yet: under Wine, loading Sera's Cousland autosave exits silently after "Loading - End" (2026-11-12 16:10Z); only main_menu.png and load_game_screen.png exist in reference/original/.
- No audio recording of the original yet (no loopback device; plan: FMOD WAV writer on a patched exe copy).
- Fight saves with a won fight are needed to test corpse decay.

## 2. Accepted as parity or improvements (don't re-file)
Letterbox absent at 16:9; `<emp>` shown plain; the Fade's radial blur; 3 m camera start and no minimum camera distance (the fade is the fix); pause-until-tutorial-queue-empty; money 0 at start; Cyrion in the next room; Duncan's weapons in inventory, unequipped; early empty journals (Dwarf Commoner/Noble, City Elf); pad A on a door closes it (and #312, which skips open doors, is a deliberate improvement); "2 0" prices; the output limiter (the original clips: an improvement); GUI sound 55 silent; hero-no-rustle where the ground has no vegetation; Leske's dialog hub loops on reply 0 (the original's structure).

## 3. Content coverage (from QA runs, sim plus app renders)
| Content | Status |
|---|---|
| Boot, intro movies, start menu, New Game, chargen (all races, classes, backgrounds) | plays; pages graded |
| Human Noble: opening → Highever → siege → Ostagar → Wilds → Joining → Tower → Flemeth → Lothering | plays (sweep chain s01–s16) |
| Dwarf Commoner: home → Dust Town → Commons → Tapster's (Oskias) → Beraht → Proving (Everd) → jail → hideout → Beraht → Duncan → Ostagar → Wilds wolves | plays end to end (qa/saves/dc_01–dc_22) |
| City Elf: house → alienage → Soris → Vaughan → wedding → raid → Valendrian/Duncan → Cyrion | plays (ce_01–ce_07); palace raid **untested** |
| Dalish: clearing → Tamlen → wolves → ruins (spiders, skeletons, Bereskarn) → mirror → camp | plays per sim's recipe; rendered up to the Bereskarn; post-mirror **untested** by QA |
| Mage: Harrowing (Mouse, Sloth, the demon) → tower | plays (mg2_*); Jowan/tower plot after waking **untested** |
| Dwarf Noble: palace → Gorim joins → feast conversations | plays to the feast (dn_*); the Proving/Trian/Bhelen plot **untested** |
| Stores (Olinda), looting containers, level-up, save/load round trip, pause/options/game menu, area map, journal | work (minor items above) |
| World map travel, party camp, companions beyond the prelude | covered only by the Human Noble chain; not QA-graded |
