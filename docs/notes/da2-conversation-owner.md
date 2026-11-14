# DA2 BeginConversation ownership

Read-only tracing of the user's DA2 analysis executable, without running the
original game. Addresses identify that image only.

Command 440 is registered at `0x52754b` to wrapper `0x512660`. The wrapper pops
the target object and optional conversation resource, then calls `0x504c60`.
That function resolves the target through `0x5a70c0` and invokes the target's
creature component at `0x5b15f0`. It does not use the script's caller as owner.

The component uses the supplied resource, or its own default at +0x2ec when
empty. Its checks at `0x5b1650..0x5b166e` require its active bit and a positive
value from its virtual +0x124 query. The conversation request at `0x5b1670`
passes that target creature's ID. The shipped script.ldf independently names
oTarget as the object that owns the conversation.

The simulation now uses the target as owner, its default conversation when no
resource is supplied, and the hero as player. The existing active/living-owner
checks therefore apply to the intended NPC rather than the script caller.
Origins' three-argument command is unchanged.

The manager path `0x5a3300 -> 0x5a1350` submits an interactive request through
the mode controller (`0x5a1593..0x5a15e5`, mode 4); it does not run it as a
replacement inside the current line action. The DA2 walker now retains that
ownership while temporarily taking its conversation off Sim to advance a reply
or start a line. A script's new interactive request waits in the existing
pending list, and the old conversation's remaining actions still run. Origins
retains its previous action behavior. Complete ambient-class presentation and
additional native admission rules remain outside this change.

In Act 1, Lirene's last line sets Tranquility flag 2. Its shipped script falls
through to a request to talk to Anders. Earlier builds allowed that request
while Anders was inactive in another area, because the calling plot script ran
on the active hero. The hero then incorrectly owned Anders's conversation.
Rejecting the inactive NPC preserves the direction to visit his Darktown
clinic instead of advancing that dialogue remotely.
