# Origins equipment and Proving actor placement

Traced for reports #1102 and #1103. Addresses refer to DAOrigins.exe; the tests
use synthetic objects and tables. The report saves remain external test assets.

## Two-handed equipment

`0xa664c0` calls `0xa66440`, which reads BITM column `0xd5b0ae22`
(`EquippableSlots`), and compares the entire mask with `1 << 0`. The native
two-handed predicate means **main-hand-only**, including launchers; checking
the animation style or a list of greatsword base types would miss cases.

The active equipment container's add hook (`0x797d10 -> 0x6c16e0`) handles
hand conflicts before applying the new item's properties and sending EQUIP:

- A main-hand-only item unequips the active off hand through `0x6b7690`.
  That removes its effects, sends UNEQUIP synchronously, and raises training
  event 1070. Ordinary displacement of the previous main-hand occupant does
  not raise 1070.
- Inserting an off-hand item through this low-level hook unequips a
  main-hand-only main-hand item through the same routine. This is distinct
  from refusing an off-hand drop on the inventory screen.

The script EquipItem handler (`0x65cda0`, call at `0x65ce5c`) and GUI message
handler (`0x6be330`, call at `0x6be3fe`) both reach `0x6bc7f0` and the container
hook. The GUI handler sets flag `0x80`, reported in EQUIP integer 1. Keeping
conflict cleanup in the shared wear hook also covers default-slot EquipItem
and template equipment. In inventory.gfx, the equipment-slot Update compares
`GUI.SelectedChar.Equipment.0.EquippableSlots` with 1 and represents the
occupied second hand with a dimmed main-hand icon. IsDropAcceptable checks
equipment access, damage and the incoming slot mask; AcceptDrop forwards
the item's ObjectID and the slot ID to `GUI.SelectedChar.EquipItem`.
The requested off-hand drop refusal is enforced at that simulation message
boundary, while native script insertion retains the executable's replacement
behavior. Automatic slot selection skips the blocked off hand.

In #1102 the hero has Everd's BaseItem 5 greatsword in slot 0 and a BaseItem 3
dagger in slot 1. Re-equipping the greatsword must return the dagger to the
backpack, remove its effects and deliver its unequip event.

## Arena marks and duplicate actors

The reported #1103 opponent is already at the script's entry waypoint:
`proving_core`'s fighter setup builds `proving_opp_00` and uses the local-jump
helper. Mainar's active instance is at that waypoint, approximately 15.69 m
from `proving_pc_enter`. This is not a failed route under #1029, a displaced
safe-location result, or a wrong JumpToObject target position.

The conversation then stages the duel. Its authored end poses move the hero
and opponent closer together when the line is released. The save contains
two Mainars: an inactive waiting-room instance with the lower ID and the
active opponent at the entry waypoint. It also retains a parked Proving
master from another area. The old first-tag lookup binds the waiting-room
Mainar and can start the conversation on the other area's master. The live
opponent consequently remains at the entry waypoint instead of taking the
conversation's duel mark.

Native actor binding is `0x7e36b0 -> 0x7cd0a0 -> 0x7f2d80`. For an ordinary
tag, the resolver uses its context creature (falling back to the controlled
creature) and calls `0x6973b0`. This searches the context's area and retains
the closest matching object by **3D distance**. Equality also replaces the
previous candidate. It does not choose the first global tag. The shared
cutscene-end script likewise looks up its next speaker with
GetNearestObjectByTag relative to the hero, rather than GetObjectByTag.
The conversation context is its owner: `0x697748 -> 0x68cf49 -> 0x7ef34c`
stores the owner's ID at conversation `+0x28`, and `0x7f0b07` copies it to
the cutscene's context field `+0x244`.

The correction uses the nearest same-area actor for Origins staging and the
cutscene's next speaker. DA2 retains its existing binding behavior. Scripted
cutscenes still restore their actors, and conversation staging still honors
the existing restore/moved-object rules. No jump, routing, or safe-location
algorithm needs changing for this report.

## Reproduction

Run `sim-work/repro/twohander.sh <thedas-mcp>` and
`sim-work/repro/arena_mark.sh <thedas-mcp>`. Each prints PASS/FAIL and writes
its prepared/result saves outside the repository. The arena check verifies
both the live actor binding and release to the authored duel mark.

Report #1105 remains with the feet-snap rendering investigation (#1034).
This change does not alter vertical placement or floor snapping.
