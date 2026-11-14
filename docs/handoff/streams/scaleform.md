# Handoff: the Scaleform player (`thedas-gfx`)

Written 2026-11-14 at main `2b745ce` by the thread that started the player ("Own Scaleform
player"). That thread wrote the inventory, the design and the first player, and merged it in
#726. Later sessions carried it much further: more than thirty `sf-*` branches merged, and
Ruffle was removed in #818. Their work is described here from main's history and
`docs/scaleform-player.md`, not from memory. Where the two disagree, trust the code and that
document.

## What this stream owns

- `crates/thedas-gfx`: our own player for the game's Scaleform GFx movies. About 16k lines.
  Since #818 it is the only UI player. There is no Ruffle, no `THEDAS_UI_PLAYER` switch and
  no fallback player.
- The glue in `thedas-ui` that hosts it: `own.rs` holds `OwnMovie`, the image fetcher, the
  font set and `OwnHost`. `UiMovie` in `lib.rs` delegates to `OwnMovie`.
- `docs/scaleform-player.md`, the design and the running status. It is the authoritative
  description of current behaviour; keep it current with every change.
- The movie inventory and tools in `OpenThedas-testdata/scaleform/` (outside the repo,
  because it is game-derived).

The UI stream ("Continue controller navigation and UI") owns everything the movies say:
bindings, token observers, `protocol.rs`, `stage.rs` screen logic, focus navigation, and the
app's composite. The line is that `thedas-gfx` behaves like Scaleform, and `thedas-ui`
decides what the game shows. Changes to `UiMovie`/`Stage` touch both; coordinate them.

## The movies (facts that shaped the design)

- 68 movies: the base `guiexport.erf` overlaid by `packages/core/patch/patch000`–`003`.
  They are all compressed GFx (`CFX`), SWF version 8, **ActionScript 2 only**. There are no
  buttons, sounds, video, static text or AS3.
- Bitmaps come from GFx `DefineExternalImage` into DDS atlases (`atl_*_dat.xml` +
  `atl_*_NN.dds`).
- Fonts: "Dragon Text" and "Nogard" come from `fonts_<lang>.gfx`, which the game installs
  as the font library. The movies import `$BodyFont`/`$TitleFont` from `gfxfontlib.swf`.
- Blend modes are mostly multiply (253 uses). Filters are rare: Glow ×2, DropShadow ×1.
- Engine calls go through `fscommand`, with answers in `ExternalCommands.fsc_retval`, and
  through `ExternalInterface.call`. The engine calls in by path (`GFxMovie::Invoke`).
- Exporter version 0x0214. Every movie carries its own AS2 framework in `DoInitAction`
  (`GUI.Element`, `SceneManager`, `TokenManager`, ...). Most behaviour bugs are about
  *ordering*, not missing built-ins.

The full inventory is in `OpenThedas-testdata/scaleform/inventory.json`, produced by
`scripts/inventory.py`, with extracted movies under `movies/`.

## Layout of the crate

| Path | What |
|---|---|
| `swf/` | Bounds-checked parser for exactly the tags the movies use, plus GFx tags. Never panics on bad input. |
| `avm1/value.rs`, `object.rs` | Values; object arena with mark-and-sweep run between frames, never during a script. |
| `avm1/interp.rs` | Bytecode loop, calls, `super` resolution, execution limits. |
| `avm1/builtins.rs`, `display_api.rs` | Object/Array/String/Math/Date…; MovieClip, TextField, TextFormat, Color, Key, Mouse, Stage, Selection, MovieClipLoader, ExternalInterface, flash.geom, BitmapData. |
| `display.rs` | Display arena with generation-checked handles. |
| `player.rs` | Timelines, the action queue, events, input, picking, focus. |
| `text.rs` | HTML subset, layout, selection and caret. |
| `tess.rs`, `render.rs`, `filters.rs` | lyon tessellation, wgpu renderer, filters and destination blends. |

## Design decisions to keep

- **Implement only what the movies use, the way Scaleform does it.** The inventory decides
  scope. Where Flash and Scaleform differ (mouse buttons passed to handlers, `onLoad` order,
  `Stage.width` as the visible area, multiply onto the real picture), follow Scaleform. The
  original game is the reference; Ruffle never was.
- **Action ordering mirrors Ruffle's AVM1 priorities.** Init actions run immediately when
  their frame is reached, before its clips are built. Construction is queued above normal
  actions, and a parent's `onLoad` is queued before its children's. Later work refined this
  to construct timeline parents before descendants. These rules fixed whole screens at a
  time; change them only with captures of several screens before and after.
- **Redraw only on change.** Any display mutation sets `dirty`; an unchanged movie is not
  drawn. Shapes are tessellated once per character and text once per layout revision.
  Picking runs only when the mouse or the display list changed. This was the point of the
  rewrite, because Ruffle spent most of its UI time re-picking every tick.
- **Multiply against the real destination.** It brings back the start menu's bloody logo,
  which Ruffle lost.
- **Scaling grids cut geometry at the grid lines** before stretching. Moving vertices alone
  smears a one-quad bitmap panel.
- **`super`** comes from the holder the method was found on, or from `this`'s prototype
  for plain/`apply`/`call` calls.

## How to build, test and verify

- Unit tests: `cargo test -p thedas-gfx -p thedas-ui` (synthetic fixtures only; never
  commit game data). Run the house loop before merging: `cargo fmt --all`,
  `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` on the
  merged head, in debug.
- Parse every movie:
  `cargo run --release -p thedas-gfx --example gfxscan -- OpenThedas-testdata/scaleform/movies/*.gfx`
  (all should parse with nothing skipped or damaged).
- Run one movie headless:
  `cargo run --release -p thedas-gfx --example gfxrun -- MOVIE.gfx fonts_en-us.gfx [frames]`.
  `GFX_RENDER=1` times script and render per tick; `GFX_TRACE_CALLS=1` logs every call by
  member name; `GFX_SHOW=_root.a.b,...` prints values; `GFX_TREE=1` dumps the whole
  display tree.
- Read the movie's own code: `gfxdis MOVIE.gfx [filter]` disassembles AS2 with constants
  resolved. `gfxshape MOVIE.gfx ID...` prints a character. **Most bugs were found by
  reading the movie's code, not by guessing.**
- Capture through the real `Stage`:
  `cargo run -p thedas-ui --example capture -- INSTALL OUT.png MOVIES [FRAMES] [WxH]`, with
  `SF_CAPTURE_SETUP` for scripted `call`/`set`.
- Audit every movie: `cargo run [--release] -p thedas-ui --example movieaudit -- INSTALL
  MOVIE_DIR bench|fuzz ...` (see the design doc).
- Engine-state screens (inventory, journal, store, conversation) render empty without a
  game. Check them with the app headless: `thedas-app --capture OUT.png --save <MCP save>
  --steps "run 2000; key i; run 1500"`. Or use a hidden window with
  `THEDAS_HIDDEN_WINDOW=1 --control` on this stream's ports, 8010–8019 (check with `lsof`
  first). Never open a visible window while Sera is playing.
- Compare with the original game where you can: Sera's Wine shots are under
  `OpenThedas-wine/runs/*/shots/`. Earlier comparison captures are in
  `OpenThedas-testdata/sf-compare/` and `sf-capture-app-*`.

## In flight (open PRs at handoff)

- #860 `sf-window-leave`: clear menu rollover and item hover when the mouse leaves the
  window.
- #864 `sf-glyph-winding`: fill overlapping glyph contours with nonzero winding (Ç, ç).
- #866 `sf-bitmap-sampling`: honour authored bitmap smoothing (tooltip and equipment
  borders).

No other `sf-*` branch has unmerged commits. Old branches (`sf-player`, `sf-filters`, ...)
are all contained in main and can be deleted.

## Known gaps (see also `docs/gaps/ui.md`)

- Text: complete `shadowStyle`, and native inline `<img>` layout in text. Currency icons go
  through `attachBitmap`, not text layout.
- Cached bitmap alpha masks. Filter intermediates cover only the viewport, so content
  outside it cannot cast a shadow into view.
- Broader interaction parity, and every screen checked with real game state. Many screens
  are not opened by the game at all yet (crafting, item upgrade, army control,
  achievements); see the UI gap list.
- Faux bold/italic: the font library has only regular faces. Fonts embedded in a movie are
  not preferred over the library's.
- Timing semantics are inferred from Ruffle and the movies, not traced in the executable.
  Each such place is marked provisional in its doc comment.

## What a newcomer must not break

- **Never panic on movie data.** Every read is bounds-checked; unknown opcodes, tags and
  built-ins are logged once per movie.
- **The redraw-on-change and pick-on-change contract.** A change that makes every movie
  dirty every tick, or re-picks every frame, throws away the reason this player exists.
  Check `GFX_RENDER=1` drew counts before and after.
- **Action ordering.** Init actions immediately, then constructors, then `onLoad` parent
  before child. Run the captures of menu, chargen, HUD, inventory and conversation before
  and after any change there.
- **Multiply onto the destination**, not into a transparent intermediate.
- **The simulation stays headless and deterministic.** The UI reads simulation state and
  never writes it except through player input.
- **No game data in the repo**, including captures and extracted movies. Use
  `OpenThedas-testdata`.
- **After a merge, grep for conflict markers** before committing (see the retro).
