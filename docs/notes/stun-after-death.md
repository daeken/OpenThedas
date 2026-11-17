# Stun and death cleanup in Origins

Static inspection of DAOrigins.exe and the shipped creature_core/rules_core
scripts; addresses below are executable virtual addresses. No original code or
resource bytes are included here.

## Removal policy

The ordinary lethal-effect path does its cleanup in the script's APPLY_EFFECT
handler, before the native death effect takes health to zero. In rules_core,
the accepted ordinary-death branch calls its effect-removal helper with both
keep-injuries and death filtering enabled (script offsets 0x9c3c–0x9c50).
The helper at 0xa04a gets the effects, excludes injuries when requested and checks
effects.gda's IgnoreDeath column before calling RemoveEffect. The creature's
DEATH event is a separate notification; setting `dead` alone does not describe
the complete lethal-effect path. The same shared helper is compiled into
creature_core at 0x7e2b (call at 0x7b11).

The executable's RemoveAllEffects handler, 0x673ab0, agrees with that policy:

- bDeath adds the IgnoreDeath predicate 0x651830, which calls 0xa653a0.
- bIgnoreInjuries adds predicate 0x651850: ability IDs strictly between 500000
  and 500100 survive.
- The removal traversal 0x7a0c90 additionally excludes duration types 4, 5 and
  6. Type 6 is EFFECT_DURATION_TYPE_DEATH; this is a duration classification,
  distinct from effect type 7, EFFECT_TYPE_DEATH. Permanent duration 3 is
  removable.

In the installed base effects table, IgnoreDeath is set only on death (7) and
mabari dominance (1068). Stun (1027), ordinary buffs, damage over time, upkeep
and heartbeat have no such exemption. These are table-driven rules: extensions
can change the values. A surviving effect can still expire by its timer.

The health boundary is 0x6bc4b0. Native paired death-animation execution zeroes
health at 0x7ab31d–0x7ab33b and sends DEATH later at 0x7ab435–0x7ab485. These
are distinct from the accepted script-driven lethal-effect branch. This trace
does not establish a universal native instruction that clears every effect
whenever a creature becomes dead, and does not justify adding one to Sim::kill
or expire_effects.

There is also a presentation-only exception: native death-effect application
0x7a12e0 changes that effect to duration 6 and priority 999. Its traversal at
0x7a137d–0x7a13df selects active type-4 visuals of duration 5 and stops their
visual handles through 0x7a0c50, retaining the effect records. Removing death
at 0x7a1410 traverses those records with the normal application callback
0x7a0e30. Thus an effect record surviving death does not always imply its
visual should remain visible. This is separate from temporary stun's script
removal and belongs to presentation parity.

## Stun's script state and presentation

rules_core's stun application subtracts 1000 from the Dexterity modifier
(0xe56d–0xe585). Its REMOVE_EFFECT dispatch selects the stun callback at
0x10a40; the callback at 0x12b5f reverses that subtraction through
UpdateCreatureProperty. The stun constructor also sets engine integer 2 to
visual effect 1009 (0x154d9–0x15508). The type's table flags are 543 and its
default animation is 271; there is no separate saved boolean `stunned`.

Native effect removal enters 0x7a0a90, marks the effect removed, and routes to
0x7a08e0. Script-handled effect types use 0x7a0840, which sends REMOVE_EFFECT
(32). The traversal 0x79f5d0 deletes successfully removed entries through
0xa64840; if the entry was the selected effect, that function chooses its
replacement through 0xa64640 (highest positive priority), or clears the selected
pointer when none remains. The removed stun therefore no longer supplies its
animation or flags. The traversal also notifies its owner after removal. Visual
cleanup is separate from the script callback: 0x7a0150 stops the effect's visual
handle through the owner's virtual method, invalidates that handle and updates
visual selection through 0x79f7a0. That selection considers the remaining
effects; it does not leave the removed effect as the active visual.

## OpenThedas

Sim::apply_effect runs APPLY_EFFECT with the lethal effect current. Nested
RemoveEffect calls remove the actual stored effect and run REMOVE_EFFECT with
that removed effect current, then restore the outer lethal effect. The accepted
death effect subsequently calls Sim::kill. RemoveAllEffects in engine.rs
already implements the duration, injury and IgnoreDeath filters above.
expire_effects only handles elapsed timers; kill clears commands and combat
actions and queues DEATH. Neither should replace the script's selective removal
with a blanket effect clear.

One separate mismatch remains in the script-facing RemoveEffect API: native
handler 0x674590 rejects duration 5/6 at 0x674625–0x674630, then removes by ID
through 0x7a0c90, which also protects duration 4. Our engine.rs RemoveEffect arm
calls the unconditional internal Sim::remove_effect directly. Consequently the
shipped helper's individual removals can over-remove duration 4–6 records here,
although RemoveAllEffects itself has the correct guard. The exemption matrix
unit test covers that latter API, not the entire shipped helper. This mismatch
cannot explain a retained temporary stun; it is recorded for a separate API
parity change rather than changing runtime sim behavior in this handoff.

Presentation reads attached VFX from Object::effects (visual_effects in
thedas-ui/models.rs). Its ordinary dead motion selects a non-looping death
animation. A corpse whose effect list is already clear should therefore be
investigated at presentation/VFX lifetime and scene rebuilding, including
Live::show_effects, rather than by removing more simulation effects.

The independent synthetic unit fixtures cover both temporary and permanent
stun, damage over time, injuries, IgnoreDeath, duration types 4–6, callback
execution exactly once, nested current-effect restoration and corpse
save/reload. tools/repro/stundeath.sh exercises the installed game handlers over
MCP and records the effects, aggregate flags and Dexterity modifier around
stun, lethal effect and reload. It mounts only its own two generated scripts in
a private list; it does not modify the install or a player's mod list.

## Runtime result

The repro passes on the exact playtest MCP build 81af4d72. Starting from the
Proving report 1103's save_now.json, active Mainar (855) carries stun 1027,
engine visual 1009, flags 543 and Dexterity modifier -1000. Death leaves an empty
effect list, flags 0 and modifier 0 in the same simulation millisecond, with
120 seconds still left on stun's timer; save/reload retains that clean corpse.
The watched MCP call trace confirms rules_core tests stun's IgnoreDeath as 0,
runs its REMOVE_EFFECT callback with effect type 1027 current, restores 1000
Dexterity, accepts the outer lethal effect, then delivers DEATH at 2692149 ms.
The environmental caster in the fixture intentionally avoids starting an arena
fight: this between-duels save has neutral opponents, on whom rules_core rejects
a hostile creature's stun. The real creature_core/rules_core handlers still
apply and remove the effect.

That source save also contains the earlier dead Oskias (706), with an empty
effect list. No supplied save captures the particular stunned corpse from the
verbal playtest report. The sim failure is not reproduced; the lingering visual
reported in the app is a rendering handoff, not evidence for changing kill or
expire_effects. This PR changes tests and trace notes only; DA2 runtime is
unchanged.
