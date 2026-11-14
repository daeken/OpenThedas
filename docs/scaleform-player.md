# Our own Scaleform player (`thedas-gfx`)

Status: active, 2026-11-14. `thedas-gfx` is the only UI player. Ruffle and the runtime
backend selector have been removed.

## Why

The player implements the ActionScript 2 and Scaleform features these movies use, shares
the renderer's wgpu device, and redraws only changed movies. The engine bridge retains its
callbacks, token observers, synchronous query replies and text layout helpers. Mouse button
arguments, topmost-entity queries and release ordering now use the player's native behavior.

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

The player lives in `crates/thedas-gfx`:

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
   Intervals preserve their scheduled phase and catch up in deadline order. Delays use
   integer milliseconds with a 10 ms minimum; a bounded scheduler clock limits catch-up
   work without rewinding `getTimer`. Removed clips stop receiving method timers.
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
   with host multiply passes retained for the existing scene composite.
6. **Change tracking**: any change to a display property, frame, text, or the display list
   marks the movie dirty. A tick that changes nothing does no rendering and the texture is
   reused. Static subtrees can be cached to a texture later if profiling asks for it.
   Dirty mouse picking is refreshed after frame/interval actions and wheel listeners, so
   clips moved or hidden beneath a stationary pointer receive the corresponding rollover
   changes. Unchanged display lists skip the pick walk.
   Native topmost-entity queries include a clip's own dynamic fills beneath its children,
   in world coordinates, while respecting timeline and script masks. Transparent fills
   are still entities; decorative drawings do not gain button events or text focus.

## Runtime and validation

- Every movie opened by `thedas-ui::Stage` uses `thedas-gfx` directly from its GFx bytes.
  There is no backend selector or fallback player.
- Unknown opcodes, tags or built-ins are logged once per movie.
- Compare hidden-window captures of the menu, chargen, HUD, inventory and conversation
  against the previous own-player build. Exercise the dwarf commoner and human noble
  chargen click-through as well as the simulation gates.
- Historical comparisons under `OpenThedas-testdata/sf-compare` remain unchanged.
- A movie that did not change is not drawn again.
- Display objects stringify to their paths and unnamed timeline clips/text fields receive
  unique `instanceN` names. Host tree walks can now configure notification multiply passes
  (removing the black bands behind popups) and find menu buttons. Popup vertical placement
  retains the previous own-player layout in the choice/text comparison recipes.
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
  button alongside Report Issue using the same menu setup on the player.
- Clicking editable/selectable text places the caret at the nearest character boundary.
  Dragging extends the selection, including outside the field until release; Shift-click
  retains its anchor. Picking uses the field's inverse world transform and the layout's
  aligned, wrapped and scrolled character advances, including spaces and missing glyphs.
- Remaining work includes complete text `shadowStyle`, native inline `<img>` layout,
  cached bitmap alpha masks, and broader interaction parity.
- Tools: `cargo run -p thedas-ui --example capture -- INSTALL OUT.png MOVIES [FRAMES]
  [WxH]` captures the player once and reports timings. `SF_CAPTURE_SETUP` supplies optional
  `call`/`set` steps. This replaces the former `sidebyside` comparison example.
  `thedas-gfx` examples: `gfxscan` (parse), `gfxrun` (headless run; `GFX_RENDER=1` times
  rendering, `GFX_TRACE_CALLS=1` logs calls, `GFX_SHOW=paths` prints values), `gfxdis`
  (disassemble ActionScript), `gfxshape` (print a character).
