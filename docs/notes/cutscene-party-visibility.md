# Origins cutscene party visibility

Static trace, 2026-11-15. Addresses refer to DAOrigins.exe. No runtime capture or
original-game execution was used for this trace.

1. `GameModeCutscene`'s primary vtable is 0xb0e76c (RTTI name confirms the class).
   Its enter method 0x692980 takes the pending cutscene into +0xd0; at 0x692b63 it
   calls 0x7cd0d0. That sets bit 1 of cutscene byte +0x99 and starts playback.
   `GameModeDialog` also calls this playback method at 0x68e5a0 for a line's cutscene.
2. Cutscene update 0x7e3d30 binds each actor at 0x7e3e90 -> 0x7e36b0 before starting
   the scene. The binding call 0x7e30b0 clears the bound object's byte +0x23d at
   0x7e310b. Normal creature update (0x6bd591) consults this byte before updating its
   visibility from the fade system, so a bound actor's visibility is now scene-owned.
3. After all actor resources and bindings are ready, update reaches 0x7e3f54.
   When the playback bit is set and global byte 0xbf9ba8 is enabled, it calls the
   current area's 0x67c980 with argument 1. The global is a tool/editor gate:
   0x405000 sets it to the inverse of the tool state obtained through 0x5c9170.
4. The area helper traverses the object-ID list at area +0x694. For every resolved
   object it asks the party object whether the ID belongs to its moving-party list,
   +0x234 (0x631830 -> 0x8663a0). Party members receive virtual +0x134(0) and byte
   +0x23d=0 (0x67ca9e..0x67caa8). Other objects receive virtual +0x134(1).
5. For a creature the primary vtable +0x134 is 0x6a0ea0; it calls 0x69b640 to set
   visibility byte +0x23c. Visibility query +0x138 is 0x6a0e70, which first reads
   that byte at 0x6951c0. Updating visibility at 0x69a7a0 sends the result to the
   creature's render models, including its attached objects, through render virtual
   +0x9c/+0xa0. This is a rendering visibility switch, not object destruction or a
   teleport.
6. Immediately after hiding the party, update iterates the cutscene's bound records
   (cutscene +0xec, record size 0x4c) at 0x7e4020..0x7e40a7 and explicitly calls each
   resolved bound object's virtual +0x134(1) at 0x7e409f. Therefore a bound party
   member is visible; an unbound party member stays hidden. Actor visibility tracks
   may subsequently hide a bound actor.
7. Ending a cutscene restores each bound object's automatic visibility byte to 1
   (0x7e1336), then calls area helper 0x67cad0 at 0x7e1784. That helper calls virtual
   +0x134(1) and sets byte +0x23d=1 for every ID in the area list, restoring normal
   visibility updates for the unbound party too.

Conclusion: the full-cutscene rule currently expressed by
`Sim::cutscene_hidden_party` is confirmed for normal Origins gameplay. It is the
uncast moving party, not all uncast area creatures, that is hidden. Actor binding
and visibility tracks determine the exceptions. For `bdccs_intro.cut`, the hero at
the slums `start` waypoint is unbound, so this rendering rule explains why his body
does not show in the brazier; no displacement from the waypoint is required.

Limits: the global tool/editor override is not represented in the simulation;
Dragon Age II has not been traced here. OpenThedas' separate inline conversation
presentation does not currently ask `cutscene_hidden_party` about its staged cast;
this trace does not claim that separate path is complete. Ordinary full-scene
visibility needs no behavioral change.
