# Level mesh point-light selection

The Cousland review now has an original hall draw trace. See
[Cousland hall review](cousland-level-light-selection.md): the seven identified
floor/carpet draws exclude the legacy sun and match this picker's point slots.
The proposed cross-room fallback was rejected by that evidence. The original
free-camera bright-window comparison still needs a matching exploration trace;
this conversation-camera result does not establish framebuffer parity.

Level mesh renderables take DAOrigins.exe's ordinary per-object pass (0x4654e0 →
0x464310). They use the level lists: AffectsDomain exactly 1 is character-only;
other domains belong to the level. RML 3164 controls light-room reach. Static
points fill A/B/C, other points D/E. Version 0 influence depends on sphere
intersection and distance, including zero-colour lights; Version 1 also weighs
brightness. Selection chooses the strongest positive candidates, then orders the
selected links by priority, Version ascending and influence descending.

The owning mesh node supplies the light bound, separately from the RML
placement/LOD sphere. The MSH loader reads 8020/8019 as (centre.xyz, radius) at
0x57edf2; 0x57e640 returns the resolved chunk's sphere. Mesh maintenance
0x55ad80/0x55a450 transforms its centre and copies its radius unchanged, and
0x462520 reads that world sphere through renderable +0x70 (0x559ea0). The room is
propagated from the containing layout (0x548d50 → 0x55e900), independently of
bounds. Replacing room assignment with a camera room or nearest box is unsupported.

The renderer now uses the shared picker for level meshes, including extra-light
updates and moved objects. Level-owned draws use stored chunk spheres for both
initial and extra-light selection. Placement/culling bounds are retained.
Missing chunk spheres fall back to existing placement bounds provisionally.

The final link comparator at 0x461eb0 orders lower Version first, after membership
has been chosen by influence. The special priority byte +0x194 starts at zero;
its runtime overrides remain unrepresented. Equal-score registration ordering,
x86 reciprocal/reciprocal-square-root estimates, second receiver rooms, hidden
owners of D/E lights, cached/transitional links and runtime room changes remain
provisional. The ordinary square-root/division formulas are mathematical
approximations, not bit-identical SSE emulation. These limits must not be hidden
by copying observed registers or overriding rooms for a camera.

## Commons verification

The matched-draw harness retains 124 strict draws (vertex count, exact lightmap
upload signature, atlas region and world matrix), 17 provisional atlas matches,
and 8 unmatched original Static1 draws. Light equality compares all components
of PS c0–c11 at tolerance 0.0001; c12 is shadow data in this shader. Provisional
matches are reported separately. The original's live room IDs, bounds, selected
link history and shader booleans were not captured by this harness.

The baseline is main 6d66c10e98f80854e70b694c530b1752eec1c945. Live renderer
ConstantContext dumps reproduce **0/124 strict, 0/17 provisional** light-register
matches. The shared picker on placement bounds gives **100/124, 0/17**. Final
chunk-bound results and capture pixel measurements are recorded below after the
candidate run.

Local reproducibility inputs and generated artifacts are under
`OpenThedas-testdata/codex-captures/level-light-selection/`; the prior harness is
under `codex-captures/commonsdraws/`. No original game data or disassembly is
included in this repository.

Final live candidate dumps give **104/124 strict, 0/17 provisional** (104/141
linked). All 141 draw identities and confidence labels are unchanged. No main
match regresses (main has zero full-pack matches); all 100 matches from the
placement-bound switch are also retained. Both probes contain 1,316 Static1 scene
draws, the same 324 resource identities, no shader preparation failures and no
scene warnings. The eight unmatched original draws remain outside the metric.

The four additional strict fixes are original calls **6034511 / 6035905 /
6035982 / 6035994**, scene draws **757 / 756 / 755 / 684**: the three sign chunks
and a market divider used one model-wide placement sphere. Their individual
stored spheres remove an incorrectly eligible third light. The wall (draw 633)
now uses radius **17.3826561**, instead of **21.2034245**, and retains the verified
black radius-100 / orange radius-15 / orange radius-40 selection in A/B/C.

Every remaining draw has at least one original-selected point light whose
position/radius identifies an authored light in a room that does not reach our
receiver room. All authored reach lists are empty; all 38 scene point lights are
Version 0. Thus the observed rejection is a **room-reach input discrepancy**, not
a final Version-order error, and the chunk-bound correction cannot admit those
lights. The original's live room pointers/reach lists, second-room flags,
registration and light-set cache stamps were not recorded. Attributing the
runtime discrepancy specifically to receiver assignment, light assignment,
reach-list state, a second room, or a cached selection is **provisional**. No room
override or guessed priority/cache fix is applied. Sorting cannot restore a
candidate rejected before sorting.

The strict residual calls are:
`6034795, 6035237, 6035250, 6035262, 6035275, 6035312, 6035325, 6035582,
6035594, 6035679, 6035692, 6035705, 6035717, 6035766, 6036023, 6036036,
6036045, 6036084, 6036094, 6036107`.

The provisional residual calls are:
`6034320, 6034390, 6034702, 6034890, 6034899, 6034913, 6034922, 6034947,
6035397, 6035407, 6035743, 6035756, 6035857, 6035869, 6035880, 6035892,
6035969`.

Executable evidence was freshly checked against SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
The final integration incorporates origin/main **e401ccbc**; the intervening
upstream changes affect simulation/UI, with no changes to this static viewer's
renderer or scene-light inputs. This is a light-register comparison, not complete
shader or original framebuffer parity. Non-unit-scale model paths beyond the
traced mesh-node maintenance remain provisional.

## Commons image comparison and checks

The capture runner compares our own baseline and clean candidate binaries at QA's
Commons camera: `(301.1949158,342.5385132,14.4111252)`, yaw/pitch `92.5/-5`,
1024×768, High detail, faithful/original shaders, shader time zero, frame effects
on, AA 1, no mods/audio, headless level viewer without dynamic actors. The two
repeats on each side are **exact RGB matches** (maximum repeat delta zero).

**15,025/786,432 pixels (1.910528%) change** at max absolute RGB channel delta
**>8/255**. Maximum delta is **78/255**; mean absolute channel delta **0.240041**.
The three-panel comparison includes a **Delta ×4** heatmap (black=0, red=32 raw
channel units, yellow at ≥63.75). Warm highlights change locally around the
doorway, pillars and left-side stonework; the broad foreground floor and lava
streams look essentially unchanged. This does not resolve the separate original
wall brightness/color discrepancy.

Artifacts: `commons-before-after/report.json`, `report.md`, and
`delta/commons.comparison.png` under the job capture directory. Binary SHA-256,
resolved scenario, exact commands and per-process logs are retained there. The
PNG/caption are staged under `media-staging/level-light-selection/` for review.

Merged-result checks: **fmt PASS; clippy workspace/all targets with warnings
denied PASS; workspace tests 1,405 passed, 25 ignored, 0 failed** (31 result
groups, including doc tests); clean application build PASS; capture runner PASS.
The three new sphere/order tests pass. No simulation behavior changes in this
branch, so the simulation sweep gate is not applicable. Original-register
measurement covers this Commons fixture; other areas remain unmeasured here.
