# Tutorials during the Dwarf Commoner intro

The Origins tutorial queue at `0x693f10` checks duplicate entries at
`0x694032`. Its requested pause passes the game-mode check at `0x6940cd`;
only explore and combat (0 and 1) reach the pause at `0x6940ec`.
`SetTrainingMode` has the corresponding check at `0x692f81`.

Our cutscene clock is separate from `world.game_mode`: the Dwarf Commoner
opening plays `bdccs_intro.cut` while the latter is still explore. Starting
training mode 1 through the hidden client's MCP control during that opening
reproduces a frozen camera clock: an additional seven seconds of frame input
leaves the cut on the same pose. Origins now
holds both tutorial presentations and their effective pause while a cut is
playing or queued. The pending queue, lesson step and requested pause remain
intact, and become effective after the cinematic handoff. Saving restrictions
remain intact. DA2 keeps its existing behavior.

Synthetic tests cover queued and active cuts, pause release, preservation of
pending lessons, the controller tutorial-list marker, and the DA2 boundary.

## Camera audit

The native active-camera update at `0x7dd6b8` reads the camera actor's world
position (`0x7cdd30`), rotation (`0x7cdea0`) and FOV each tick. Its move-spline
update (`0x7d32f0`) rotates the three sampled curves by the actor base
quaternion, adds its base position, then writes the resulting pose through
`0x7d0660`. The existing track evaluator and cut-camera override follow this
path; this patch does not replace it with the gameplay camera.

A hidden-window capture of the opening reproduced a red geometry obstruction
around 12.3 seconds while Camera 11 was moving on its track. The existing
native recording `reference/original/triage/dc_intro_beraht` also shows that
surface in frame 460, then clears it by frame 470. The later cuts likewise
follow their authored camera actors. A matched-camera diagnostic disproved
an initial suspicion that removing a placeable fixed it: the earlier clear
capture had simply reached a later point on the track.

The transient red view is therefore present in the native opening too. The
reproduced training pause can hold it indefinitely; allowing the clock to run
lets the camera continue through it. No geometry or camera-track workaround
is applied. No executable bytes, original frames or game assets are included
here.
