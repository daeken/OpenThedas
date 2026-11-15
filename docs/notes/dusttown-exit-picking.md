# Dust Town exit picking

The Commons exit in `bdc200ar_slums` is `bdc200ip_to_commons`, appearance 159
(`Area Transition, Invisible, Wide`). Its state controller is
`StateCnt_AreaTransition`, not `StateCnt_Door`. The shipped pathing grid is
walkable at the marker, and the saved reproduction has no destruction-time
navigation patches. Neither #870 nor #884 changed the pointer picker.

## Native use path and scope

Read from the local DAOrigins.exe disassembly, without running the original:

- Script handler `COMMAND_COMMANDUSEOBJECT` at `0x66f770` reads the target
  object and action, stores them in the command, and constructs command 31
  (`0x66f825`–`0x66f852`). A use is an object/action request, not a move to the
  ground behind the selected object.
- The final use executor at `0x7a78f0` resolves the target and validates it.
  `0x7a7a11`–`0x7a7a43` compares the requested action with the placeable's
  available actions. Its ordinary USE branch (`0x7a7af8`–`0x7a7bc5`) builds
  event 14, computes the user's side from the positions and the placeable's
  forward vector, and dispatches it to the target. Our existing `use_event`
  and `CommandUseObject` execution already supply that script entry point.
- The exact native approach distance, physics path follower, and pointer-query
  selection policy remain untraced. This change does not replace those with a
  new doorway rule. In our repro the transition is excluded by `is_door`
  because it is `StateCnt_AreaTransition`; it cannot contribute cells to
  `closed_doors`, `in_closed_doorway`, or the door cut-off in `route_to`.

The approach endpoint and off-grid recovery were suspects, not established
causes. A direct use of the transition and a pointer click are separate checks:
the former can pass even while the latter cannot select this invisible object.
The saved marker's destination locals name `bdc100ar_commons` and
`bdc100wp_from_slums`; the shipped `placeable_core` reads the destination locals
and ultimately calls `DoAreaTransition`.

The appearance's `plc_invisowide_01_0.mmh` has no mesh pieces. Its PHY does have
a box in the placeables group, beneath the model's root node. Previously the
app built both pick triangles and fallback bounds from rendered draws; the
marker consequently had neither. A click through the arch could only hit
ground farther away, or nothing. Walking against the area's boundary did not
activate the placeable's normal use handler.

For an interactive placeable without rendered draws, the picker now uses its
primitive PHY shapes in the placeables group, with the model-node, shape and
object transforms. Visible placeables keep their rendered-triangle picks.
These fallback shapes are used for selection only, and do not become movement
or camera collision. Using the marker then follows the ordinary approach/use
command and the shipped `placeable_core` area transition.

This diagnosis concerns picking, rather than route planning, off-grid movement
recovery or per-frame movement. It does not establish a cause for the separate
jerky-movement report. Grid edge projection and routing remain provisional;
they were inspected but are unchanged by this fix. Native pointer selection
itself has not been fully traced; using the marker's shipped physics volume is
an asset-backed fallback, rather than a claim of complete native picker parity.

The asset-dependent regression can be run headlessly with a DAO install:

```sh
THEDAS_INSTALL=/path/to/game cargo test -p thedas-app \
  dust_towns_invisible_exit_has_a_pick_volume_without_rendered_meshes -- --ignored
```

The hidden-window MCP candidate repro uses the saved Dwarf Commoner state with
the hero at `[347, 278, 9.3]`, facing the exit. In a 1280×720 physical window,
`hover 640,290; click 640,290; run 200` selects the `area_transition` cursor,
moves the hero to `[345.899, 278.220, 9.344]`, and queues
`CommandMoveToLocation` followed by `CommandUseObject`. After running on and
waiting for loading to finish, the hero is in `bdc100ar_commons` at
`[301.065, 345.524, 12.724]`, with an empty command queue. A further ground
click moves him 1.94 m to `[302.483, 346.841, 12.688]`. The run uses original
shaders and `THEDAS_HIDDEN_WINDOW=1`; it never runs the original game.

No game assets, native bytes or disassembly are included here.
