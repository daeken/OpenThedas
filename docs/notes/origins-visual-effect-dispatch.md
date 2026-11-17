# Origins visual-effect dispatch and old snapshot repair

Static trace of Origins 1.05, image base `0x400000`, executable SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are VAs. No original-game execution. Script offsets below come from
`thedas script disasm <install> rules_core` and `creature_core`.

## Native type 4 does not raise APPLY_EFFECT

`ApplyEffectVisualEffect` (`0x65d300`) constructs type 4 at `0x65d3ce`, puts
VFX in integer zero, sets duration type, milliseconds, creator and ability,
and applies through the target's effects interface at `0x65d45b..0x65d45f`.
`Engine_ApplyEffectOnObject` (`0x65ca20`, call at `0x65cb5c`) shares that entry:
`0x7a1a20 -> 0x7a1920 -> 0x7a0e30`.

`0x7a0ee8..0x7a0f1a` selects dedicated handlers for native types 1 through 11
before reaching the general script-effect path. Initialization at
`0x7a1881..0x7a188a` installs `0x7a00a0` in the type-4 slot. That handler:

- Checks for an existing visual handle (`effect +0x74`) and checks a target
  condition for duration type 5 (`0x7a00b0..0x7a00d6`).
- Calls `0x7c4a60`, whose gate only rejects an instant effect when the target's
  interface state exceeds 2. Duration 5 passes this gate.
- Reads VFX integer zero, requests the visual through interface virtual `+0x3c`,
  stores its visual handle, and writes the target into effect object zero
  (`0x7a00e6..0x7a0119`). The ordinary object visual factory is described in
  the separate instant-VFX lifetime trace.
- Updates visual priority through `0x79f7a0` and returns zero. This helper
  queries existing visuals and their priorities; it does not deliver events.

The caller then inserts the effect into the native effect list via `0xa64900`
at `0x7a19f5..0x7a19fa`. Its return-1 branch marks an unsuccessful effect for
removal; successful creation does not enter that branch. The type-4 handler
never calls `0x79fe90`, which constructs event **0x21 (APPLY_EFFECT)** at
`0x7a005a` and dispatches it through interface virtual `+0x28` at `0x7a006e`.
Thus no script validity answer participates in this visual's application.
The paired removal table entry (`0x7a188a..0x7a1894`) is `0x7a0150`: it
requests visual removal and updates priority, without REMOVE_EFFECT.

The simulation now bypasses both script hooks for Origins type 4, preserving
identity, creator, duration and visual attachment. Both public object APIs,
party application and direct simulation application converge on this path.
Nested application leaves the outer current effect and its validity intact.
DA2 keeps its existing policy: its native dispatcher is **untraced here**.

## Why rules_core rejected the misplaced event

`rules_core`'s handler at bytecode `0x312e` reads the current effect and type
(`0x3130`, `0x314d`). It initializes the acceptance result to -1 (`0x3162`).
It reads effects table 136's `SimpleEffect` at `0x36af..0x36d1`; type 4's
installed value is zero. The subsequent type switch (`0x3716..0x3c12`) has
no type-4 arm. The unhandled -1 becomes zero at `0x42c4..0x42ea`, passed to
`SetIsCurrentEffectValid` at `0x4375`. This is consistent with type 4 being
native, not evidence that the original rejects the wisp's visible body.

`creature_core`'s SPAWN path tests `CREATURE_SPAWNED` at `0xf93`, sets it
at `0xfc4`, and exits early for `CREATURE_SPAWN_DEAD == 1`. Otherwise it reads
appearance table 2's `CRUST_EFFECT` (`0x106d..0x10ac`). For a positive value,
`0x10d5..0x10fb` applies it with creator=target=self, duration type 5, zero
seconds and ability zero. Appearance 14 selects VFX 3006, Wisp Crust.

## OpenThedas snapshot revision 12

Original saves retain their effect lists. This repair addresses information
our old application path dropped; it is **not a native load-time behavior**.
The supplied playthrough's `fade-start.json` (revision 10, time 73 ms) already
has all fifteen wisps guarded as spawned and no crust. Replaying SPAWN would
hit the guard, while clearing it would repeat unrelated initialization.

For Origins saves older than revision 12, reconstruct only a missing positive
appearance-table crust on a living creature whose spawn guard is exactly 1
and whose spawn-dead local is not 1. Inactive creatures also need their saved
body for later activation. Preserve any existing type-4 effect with that VFX,
regardless of its other fields, and all unrelated effects. Apply the reconstructed
effect with the script's exact creator/duration/ability values in object-ID order.
New saves never perform this repair, so intentional later removals persist.

**Migration limitation:** an old save cannot distinguish a deliberately removed
crust, a script that set the spawn guard without running creature_core, or a
changed appearance from the old engine rejection. This data-driven repair assumes
the current appearance's usual spawn crust for the bounded eligible population.
It does not reconstruct arbitrary lost VFX or repair dead creatures.

**Remaining provisional behavior:** visual-manager allocation/failure, duration-5
target eligibility, animation-driven INSTANT-to-TEMPORARY conversion, cessation,
and priority arbitration are not implemented by this bounded dispatch fix.
Existing presentation timing remains as before. No rendering fallback is added.

## Reproduction and regressions

`sim-work/repro/fadevfx.sh` loads the supplied fade-start snapshot through MCP,
checks the fifteen repaired wisps, creates a fresh wisp and delivers its queued
SPAWN, then verifies VFX 3006 and stable identity through save/reload. It prints
PASS/FAIL and exits nonzero on failure. Run under the shared cargo slot and
analysis cap as its header describes.

Synthetic tests cover a rejecting hook through the shortcut, generic object,
party and direct APIs; outer effect context; identity/attachment; native removal;
continued rejection of script effects; DA2 preservation; and deterministic,
idempotent, revision-gated repair with existing/unrelated/dead/unspawned controls.

## Validation against main 3a429faf

Fmt and workspace clippy with warnings denied pass. Workspace tests: **1,461
passed, zero failed, 25 ignored** (including both new synthetic regressions).
The MCP repro fails on main's missing-crust assertion and passes on the candidate:
all fifteen saved wisps repaired, fresh wisp 505 spawned with VFX 3006, and its
effect unchanged through save/reload.

The five-step paired gate has ten successful exits, no refused saves, identical
story logs, ordered plot/command traces and ending areas. Of 35 complete paired
saves, 29 are byte-identical. The six differing outputs differ only in revision,
effect lists and allocation counters: three Ostagar wizards retain their three
scripted VFX 5039 applications each, and Gazarath's missing crust 3011 is repaired.
That one repair shifts subsequent effect identities by one, including location
effects. All other saved fields, including health, properties, plots and RNG,
agree. Fifteen engine-log entries differ only in transient handle numbers;
four extra entries report the exact Gazarath repair at two older-save loads.
All other engine-log entries agree.

Both standalone Tower runs reach Flemeth with matching step/call checkpoints,
ending simulation time 1,979,312 ms and 23 harness heals. Five 600-tick determinism
inputs (the standard four plus fade-start) pass repeat and save/reload on **both**
binaries: ten checks, twenty equal-world comparisons.

Hidden-window live captures of the supplied invisible-attacker snapshot hold
both simulations at 18,353 ms and the same camera. Main saves wisp 357 with an
empty effect list; the candidate saves VFX 3006 and renders the bright wisp body.
The staged comparison includes an absolute RGB Delta ×4 panel: **4.1115%** of
displayed 1280×720 pixels exceed a maximum-channel difference of 2/255. These
are live presentation frames after particle warm-up; small background presentation
differences remain in the delta. A first-sync offscreen capture does not warm
the particle presentation and is not used as visual acceptance evidence.
