# Origins paired death blows

Static trace of the GOG Origins executable at image base 0x400000. Original-game
execution was unnecessary. Addresses refer to executable code, and all descriptions
below are independently written interpretations.

## Constructor, preparation and execution

`script.ldf` assigns COMMAND_TYPE_DEATHBLOW 38. Constructor 0x66e860 stores its
object argument in object slot 0 and its death type in integer slot 0, sets the
command's type to 38 at 0x66e967, and enables its override flag. The public
preparation table registers 0x745e10 for type 38 at 0x747767. Its completion handler
is 0x747360. The execution table initialized at 0x7ab954 is a distinct table of
internal steps. Its entries are indexed by internal step number. Applying
`(38 + 23) * 4` to that table does not locate this public command in this image.

Preparation stops locomotion at 0x745ebc and resolves the other creature, excluding
self. A default type of zero goes through the animation-selection callback at
0x74613f. Positive types identify rows of SyncDeathAnimations, M2DA 102. The
paired actor task is installed at 0x7461d2–0x746234, with death-task classification
6. Attacker playback is queued through 0x7a5fc0 at 0x7463ef. That helper appends
internal animation step 37, whose executor is 0x7a4390. The victim's animation
state is advanced through internal step 51, executor 0x7aac10.

0x5ca740 reads either SyncDeathAnimations (102) or UnsyncDeathAnimations (103),
choosing the attack column 0xf95409f5 or reaction column 0x2855abec. The result is
an animation-table row, resolved to its action and the creature's animation tree.
For example, sync row 1 pairs attack row 300 with reaction row 306. The shipped
one-weapon and shield trees provide the attack, and combat_humanoid provides its
reaction.

Both sides are prepared together. The victim's reaction does not wait for the
attacker's command-completion notification.

## Health, stored impacts and DEATH are separate

The animation-event receiver 0x6c0bf0 routes runtime kind 1 to 0x6a6230 at
0x6c0c45. 0x6a6230 chooses the stored main-hand result, or the off-hand result when
the marker text contains `Left`, and calls 0x6a6080 to deliver ATTACK_IMPACT.
This is the stored killing result's normal hit-event path. Command completion
contains no substitute hit event.

A paired death animation can have **no kind-1 hit marker**. The shipped
`mh_c.sw_dblow_atk_v1` has visual-contact markers of kind 19 at 500 and 1370 ms.
They do not route to 0x6a6230. Treating those contacts as killing impacts would
invent script events.

The victim's death task has its own health transition. On executing task
classification 6, **0x7ab31d–0x7ab33b reads current health and sets it to zero if
positive**, before testing its animation deadline. `IsDead` (0x65a3a0) calls
0x695300, which reads current health through virtual +0x120 and returns true for
zero or less. Thus the paired victim can count as dead from the reaction's start,
before a stored hit, and before the DEATH script notification.

The same death-task path checks the remaining animation time at
0x7ab401–0x7ab419. When it expires, it marks notification sent, constructs event
**4, DEATH**, at 0x7ab435–0x7ab466, takes its creator from the task's stored partner,
and dispatches it at 0x7ab485. Health zeroing and DEATH delivery have different
native boundaries. The simulator retains that distinction. A clip without a hit
marker produces no additional ATTACK_IMPACT on completion.

For the unscaled row-1 human pair the attack lasts 2967 ms, the reaction lasts
3300 ms, health reaches zero at reaction start, and DEATH is due after the
reaction. Native frame scheduling can defer either boundary to the update that
executes the corresponding task. Simulation deadlines use integer milliseconds.

## Placement

Preparation creates the partner context at 0x746325–0x7463d9. Ordinary pairs set
its signed separation to the negative combined body radii plus 1.2 m at
0x746384–0x74639a. Types 19 through 23 select the large-creature placement tables
118 through 122 and the other creature's appearance row.

0x7a5a30 computes the placement. It reads table separation and actor orientations
through 0x5ca7b0, with column hashes 0x414a699c, 0x92321b14, 0x85ffd766 and
0x909085da. Without a table match it uses the current partner bearing. Signed
separation shifts the actor along that bearing. The final optional floor
projection calls 0x73d520 at 0x7a5e62. Playback installs the resulting placement
through 0x7a3370.

## Sim contract and provisional boundaries

The victim exposes `combat.death_blow_reaction`, containing attacker id,
`started_ms`, `ends_ms`, the animation-table row `animation`, and its `action`.
Rendering can choose the paired reaction immediately and preserve its final
pose after the deadline. The record survives death and save/load, and resurrection
clears it and cancels its pending DEATH notification. The attacker exposes
`combat.death_blow` with its selected action, deadline and outstanding hand-hit
markers. Each authored hand marker sends that hand’s stored result. Results remain available
until command completion, as 0x6a6080 does not clear the native slot.

Explicit pair rows use the traced table columns. **Provisional** default selection
uses the first playable pair in table order. The native callback's filtering,
random variant choice and unsynchronized fallbacks remain unported. Animation-tree
precedence follows the existing ability lookup and remains provisional. Default
selection consumes no additional simulation randomness.
Clip milliseconds use ANBT and GAD speed metadata. **Provisional** native
playback modifiers and blend-dependent duration adjustments remain unported.

Ordinary pair separation and opposing facing are implemented. **Provisional**
placement is immediate, with the attacker's previous height. The native placement
blend, floor projection, large-creature table geometry and animation root motion
remain unported. Missing pair resources retain the old 500 ms stored-result
fallback. DA2 keeps its existing command behavior.

Older saves with an active opaque death-blow command restart preparation at load
time because those saves contain no animation start. Saves containing the new
paired runtime resume its saved deadlines and next-hand index.

The old-save restart and resurrection cancellation are simulation compatibility
choices. **Provisional** native save restoration and interruption of an active
death task remain untraced.

**Provisional departure completion:** native area unload during an in-flight paired
death remains untraced. Before an area transition sends EXIT or parks the old
area, complete that area's pending paired reactions and deliver their queued
DEATH notifications immediately, at the current simulation time. Direct
`enter_area` departures do the same before switching areas. Shorten the reaction's
`ends_ms` to the departure time, retaining its final corpse pose. Remove the
notification before calling scripts so a repeated/reentrant departure cannot
deliver it twice. Already-held paired notifications for the departing area from
older saves are also completed. Drain the immediate notification chain generated
by those DEATH handlers before EXIT/parking too: the Tower ogre signals its area
asynchronously, and parking that signal still loses the plot completion even
after DEATH itself is delivered. Track newly queued events so pre-existing events
retain their order and deadlines, and do not advance time to future follow-ups.
Notifications to other parked areas retain the ordinary holding policy.
Ordinary delayed events and deaths in other areas
keep their existing deadlines. Staying in the area still uses the authored reaction
deadline.

This prevents a provisionally selected reaction duration from withholding story
completion after departure. It does not claim native unload fidelity or change
the untraced pair-selection callback. Replace this boundary if a native trace
establishes a different policy.

### The 12.5-second default-selection reactions

There is no universal 12.5-second fallback constant. An audit of the shipped base
Origins combat trees and `apr_base` identifies these creature types whose
provisional first-playable default selection can resolve to a 12500 ms reaction:

| Appearance | Type | Combat tree |
| --- | --- | --- |
| 47 | Ogre A | `c_dsogre_cbt.anb` |
| 72 | Pride Demon | `c_pdemona_cbt.anb` |

Both trees map `db_c_ws_blade_die_v1` (animation 268) and
`db_c_dw_blade_die_v1` (270) to `c_dsogre.dblw_dmg_v1`, with duration 12.5 seconds
and unit playback speeds. The first matching pair depends on the attacker's
playable actions; the Tower run selects 268. These are asset-derived lengths
under an untraced default selector, not verified native selection for those
victims. Add-on/mod appearances using those trees inherit the same possibility;
this inventory covers the base Origins assets. Missing playable pairs use the
separate legacy 500 ms stored-result fallback, not 12.5 seconds.

## Oskias regression

`tools/repro/deathblow.sh` is also installed in the external test-data repro
directory. It loads a copy of `oskias-0420/save_before.json` through MCP, replacing
the hero's pending attack with a default death-blow command and a stored lethal
main-hand impact. The report's world, health, equipment and RNG are preserved.
This isolates the executor from combat-roll changes and the app's auto-attack loop.

With the provisional first-playable selection, the dwarf dual-wield pair is attack
304, `db_h_dw_attk_v1`, and reaction 314, `db_h_dw_die_v1`. Independently inspected
asset durations are 3133 and 3467 ms. The repro checks reaction and IsDead at 0 ms,
DEATH exactly once at 3467 ms, and the authored hit schedule. This attacker clip
contains no kind-1 hit, so its expected stored-hit schedule is empty.

## Initial PR validation and regression differences

Baseline is merged main `c609d2b7`. Formatting and workspace/all-target clippy with
warnings denied pass. Debug workspace tests pass with 1385 passed and 22 ignored,
including nine new synthetic-asset tests. The Oskias executor fixture reports
500 ms to health zero on baseline, and 0 ms on the branch, with DEATH at 3467 ms.

Round 2 uses its s01–s12 recipe and the master-save s13 run, with prebuilt binaries
and serial MCP processes. The original wrapper checks out a different worktree,
so a test-data-local adaptation runs those same recipes without checkout changes.
All 24 chain invocations and both s13 invocations return zero, with no refused
saves. Every step reaches its expected area. Gate differences require sim review.

| Step | Changed log lines | Plot events, main / branch | Explanation |
| --- | ---: | ---: | --- |
| s01–s08 | 0 | identical | All compared outputs agree. |
| s09 vials | 18 | 58 / 60 | First divergence follows the identical finisher queued at 578800 ms. Its victim's pending attack disappears at reaction start. The finisher completes at 583003 on the branch, versus 579370 on main. Subsequent combat and loot RNG diverge. |
| s10 cache | 4 | 48 / 47 | Carries s09's different position, time, loot and combat state. Conversation text agrees. |
| s11 Joining | 18 | 21 / 21 | Every changed line is the inherited 7054 ms time offset. Conversation, party and ordered plot events agree. |
| s12 tower approach | 4 | 97 / 90 | Later camp combat follows the different RNG/state. The branch enters the tower before the optional Alistair tower-base conversation. |

s09's wolf gains a mana item, the hurlock loses its add-in weapon drop, the
axe/shield group's grenade becomes chain armor, and the genlock drops its vials
without main's ammunition and health item. Round times and the next target's
distance change accordingly. The end is 16896 ms earlier, with three harness
heals versus four. Its two extra plot events are `TUT_COMBAT_BASIC_1` and
`TUT_MODAL_1`. s10 has one fewer `TUT_MODAL_1` event, a different cache approach
position, nine harness heals versus six, and returns 7054 ms earlier.

s12's council retains that 7054 ms offset. Its tower arrival is 252353 ms earlier,
with three harness heals versus seventeen. The fourth party member's allocated id
changes from 3925 to 3910 after the differing object/loot creation sequence.
The seven absent plot events are Alistair's event-on true/false and tower-base
true/false pulses, `PRE_BEACON_NIGHT_CAMP_TOWER_TALK_DONE`, one combat-basic
tutorial pulse, and the injury tutorial. `PRE_BEACON_TOWER_REACHED` is set in both.

### Initial master-save Tower comparison (before departure completion)

Both runs reach Flemeth's hut. Main takes 560 step calls, 11965 MCP calls and 25
harness heals, ending at 1918259 ms. The branch takes 604 step calls, 12661 MCP
calls and 23 heals, ending at 1979312 ms. The first command divergence again
follows an identical finisher admission, at 1424011 ms. Its victim loses a pending
attack and its attacker completes at 1427075, versus main's 1424608.

The plot traces contain 212 / 193 events. The branch has ten fewer combat-basic
tutorial pulses and two fewer Alistair release-hounds event cycles, each cycle
containing event-on true/false and release-hounds true/false. These pulses finish
with the same values. Party combat, injuries and recovery also diverge, producing
the branch's salve and injury tutorials.

The ogre exposes the area-departure boundary above. Provisional default selection
chooses reaction 268, `db_c_ws_blade_die_v1`, lasting 12500 ms. It starts at 1965667
and DEATH is due at 1978167. The harness sees IsDead and lights the beacon before
that deadline. The final save contains the ogre's DEATH in the departed area's
held-event list. Consequently the branch defers `PRE_BEACON_TOWER_OGRE_DEAD`,
`AUTOSS_PRE_OGRE_SLAIN`, ogre codex, and Aegis loot/codex updates. Its beacon use
sets `PRE_BEACON_REACHED`. These four main-only and three branch-only flags account
for every final plot-state difference. Native selection and departure handling
still need follow-up tracing. The departure-completion revision below supersedes
this initial Tower result.

### Determinism and presentation

Repeat and save/reload worlds agree in all six detchecks. Five run 600 ticks from
siege, Ostagar, Wilds-vials, named mid-Tower combat and the original Oskias save.
The sixth runs 50 ticks from the active Oskias pair, saving and reloading before
the attacker's deadline and the victim's DEATH event.

Hidden captures compare the same Oskias fixture at approximately 391 ms. The
staged comparison includes amplified absolute RGB delta ×4. Pixels with any
channel difference greater than 2/255 comprise 6.034722% of the frame. Rendering
currently shows the generic death animation and the new placement. Consumption
of the paired reaction action is the rendering handoff described above.

## Departure-completion review validation (2026-11-16)

Merged `origin/main` at `6d66c10e`. The merge preserves main's DA2 locomotion reset
inside the shared kill helper. Formatting and workspace/all-target clippy with
warnings denied pass on the merged code. Debug workspace tests pass with
**1403 passed, 25 ignored, 0 failed**, including all twelve paired-death tests.
The three departure regressions cover completion before EXIT and parking,
exactly-once notification across return, save/load of queued and held deaths,
the immediate script-notification chain, and isolation from pre-existing
immediate events, delayed events and other areas.

The rebuilt candidate passes the Oskias repro: reaction and IsDead at 0 ms,
attacker completion at 3133 ms, and DEATH exactly once at 3467 ms. All six
repeat/save-reload detchecks pass again, using the five 600-tick saves and the
50-tick active paired snapshot described above. Builds and headless runs use
the dedicated target directory and shared cargo-slot wrapper, with serial MCP
processes. Main and candidate use isolated build fingerprints to prevent a stale
baseline executable from being reused as the candidate.

### Tower departure and loot acceptance are separate checks

The unchanged fast-beacon recipe reaches Flemeth's hut on both binaries. Main
takes 560 step calls / 11965 MCP calls / 25 harness heals; the candidate takes
604 / 12661 / 23. Its ogre reaction starts at 1965667 ms and would naturally
finish at 1978167 ms. Departure instead delivers DEATH exactly once at
1971612 ms, before all three area EXIT notifications at that same timestamp.
The ogre's immediate area notification is also delivered before parking. Neither
the paired DEATH nor that notification remains held. Ogre-dead, ogre-slay
autosave and ogre codex now match main.

**The unchanged fast recipe does not have full loot/plot parity.** It polls
IsDead to finish combat and immediately enumerates body bags for looting. At
that point the paired victim has no body bag yet: its DEATH handler creates it.
The recipe then walks to and uses the beacon, with no later loot pass. Aegis is
present in the departed area's bag, but was never taken by the hero, so its
pickup codex remains unset. The late ogre-dead callback also observes Alistair
already near the beacon: the shipped plot handler sets `PRE_BEACON_REACHED`
when he is within 2.5 m of the beacon waypoint, and leaves its beacon-dialog
event active when the transition interrupts it. The fast run's remaining final
differences are Aegis codex, beacon-reached, Alistair event-on, and the two
combat-dependent injury/salve tutorials. Delivering DEATH at departure cannot
retroactively perform an earlier item pickup or reproduce earlier proximity.
No inventory transfer or plot-flag reset is invented to hide these differences.

A second master-save comparison uses the same recipe with one explicit adapter
on **both** binaries: after combat, wait for any active paired reaction deadline
before the existing dialogue/loot pass. It advances ordinary simulation time;
it does not alter a save, shorten a clip or write a plot flag. On the candidate
only the ogre needs this wait. Both runs reach Flemeth's hut with **identical
final Tower plot flags**, including ogre-dead and the absence of beacon-reached.
Both have ogre-slay autosave, ogre codex, Aegis in the hero's inventory, and
Aegis codex. Main takes 560 step calls / 13007 MCP calls / 25 heals, ending at
1918259 ms; the candidate takes 605 / 13572 / 23, ending at 1988412 ms. The extra
MCP calls inspect pending reactions. Candidate DEATH runs at its natural
1978167 ms deadline, before EXIT at 1980712 ms.

The reaction-aware plot traces contain 212 / 196 events. Main has ten additional
combat-basic tutorial pulses and two additional Alistair release-hounds cycles
(event-on and release-hounds, each true/false). Their final values agree. The
only remaining final **global** flag differences are the candidate's injury and
salve tutorials, caused by the previously explained combat/RNG divergence.
There are no remaining Tower, loot, autosave or codex differences in this run.

The fast run exercises the provisional departure boundary; the reaction-aware
run exercises complete corpse looting and final story parity. This distinction
is necessary because the traced executor deliberately separates IsDead from
DEATH. The fast run is not presented as passing unchanged-recipe loot parity.
The external `s13-counts.sh` adapter selects the second mode with
`DEATHBLOW_WAIT_CORPSES=1`. Logs and assertions are in
`codex-captures/deathblow/departure/tower-analysis-fast.log` and
`tower-analysis-wait.log`, with saves/traces under
`gate/trace_deathblow-departure-{A,B}` and
`gate/trace_deathblow-departure-wait-{A,B}`. The external collector also corrects
its nonexistent `objects` MCP call to `list_objects`; both final Tower variants
return zero and collect their traces successfully.
