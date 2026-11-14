# DialogIsAmbient script context (SCR-086)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

The getter 0x65dc00 reads the script-context byte at game +0x214. Two setters
have different sources:

- Condition evaluation reads node field 12207 (0x2faf) at 0x7f049a..0x7f04bf and
  writes its boolean value to game +0x214 at 0x7f04ca. This is the line's flag.
- Action evaluation 0x7eedc0 copies conversation +0x35 to game +0x214 at 0x7eee00
  before running the action script at 0x7eee37. The per-node action adapter
  0x7f0560 gathers the action parameters, then calls that helper at 0x7f059b.
  Conversation-level actions use the same helper at 0x7f0fe8.

Conversation construction 0x7f1850 initializes +0x35 to false (0x7f18a9). When a
line is ambient, the line-advance path tests the flag via 0x7ef660 and enters
ambient mode at 0x7f2033 before running the node action at 0x7f2170. Thus a newly
ambient line acts in ambient context. Once in that mode, actions read the mode
even when an intervening unspoken reply does not carry the line flag. The reply
action path at 0x7f1996 calls the same node-action adapter.

OpenThedas already held the getter context, the line flag, and the conversation's
ambient_run state. Conditions retain their line-based context. Actions now use
ambient_run or the newly ambient line, including unspoken replies and the other
node-action call sites. DA2 keeps its prior line-only context pending its own trace.
The regression distinguishes conditions from actions on the same unflagged reply
and queries the actual engine getter in ambient and ordinary conversations.

Conversation selection, the existing rules for classifying NPC conversations,
line timing, and the ambient-to-interactive transition are unchanged; this trace
does not establish those separate policies. No UI, rendering, table, or save
format changes are involved.
