# DA2 projectile flight

Read-only tracing of the user's analysis executable, with shipped M2DA data;
no original-game session was run. Addresses identify that image only.

The table-4 loader at `0x5aa180` reads acceleration (`0xfd480c46`), minimum
speed (`0xdb26d384`), maximum speed (`0xfd919690`), impact persistence
(`0x0796e13b`), and ballistic mode (`0xa2166177`). Initialization at
`0x5c0030` reads initial speed (`0x883650aa`) and gravity (`0x90fc93bb`).

For non-ballistic shots, `0x5ee4a0` updates speed by acceleration times the
supplied update duration, clamps it, aims velocity toward the current target
point, then advances by velocity times duration. Travel is clamped at the
destination; arrival tolerance is 0.01 metres. `0x5aa520` resolves an attachment
if available, otherwise the target object's position, otherwise the stored
point. Common staff projectiles start at 50 m/s with acceleration 10 and bounds
15–100; Arcane Bolt starts at 60. These are resource values, not new constants.

This patch implements that non-ballistic motion using the simulation's existing
time advances after actor movement. It does not infer a fixed native timestep:
the semi-implicit update depends on the duration partition supplied by the
caller. A shot stores its current position, speed, target/last target point,
distance travelled, update time, and one owned impact payload in the save.
`0x5ef840` expires shots after 1000 metres independently of arrival. Impact
persistence/fade is visual; the headless object is removed after its arrival
event has been delivered. Custom event replacement changes the payload without
restarting flight; `0x5e9d40` supplies the projectile as its creator at arrival.
Older fixed-deadline saves and Origins retain their existing path.

The ordinary ability executor now captures its impact event at release, sends
the existing projectile-launched event then, and runs ability_core with that
captured payload on arrival. Later command results cannot rewrite it. Ranged
basic attacks selected through the DA2 ability path use this same mechanism.

Limits: attachment transforms are not available to the headless simulation, so
flight uses object positions. Native alternating/extended collision sweeps
(`0x5ede60`) are not represented by the existing sight-query API and remain
unimplemented. Ballistic rows and missing/invalid rows keep the previous path
(instant ordinary ability impact, fixed-speed script projectile). Native blend
graph, miss trajectory, retained visual projectile and frame parity are not
claimed. This is a flight/delivery correction, not proof of original difficulty.
