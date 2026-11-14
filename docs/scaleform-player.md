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
  Strokes: 38 line styles in all. Authored strokes retain round/flat/square endpoint caps,
  round/bevel/miter joins and miter limits. Shape4's no-close flag keeps meeting endpoints
  capped; legacy shapes and morph strokes use their implicit round caps and joins.
  Masks (clip depth): 177. Colour transforms everywhere.
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
   Timeline-placed registered classes construct parents before descendants. This preserves
   the GUI framework's last-created element and associates post-load callbacks with the
   intended child, including popup centring before the text resizes its box.
   Timeline jumps rebuild a symbol removed and placed afresh at the same depth, including
   its name, constructor and callbacks. Rewinds preserve script state only while the
   instance's original placement remains in the target frame. This keeps chargen's
   appearance navigation and load-screen headers from retaining the previous scene.
   `MovieClip.setMask` accepts clip objects and activation-relative paths, transfers a mask
   from its previous owner, and clears with null/undefined. Removal detaches both links.
   Dynamic masks use filled geometry in stage space, ignore visibility/alpha and filters,
   hide the mask's normal picture, and clip the finished filtered subtree and mouse picking.
   Stroke-only shapes do not provide mask coverage. Cached bitmap alpha masks remain separate.
4. **`text`**: HTML-subset parser (p, br, font face/size/color, b, i, u, img), layout with
   word wrap, autosize, `textAutoSize` shrink, shadows; glyphs from the font library's
   DefineFont3 outlines, rasterised into a glyph atlas at the drawn size.
   Focused fields track a selection anchor and caret. Ctrl+A and `Selection.setSelection`
   replace/delete the selected range; Shift navigation extends it. Selection getters and
   setters translate script UTF-16 indices to the text storage's character boundaries.
   Programmatic `Selection.setFocus` selects all existing text, including refocusing the
   same field, so typing replaces a prefilled save name. Invalid targets preserve focus;
   null/undefined clear it. Mouse focus still places the caret from the pointer.
   Layout retains character advances, including spaces, for selection highlighting and the
   insertion caret. Focused ranges draw white text over black highlights; an input caret
   blinks at 500 ms intervals, resets on edits/navigation, and disappears on focus loss.
   Blink boundaries invalidate the picture without discarding the cached text layout.
5. **`render`**: wgpu. Shapes are tessellated once per character (lyon) into cached vertex
   buffers; bitmaps (atlas DDS pages) uploaded once; scale9 drawn natively. Colour transform
   and blend mode per draw; multiply and the few other modes against the scene picture,
   so the separate multiply pass goes away.
6. **Change tracking**: any change to a display property, frame, text, or the display list
   marks the movie dirty. A tick that changes nothing does no rendering and the texture is
   reused. Static subtrees can be cached to a texture later if profiling asks for it.
   Dirty mouse picking is refreshed after frame/interval actions and wheel listeners, so
   clips moved or hidden beneath a stationary pointer receive the corresponding rollover
   changes. Unchanged display lists skip the pick walk.
   Native topmost-entity queries include a clip's own dynamic fills beneath its children,
   in world coordinates, while respecting timeline and script masks. Transparent fills
   are still entities; decorative drawings do not gain button events or text focus.

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
- Display objects stringify to their paths and unnamed timeline clips/text fields receive
  unique `instanceN` names. Host tree walks can now configure notification multiply passes
  (removing the black bands behind popups) and find menu buttons. Popup vertical placement
  still differs from Ruffle by about 45 pixels in the choice/text comparison recipes.
- Engine render targets (`paperdoll`) replace their external placeholder shapes at the
  authored bounds. Same-size frames update the existing GPU texture and cached bindings;
  clearing a target makes it transparent.
- Glow/DropShadow/Blur filters flatten the subtree, repeat separable box blur by quality,
  then composite premultiplied colour with strength, inner/knockout and source visibility.
  Parent stencil masks clip the result; nested filters and ordered filter lists are supported.
  Intermediates currently cover the viewport, so content outside it cannot cast a shadow
  back into view; cropped/padded filter targets and pooling remain optimizations.
- Layer blend groups render to isolated targets. Alpha/erase modify that layer's picture;
  outside a layer they draw nothing, matching Flash and avoiding the map's black border.
- Difference, invert, overlay and hard light flatten their source subtree and blend with
  a resolved destination snapshot. The shader blends straight colours in the overlap and
  preserves premultiplied source/destination coverage elsewhere. Render-pass boundaries
  preserve open stencil masks and subsequent sibling order. GPU readback covers transparent
  pictures, overlapping children, consecutive destination blends and clip-depth masks.
- Export-backed `BitmapData.loadBitmap` and `MovieClip.attachBitmap` let the shared host
  inline-image helper place copper/silver/gold icons in inventory and store prices. Timeline
  jumps distinguish text characters, so the unaffordable-price state can replace gold text
  with its authored red definition. Native text-layout `<img>` rendering remains separate.
- `MovieClip.duplicateMovieClip` constructs a fresh instance of the registered class with
  the source's transform, colour, drawing and clip events. The host can now add its Mod Mode
  button alongside Report Issue using the same menu setup on either backend.
- Clicking editable/selectable text places the caret at the nearest character boundary.
  Dragging extends the selection, including outside the field until release; Shift-click
  retains its anchor. Picking uses the field's inverse world transform and the layout's
  aligned, wrapped and scrolled character advances, including spaces and missing glyphs.
- Remaining work includes complete text `shadowStyle`, native inline `<img>` layout,
  cached bitmap alpha masks, and broader interaction parity.
- Tools: `cargo run -p thedas-ui --example sidebyside -- INSTALL OUT.png MOVIES [FRAMES]
  [WxH] [ORIGINAL.png]` (Ruffle and ours, optionally after a shot of the original);
  `thedas-gfx` examples `gfxscan` (parse), `gfxrun` (run headless; `GFX_RENDER=1` times
  rendering, `GFX_TRACE_CALLS=1` logs calls, `GFX_SHOW=paths` prints values), `gfxdis`
  (disassemble ActionScript), `gfxshape` (print a character).
