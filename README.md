# OpenThedas

An open source replacement engine for Dragon Age: Origins (and later Dragon Age II), written in
Rust, that runs on the original game data. Parity with the original comes first; modernization
(mod management, post-processing, new rendering paths) comes after and must never require breaking
parity.

You need your own copy of the game. Nothing from the original game is, or will ever be, in this
repository.

## Status

Last updated 2026-11-13 16:45 UTC, main b48ed11

For everything that is wrong or missing, in detail and split into items that can be picked up in
parallel, see [docs/KNOWN_GAPS.md](docs/KNOWN_GAPS.md).

Legend: ✅ done, 🟡 partial, ⬜ not started. QA notes come from QA runs 8–20, with fixes merged since then noted.

- ✅ **Boot flow**: EA logo and intro movies play with their sound (decoded via ffmpeg, looked for beside the executable, on the path, and in Homebrew on macOS), then the start menu.
- ✅ **Start menu**: New Game, Load, Options and Quit are wired; QA graded it near pixel-identical to the original.
- 🟡 **Character creation**: full flow with the original's stage, blended camera presets and portrait.
  - Appearance menu: preset, hair, beard, the exe's per-race eye and nose shapes, makeup and complexion. Also colour sliders, difficulty and name.
  - Partial: the remaining face-shape sliders and hair-colour details.
  - QA flagged attribute arrows that never show a disabled state.
- 🟡 **Conversations and cutscenes**:
  - Done:
    - the reply list, subtitles and voice-over, with dialog tags (`<emp>` and the rest) from the Dialog Tags table;
    - movie tracks and cutscene camera tracks;
    - cutscenes named by conversation lines (the Human Noble, Dalish and Dwarf Noble intros);
    - the original's default speaker framing for unstaged lines;
    - cutscene and conversation actors posed and animated by their tracks, and conversation lines leaving creatures on their stage marks in the sim, as the exe does (Oskias sits; the Dalish hunters face the hero);
    - conversations start as the exe starts them: dialog game mode for the talk, and talk commands go through the dialogue event to the target's script. A first line that plays before its screen is up no longer stalls the conversation;
    - cutscenes and conversation lines draw the depth of field and the fades their own tracks ask for, and every conversation gets the original's second depth of field from the game's tables;
    - speaker and listener turn head and eyes to each other, and cutscene actors follow their head-tracking tracks;
    - a cutscene animation's baked root motion moves the actor and stays after the clip (a character who turns stays turned), actors hold their base pose (seated characters sit), and cutscene props, actor visibility and weapon draws are played;
    - cutscene cameras shake as their shake tracks ask;
    - line cutscene cameras are placed with their cutscene's stage, and ActionJumpToCamera puts the camera on stage cameras: the City Elf Vaughan scene plays as staged shots.
  - Open:
    - cameras and actors linked with LinkToActor are placed (provisional: links use the target's root frame);
    - a cutscene's last view is held only for the conversation line that queued it (provisional), which frames the City Elf opening as the original does and frees the next reply's shot;
    - additive animation clips (the `.ani` additive flag, 196 of the 4,520 clips, most of them conversation gestures) are laid over the pose beneath them as the exe does, instead of replacing it; this fixes arms pointing straight up during gestures. Their weight is provisional (`RND-064`), and two other overlay kinds still overwrite (`RND-071`).
- 🟡 **Exploration and camera**:
  - Walking, click-to-pick, doors and companions following. Followers come along on area changes, and they stand in the exe's formation slots and follow by its rules.
  - The area map shows map notes and only perceived enemies.
  - The follow camera uses the exe's 0.25 m sphere sweep, pull-in and ease-out, and its starting shot (3 m behind, at the leader's height). With tutorials on, a new game's first control is the tactical overhead view, as in the original: the first training step zooms fully out (`SetZoomLevel`), and zooming back in by wheel, Page Down or the pad ends the step. Page Up and Page Down zoom while held. The field of view narrows from 60° to 42.5° over the far half of the zoom, as the exe sets it every frame, so the tactical view frames like the original's.
  - Doors are posed by their state (open, Open_2, swinging, fallen), and their camera and click shapes follow the pose.
  - With a pad, walking aims at a point ahead and the route goes round obstacles; it no longer stops dead at the first one.
  - Creatures the camera is inside fade out as the exe does (0.25 m sphere against the models' creature-group physics shapes, head included; opacity 0.05; not in dialog or cutscenes). Provisional: the shapes are placed by rest pose, not the animated skeleton.
  - With a pad, A walks through open doors instead of using them.
  - The follow camera resets behind the leader after a load and at the end of a cutscene, as the exe does, and keeps its zoom across loads.
  - Loading screens: a new area, a save load and a new game build on a worker thread while the window keeps presenting the original's loading movie (load_town for a save load, a new game or a change of area list, with the script's hint; load_light for a change within one area list). The screen goes up before the old area goes, as in the exe: a scripted transition is held one update (33 ms, provisional length), the world holds still, its sounds duck and the camera resets as the world comes up. load_town plays its outro in about 1.6 s at 60 fps. A world map journey keeps the map up as its loading screen. Headless captures load within the frame as before; `THEDAS_ASYNC_LOADS=1` uses the worker there too (frames wait for the load), `THEDAS_HIDDEN_WINDOW=1` runs the window unseen for driving it with `--control` (a test hook), and `THEDAS_NO_LOAD_MOVIE=1` leaves the movie out.
  - The field of view narrows from 60° to 42.5° toward the far, tactical end of zoom, as the exe does. Page Up and Page Down zoom.
  - Creatures glance at the player's character while exploring.
  - Creatures placed by an area stand on its floor, as the exe projects them (this was the "see-through guard" in the Dust Town opening).
- 🟡 **Combat and simulation**:
  - NWScript VM with a large share of the engine functions; melee and ability timing; threat; AI-driven party; tactics; loot.
  - Containers hold their template's items and open on USE with the original loot panel (Take All, Close).
  - Perception and combat mode work as the exe does them. Sight lines are cast through the level's collision (`.phy`) and placeables, and a closed door blocks sight with its own collision. SetCanLevelUp follows unspent points.
  - Weapons are sheathed out of combat and drawn in combat; draw and sheathe animation timing is not traced.
  - Melee approach works in the live app (#269), and the Proving duel can be finished with the pad.
  - Melee attackers take slots on the target's 40-slot melee ring at the bodies' reach, as the exe places them, and step into place before swinging. The default body radius is the exe's 0.6 m.
  - Per-hand attack results are exposed read-only for the presentation (swing and impact sounds).
  - All six origins play past their openings in the simulation (QA).
  - Corpses decay and fade as the exe does them; a body bag takes the corpse's place and draws its corpse model.
  - Live creatures block each other as the exe's capsules do (slide, no push, no re-path; the dead do not block), and a blocked walker keeps its own speed. QA graded it in the live app.
  - Ability functions ignore unknown ability ids as the exe does, and saves drop invalid ones on load.
  - Training tutorials (the original's step-by-step first-play lessons) run from the scripts: SetTrainingMode, BeginTrainingStep, the completed-steps gate and the events that end each step. Every lesson can be finished, with mouse and keyboard or on the pad alone: each step's ending event has a sender and is delivered to the tutorial script at once, as the exe does, including while a screen or a tutorial popup holds the game. As in the exe, saving is refused while a lesson is active, and a load clears it.
  - The engine raises the mana-or-stamina-depleted, area-load-from-save and module pre-save events. Opening a screen tells the module. The damaged event and eight others were confirmed to come from scripts only.
  - Followers walking with the leader take their formation slots.
  - Visual effects applied at places (the Proving crowd) are exposed to rendering and drawn.
  - A finishing move lands the kill the rules stored for it, so bosses (whose death is always a finishing move) can die; before this the Dalish origin's bear could not be killed. Provisional: the blow lands when the command ends, not at the animation's hit event.
  - A walker turned aside by a body no longer walks back to the waypoint it stood on (this stalled the Tower's third floor).
  - Not every script engine function is implemented.
- 🟡 **Party, inventory, journal and other UI screens**:
  - Wired to the simulation: inventory (with paperdoll and item examine), character record, level-up, store, tactics, journal and codex, options, area map, HUD notifications and the screen tab bar.
  - Enemy health bars and the target's name show in fights.
  - Tutorial popups show with their key names. They wait under full-screen menus, and the game stays paused, as in the original, until every queued popup is dismissed (pad A or the panel's Next/Close). The "Paused" banner shows meanwhile, and with a pad the popup title adds "(A: Next)" or "(A: Close)".
  - Fixed since QA run 8: recipes are learned, stacks merge, inventory rows are no longer dimmed, and the Level Up button follows unspent points.
  - New saves are named New Save, the save-name dialog takes typed names, and the save shows the class icon.
  - With a pad: the D-pad types letters into a focused text field, and focus reaches empty equipment slots.
  - Items with no icon of their own show their base type's.
  - Save screens show the hero's name, level and class. The character record lists Heroic Accomplishments.
  - Script message boxes show, answer the script, and pass typed text along (the dog-naming prompt works).
  - The auto-pause and show-tutorials options reach the simulation.
  - Level Up shows the member's portrait. In combat each party portrait shows what that member is doing, and hovering a portrait shows the class.
  - Ability, talent and spell lists are built as the exe builds them. A new codex entry shows the large alert only at the plot priorities the exe uses.
  - Open: screens have not all been audited against the original.
- 🟡 **Rendering**:
  - Working: areas, terrain, sky, fog, clouds, bloom, colour grading, dynamic sun shadows (Low to High detail) and the original shaders.
  - Characters have hair, sampled with its mips and noise. Bare hands and arms take the head's skin tint.
  - Characters and props pick their lights by the exe's rules (room reach, light class, influence).
  - Lava draws with its own shaders. Level-wide layouts draw (horizons, water, tents), with probe cubemaps as the environment map. Missing material textures fall back to the parameter's default, not black.
  - Per-object opacity, for the camera fade and corpse decay. Faded objects cast no shadow, as in the exe.
  - Level terrain and static meshes take no ambient light nodes, as traced; this removed a green cast from daylight ground.
  - Each area is graded with its own colour curve and brightness, contrast, hue and saturation from the area table, as the exe does. Daylight areas now match the original's brightness and warmth (QA: alienage and Commons).
  - Looking down from high above the party cuts away ceilings marked for it in the models, with the exe's height threshold and 0.2 s fade.
  - Only per-frame shader constants are rewritten per draw, and first-use work is done at the end of a load, which took about a fifth off frame time and removed the stutter after arriving in an area.
  - Open:
    - Trees, bushes and grass are real geometry from a clean-room SpeedTree generator written from our own spec (`docs/speedtree/`): trunks, branches, fronds, leaf cards and mesh leaves, with the game's distance rule for switching to billboards, batched per ground cell. Branches, fronds, leaf cards and mesh leaves draw with the game's own shaders, with its fades between distance levels (#427, #431, #432), and sway by the game's wind simulation fed from the level's wind file (#462, #463, #470). The far billboards are a stand-in that does not fade in, and which wind copy a tree takes is provisional (`RND-075`, `RND-076`).
    - Emitters in a cut-away model do not fade with it.
  - The backend stays on wgpu, and its types are kept inside `thedas-render`.
  - Every shader, the original's 610 and ours, is checked in tests against wgpu's translation for Direct3D 12, Vulkan and OpenGL. On a machine that offers only OpenGL (some virtual machines) the game runs there; an effect the backend refuses is logged once and left out. Untested on real OpenGL hardware.
- 🟡 **Audio**:
  - Merged:
    - playback: FMOD event (`.fev`) and bank (`.fsb`) playback, mixer, the exe's slider-to-dB volume mapping, event and sound-definition pitch, and per-play random pitch and volume, so repeated sounds vary;
    - music: menu and area music, SetMusicIntensity driving `music_state` (kept in saves), with changes timed to the music's markers as the original does;
    - world: world reverb (area presets and reverb triggers as traced, levels per I3DL2 with room LF), placed 3D sounds with their cones;
    - movement: footsteps by walkmesh surface, foley and vegetation rustles, as traced;
    - effects: UI sounds, boot movie sound, script PlaySound, animation sound events (swings, armour foley) and bodyfalls; blows sound at the hit frame with the weapon's action;
    - voices: conversation and cutscene voice-over, sound-set and engine barks with the exe's sound-set player (counts, shuffle bag, gender), creature sound-set events, party picker lines, the chargen voice preview.
  - Also merged: the ducking of world sound in conversations, cutscenes, death and loads (#392), music volume triggers, area music carrying on across areas that share a theme, and placeables' animation sounds.
  - Not yet: voice-over positioning, and the master and secondary reverb chain. (The PlayMusic and StopMusic script calls do nothing in the original either, traced in #395.)
  - Sound is off with `--control` (unless `--audio`) and in headless captures.
- 🟡 **Controller**: Xbox 360-style layout; menus and in-game screens use focus navigation, and A dismisses tutorial popups, then opens the screen a paused training lesson waits for (level-up, inventory, tactics). The PC right-click radial menu doesn't exist as a movie in the game data, so LT opens a quickbar picker instead. In the inventory A equips and unequips and Y puts a usable item in the first free quickbar slot; on Tactics the pad reaches the Presets and Behaviour pickers; B in an open dropdown closes only the list.
- ✅ **Saves**: F5/F9 quick save and load, save/load screens with a picture of the game for each save made in a window, typed names. Revision 7 fills older saves' empty containers.
- 🟡 **Keyboard and mouse**: W, S, Q and E walk the leader relative to the camera and A and D turn the camera, as the game's key map binds them; click to move, hover names, and name and health floaties shown as the original shows them (hovered, targeted, or with Tab held). Selection rings are drawn under party members and fighting enemies (when an enemy shows its ring is provisional).
- ✅ **Performance**: frustum culling, shadow caching and skinned-draw culling (Lothering about 8 ms, the chapel about 1.5 ms offscreen). Model node trees are read with a heap stack, so small worker-thread stacks are safe.
- 🟡 **DLC and add-ins**: encrypted DLC archives are read (passwords from a keys file the player supplies, recovered from the player's own GOG Ultimate Edition executable, or on other editions found once from each archive's own digest and kept in the player's data folder; nothing is shipped). Every enabled add-in's `core` folder is mounted, and its `module` folder only when it extends the base campaign, so stand-alone campaigns cannot replace base-game files. Add-ins are found from the original's `AddIns.xml` registry as well as from manifests, which brings in the promotional items' add-ins. The add-ins' area-list scripts run as in the original, so a new game hands out the first DLC item; DLC map pins and camp additions do not appear yet (`FMT-047`). Install paths are matched without regard to case, so the game runs from a case-sensitive file system.
- 🟡 **Issue reports**: F8, both sticks clicked or the menu's Report Issue entry saves a screenshot, the current save, a save from about a minute earlier, the log, recent input and settings to the user's data folder (`Documents\OpenThedas` on Windows), and can send it to the project's report server.
- 🟡 **Mods**: an ordered list of mod layers sits above the whole resource stack (later entries win; nothing is extracted over the game's files), `.dazip` packages in the list are unpacked into a cache and join as add-ins, and the original's folders in the player's Documents are read in the original's places. `.7z` archives are unpacked into the same cache. `THEDAS_MODS=none` turns all of it off; `thedas mods <install>` lists the layers. How to use it: [docs/mods.md](docs/mods.md).
- 🟡 **DA2**: scoping. Archives whose entries carry only name hashes (ERF V3.0) resolve by name; patch talk tables and material files are read; nothing of the game runs yet.

### Known blockers

None open. The Proving armour chest was fixed by #253 and #254.

### Origin openings

- 🟡 **Human Noble**: intro movie and cutscene with narration play. The opening plays through the siege to Ostagar and on to Lothering in the regression sweep.
- 🟡 **Dwarf Noble**: intro cutscene plays and the opening is explorable. Cutscene actors are not animated yet.
- 🟡 **Dwarf Commoner**: intro cutscene cameras play and the camera starts inside the house. The path plays forward through the tavern and the Proving.
  - Oskias sits and the duel can be finished with the pad.
- 🟡 **City Elf**: explorable from inside the room. Cyrion waiting in the next room matches the data. Vaughan's scene plays as staged shots.
- 🟡 **Dalish Elf**: intro cutscene plays and the journal lists the origin quest. After the opening talk the three hunters stay on their marks facing the hero.
- 🟡 **Mage**: explorable.

### Planned modernization

- ⬜ **Background area preloading**: start loading the next area's assets as the player heads toward an exit. Presentation-only; sim behaviour and parity are unchanged. Planned for the rendering stream.

Deliberate differences from the original:
- **Controller support**: Xbox 360 layout. The PC original removed it.
- **Spaces allowed in save names.**
- **Audio output limiter at −0.3 dBFS**: an improvement. The original creates only reverb DSPs and simply clips when a fight sums past full scale.

### Milestones

Milestone M0, "read everything", is done: archive readers, layered resource lookup, and
GFF3/GFF4/GDA/TLK parsers, checked against every file in a DA:O install by the corpus tool.

Milestone M1, "walk through one area", is in progress. Typed readers cover the whole chain from
an area file to the GPU (areas, layouts, room layouts, models, meshes, materials, DDS), and
`thedas-app` loads an area (the Spoiled Princess, `lgt101d`, by default) and lets you fly
through it. World geometry is drawn with the game's own shaders: `thedas-shader` translates the
D3D9 bytecode in `shaders.erf` to naga IR (all 532 shaders translate and validate), and the
renderer binds constants, samplers and render states the way the material files describe.
Materials outside the Static1 family fall back to a hand-written stand-in unless you pass
`--shaders all`.

The interface is the game's own Scaleform movies, played by [Ruffle](https://ruffle.rs) on the
renderer's wgpu device: the main menu, start menu and HUD movies load, render with their fonts
and atlas images, and talk to the engine through the same `fscommand`/`ExternalInterface`
protocol the game used. The HUD shows placeholders until engine values are bound to the
simulation.

## Layout

| Crate | What it does |
| --- | --- |
| `thedas-vfs` | ERF V2.0/V2.1/V2.2/V3.0 and RIM archives, DLC decryption, layered lookup with provenance, install discovery |
| `thedas-formats` | GFF3 and GFF4 readers and writers, GDA tables, talk tables; typed readers for areas (`.are`), layouts (`.arl`, `.rml`), models (`.mmh`), meshes (`.msh`), materials (`.mao`, `.mat`), DDS textures and Scaleform GFx movies |
| `thedas-shader` | D3D9 shader bytecode and constant-table parsing; translation to validated naga IR |
| `thedas-render` | wgpu renderer: loads an area's static geometry, draws each material with its translated original shaders (or a stand-in), captures |
| `thedas-app` | The `thedas-app` binary: a window with a free-fly camera, or a headless capture |
| `thedas-script` | NWScript: the `script.ldf` engine-function table, NCS bytecode parsing and disassembly, and the stack VM |
| `thedas-sim` | The headless, deterministic world: modules and areas loaded from their files, objects, events and the event queue, engine functions for the NWScript VM, plots and conversations |
| `thedas-ui` | Scaleform GFx movies on Ruffle: GFx-to-SWF rewriting with atlas images inlined, offscreen rendering into a wgpu texture, and the engine side of the game's GUI protocol |
| `thedas-mcp` | An MCP server over the simulation, so an AI assistant (or any MCP client) can play and test the game |
| `thedas-cli` | The `thedas` tool: `ls`, `extract`, `dump`, `resolve`, `corpus` |

Planned crates follow the architecture doc: `thedas-video` (movies currently go through an external `ffmpeg`).

## Playing

```sh
./target/release/thedas-app --install "$GAME"
```

With nothing else, the game boots to its start menu (startmenu2.gfx): New Game goes through
character creation, Load Game lists the saves in `~/.openthedas/saves` (`--saves` for another
folder), Options and Quit work. In a game, Escape (Start on a pad) opens the game menu: save,
load, options, and Exit Game back to the start menu. The logo and intro movies the original
plays before the menu are shown when `ffmpeg` is installed (`--no-intro` skips them). `--new-game` (with `--quick-start`)
and `--save <file>` still start a game directly.

## Walking through an area

```sh
./target/release/thedas-app --install "$GAME" --viewer                # the Spoiled Princess
./target/release/thedas-app --install "$GAME" --area lot120ar_danes_refuge
```

WASD moves, Q/E (or C/Space) go down/up, Shift is faster, hold the right mouse button or use the
arrow keys to look, P prints the camera, F12 saves a PNG, Esc quits. `THEDAS_INSTALL` can stand in
for `--install`. `--shaders original` (the default) uses the game's Static1 and Black shaders,
`--shaders all` every original shader that translates, `--shaders standin` none.

For side-by-side comparisons, render a fixed view with no window:

```sh
./target/release/thedas-app --install "$GAME" --capture out.png \
    --camera=-18,6,2.2,-20,-10 --size 1280x720
```

The camera is `x,y,z,yaw,pitch` in world units and degrees; Z is up and yaw 0 looks along +X.

`--ui` plays interface movies over the scene: `--ui menu` (the in-game menu), `--ui hud`
(portraits, quick bar, minimap), or any comma-separated movie names (`--ui startmenu2`).
`--area none` shows the interface alone. With `--capture`, `--ui-frames` sets how many movie
frames run before the shot:

```sh
./target/release/thedas-app --install "$GAME" --area none --ui startmenu2 --capture menu.png
```

How the Scaleform-only parts are handled, all provisional (inferred from the movies' bytecode):

- `DefineExternalImage` (6,419 in a full install): the movies name images that live in the
  `atl_*_dat.xml` texture atlases. `thedas-formats::gfx` rewrites each movie to a plain SWF,
  and `thedas-ui` crops the image out of its atlas page and inlines it as `DefineBitsLossless2`.
- `ExporterInfo` is dropped. Ruffle reads the `GFX`/`CFX` signatures natively.
- Fonts come from `gfxfontlib.gfx` through `importAssets`, served from the game's resources.
  Other locales would need the font-config mapping to `fonts_<locale>.gfx`.
- The game calls movie functions by path (`GFxMovie::Invoke`). A small injected frame-one
  action registers an `ExternalInterface` callback that does the same, so the engine can call
  `_global.GUISystem.StartSceneFromGame` (without it the movies run in standalone Flash mode
  and show raw `LOC_*` keys).
- `ExternalCommands` functions that return values send an `fscommand` and read
  `ExternalCommands.fsc_retval`, which the game set from its handler. Ruffle's handler cannot
  write into the movie, so those functions are wrapped to fetch the answer through
  `ExternalInterface` instead. That is how `GetTlkString` reaches the talk table.
- Engine values (`GetValue`/`GetNumber`/`GetString`, `SetValue`) are a dotted key-value store;
  hundreds of HUD keys (`GUI.SelectedChar.Stats.QuickSlots.N`, `Party.MemberSelected.N`,
  `Area.Minimap*`) are not bound to the simulation yet.

## Using the CLI

```sh
cargo build --release
GAME=~/path/to/Dragon\ Age\ Origins   # the folder holding packages/ and modules/

./target/release/thedas ls "$GAME/packages/core/data/2da.erf"
./target/release/thedas dump "$GAME/packages/core/data/2da.erf:abi_base.gda"
./target/release/thedas resolve "$GAME" abi_base.gda
./target/release/thedas resolve "$GAME" lgt101d_room000.dds --level lgt101d
./target/release/thedas script disasm "$GAME" setplayerimmortal
./target/release/thedas script run "$GAME" setplayerimmortal       # engine calls stubbed, printed
./target/release/thedas script stats "$GAME" --report actions.json  # engine functions by call count
./target/release/thedas sim "$GAME" --area cir110ar_inn --ticks 200 --trace sim.jsonl
./target/release/thedas sim "$GAME" --area cir110ar_inn --talk urn300cr_innkeeper --replies 1,0
./target/release/thedas corpus "$GAME" --report corpus-report.json
```

`corpus` opens every archive in the install, reads every entry, parses every GFF3/GFF4 resource
(including loose files such as talk tables), writes each one back out and checks that it reads
back to the same document, and parses GDA, TLK, area, layout, model, mesh, material (XML) and DDS
files into their typed forms. It exits non-zero
on fatal failures. DLC passwords come from a keys file the player supplies (the format of the
commonly shared `erfpasswords.txt`; `--keys <file>`, else `THEDAS_KEYS`, else `keys.txt` in the
data folder), else for DA:O from the local GOG Ultimate Edition `bin_ship/DAOrigins.exe`; each is
checked against the archive header. Dragon Age II's DLC can only be read with a keys file.
Encrypted entries whose passwords are unavailable are counted in the `encrypted` column.

## Driving the simulation over MCP

`thedas-mcp` is a [Model Context Protocol](https://modelcontextprotocol.io) server (stdio) over the
headless simulation. Register it with Claude Code:

```sh
cargo build --release -p thedas-mcp
claude mcp add thedas -e THEDAS_INSTALL="$GAME" -- "$PWD/target/release/thedas-mcp"
```

Then ask Claude to, say, "load the Spoiled Princess and talk to the innkeeper". Tools:

| Tool | What it does |
| --- | --- |
| `load` | Load a module and area (default module `single player`; e.g. area `cir110ar_inn`) |
| `status` | Time, area, hero, conversation, script errors, most-called unimplemented engine functions |
| `list_objects`, `inspect_object` | Objects by tag and kind; full state of one object |
| `step` | Advance time, delivering events and running commands |
| `get_trace` | What scripts and the engine did since the last call, filtered by kind |
| `interact` | Talk to a creature or use a placeable as the hero |
| `dialog_state`, `choose_reply`, `end_conversation` | Read and drive the current conversation |
| `move_hero`, `enter_area`, `list_areas` | Walk (time runs, triggers on the way fire, followers keep up; or `teleport`) and travel |
| `plot_flag`, `local` | Read or set plot flags and local variables |
| `call_engine_function`, `run_script` | Call any engine function or compiled script directly |
| `chargen_event` | Drive character creation yourself after `load` with `chargen: "manual"` (by default a new game takes the chargen script's defaults and starts the origin) |
| `attack` | Order the hero to attack a creature and run time until the attack resolves (the game's own rules scripts roll hits and damage) |
| `save_state`, `load_state` | Snapshot and restore the world, in memory or to a JSON file (`path`) |
| `ui_open`, `ui_close` | Play interface movies (`menu`, `hud`, `startmenu2`, any movie names) on a headless GPU, bound to the simulation |
| `ui_screenshot` | The open movies as a PNG image |
| `ui_click`, `ui_hover`, `ui_step` | Mouse input at screenshot pixels and frame stepping; return what the movies sent to the engine |
| `ui_invoke`, `ui_values` | Call a movie function by path; see the engine values the interface reads and which ones are still unanswered |

## Development

```sh
cargo test
```

Unit tests use synthetic fixtures only. Tests never need, and must never commit, game data.

To compare frame cost between builds, set `THEDAS_BENCH_FRAMES=n` on a headless capture. The
capture's final frame is then drawn `n` more times first, and the time of each draw, including
its read back to memory, is printed as `bench: n frames, avg … ms, worst … ms`. Use the same
save, steps and size on both builds; the read back makes the numbers an upper bound, so compare
ratios, not absolutes. Unset, nothing changes. `THEDAS_BENCH_DUMP=1` also prints every frame's
time in order, to tell a first-frame cost from a recurring one.

```sh
THEDAS_BENCH_FRAMES=30 thedas-app --save wilds.json --size 1280x720 --capture x.png --steps "run 2000"
```

xoreos is a useful reference for file formats. It is GPLv3 and this project is MIT, so read it
to understand a format, then write our own implementation; never copy its code. The same applies
to any proprietary SDK source.
