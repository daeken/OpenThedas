# Origins native 860: RequestTarget

Own prose from the read-only GOG 1.05 executable trace. DA2 dispatch is unchanged.

Registration `0x67928e` selects `0x664840`. The handler reads an integer target
mask, two floats, an integer callback event type, a recipient object, and a
string script override. It supplies the script caller separately to `0x70bb70`
through the keyboard/controller client paths. That routine resets the previous
request (`0x70ab60`), enables script targeting, stores the mask, first float,
event type, caller, recipient and script, and selects circular targeting when
the first float is nonzero. The second float is not read by that routine.
The float is a targeting radius, not a timeout.

The target checks (`0x70d550`, `0x70f030`) distinguish self (mask 1), non-hostile
creatures (2), hostile nonmembers (4), placeables (8), items (16), ground (32),
dead creatures (64), and hostile active party members (128). Self and dead
creature checks precede the relationship checks. The creature health predicate
is the same integer-current-health virtual used by primary selection. The
protected creature attribute at `+0x4e8` is named IsHostile by the reflection
binding at `0x6b4859`. Party membership uses `0x631830`.

On selection, `0x70d8d0` builds the supplied event type, with the currently
controlled creature as creator (`0x694990` -> `0x5d5cc0`), object parameter 0
equal to the selected object (invalid for ground), and vector parameter 0 equal
to its position or the selected ground point. It optionally overrides the
recipient's script, sends synchronously through `0x64ad80`, then the client
clears targeting through `0x7113f0`. A replacement request created by the
callback is consequently cleared too. Replacing or cancelling an outstanding
request itself sends no callback.

The simulator stores additive optional TargetRequest state and routes player
move/use/talk/attack clicks through the request before ordinary orders. Invalid
choices consume the click and leave targeting active. The public response and
cancel methods also support a headless caller. The second float does not affect
the request, and no automatic completion is invented. JSON snapshots retain a
pending request; old snapshots default to none.

The cached IsHostile attribute is used when explicitly present; otherwise target
validation uses the simulator's existing relationship calculation. The radius
is retained for presentation, but a circular targeting reticle is not rendered
here. The native client-specific cursor and camera changes remain presentation
limitations. Additional object kinds use the existing CCObject mapping.

Five tests invoke inferred 860 dispatch and cover field mapping, ignored second
float, replacement/cancellation, callback payload and current creator, native
object position, ground targeting, click consumption, all mask categories,
invalid IDs, JSON compatibility, and unchanged DA2 dispatch.
