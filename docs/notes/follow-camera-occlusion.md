# Origins follow-camera limits, collision and cutaway

Addresses are virtual addresses in the GOG 1.05a `DAOrigins.exe` (image base
`0x400000`). The reference executable SHA-256 is
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
The observations are from static analysis and OpenThedas captures.

## Follow-camera limits and area data

`CameraControllerExplore` is constructed once at `0x641918`. Its update
(`0x7714b0`, vtable `0xb1d954` slot `0x1c`) writes far arm **20 m** to
controller `+0x0c` and near arm **3 m** to `+0x1c` every frame
(`0x7714d0–0x7714df`, constants `0xc0e00c` and `0xc0e010`). The base
constructor `0x7eea24–0x7eea2f` sets the request bounds `+0x64` and
`+0x68` to **−1** and **+1**. Player input is clamped at `0x7ee680`.
`SetZoomLevel` (`0x666790`) maps its script argument from [0, 1] to the same
range and base update `0x7eec23–0x7eecbf` steps the request. The requested
fraction is `(zoom + 1) / 2` (`0x7ee770`), fed to the game camera by
`0x6c2bf0`. Game-camera `+0x24` is the effective fraction, which
`0x6c2dbf–0x6c2e9c` eases to the request. The follow arm interpolates linearly
between the two distances using that fraction (`0x76efa0 → 0x6c30d0 →
0x6c2360`).

| Mode | Limits and preset |
| --- | --- |
| Explore, effective zoom ≥ 0.5 | Requested arm 3–7.25 m. Polar angle (from +Z) clamps to **30–150°** at `0x76f1d7–0x76f1f8 → 0x6c2d20–0x6c2d63`, equivalent to pitch **−60 to +60°** in OpenThedas. Fresh activation uses 85° polar at close zoom (`0x76f550`, `0x76f9f8`). FOV is 60° at `0x76f14e`. Collision can shorten the arm to 0.25 m. |
| Crossing to tactical | The controller tracks the side of 0.5 at `+0x90` and requests the far endpoint on an outward crossing (`0x76fbf0–0x76fc53`). In flight, `0x76f221–0x76f26c` blends the recorded entry pitch (`+0x98`, `0x76fc41–0x76fc4a`) toward 45° polar, and `0x76f163–0x76f19f` blends FOV toward 42.5°. |
| Fully tactical | **20 m** arm, **45° polar** (pitch −45°), **42.5° FOV**, look offset **1.8 m** (`0x76f33d–0x76f365`, `0x76f18a–0x76f19f`, `0x76f6c6`). Both layout types reach this endpoint. Indoors skip the clearance gate and use collision mode 0. Outdoors use the entry/retry and eye-overlap checks below. The registered `ToggleTacticalCameraMessage` handler at `0x722ae0` is a success stub in this build. |

The traced follow orbit bounds the eye height through the arm and pitch.
At full tactical zoom, the nominal eye is **15.942 m above the framed ground target**
(`1.8 + 20 cos 45°`). Ground Z is calculated separately for tactical framing:
`0x770e80` collects party creatures, `0x770068–0x7700ef` reads their Z
coordinates at creature `+0x128`, and `0x7700f3–0x770167` takes the greater
of mean Z and highest Z minus `0.25 × 20 × cos 45°` (3.536 m). This keeps
the highest member within the nominal vertical framing margin when party
members stand at very different elevations. Controller `+0xe8` eases to that
height (0.33 s scale at `0x77016b–0x77021a`). During the tactical transition
the ground pivot blends to it at `0x7712fd–0x77133e`; the look offset is
applied separately by `0x76f640`. The original also has tactical XY pan and
recentering logic at `0x7710eb–0x771237`. A proposed pan has to pass the
party framing test `0x770230`, followed by the exterior eye clearance test
`0x76f420`, before committing the offsets (`0x770711–0x77073b`). These
captures reconstruct an unpanned follow target and the reported yaw.

The six report ARE roots identify the environment through `Layout`.
Their top-level fields contain no distance, pitch, height-cap, or tactical
availability setting. The ARL loader writes `ARL_LAYOUT_SUN`
**3122** to the interior/exterior classification at layout `+0x7c`
(`0x5d168e`, `0x5d1a7b`); the controller copies the interior predicate
(`0x67b860`) to `+0x9d` on resume (`0x76fd78–0x76fd7f`). It reads
`ARL_CUTAWAY_HEIGHT` **3129** via layout `+0x78` (`0x5d15e4`,
`0x76fd30`), a render threshold described below.

| Report | ARE `Layout` | ARL 3122 | ARL 3129 | Cutaway threshold |
| --- | --- | --- | --- | --- |
| #1069 Palace | `orz303d` | 0 | 1 | 4 m |
| #1082 Castle | `hrt201d` | 0 | 2 | 5.5 m |
| #1066 Dust Town | `orz200d` | 1 | 2 | 5.5 m |
| #1070 Alienage | `den200d` | 1 | 2 | 5.5 m |
| #1071 City Elf house | `Den201d` | 0 | 1 | 4 m |
| #1137 Forest | `brc300d` | 1 | 2 | 5.5 m |

The stock `2da.erf` has `camera_selection.gda`, with `ID`, `NAME`, and
`VALUE` columns (hashes `0x66fbc936`, `0xb22d0123`, `0x3fc13de1`).
Its two rows are Close Up (0.95) and Wide (0.05). It has no entry in
`m2da_base.gda`, and there is no read of its values in the traced follow
controller. The `conversation_dof_*` tables
feed the conversation path (resource initializers `0xad0fb0` and
`0xad0fd0`), and `portraits.gda` provides a `CameraDistance` column for
portrait framing. The table read affecting this follow path is the actor's
`apr_base.gda` `HEIGHT` or `HEIGHT_F` (`0x748a40`, `0x6a1a30`,
`0x7edc40`), which drives the close look offset. The 20 m and 3 m constants
are assigned unconditionally by the follow update.

On a wheel zoom-out from exploration, OpenThedas previously allowed stops
at intermediate distances beyond 7.25 m. Origins requests the far endpoint
when that input crosses the split. In the palace
reproduction, the pre-PR build used an approximately 15.6 m arm at
−45°; in the castle it used 13.2 m at −23.67°. PR #1144 already selected the
20 m / −45° endpoint for both reports. The corrected branch uses that
endpoint with the party-based target Z. Both layouts use the same static
bounds. OpenThedas still
approximates the effective-zoom easing and does not preserve a separate
captured entry pitch.

## Camera controller and physics

| Condition | Original behaviour and evidence |
| --- | --- |
| Close zoom, at or above 0.5 on the internal -1 to +1 scale | `0x76f0b0` selects collision mode 1. The update dispatch at `0x6c4df9` calls `0x6c3710`. The 0.25 m capsule sweep at `0x6490b0` uses the target with its look offset and the desired eye, with group mask `0x1024` (argument at `0x6c3758`). |
| Pull-in | `0x6c32c0` limits the pull to the arm length minus 0.25 m (`0x6c336c`). It can approach at 15 m/s (`0x6c34f4`) while the candidate eye-to-target segment is clear (`0x6c3574–0x6c357f`, same mask). A blocked segment makes the correction immediate. |
| Ease-out | `0x6c35a0` caps speed at 15 m/s and accelerates at 10 m/s². Mode 1 checks the outward step with another 0.25 m sweep at `0x6c36ac`. Mode 2 accelerates 30 times faster (`0x6c36cf–0x6c36db`). |
| Between 0.5 and the far limit | `0x76f102` selects mode 2, with no inward sweep. The controller can pass through this band during a zoom transition. |
| Tactical entry | Outdoors, `0x76faa0–0x76fb30` can hold the requested zoom on the close side. `0x76f380 → 0x6c3270 → 0x6c29d0` projects the current yaw and pitch from the pivot using game-camera `+0x4c`, the **20 m far arm** written at `0x6c3118`. `0x6c28b0` tests that point with a 0.25 m sphere and `0x1024`. This is independent of current interpolated distance and collision pull-in. The interior predicate bypasses this test. |
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

Current room visibility is distinct from its render enable bit. The pass
appends each currently visible ID to the area instance's seen-room list
(`+0x460`, `0x6888a1–0x6888e2`). `0x688944–0x688990` dispatches three
states. Current rooms receive state 1 and render enable true. Previously
seen rooms receive state 2 **and render enable true**. Only rooms outside
both sets receive state 0 and false. Room vtable slot `0x58`
(`0x5d2210 → 0x5d2150`) applies the modulation, targeting 1.0 for current
rooms and 0.7 (`0xb58750`) for seen rooms. Slot `0x7c` (`0x5d2360`)
propagates render enable to room models. `0x548a30–0x548a4f` sets the
node's `0x200` disable bit when hidden and calls `0x53ad40` to update
Umbra. `0x7e3f76 → 0x67c980` enables rooms for a cutscene and
`0x7e1784 → 0x67cad0` restores filtering.

The first PR revision's hard mask approximated just the current set, with
no seen-room history or modulation. It culled adjoining rooms that could
remain drawn in Origins. The branch drops that mask and the additional RML
parsing. The occupied palace room would still draw its unmarked black
meshes under the original room rule.

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

The existing stock screenshot `city_elf_house_first_control.png` shows a
black foreground surface over the controlled character with the selection
circle still visible at the tactical tutorial start. It establishes that the
stock game can show this kind of occlusion at an allowed zoom. The reference
is at the origin's initial player position. The #1071 reproduction is at the
exit.

## Revised captures

The six tactical replays were taken with the clamped 20 m arm and −45°
pitch from the supplied saves or reconstructed report positions. The
captures are unpanned. A stock palace frame at maximum zoom was unavailable.

| Report | Revised result |
| --- | --- |
| #1069 Palace | Eye `(-103.781, 6.858, 14.942)`. The two party members are at Z −1, so the height correction leaves the eye in the same place as the first PR revision. Adjacent floors and walls return. The black surface in room 933 still hides both actors. |
| #1082 Castle | Eye `(-52.247, 23.045, 18.246)`. Averaging Aedan's Z of 1.669 and the dog's Z of 2.938 raises the eye by 0.635 m at the same 20 m arm and yaw. The black corridor geometry still hides Aedan. |
| #1066 Dust Town | The party elevation raises the eye by about 0.018 m. The near roof is still in front of both actors. |
| #1070 Alienage | The house still blocks the hero at the arrival waypoint. At the comparison threshold, the revised frame matches the first PR revision. |
| #1071 City Elf house | The door still appears isolated against the black shell. At the comparison threshold, the revised frame matches the first PR revision. |
| #1137 Forest | The party is visible at the tactical bend with collision-bearing tree cutaway. The close report's 11.5 m intermediate arm at pitch −18.33° hides both actors on the pre-PR controller. The reconstructed shot from the same position, yaw and pitch at the 7.25 m exploration limit shows both actors, with the eye moving about 4 m closer to the pivot. |

The remaining palace, castle, Dust Town, Alienage and house occlusions
occur within the traced zoom and pitch limits. All six reports remain
open. The forest captures verify two poses from its report.
