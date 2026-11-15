# Origins placeable state acceptance

`SetPlaceableState` at 0x65ee60 resolves a placeable through 0x5d5b40 and
calls 0x74f9d0 with force zero. Successful placeable actions at 0x74fc10
reach the same ordinary state path. The action path first skips a target
whose low unsigned short is 0xffff at 0x74fc40. Explicit setter requests
continue through the ordinary setter's bounds check. The action controller
at 0xa3bb58 selects its primary or variation target directly, including
zero and the absent sentinel, according to the variation argument.

The appearance setup at 0x7508c5 reads table 93, column 0x9a320a51, using
the unsigned low 16 bits of appearance. A nonempty name opens the controller
through 0xa3be40. That manager rejects missing and zero-row controllers.
The controller loader at 0xa3bc73 allocates one record per table row position.
Its record count supplies the state bounds, including for sparse row IDs.

The ordinary setter first requires that loaded controller. It compares the
current state and request as unsigned shorts and returns immediately when
they agree. Otherwise it queries the transition and destination-pose
animations, then stores the requested unsigned-short state if it fits the
controller count, or zero if it exceeds the count. The comparison precedes
the bounds check, including for an already out-of-range current state.

The simulation now applies these acceptance rules through both the script
setter and successful action results. Synthetic engine-call regressions
exercise unsigned-short requests and appearance lookup, sparse controller
IDs, absent/empty controllers, same-state requests, invalid object types,
failed actions and the existing DA2 path.

## Playback continuation

The native ordinary path plays a nonzero transition at rate 1 with the
default blend argument. A nonlooping transition sets the outstanding
completion byte. A looping transition clears it. A zero transition leaves
the byte as it stood. The destination's diagonal animation supplies the
subsequent pose through 0x6956d0.

The playback emitter at 0x815990 receives a previous/current clip-time
window. In forward playback its synthetic event 13 occurs when previous
time is strictly below the loaded clip duration and current time reaches
or passes it. Its loop argument suppresses that endpoint notification.
The caller at 0x811d10 derives this argument from the instantiated tree
node's once flag. The loaded ANI duration comes from field 4009 through
0x811ce0 and 0x815fb0. This loader also reads the ANI tracks (4005);
it must not be confused with the separate GAD duration query at 0xa38ed0.

Playback progress uses unsigned-short normalized positions. The update at
0x811be0 multiplies elapsed time by the playback rate, truncates the step,
and updates the normalized position. The conversion at 0x811d10 uses
65535 as its scale. The animation system advances active nodes at
0x814260, including their blend state. Action changes at 0xa5f9c0 can
blend the outgoing action toward zero while the incoming action starts.
Completion scheduling consequently needs the saved playback and blend
state as well as the clip duration.

The listener at 0x694ee0 sends event 13 to CCPlaceable's completion handler
at 0x74d200. That handler clears the outstanding byte and checks current
health and the current appearance's destruction predicate. On the enabled
nonpositive-health path it targets opacity zero and requests destruction
after 3000 ms. The request at 0x696a60 preserves the first pending request.
Opacity advances using client delta divided by 500 through 0x695a90.

The ordinary state-controller path now retains one-shot nodes in the
simulation, including their unsigned-short progress and last update time.
It starts at rate 1: 0x813d70 converts the requested rate to a float
`65535 / duration`, resets both positions and enables the node. Neither
the ANB default speed nor GAD field 4010 multiplies this explicit rate.
0x811be0 truncates each update's normalized increment to an integer and
uses its low unsigned short before adding and clamping. Consequently,
splitting updates can change the endpoint tick; the saved normalized clip
position must not be replaced by a rounded duration deadline.

The loop query at 0xa5e940 reaches controller 0x5474a0 and component
0x813ca0. A loaded node supplies the inverse of its once flag. Missing
nodes fall back to animation-table flag bit 1. A nonzero transition updates
the outstanding byte even if no playable resource is found; a zero
transition leaves existing playback and that byte alone.

Replacement is not blanket cancellation. 0x814260 advances every active
node, including an outgoing node at zero weight. 0x811c70 deactivates a
once node only when its position has reached 65535 and its weight is no
longer positive. The endpoint emitter still runs at zero weight. Replaying
the same node resets that node rather than adding a second instance.
Completion 0x74d200 does not guard on the outstanding byte or require the
notifying node to remain the current action. These details matter when a
living object starts a transition and is killed before an outgoing node
finishes, or when a looping replacement clears the outstanding byte.

Notification 11 is distinct from endpoint 13. 0x815d6d tests a nonnegative
per-node notification threshold; it does not announce every replacement.
0xa5f9c0 sets that threshold through the optional transition lookup
0xa5f350. The transition registry is installed by 0xa5d5a0, whose direct
setup call is in the creature path at 0x6bdfb9. Ordinary placeables do not
install that registry. Their playback starts with threshold -1 at
0x813e2b. The listener routes notification 11 through 0x696bd0: ordinary
clips reach completion, while the GAD 4011/4012 special modes request a
weight change instead. This is not a reason to cancel all outgoing nodes.

Completion clears the saved outstanding byte, rechecks current signed-short
health and the low-short appearance predicate, records the opacity-zero
target time and queues removal after 3000 ms. Existing removal requests
win. Both in-flight nodes and the first removal deadline survive JSON
reload; older snapshots do not invent an animation from their stored state.
Parked playback pauses and refreshes its clock, and Active is independent.
DA2 keeps its previous path.

Presentation opacity interpolation, default-pose playback and other
animation entry points remain open; this slice handles ordinary
state-controller transition endpoints and removal. State-change PWK
installation and full physics remain open as recorded in the walkmesh
note. These static traces used the existing Origins analysis files. The
original executable was never run, and this note contains only our own
description.
