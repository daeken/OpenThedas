# Cutscene animation root coordinates

The Dwarf Commoner opening line uses `39366.cut`. Beraht jumps to the stage's
`beraht` place and plays `mh.dg_wandereast`. Rotating its baked translation by
that place's orientation again sends his movement away from his walking direction.

The DAOrigins.exe animation update at `0x7db400` has two clocks. It sends the
speed/offset-adjusted animation time to the animation player (`0x7ce1b0`), but
sends elapsed track time to the root sampler (`0x7d92e0`, call at `0x7db7c7`).
The exported position and rotation lists therefore use the track clock.

The root sampler reads position field 5408 and rotation field 5409. Its position
conversion (`0x7d3c80`) applies retained animation turns (`0x7d3a90`), then the
sampler adds that vector directly to the actor's cutscene-space position
(`0x7cf640`, setter `0x7d0660`). It does not turn that vector by the actor's stage
orientation. The root rotation is still composed with the actor's orientation
(`0x7cf6f0`, quaternion product `0x4cb640`, setter `0x7cf6b0`). The cutscene's own
placement transforms the resulting pose into the world once.

The actor used to anchor an object-relative cutscene has an extra position
conversion branch in `0x7d3c80` (actor equals cutscene `+0x22c`). That path is not
implemented here. Retained transforms at track boundaries, especially overlapping
root tracks and fields 5411/5413, also need further work. This change corrects the
ordinary staged actor's coordinate frame; it does not claim complete root blending.

Synthetic regression coverage checks a rotated actor moving along a baked stage
axis, a retained turn affecting the following move, and a speed/offset change
leaving the baked root clock alone. No game data or executable bytes are fixtures.
