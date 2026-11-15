# Origins placeable conversation owners

`BeginConversation(target, owner, resource)` must accept living, active placeable
owners. The Dwarf Noble sarcophagus (`bdn210ip_floor_trap_door`) uses this path
through `bdn200ip_floor_trap_door`'s USE handler and its inlined `UT_Talk`.
Rejecting every owner except creatures silently prevents its puzzle dialogue.

Static trace: DAOrigins.exe SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`,
image base `0x400000`; addresses below are VAs. No original executable was run.

- Command 440, `0x65d9a0`: when target equals owner it substitutes the hero
  (`0x65da95..0x65daaa`). It resolves the owner, calls the virtual at
  `[owner+0x70]+0x6c`, tests death through `0x695300`, then calls `0x6976b0`.
- The virtual is **not a creature-type test**. The placeable constructor installs
  `0xb1bf5c` at `+0x70` (`0x750d61`); its slot `+0x6c` is `0x699fa0`, also
  present in the common object's `0xb0ed8c` table. That function checks bytes
  `+0x255` and `+0x3b1` relative to this subobject, corresponding to object
  offsets `+0x2c5` and `+0x421`. The latter byte's meaning is not established.
- `0x695300` tests whether the object's current-health virtual (`+0x120`)
  returns at most zero. The placeable main vtable `0xb1c084` uses `0x750f50`,
  reading its independent current health (`+0x66c`). The sarcophagus template
  supplies Health 1. This is independent of a saved creature-style dead flag.
- `0x6976b0` checks active and death again, uses the object's own dialogue at
  `+0x3d8` when the resource is empty, creates the conversation via `0x68cee0`,
  and requests game mode 4 at `0x697765`. It contains no creature-only check.

The fix adds Origins placeables to the existing owner gate and uses the existing
placeable current-health accessor. Creature eligibility and DA2's two-argument
command remain unchanged. This does not claim to implement the unidentified
`+0x421` flag. Synthetic tests cover owner/default-resource resolution, inactive
and dead placeables, signed template current health and explicit older-save
property currents. The external Dwarf Noble probe exercises the shipped scripts.

The supplied explorer has a separate routing omission: it neither positions the
three followers on the pressure plates nor interacts with this placeable. The
story also requires actually taking the shield, completing Gorim's conversation,
and bringing a party member back through the betrayal trigger. See the external
`origdn.report.md` for the save-specific route and observed validation results.

## Pressure-plate area-of-effect creators

The same origin exposes another creature-only filter in `create_aoe`. Each plate
applies its persistent AoE with itself as creator. On ENTER, the pad script reads
`GetEventCreator`, sets that placeable's `TRIG_TALK_REPEAT` to the AoE member count,
then reads it back. Only count one advances a plate flag. Filtering the creator
to creatures makes both local calls use OBJECT_INVALID, so no plate flag is set.

In the same pinned exe, `0x7a03bf` reads the effect's creator at `+0x40` and
passes it to `0x7e4720`, which stores the id unchanged at AoE `+0x4ac`.
`0x7e5c02..0x7e5c0e` forwards that field as ENTER's creator to `0x698850`;
`0x7e5c7c..0x7e5c88` does the same for EXIT. The first heartbeat copies it to
event `+0x24` at `0x7e4ba3..0x7e4bad`. None of these paths filters the creator's
object type. Origins now preserves the id; DA2 retains its existing policy.

Previously saved chamber AoEs already contain OBJECT_INVALID instead of the
plate's creator. That information is lost: the fix does not guess a creator on
load. Resume the supplied earlier `dwarf_noble_03_bdn200ar_ruined_taig.json`
and enter the chamber freshly. Saves made after the fix preserve creators through
save/reload, covered by a synthetic ENTER/EXIT/heartbeat test.
