# Origins leader and primary control

Own interpretation of the read-only GOG 1.05 executable trace. DA2 dispatch is
unchanged. The two Origins selectors are separate; selecting a character does
not implicitly replace the party leader.

* Native 625 (`0x661320`) resolves the party manager and calls `0x636180`.
  That method requires a resolved creature, writes the party's leader attribute
  (`+0x1e8`), and moves the creature to the front of the party member vector
  (`+0x234`) if it is already in that vector. A nonmember can become leader
  without being added. Repeated assignments retain the remaining member order.
  `GetPartyLeader`, native 624 at `0x66ac20`, reads that attribute.
* Native 815 (`0x661050`) resolves the creature and first calls `0x6a5f40(true)`
  to set its controllable byte (`+0x7f9`). It then calls virtual `+0x1d0`, which
  is `0x6a3660` on CCCreature. This tests signed integer current health, not
  the creature's saved death flag or its active flag. Missing health, fractional
  positive health below one, and SSE2 integer-indefinite conversions fail.
* Eligible primary selection calls `0x6368f0` with arguments `(id, false, true)`.
  It replaces the primary selection list; it does not write the leader
  attribute. There is no active-party membership test in the native path,
  despite the script declaration's comment. A valid creature outside the party
  can be selected. Player selection subsequently replaces that selection.

`World.party_leader` and `party_order` retain the explicit leader and reordered
enumeration without changing the hero/follower membership registry. Older JSON
worlds default to the hero and the existing order. The regular selected member
still uses `World.controlled`; `script_controlled` records only a native-selected
nonmember so normal member removal retains its existing hero fallback. Player
selection clears that override. The native also restores the creature's typed
controllable state before testing health.

The original updates selection highlights, clears the previous primary's input
motion, refreshes party UI and formations, and selects audio/camera cues. Those
presentation-specific side effects are not recreated here. The simulator's
existing controlled-character behavior drives AI, formation following, player
orders and the camera; changing control does not clear script command queues.

The `engine::tests::party_control_` tests cover independent selectors, successive
leader changes and party enumeration, integer-health boundaries, controllability
restoration, invalid object kinds, nonmember selection, player handoff, JSON
defaults and round trips, and unchanged DA2 dispatch.
