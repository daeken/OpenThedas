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
node's once flag. The loaded GAD duration comes from field 4009.

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

Animation playback, replacement/completion delivery, fading and destruction
scheduling remain open under SCR-009. State-change PWK installation and door
group restoration are implemented in `placeable-initial-state.md`; full
physics remains open. These static
traces used the existing Origins analysis files. The original executable
was never run, and this note contains only our own description.
