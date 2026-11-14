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
Origins' three-argument command is unchanged. This does not implement every
native conversation-manager admission rule or cinematic behavior.

In Act 1, Lirene's last line sets Tranquility flag 2. Its shipped script falls
through to a request to talk to Anders. Earlier builds allowed that request
while Anders was inactive in another area, because the calling plot script ran
on the active hero. The hero then incorrectly owned Anders's conversation.
Rejecting the inactive NPC preserves the direction to visit his Darktown
clinic instead of advancing that dialogue remotely.
