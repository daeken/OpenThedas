# DA2 line-performance actor activation

Karl's first Chantry confrontation reaches Tranquility flag 7 and changes all
five members of team 104000 to hostile. Previously they remained inactive:
the script does not enable them at that handoff. The line performances do.

## Native and authored evidence

The conversation's `cln_and121_karl.crf` contains LINE performances. Their field
31102 identifies the spoken line; CUT field 5200 contains actors with world
bindings in 5216. The confrontation's performances bind `and_templar_1` through
`and_templar_4` and `and121cr_templar_captain`. They are existing area creatures,
not new cutscene-only models.

DA2 constructs the cutscene's object records through 0x5a1840 -> 0x5a0dd0 ->
0x59c030. Taking a world object calls 0x53d020. At 0x53d0b1..0x53d0bb it calls
the object's +0x144 virtual with true. The ordinary activation path,
0x4dbd70 -> 0x5de100, uses that same virtual with true at 0x5de192..0x5de19e.
The take operation also saves placement and suspends ordinary actor processing;
those additional cinematic operations are not all implemented here.

Release goes through 0x59c140 -> 0x585b80. It restores processing and placement
but does not restore the object's earlier inactive state. Activation therefore
survives the performance. Field 5235 is a placement/walkmesh option, not the
activation flag; it must not be used to decide whether the actor participates.

## Repair and limits

DA2 conversation lines now read their own performance bindings, indexed by the
actual line's talk-table id. A line with the no-performance flag (30200 bit 3)
does not take actors. Beginning a performed interactive line enables its bound
world creatures/placeables in the current area, resolving OWNER, PLAYER and
ordinary tags through the existing actor resolver. Missing bindings create no
objects. Other lines' actors, inactive objects elsewhere and Origins are left
alone. Conversation completion does not undo this activation.

This implements activation only. DA2's performance camera, placement and actor
processing suspension still need their own native-backed work; the Origins
staging path is unchanged. In particular, this does not reposition Anders from
his area-load location into the confrontation. Do not claim complete cinematic
or combat parity from this change.

## Installed-resource replay

Starting from the pre-dialogue Chantry checkpoint at t=1314315, the same driver
walks to Karl and follows the same replies. The baseline reaches t=1326515 with
the five templars inactive and no fight. With activation, all five become active,
the authored hostility handoff starts combat, and the driver wipes at t=1364215.
Both have zero script errors. Tranquility completion is not claimed. No plot
forcing, direct activation, developer healing or kill aids are used.

The inherited checkpoint has an inconsistent arrival: the current area is the
Chantry, but the hero still belongs to Hightown Night. The old driver kept its
Hightown-door destination after Anders's conversation naturally transitioned
inside. Its next walk took the existing cross-area fallback back to Hightown;
the later walk to Karl moved the hero inside again without elapsed time.
This remains a matched activation test, not faithful-arrival evidence.

Correcting the private driver to stop on the area change reaches the Chantry
at t=1312315 with hero and current area both 6375. From this checkpoint,
activation alone starts the fight but wipes at t=1414127. Combining this repair
with the separately reviewed stored-combat-target change wins the fight, follows
Karl's final conversation, returns to Darktown and completes Anders's dialogue
at t=1504060. A second run starting back in Hightown reproduces that endpoint.
The party is alive, Anders joins the active followers, and both runs have zero
script errors without developer healing, kill aids, plot forcing or manual
activation. Full cinematic placement/processing parity remains unfinished;
neither omission alone is established as the cause of every earlier wipe.

Synthetic regressions cover the spoken-line/performance mapping, no-performance
flag, bindings without world objects, current-area resolution, persistence on
conversation end/save round-trip and Origins isolation. Native/game bytes and
installed resources remain outside the repository; only our code and prose are
committed.
