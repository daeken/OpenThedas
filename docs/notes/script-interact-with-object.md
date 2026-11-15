# Origins ambient interaction requests

Static trace of Origins 1.05, image base `0x400000`, using the read-only
DAODecomp dump. No original executable was run. These notes describe our
understanding, not executable material.

Native 587 is registered at `0x678903` with handler `0x65c110`. It reads
two objects, an interaction id, pose, loop count, exit flag and skip flag.
The first object must resolve as a creature (`0x5d58c0`); otherwise the
entire operation stops. It writes that creature's requested interaction,
pose, loops and exit flag at `+0xc74..+0xc80`. If the target is also a
creature, the same writes and command admission are performed for it with
the first creature as target and the reciprocal role.

`0x6a9390` updates those request fields even when interaction type 37 is
already active. In that case it leaves the command and its target alone.
Otherwise `0x6a3920` creates type 37, retaining target and role, exit and
skip flags, then admits it at the front with table-selected add behavior
(`0x73cd10`). The simulation reuses its existing AddCommand handler.

The type-37 factory at `0x743690` appends internal animation action 53.
Unless the skip flag is set, it first resolves a placeable target and calls
`0x5ca860` with table 123 and the interaction id. A valid relative point
outside the actor's native tolerance produces move-to-location and turn
actions. Other targets or an unavailable relative point produce
move-to-object and face-object actions. The placeable helper reads columns
`0x414a699c`, `0x92321b14`, `0x85ffd766` and `0x909085da`; a nonpositive
first value rejects the relative point. Angles are degrees. Table 94's
shipped interaction row selects add behavior 2 for both matching and
different active types, so ordinary active and waiting commands are cleared.

The animation executor is `0x7a6ea0`, installed as internal action 53 at
`0x7abb34`. It resolves the retained target, failing with -7 when absent.
It selects table 123 for placeables and table 124 for creature partners.
`0x7a4bc0` selects entry, numbered pose and exit cells separately for the
primary and reciprocal roles; absent animation ids fail with -3. A changed
interaction id restarts entry. A changed pose replaces the current pose
only while in the pose phase. The loop count and exit choice are read from
the live creature fields. Zero loops or a nonpositive pose skips the pose;
a negative loop count holds execution. A finite looping animation uses its
length multiplied by the loop count; a nonlooping animation uses its own
length. The exit flag controls the final phase.

The headless implementation retains requests and phase progress in saves,
uses the existing navigation/mover and command-completion paths, and reads
animation lengths through the existing cached animation reader. Its movement
fallback uses the simulation's existing interaction reach; native tolerance,
turn rate, animation-tree arbitration, linked animation sequences and root
blending remain approximations. This change does not add presentation of
the interaction animation or cancellation exit blending. It does not change
CommandPlayAnimation or CommandFly, and DA2 keeps its previous unanswered
dispatch path. Synthetic tests exercise engine dispatch, reciprocal roles,
live changes, finite and infinite phases, move/skip behavior, invalid data,
target destruction and saved state.
