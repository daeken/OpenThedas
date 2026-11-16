# Origins native 861: SetPlotActionCooldown

Own prose from the read-only GOG 1.05 executable trace. DA2 dispatch is unchanged.

Registration `0x679472` selects handler `0x665b20`. It reads an action ID,
float seconds, callback event type, recipient object and optional script
override, and supplies the script caller separately to `0x6fcf40`.

That routine searches the manager's existing action records (`0x6fcad0` /
`0x6fca50`). Invalid IDs are inert. Negative or unordered durations are ignored;
otherwise it replaces total (`+0x9c`), CooldownTime (`+0xa0`), CooldownPercent
(`+0xb0`, initially 1), event type (`+0xc0`), caller (`+0xc4`), recipient
(`+0xc8`) and script (`+0xcc`). This setter can shorten an existing cooldown.

Action records come from table 142, PlotActionDefines (`0x6fda69`), whose
column `0x16a3cb96` supplies their set/group. `SetPlotActionSet` calls `0x6fdb40`,
which rebuilds the active vector from records with a nonzero state and matching
group; set zero empties it. The update at `0x6fc7b0` walks only that active
vector, skips a paused game or an unresolved controlled creature, and decreases
positive remaining time by the frame's seconds, clamping to zero. It updates
the fraction as remaining/total. Merely disabling the buttons does not empty
the active vector or suspend the clock.

On a positive-to-zero transition, the update sends the configured event type
synchronously through `0x64ad80`, with the current controlled creature as
creator and integer parameter 0 equal to the action ID. It applies a nonempty
script override and addresses the supplied recipient. Zero durations never
enter the positive-clock path, so they do not immediately fire a callback.

The simulator retains serialized cooldown state on PlotActions, updates it as
simulation time advances, and settles the previous set/state before script
changes to selection or enabled state. Hidden/inactive intervals do not count
when an action returns. Expiry is recorded before delivery so callback-driven
replacement is safe. Raw float bits preserve positive infinity and its NaN
fraction in JSON; older worlds default to no cooldowns. The remaining time and
fraction are available through `plot_action_cooldown`.

Existing army-control presentation is incomplete (UI-197); this implements
the script timer and callback state without adding that screen. The simulation
clock is advanced by its caller while gameplay runs, so a globally paused
client must continue to hold that clock as it already does.

Five tests invoke inferred 861 dispatch and cover field mapping, callback
payload, single expiry, active set/state changes, button disablement,
replacement with shorter durations, invalid/negative/NaN input, zero, absent
control, infinity/NaN JSON preservation, old snapshots and unchanged DA2
dispatch/clock behavior. No asset parsing changes are introduced.
