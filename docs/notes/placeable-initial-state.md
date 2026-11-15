# Origins placed state initialization

Static trace of the GOG 1.05a executable, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are virtual addresses; the original game was not run.

A read-only census of the base campaign's designerareas.erf confirms 862 of
5,581 instances have nonzero InitialState: 713 locked, 106 open, 27 Open_2,
eight Dead and eight Dead_2. Redcliffe's Dwyn and Owen door instances both
start at row 1; the Dwarf Commoner home door starts at row 2.

The area instance uses the GFF3 label `InitialState`. The property-name helper's
PE string lookup identifies its ASCII name at `0xb1be60`; it is not a GFF4
numeric field. The ASCII placeable reader `0x750370` reads it at
`0x7506ba..0x7506df`, keeping the current word as the missing-field default and
storing the low unsigned 16 bits at object offset `+0x658`. Template values are
loaded first. `Useable` is read separately immediately afterwards.

The current state is also the binary save field **16100 / 0x3ee4**:
`0x74d650..0x74d667` writes the same word, and `0x74fe42..0x74fe62` reads it
back, using the existing word as default. Loading a save therefore restores
current state; it does not reset the object to its area's initial value.

Model setup `0x752a7c..0x752a88` and model refresh `0x75206a..0x752076`
force the current state through `0x74f9d0(state, 1)`. Unlike the ordinary
script setter, this bypasses the equal-state return. Both require a loaded,
nonempty appearance controller. The request is an unsigned short, bounded by
the controller's record count, with invalid requests becoming zero. A missing
controller leaves the word read from the instance intact.

The forced path plays the destination's diagonal animation directly
(`0x74fa8d..0x74fab0`), rather than playing a transition from the template's
state. The existing renderer's first-sighting pose selection provides that
behavior. The state path sends no script event. The action records it reads
are controller actions, not events. Health remains a separate value: a state
named Dead does not itself constitute a lethal health write. Native forced
initialization of an already nonpositive destructive object can request
destruction; animation/fading/removal continuation remains outside this fix.

The state path removes the old PWK footprint (`0x74fb27`, operation 1), updates
door physics (`0x74fb3d`), then installs the new footprint (`0x74fb4e`, operation
0). Area registration also installs the current grid at `0x750caa`. Both use
the eligibility gates at `0x74dba0`: unsigned state index in range, non-null
grid, original PHY mask without Items bit 1 unless appearance type is 1, and
positive health when the appearance's destruction predicate is enabled.
Footprint filenames retain the full appearance integer; table lookup uses
its low unsigned short. See `script-placeable-walkmesh.md` for the PWK layout.

At `0xa46cc0`, operation 0 copies every transformed source cell kind over
destination kinds below 8; destination kinds 8 and above survive. Operation
1 changes destination kinds 7/8 to 1 without consulting the source kind.
Neither changes heights or surface materials. Iteration is column then row.
The implementation saves sparse per-area writes and uses the existing cache
reconstruction on reload. Later transitions must replace the initial footprint
too, so state setting now performs the same clear/install pair.
The active setter is CCPlaceable vtable `0xb1c084` slot `+0x140`,
`0x74f200`: after the base active write it installs for active and clears for
inactive. Model setup applies the loaded active flag through that same slot
at `0x752096` / `0x752aa3`. Initialization therefore clears an inactive
object's footprint, and SetObjectActive updates PWK along with the flag.

For appearance type 1, `0x74e850` asks `0x74bcc0` for the active controller
state's blocking predicate (column hash `0x86a27132`). `0x74d350` restores the
original PHY group for a blocking state and sets group 1 otherwise. This keeps
an initially open door out of creature and sight collision.
The doorway approximation checks the restored group's NonWalkable bit, rather
than Creatures: the home door's PHY belongs to Placeables | NonWalkable and has
no Creatures bit. Group 1 still disables both doorway and sight blocking.
The locked state
is the controller row itself: Closed_Locked offers Unlock rather than Open;
there is no additional lock boolean to seed. PickLock and key requirements
remain their separately loaded instance/template fields.

Finally `0x74fb61..0x74fba1` disables interaction when both state action IDs
are zero. Controller loading at `0xa3bccf..0xa3bdcf` reads up to two
action/target/variation triples after the state-animation columns and before
the blocking column. This path never enables an object disabled by Useable.

OpenThedas initializes after template and instance overrides and before SPAWN
delivery. Save revision 10 repairs only Origins placeables lacking State but
carrying InitialState, in stable object-ID order for overlapping footprints.
Explicit saved State and existing pathing patches survive; revisiting a known
area does not initialize again. DA2 retains its existing behavior.
Migration uses an unloaded object's remembered active flag, so parking an area
does not turn its initially closed doors into group-1 objects on return.
