# Known gaps

What is wrong or missing in OpenThedas compared with the original Dragon Age: Origins, in as much
detail as we can currently give. The [README](../README.md) says what works; this says what does
not, so that people (and agents) can pick items up in parallel without stepping on each other.

Last full pass: 2026-11-12, at main `f5ae46b`. Last refresh: 2026-11-13 16:45 UTC, at main
`b48ed11`. The counts measured against a real install were added at `477f24e`. It is refreshed with the
README status.

## How to read it

The list is split by area. Each area file has up to two parts:

- **The owning stream's list.** Written by whoever works on that area day to day. These entries
  usually say what the original executable does, because they come from traces of it.
- **A code inventory.** Every provisional marker, stub, ignored input and absent feature found by
  reading the code. Each entry has an ID (`RND-043`, `SIM-012`), what the original does where
  that is known, what we do, where in the code, a rough size and what it depends on.

The two parts overlap in places. They are kept apart on purpose for now: the first knows the
original, the second is exhaustive about our code. Where they disagree, trust the stream's list
and fix the inventory.

Kinds: **bug** (we do something wrong), **missing** (no code), **provisional** (a guess that
works until someone traces the original), **unverified** (written, never checked).

Sizes: **S** under a day, **M** a few days, **L** a week or more.

"Original: not traced" means nobody has yet checked what the original does. Parity is the goal,
so an untraced item starts with a trace, not with a guess.

## The areas

| Area | File | Inventory items |
| --- | --- | --- |
| Simulation and gameplay | [gaps/simulation.md](gaps/simulation.md) | 161 (`SIM-`), plus ability coverage by family |
| Script VM and engine functions | [gaps/scripting.md](gaps/scripting.md) | 96 (`SCR-`), plus the measured lists of unimplemented functions, unraised events, idle commands and stored-only effects |
| Rendering | [gaps/rendering.md](gaps/rendering.md) | 152 (`RND-`) |
| Interface, input and controller | [gaps/ui.md](gaps/ui.md) | 180 (`UI-`), plus the movies never opened and the values no screen gets answered |
| Audio | [gaps/audio.md](gaps/audio.md) | 55 (`AUD-`) |
| Cameras, cutscene presentation, startup | [gaps/cameras-cutscenes.md](gaps/cameras-cutscenes.md) | 41 (`CAM-`) |
| File formats and file system | [gaps/formats.md](gaps/formats.md) | 47 (`FMT-`), plus a table of every resource type and the tables and types nothing reads |
| Tooling, platforms and project | [gaps/tooling.md](gaps/tooling.md) | 23 (`TOOL-`) |
| What a player sees (QA) | [gaps/qa.md](gaps/qa.md) | open findings, accepted differences, content coverage |

## The big pieces

Whole systems that are absent or only sketched. Most of the individual items hang off one of
these, so they are the places where one person or stream can take a large, separable job.

### Simulation and scripting

- **Many commands do nothing.** Attack, ability, the moves (including move-away and multi-point
  walks), jumps, turn, wait, weapon switch, conversation and use run. Play-animation, sheathe,
  death blow, fly and the rest sit in the queue for half a second and report success
  (`SIM-001`, `SIM-002`, `SCR-055`). Scripts build the idle ones at about 1,200 call sites, led
  by play-animation (829) and fly (353).
- **Effects are mostly stored, not enforced.** The engine acts on death, resurrection,
  area-of-effect, upkeep and damage over time. Other effect types go to the target's script and
  are then only kept; the effect table's flags that should block movement, combat or talents
  block nothing (`SCR-045`). Movement-speed effects (built at 455 script sites) now change speed.
- **Some engine events are still not raised.** About 55 of the 111 named event types are raised
  by the engine. Tracing every sender in the original showed that damaged, spell-cast-at,
  combat-initiated, unlocked and five others come only from scripts, so nothing is owed there.
  Still missing with an engine sender in the original: item on-hit, trap-triggered,
  placeable-collision, placeable-click, gift-item and use-plot-action (`SCR-063`,
  [gaps/scripting.md](gaps/scripting.md)).
- **Missing engine functions fail silently.** A function with no implementation returns a
  default and the script carries on down the "nothing there" branch (`SCR-001`). Measured against
  a real install, 66 functions that game scripts call are unhandled, at 1,862 call sites; the
  full list by call count is in [gaps/scripting.md](gaps/scripting.md).
- **Movement is a timed path, not the original's physics controller.** No step offset, slope
  limit or wall sliding, and the pathfinder is an untraced grid search (`SIM-011` to `SIM-013`).
  Creature bodies do block each other (#320, #322, #329; graded by QA).
- **No patrols or ambient life.** Nothing walks a multi-point route, and there is no ambient
  animation, commoner walking or engine-side behaviour for idle creatures (`SIM-079`).
- **Blows and projectiles are not timed by animation.** In the simulation both hands land when
  the attack ends, and projectiles arrive instantly (`SIM-041`, `SIM-047`, `SIM-063`).
- **Traps.** No detect, arm, trigger or disarm logic (`SCR-082`).
- **Cutscene tracks.** A handful of track kinds are read; effects, fades, props, time dilation
  and facial tracks are dropped, and `LinkToActor` cameras are not followed (`SIM-140`,
  `SIM-141`).
- **Saves are our own format.** Original `.das` saves cannot be read or written, and
  character import is ignored (`SIM-160`, `SIM-112`, `FMT-019`).
- **Beyond the prelude is largely unexercised.** All six origins play past their openings and
  the Human Noble chain reaches Lothering. The world map, party camp and later companions have
  only that one chain behind them; the Landsmeet, the final battle, the DLC campaigns, Awakening
  and Dragon Age II have nothing (`SIM-157` to `SIM-159`, [gaps/qa.md](gaps/qa.md)). Even the
  Human Noble chain only survives with a developer heal script running 63 times.
- **Most abilities have never run.** Of 550 abilities at most 156 are mentioned in any test or
  log, and no test asserts what a single ability does; 138 of 147 monster abilities are
  untouched ([gaps/simulation.md](gaps/simulation.md)).

### Rendering

- **Shader families outside the default set** fall back to a hand-written stand-in or are
  skipped (`RND-112`, `RND-113`).
- **SpeedTree.** Trees draw as real geometry on the game's own shaders, with distance levels,
  billboards that fade in far away and wind sway. Some LOD and billboard details and the
  wind's per-tree details are provisional (`RND-075` to `RND-078`).
- **Effects on creatures and items.** Spell effects and impact flashes draw their particles
  on the body, but not their meshes or lights, and weapon glows are not drawn
  (`RND-094`).
- **Animation blending.** No blend weights, crossfades, inverse kinematics or root motion
  (`RND-064` to `RND-068`).
- **Faces.** Conversation lines, cutscenes' own voiced lines and lines said in passing are lip synced
  (FaceFX), but narration mixed into a cutscene's sound is not, animations' morph tracks are skipped, and head-morph bone offsets are read
  but not applied (`RND-050`, `RND-069`, `RND-070`, `FMT-027`, `AUD-052`, `CAM-013`).
- **Lighting.** Level geometry still uses an older light-picking rule; spot lights, animated
  lights and point or spot shadows are missing (`RND-025` to `RND-030`, `RND-046`).
- **Visibility and detail.** Frustum culling only; no occlusion culling, mesh level of detail or
  terrain detail levels (`RND-001` to `RND-003`).
- **Video options.** Graphics detail, VSync, full screen, resolution and gamma have no effect
  from the options screen (`RND-108`, `RND-110`, `RND-150`, `RND-151`).
- **Screen effects.** Distortion other than heat haze from particles (`RND-107`), cutscene
  screen effects other than depth of field and fades (`RND-158`), script-driven screen effects,
  anti-aliasing,
  decals, trails, weather and area-map fog are not drawn; transparent draws
  are not sorted.

### Interface and input

- **Screens never built**: crafting, item upgrade, the slideshow, the army
  screen, the party camp gift screen, and the online, downloadable-content and
  achievement screens (`UI-052`, `UI-126`, `UI-127`, and the stream's list).
  Of the install's 68 interface movies, 37 are never opened.
- **Screens ask for values nothing answers**: 220 on the HUD (quickslot cooldowns, current and
  pending actions, summons), 170 on spells and talents, 98 on skills, 42 in the inventory
  ([gaps/ui.md](gaps/ui.md)).
- **Options are not saved**, and most video, control and game options change nothing
  (`UI-053` to `UI-066`).
- **Keys cannot be rebound.** The key map is hard-coded (`UI-059`), and the movement keys
  borrow the pad stick's walk (`UI-213`).
- **No selection box or group selection with the mouse** (`UI-123`).
- **Controller.** No button glyphs or prompts, no radial menu, no remapping, rumble or
  non-Xbox layouts (`UI-172`, `UI-180`). Controller support is this project's addition; the PC
  original has none.
- **English only.** Other languages, their fonts and other aspect ratios are unchecked
  (`UI-184` to `UI-190`).

### Audio

- **Event detail.** Only volume envelopes are applied; pitch, pan and reverb-level envelopes
  and layer effects are ignored (`AUD-003`, `AUD-004`).
- **Music.** No streaming: every sample is decoded whole (`AUD-026`).
- **Reverb.** The secondary reverb instances are not modelled.

### Platform, formats and project

- **DA:O DLC passwords on editions other than GOG are found by a search** over the archive's
  digest (a few seconds once, then kept in the player's data folder), not yet tried on a real
  non-GOG install. DA2 DLC needs the player's keys file, and the XOR reader is checked only
  synthetically (`FMT-001`, `FMT-002`).
- **Only the base campaign can be played.** Awakening, DLC campaigns and toolset modules cannot
  be selected (`FMT-014`).
- **Windows and Linux are untested**, continuous integration is switched off, there is no
  packaging, the install is never found automatically, and the regression harness lives outside
  the repository (`TOOL-019` to `TOOL-023`, [gaps/tooling.md](gaps/tooling.md)).
- **Facial animation data has no reader.** The 308,332 `.fxe` entries, most of the install by
  count, are not parsed ([gaps/formats.md](gaps/formats.md)).
- **Tables nothing reads**: load hints (351 per-area lists), gestures, synchronised animations,
  crafting recipes, item properties, gift values, difficulty and auto-balance settings
  ([gaps/formats.md](gaps/formats.md)).

## Small bugs worth a quick fix

Concrete defects found during the code read. Each is small and independent.

| ID | What |
| --- | --- |
| `SCR-059` | A command pushed to the front leaves the displaced command with its old timer |
| `SCR-074` | Comparing two effects, locations or events compares handles, so equal values differ |
| `SIM-148` | The module's start position is parsed and never used (barely visible: single player's start area is left at once) |
| `RND-043` | Alpha-tested casters are skipped, so foliage, fences and hair cast no sun shadow |
| `RND-095` | Location effects with an instant duration are discarded and never drawn |
| `RND-139` | Compressed textures lose their smallest mip levels |
| `FMT-009` | The `Offers/` folder is not mounted for resources, though audio reads it |

Several stale comments and README sections were also found; they are listed in the area files
(`UI-145`, `CAM-036`, `TOOL-001` to
`TOOL-003`).

## What this list cannot see yet

- **Data-side coverage is static.** The measured sections (engine functions, events, commands,
  effects, interface movies, tables, resource types, abilities) count references in compiled
  scripts and in our code. They do not say what the original does with each, and "no known
  reader" for a table is inferred. They were gathered at main `1eb844a` and are not re-measured
  on every refresh.
- **Behaviour that looks implemented but differs from the original.** The inventory reads our
  code, not the original's. Play-testing against the original finds these, and they land in
  [gaps/qa.md](gaps/qa.md).
- **The late game.** Nothing past Lothering has been played with a critical eye.

## Keeping it useful

- Taking an item: say so in your pull request title or description with its ID.
- Fixing an item: delete its line in the same pull request. If only part is done, rewrite the
  "Ours" text.
- Finding a new gap: add it to the area file with the next free number. Do not renumber.
- If you trace the original for an item, put what you found in "Original", with addresses.
- Never paste game text or data here. Names of resources, tables and functions are fine.
