# Our own Scaleform player (`thedas-gfx`)

Status: design, 2026-11-14. Ruffle stays the default until the new player matches it movie by
movie.

## Why

Ruffle costs about 15 ms of a 22 ms release frame. It redraws every movie every frame, runs a
general Flash player (AVM1 and AVM2, every tag, browser semantics), and needs a layer of
workarounds to behave like Scaleform (`thedas-ui/src/avm1.rs`: injected invoke callback,
wrapped `fsc_retval` getters, empty-string marker, mouse-button global, put-off `onLoad`,
put-off `HideMovie`, tooltip relayout, multiply pass, scale9 relayout, `Stage.width` via
no-scale mode). A player written for these movies can do each of those the Scaleform way
natively, and only redraw what changed.

## What the movies use (inventory of all 68, patches applied)

Script: `OpenThedas-testdata/scaleform/scripts/inventory.py`; output `inventory.json` next to it.

- Every movie is a compressed GFx (`CFX`), SWF version 8, **ActionScript 2 only** (no DoABC).
  Exporter info 0x0214. Stage 1024x768 (credits, deathscreen, load_town 1280x768); 30 or 60 fps.
- Tags: ShowFrame, PlaceObject2 (175k), PlaceObject3 (279), RemoveObject2, DefineSprite (8.7k),
  DefineShape 1-4, DefineMorphShape (8), DefineEditText (706), DefineFont3 (only in the font
  movies and a few others), CSMTextSettings, DefineScalingGrid (94), FrameLabel, ExportAssets,
  ImportAssets2 (only `$BodyFont`/`$TitleFont` from `gfxfontlib.swf`), DoAction, DoInitAction
  (5k: each movie carries its own AS2 framework), GFX DefineExternalImage (2k, into DDS
  atlases), ExporterInfo, FileAttributes, SetBackgroundColor, Metadata.
  **None of**: buttons, static text, sounds, video, bitmaps embedded as SWF bitmaps, AS3.
- Fills: bitmap clip (2.3k, mostly non-smoothed), solid (577), linear 28, radial 20, focal 1.
  Strokes: 38 line styles in all. Masks (clip depth): 177. Colour transforms everywhere.
- Blend modes: multiply (253), overlay 5, difference 4, hardlight 4, invert 2, add, erase,
  screen 1 each. Filters: Glow 2, DropShadow 1. cacheAsBitmap 3.
- Text: 706 edit texts, almost all `useOutlines`, read-only, non-selectable, 476 HTML; 18 input
  fields. Fonts: "Dragon Text" and "Nogard" from `fonts_<lang>.gfx`. Scaleform extensions
  used: `shadowStyle`, `shadowColor`, `textAutoSize`, `hitTestDisable`, `noInvisibleAdvance`.
- AVM1 opcodes: 57 distinct, all SWF 5-7 era (DefineFunction2, Extends, StrictEquals; no Try,
  With or SetTarget).
- Built-ins reached: Object (registerClass, watch, `__resolve`), Array
  (sort, sortOn, splice, ...), String, Math, Number/Boolean conversions, Date.getTime,
  setInterval/clearInterval, ASSetPropFlags, Function.apply/call, Key (isDown, getCode,
  addListener), Mouse (addListener), Stage (width/height, addListener), Selection
  (setFocus/getFocus), MovieClip (attachMovie, removeMovieClip,
  gotoAndStop/Play, play/stop/next/prevFrame, getBounds, localToGlobal/globalToLocal,
  hitTest, swapDepths, getDepth, getNextHighestDepth, startDrag/stopDrag, loadMovie,
  unloadMovie, drawing API: clear/beginFill/moveTo/lineTo/endFill),
  MovieClipLoader.loadClip, TextField (text, htmlText, textWidth, autoSize, wordWrap,
  scroll/maxscroll, setTextFormat/setNewTextFormat/getTextFormat), TextFormat, Color
  (setTransform), flash.geom.Point/Rectangle, flash.external.ExternalInterface, fscommand.
- Engine calls: all through `fscommand` (SendMessage, GetNum2DARows, CreateContext, ...) with
  the answer read from `ExternalCommands.fsc_retval`, plus `ExternalInterface.call` for the
  value getters and token observers; the engine calls in by path (`GFxMovie::Invoke`) and
  `SetVariable`/`GetVariable`.

## Shape of the player

A new crate, `crates/thedas-gfx`, with no Ruffle dependency:

1. **`swf`**: our own bounds-checked parser for exactly the tags above plus the GFX ones, into a
   character dictionary (shapes, sprites with per-frame command lists, edit texts, fonts,
   external images, scaling grids, exports) and pre-decoded AVM1 bytecode blocks.
2. **`avm1`**: an AS2 interpreter. Values are small enums; objects live in an arena with
   generation-checked handles (no GC pauses: a mark-and-sweep run between frames when the
   arena grows). Display objects are objects too. Only the built-ins listed above.
   Scaleform semantics where they differ from Flash: `Invoke(path, args)` and
   `Set/GetVariable(path)` from the host; `fscommand` answered synchronously so
   `fsc_retval` is set before the call returns; mouse button passed to press handlers;
   `onLoad` after the frame's code; `Stage.width/height` report the visible area.
3. **`display`**: the display list (MovieClip, Shape, EditText, Morph), timelines, depth
   rules, mouse picking (with `hitTestDisable`), focus and keyboard routing, input text.
4. **`text`**: HTML-subset parser (p, br, font face/size/color, b, i, u, img), layout with
   word wrap, autosize, `textAutoSize` shrink, shadows; glyphs from the font library's
   DefineFont3 outlines, rasterised into a glyph atlas at the drawn size.
5. **`render`**: wgpu. Shapes are tessellated once per character (lyon) into cached vertex
   buffers; bitmaps (atlas DDS pages) uploaded once; scale9 drawn natively. Colour transform
   and blend mode per draw; multiply and the few other modes against the scene picture,
   so the separate multiply pass goes away.
6. **Change tracking**: any change to a display property, frame, text, or the display list
   marks the movie dirty. A tick that changes nothing does no rendering and the texture is
   reused. Static subtrees can be cached to a texture later if profiling asks for it.

## Switch and parity

- `thedas-ui`'s `UiMovie` keeps its API; inside it becomes a choice between the Ruffle player
  and ours. `THEDAS_UI_PLAYER=ruffle` (default), `own`, or `own:mainmenu,quickbar` to take
  over movie by movie. The app does not change.
- Ruffle-only workarounds in `avm1.rs` stay with the Ruffle backend; ours implements the
  behaviour they emulate directly.
- Per movie: a headless capture of Ruffle and ours from the same state, side by side, and
  a pixel diff, before that movie leaves the list. Order: mainmenu, startmenu2, HUD
  (portraits, quickbar, minimap), conversation, then the book screens.
- Unknown opcodes, tags or built-ins are logged once per movie, never panics.

## Status and how to try it (2026-11-14)

- `THEDAS_UI_PLAYER=own` plays every movie on `thedas-gfx`; `own:mainmenu,portraits` only
  those. Unset (or `ruffle`) keeps Ruffle.
- Matching Ruffle side by side, with no engine state: mainmenu, startmenu2 (its logo's multiply
  now matches the original game, which Ruffle loses), portraits, quickbar, minimap,
  optionsmenu, newgame. All 66 movies start and run headless without errors.
- Release build, idle: 10 µs to 9 ms of script per frame per movie (quickbar 0.27 ms against
  Ruffle's 2.1 ms); a movie that did not change is not drawn again.
- Engine render targets (`paperdoll`) replace their external placeholder shapes at the
  authored bounds. Same-size frames update the existing GPU texture and cached bindings;
  clearing a target makes it transparent.
- Not yet: Glow and DropShadow filters (3 uses), text `shadowStyle`, overlay / hard light /
  difference / invert / erase blends (drawn normal), `setMask`, inline `<img>` in text, the
  and movies checked with real engine state (book screens,
  popups, conversation).
- Tools: `cargo run -p thedas-ui --example sidebyside -- INSTALL OUT.png MOVIES [FRAMES]
  [WxH] [ORIGINAL.png]` (Ruffle and ours, optionally after a shot of the original);
  `thedas-gfx` examples `gfxscan` (parse), `gfxrun` (run headless; `GFX_RENDER=1` times
  rendering, `GFX_TRACE_CALLS=1` logs calls, `GFX_SHOW=paths` prints values), `gfxdis`
  (disassemble ActionScript), `gfxshape` (print a character).
