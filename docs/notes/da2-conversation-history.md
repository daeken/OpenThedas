# DA2 conversation eligibility and history

Read-only tracing of the user's DA2 analysis executable, without running the
original game. Addresses identify that image only.

Graph collector `0x59c8e0` tests each candidate line through virtual method
`+0x18` at `0x59cb5d`. Both ConversationLineAmbient and
ConversationLineInteractive use `0x552d30` there. This eligibility method:

1. Reads field 30200 through `0x545ec0`. If bit 4 is set, it rejects a line whose
   local line ID is already in the owning conversation's history (`+0x154`).
2. If bit 5 is set, it calls `0x54e530`, which reads the TLK ID in field 30201,
   and rejects it if present in the global history (`+0x3ec` on `0x5a6c90`'s
   result).
3. Evaluates the authored plot condition at line `+0xa8` through `0x54e640`.

Line execution at `0x57ca90` calls `0x56f540`. The latter records both the local
line ID and global TLK ID, regardless of whether that line's own flags request
history filtering. The group consumer at `0x56f620` records consumed lines too.

The adapter now retains the flag word on actual CNV lines and uses both history
checks before their plot condition. Local history is saved with the active
conversation; global TLK history is saved in the existing dialog-history set.
Wheel paraphrases are virtual adapter nodes: they test the target line's
eligibility but are not themselves recorded as spoken lines. Otherwise a choice
could either repeat forever or hide its own target before that target speaks.
The diagnostic conversation_conditions path now loads CNV through the same
reader as normal interaction.

This does not justify skipping ambient-class or bit-64 lines. The earlier
`0x545f10` / `0x561be0` path controls performance behavior, not this graph
eligibility check. Ambient/interactive switching, remaining performance flags,
wheel icon rendering and complete cinematic behavior remain unfinished.

Older saves have no retroactive global spoken-line history. An already heard
line may therefore repeat once after upgrading; normal playback records it for
subsequent interactions. No inferred quest flags or synthetic history are added.
