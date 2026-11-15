# Ambient dialog lifetime on area exit

Static trace of the GOG DAOrigins.exe, image base 0x400000, SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

The game-mode update at 0x69468b calls ambient manager update 0x6ff570.
That update suspends playback in some modes (0x6ff5a3..0x6ff633), iterates
retained ambient entries, subtracts elapsed time at 0x6ff6e5, waits for voice
completion, then advances the conversation through 0x7f1910 at 0x6ff796.
It does not itself test the owner's active byte or compare its area. The
speaker lookup at 0x6ff6f9 is for the voice-playing test; a failed lookup does
not by itself remove the entry. Thus existence in our object registry is not
evidence that the ambient conversation should survive a transition.

The area-unload state path 0x6c8420 calls 0x6c5e40 at 0x6c84ee. That helper
calls the ambient end selector 0x6ff8e0 with owner -1, all=true, stop=true at
0x6c5e94..0x6c5ea1, before continuing the unload. The selector invokes
0x6ff020 for matching entries (0x6ff9b6..0x6ff9be). That routine stops the
voice, removes the manager entry, releases the conversation's participants,
and destroys the conversation (0x6ff133..0x6ff16a). This is cancellation,
not a suspended conversation to resume after visiting the area again. It
also includes conversations owned by party members leaving with the player.
Creature destruction and removal also select ambient dialogs involving the
creature (0x69a1aa, 0x6b6e75); these are distinct from merely setting activity.

Setting activity at 0x698cd0 writes +0x2c5 and updates object presentation
and registration. No direct ambient-manager end call was found in that setter.
New conversation requests require that byte at 0x6976b0. This trace establishes
unload cancellation; it does **not** establish a standalone active-byte test in
the original ambient timer loop, nor all indirect effects of deactivation.

OpenThedas retains inactive objects from unloaded areas. It now explicitly
cancels Origins ambient runs and pending ambient presentation lines when an
area unloads. As a defensive adaptation to that retained-object model, it also
requires a living, active owner in the current area before advancing or
promoting a retained run. Each run remembers its starting area, so an owner
travelling with the party cannot carry a locally advancing run across unload.
The same checks apply after node scripts/conditions, which can change activity
or area during one long simulation tick. DA2 retains its previous behavior;
its lifecycle has not been traced here. Ambient selection, duration and the
existing ambient-to-player choice policy are otherwise unchanged.

Regression coverage checks cancellation for inactive and remote owners,
suppression of subsequent plot actions/player choices, and area exit/return
with an active travelling owner. The recorded Flemeth replay is checked
separately against the unchanged master save and travel recipe.
