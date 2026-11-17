# Origins exploration camera collision and cutaway

Addresses are virtual addresses in the GOG 1.05a `DAOrigins.exe` (image base
`0x400000`). The reference executable SHA-256 is
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
The observations are from static analysis and OpenThedas captures.

## Camera controller and physics

| Condition | Original behaviour and evidence |
| --- | --- |
| Close zoom, at or above 0.5 on the internal -1 to +1 scale | `0x76f0b0` selects collision mode 1. The update dispatch at `0x6c4df9` calls `0x6c3710`. The 0.25 m capsule sweep at `0x6490b0` uses the target with its look offset and the desired eye, with group mask `0x1024` (argument at `0x6c3758`). |
| Pull-in | `0x6c32c0` limits the pull to the arm length minus 0.25 m (`0x6c336c`). It can approach at 15 m/s (`0x6c34f4`) while the candidate eye-to-target segment is clear (`0x6c3574–0x6c357f`, same mask). A blocked segment makes the correction immediate. |
| Ease-out | `0x6c35a0` caps speed at 15 m/s and accelerates at 10 m/s². Mode 1 checks the outward step with another 0.25 m sweep at `0x6c36ac`. Mode 2 accelerates 30 times faster (`0x6c36cf–0x6c36db`). |
| Between 0.5 and the far limit | `0x76f102` selects mode 2, with no inward sweep. The controller can pass through this band during a zoom transition. |
| Tactical entry | `0x76fbf0–0x76fc53` tracks the side of the split in controller `+0x90` and selects the far endpoint on an outward crossing. Outdoors, `0x76faa0–0x76fb30` can hold the requested zoom on the close side. Its clearance predicate `0x76f380` computes the uncorrected camera point (`0x6c3270`) and calls `0x6c28b0` with a 0.25 m sphere and `0x1024`. The interior predicate bypasses this test. |
| Deferred tactical entry | The tactical eye overlap path sets controller `+0x9e`. `0x76fbc0` retries from the exploration update, with the 2.5 m form of the eye test (`0x6c28db`, ten times 0.25 m) and a clear ray to the look point against `0x20` at `0x76f3ee–0x76f404`. Success writes the far zoom limit after that frame's collision mode has been selected (`0x77151e–0x771525`). A rejected first outward input alone leaves the flag clear. |
| At the far limit | Interior layouts select mode 0. Exterior layouts select mode 3 at `0x76f0e9`. `0x6c3180` checks the desired eye with the 0.25 m overlap and sets controller flag `+0x26` via `0x6c2930` on contact. `0x7eeca6 → 0x7edbe1` lets the flag trigger the zoom handler even with zero input. `0x76fab5–0x76fae8` returns to the split and queues tactical entry again. |

The collision stack consumes PhysX shapes. The level placements come from
ARL rooms and RML models, with the model's PHY pose under its MMH hierarchy.
`0x4` is Placeables and `0x20` is StaticGeometry. PHY fields 6067 and 6069
contribute those bits. At `0x5314b5` the SpeedTree actor loader reads collision
primitives (SPT section **12000**, with section **73000** rotations). It scales
each position and primitive by the RML placement scale (**3356**), builds its
XYZ rotation at `0x52e6d0`, and sets the shape mask `0x1001020` at `0x53167b`.
This accounts for SpeedTree hits in the additional `0x1000` group. PHY field
6252 (`Fadeable`) affects the separate `0x2000` static-solid bit at
`0x589dfe–0x58a2af`, outside this camera mask.

OpenThedas reuses the layout and placeable PHY loader with the camera mask
and adds the authored SPT primitives to the camera's collision set.
Stationary overlap and continuous sweeps use the same shape set. The exact
PhysX mesh sidedness, initial overlap conventions and shape contact skin
remain provisional. The existing pan, yaw and pitch interpolation approximates
the original controller, and the tactical gate evaluates that pose. The
camera changes are scoped to Origins layouts with readable ARL data.

## Mesh cutaway, culling and foliage

The controller compares the **placed eye** against the target's feet at
`0x76f6e0`. The height must exceed the layout threshold. ARL **3122**
(`ARL_LAYOUT_SUN`) determines exterior versus interior at `0x5d168e` /
`0x5d1a7b`. ARL **3129** (`ARL_CUTAWAY_HEIGHT`) selects 3, 4 or 5.5 m
in interiors. Exteriors use 5.5 m (`0x76fd30`). `0x76f5a0` invokes
`0x640250` for a 0.2 s fade, and `0x641f90` steps the cosine interpolation.
Activation requests an immediate update at `0x771019`.

MMH `mshh` field **6193** (`CutAway`, name at `0xafc85c`) is read at
`0x595025–0x595069` and copied to mesh `+0x1c9` at `0x55b968`.
`0x4670c0` passes the scene value to marked nodes through `0x466f40`.
`0x466930` puts partly faded meshes in the `occluder` submission group
and disables them at zero through the `0x100` attribute. Emitters of a
model with a marked mesh inherit the value via owner flag `0x8`. At
`0x6403f0`, dialog and cutscene entry force the value to 1, then restore it
on exit. This path is already implemented for architecture in OpenThedas.

The same routine stores the value globally at `0x4670c8` (`0xbf73fc`).
The SpeedTree loader sets model flag `0x40000000` at `0x533f0e` when the
tree has authored collision objects (SPT collision section 12000).
`0x52cc84–0x52ccc2` reads the global cutaway for these models and floors it at
0.12 before assigning `g_fAlphaMultiplier`. `0x52cd27` switches the branch
material state during the fade. Fronds and leaves multiply alpha and retain
their existing LOD alpha test. The original `FadingSpeedTrees` submission group
draws faded trees before the general transparent geometry. This is a scene-wide
tactical cutaway for the marked trees. The eye-proximity fade at `0x6c4130`
queries a 0.25 m sphere in the creature group `0x2` and is independent of it.
A close shot below the threshold can still have opaque branches.

## Room visibility and black materials

The loader stores interior room records in the array at layout `+0x424`
(`0x5d1a9b–0x5d1ab5`). The room count read by `0x67b4b0` is at `+0x428`.
Exterior RMLs populate the grid and sector containers at `+0x448` and
`+0x45c` (`0x5d1899`, `0x5d18c4`) and bypass this room array. The portal
visibility step applies to interiors.

Area update `0x68af71–0x68afd0` obtains the party's occupied room IDs through
`0x635fd0`. Creature `+0x150` is assigned by `0x695d10`. The room lookup
`0x695d70 → 0x67b3a0 → 0x5d1e00` uses a downward StaticGeometry ray starting
at the actor's appearance height (`0x6a1a30`, `apr_base.gda` `HEIGHT` or
`HEIGHT_F`) plus 2 m. The ray result is matched to its owning room ID.
The camera's own room is irrelevant to the selection.

`0x6886f0` enables the occupied rooms and the adjacent rooms that pass
`0x687ec0`. RML **3047** provides adjacency at room `+0xb0` (loader
`0x5d3b0a`). RML **3099** provides the portal visibility candidates at
`+0xd0` (loader `0x5d3bbe`). A neighbour with an empty 3099 list passes the
direct check. Otherwise `0x686a70` follows the portal candidates, constrained
by the source room's adjacency. `0x685100` looks up doors keyed by room pair
(ARE placeable fields **RoomID1** and **RoomID2**). `0x74bcc0` checks whether
they block; a pair with doors all closed prevents traversal. The same
`0x687ec0` predicate is also used for creature perception.

The room render dispatch `0x688944–0x688990 → 0x5d2360` toggles bit 1 on
room models. `0x548a30–0x548a4f` sets the node's `0x200` disable bit when
the room is hidden, then calls `0x53ad40` to update Umbra. `0x7e3f76 →
0x67c980` enables all rooms for a cutscene and `0x7e1784 → 0x67cad0`
restores room filtering. OpenThedas implements the visibility step. The
original also adjusts the displayed rooms' material modulation by room
state at `0x5d2150` / `0x546970`.

Normal visibility reaches Umbra at `0x466a20` and `0x467095`. Geometry is
registered with OBB or sphere models at `0x466efb`, `0x467867` and
`0x467a4b`. OpenThedas still approximates hardware occlusion with a view
frustum. The opaque `Black.mat` pass uses `DAMshOpqe1S.sta`, `CULL_CW`,
depth write, and the authored black shader. The submission queue and cutaway
tests supply no special rule to suppress a front-facing black surface in a
visible room.

## Reproduction evidence

| Report | Geometry and traced result |
| --- | --- |
| #1069, palace corridor | From the supplied save, the rays toward both actors cross `dni_livgroom_hals01_0` / `MshBlack_DNI_livgroom_hals01_0` and `blk_01_0` / `MshBlack_BLK_01_0` in `orz303d_933`. The first opaque intersections are 13–14 m from the recorded eye. Both draw `blk_master.mao` as `Black,Opaque,Static1`, face the eye, cast runtime shadows and have CutAway clear. Downward PHY rays at the actors identify room 933 itself. That room stays enabled by the portal rule and has no tactical pull-in. |
| #1070, Alienage arrival | The reported waypoint is `bec100wp_from_home`. The ray from the reported eye crosses `fhe_slum02_0` / `MshMain_FHE_slum02_1` in `den200d_0_0` at 1.83 m, with `fhe_master.mao`, `Opaque,Static1`, runtime shadow enabled and CutAway clear. At a full tactical eye, the PHY overlap is clear, and the same house remains in the sightline about 10 m from the eye. The marked `prp_fcicanopy03` behind it is eligible for cutaway. |
| #1071, City Elf house | The door remains at its authored placeable position. `fhi_ceiling_0` is a real CutAway mesh, while `blk_01_0` and the black surface of `fhi_wall_cornerin_0` face the eye and have CutAway clear. They remain with the ceiling removed. |
| #1066, Dust Town arrival | `DSE_Home02_0` / `MshMain_DSE_Home02_2` in exterior sector `orz200d_6_3` crosses both sightlines within 0.6 m of the reported eye. It draws `dse_master.mao`, `Opaque,Static1`, with CutAway clear and shadow casting enabled. The reported eye and that roof have different render and PHY surfaces. |
| #1082, Castle Cousland courtyard | At the reported eye, the leader sightline crosses `FCA_Corr01_0` / `MshFade_FCA_Corr01_1` then its unmarked `MshBlack_FCA_Corr01_1` in room 59, followed by the unmarked `MshBlack_FCA_Corr08_1` in room 65. The black pieces use `blk_master.mao`, `Black,Opaque,Static1`, and face the eye. The leader floor probe returns room 65. |
| #1137, Dalish forest | The tactical sightlines cross `brc300d_tre_f_benta` and `brc300d_tre_f_mossyan2` fronds. Both SPTs have collision objects and receive cutaway. `brc300d_bsh_f_fernbig` has no collision objects. The close view crosses `brc300d_tre_f_condnssm` fronds and is below the 5.5 m cutaway threshold. |

An optional close-camera foliage fade keyed to party sightlines could improve
the close forest view. Its behaviour would be an engine extension. The traced
implementation follows the original's height threshold and SPT flag.
