# Stream: the deferred renderer (ours, optional)

## What it owns

OpenThedas's own renderer, which sits beside the faithful D3D9 path and is chosen at run
time. The faithful path (the original's translated shaders) is the default and the
reference, and the rendering-parity stream owns it. This stream owns everything in
`crates/thedas-render/src/deferred*.rs` / `deferred*.wgsl`, the `RenderPath` switch, and
`docs/deferred.md`. Sera's goals for it: deferred lighting, spell and particle effects that
emit light and cast shadows, real-time lighting in place of lightmaps, and a one-click
toggle between the two renderers.

**The design document of record is [`docs/deferred.md`](../../deferred.md).** It is long
and current. It describes every frame step, material family, shadow kind, control and
benchmark. This file only summarises it and adds history and handoff notes.

## History

The stream started 2026-11-14 in one Claude thread. Codex took over later that day.

| PR | What |
|---|---|
| #702 | `RenderPath` toggle (`--renderer`, F11, launcher "Renderer", `renderer` step); 4-target G-buffer + full-screen Static1 lighting over every level light, room bit sets; env reflection pass; debug views |
| #724 | Glowing (additive) particle emitters become point lights each frame: torches, braziers, spells |
| #739 | Point-light shadows: 4-slot cube pool (R32Float distance, 512²/face, 0.35 m near plane so torch bowls don't self-shadow) |
| #742 | Faithful sun cascades sampled in the deferred lighting |
| #751 | Terrain (Terrain1/2/3) in the G-buffer |
| #755 | Experimental lightmaps-off mode: SH probe ambient + SSAO (`THEDAS_DEFERRED_BAKED_LIGHTING=0`) |
| #769, #776 | Shared G-buffer with opaque objects and skinned characters (GPU palette skinning); faces and eyes |
| #781, #785, #808 | Textured cutout point-shadow casters; Static1 punch in the G-buffer; blended prop punch / eyelash casters |
| #789, #805, #817 | Late transparency after all opaque forward work; ground-ring mask under G-buffer characters; batch of fixes |
| #800, #807, #832, #837, #840, #858 | Cost work: light tiles and depth slices, cube-face culling, shadow cube reuse, fused reflections, specialised pipelines |
| #843, #845, #847 | SpeedTree fronds, branches, leaves in the G-buffer; wind-deformed tree point shadows |
| #854 | Opaque cutaways and unlit `Black` admitted |
| #865 | Hair and eyelashes (forward) lit with the shared deferred lights and shadows |

No deferred branches are in flight. Every `m1-deferred-*` branch on origin is merged into
main (checked 2026-11-14 ~22:00Z), including the parked `m1-deferred-shadows`, which landed
as #739.

## How the switch works

- `thedas_render::RenderPath::{Faithful, Deferred}` on `AreaRenderer::path`.
- App: `thedas-app --renderer faithful|deferred` (default faithful); **F11** flips at run
  time; `renderer faithful|deferred|toggle` (or `key f11`) in `--steps` / `--control`
  input; launcher "Renderer" radio (`PlayOptions::renderer`).
- Faithful loads skip deferred planning entirely. The first F11 from faithful plans then
  and may pause for a moment. API callers use `AreaRenderer::new_cached_with_path`, or
  call `prepare_deferred` before setting the path. Callers of `render_main_with` must call
  `finish_transparency` on the level and each overlay before `resolve`; `capture_with` does
  this itself.
- If the GPU refuses the deferred pipelines (`gpu::checked`), the level stays faithful and
  logs it once.

## Build, test, verify

- Use your own worktree and target dir (`/Volumes/External2/projects/OpenThedas-wt/deferred`,
  target `.../deferred-target` on the shared Mac). Builds run at `nice -n 19 ... -j 2`.
  Don't touch Sera's checkout.
- Checks before a merge, on the result of merging the latest main: `cargo fmt --all`,
  `cargo clippy --all-targets`, and plain debug `cargo test`. Every hand-written WGSL
  module has a `backend_check::translates` test. It fails if any wgpu backend (GL 330
  included) can't express the shader. This is why everything uses uniform buffers rather
  than storage buffers, and reads depth as a float texture.
- Captures, headless:
  - Viewer mode gives clean, UI-free frames: `thedas-app --install <gamedata> --area ost001d
    --camera=-29.5,25.0,2.4,300,-8 --time 3 --capture out.png --size 1280x720 --renderer
    deferred|faithful`. That framing shows the Tower of Ishal torch, the canonical shot
    for flame light and shadows.
  - Save mode: `--save OpenThedas-testdata/01_cousland_start.json` (Cousland hall, day).
    Others: `04_siege_start` (night), `12_tower_fresh`, `sc_tower_midfight` (spells
    mid-fight). Don't add `pad a` steps: they talk to the NPC in front, and on older
    builds that opened a dog-naming popup over the frame.
- Debug views: `THEDAS_DEFERRED_VIEW=albedo|normal|lightmap|visibility|diffuse|specular|
  no-rooms|flat|wrap|shadows|sun-shadows|ao`. `THEDAS_DEFERRED_LOG_LIGHTS=1` logs one
  frame's lights and shadow slots. About 36 `THEDAS_DEFERRED_*` switches turn each
  addition off for before/after comparisons, and `docs/deferred.md` lists them. All are
  read once per process.
- Cost: `THEDAS_BENCH_FRAMES=120 THEDAS_BENCH_DUMP=1` with
  `THEDAS_BENCH_DEFERRED_COMPARE=<mode>` runs same-process ABBA comparisons.
  `THEDAS_RENDER_PROFILE=1` gives CPU stage and GPU pass timings. Discard warm-up frames.
  The shared Mac's load makes absolute numbers noisy.
- Control ports for `--control`: 8000-8009 belonged to this stream on the shared Mac.
- Side-by-sides: Sera wants them early and often, broken ones included. On the old setup
  they went to `OpenThedas-testdata/media-queue/deferred/` (image first, then a `.txt`
  caption naming which side is which) and were posted to Discord #media.

## Key design decisions

- **Hybrid, not a rewrite.** Only proven-equivalent material families enter the G-buffer.
  Everything else keeps its translated original shader and draws forward over the
  deferred colour and depth. This kept the faithful path untouched (read-only accessors
  only) and let coverage grow one family at a time.
- **Static1's maths, per pixel, every light.** The lighting reproduces the original's
  Static1 sum (`lm.rgb + lm.a·Σ(sun, static lights) + dynamic lights`, cos² falloff,
  Blinn-Phong ×3, fog), but without the 3-static/2-dynamic per-object cap. Room bit sets
  stop lights reaching rooms the original never lit.
- **Deliberate deviation:** Static1's falloff wraps past the radius (back to full at 2R).
  The original hides this by assigning lights per object, and it makes big floors
  brighter. Done per pixel it draws hard rings, so the deferred falloff ends at R; the
  `wrap` view shows the alternative. This explains most of the "deferred is darker on the
  Cousland floor" difference.
- **Portable limits drive the layout:** at most 32 bytes per pixel of colour attachments
  (4 G-buffer targets), uniform arrays of at most 16 KiB (192 lights), and no storage
  buffers.
- **Emitter lights are ours.** The original's particles light nothing. Strength saturates
  with the light the particles show, and the constants were tuned by eye on the Tower
  torches. Lights with no room reach every room, because room boxes misplace floors in
  multi-storey areas.

## Known gaps

From `docs/deferred.md` and `docs/notes/deferred-*.md`:
- G-buffer surfaces use authored mesh tangent/binormal frames, with derivatives for
  missing or degenerate frames. Nonzero Static1 tints transform the complete fused baked
  lighting sum before fog. Tinted draws retain faithful shading in probe/SSAO, diagnostic
  and separate-reflection modes; see `docs/deferred.md` for the palette limits.
- Not yet in the G-buffer: hair (lit forward), unsupported alpha/blended families,
  Char2 (low detail), and far tree billboards. Character rim light, skin scattering,
  gore/heraldry and remote materials are not represented.
- Point shadows: 16 slots × 6 faces in six atlas sheets; other point lights are unshadowed. Forward
  families other than the listed cutouts and trees don't cast.
- Lightmaps-off is experimental. Probes are the original's baked irradiance, SSAO is
  basic, and light leaks and probe discontinuities can occur.
- The G-buffer is single-sample. Under MSAA, deferred silhouettes and cutouts get
  FXAA-level edges.

## What a newcomer must not break

- **The faithful path.** It is the default and the parity reference. Deferred code must
  not change faithful frames. Faithful loads must not plan deferred work, and deferred
  code may only use read-only accessors on `D3dPass`. Check with a faithful capture
  before and after your change.
- **Portability.** Keep every WGSL module passing `backend_check`, and stay inside the
  attachment and uniform budgets above.
- **Comparison switches.** Keep each addition's `THEDAS_DEFERRED_*` off switch and debug
  view; that's how regressions get found.
- **Ordering contracts** with MSAA, late transparency and the ground mask: deferred
  colour and depth are written before any forward pass, transparency waits for every
  renderer's opaque work, and `finish_transparency` runs before `resolve`.
- **No game data in the repo**, and anything taken from guesswork is marked provisional.
