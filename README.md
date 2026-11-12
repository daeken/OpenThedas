# OpenThedas

An open source replacement engine for Dragon Age: Origins (and later Dragon Age II), written in
Rust, that runs on the original game data. Parity with the original comes first; modernization
(mod management, post-processing, new rendering paths) comes after and must never require breaking
parity.

You need your own copy of the game. Nothing from the original game is, or will ever be, in this
repository.

## Status

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

Planned crates follow the architecture doc: `thedas-audio`, `thedas-video`.

## Playing

```sh
./target/release/thedas-app --install "$GAME"
```

With nothing else, the game boots to its start menu (startmenu2.gfx): New Game goes through
character creation, Load Game lists the saves in `~/.openthedas/saves` (`--saves` for another
folder), Options and Quit work. In a game, Escape (Start on a pad) opens the game menu: save,
load, options, and Exit Game back to the start menu. The logo and intro movies the original
plays before the menu are not shown yet (no Bink playback). `--new-game` (with `--quick-start`)
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
on any failure. DLC archives encrypted with a key we do not have are counted, not failed.

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

xoreos is a useful reference for file formats. It is GPLv3 and this project is MIT, so read it
to understand a format, then write our own implementation; never copy its code. The same applies
to any proprietary SDK source.
