# Runtime command types absent from script.ldf (SCR-057)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

Each constructor below calls the same type setter, 0x742090, which writes its
argument to the byte at command +0x2. GetCommandType (0x66eba0) returns exactly
that byte, read at 0x66ebd4. These are observable script values, not arbitrary
internal labels.

| Constructor | Runtime type | Assignment |
| --- | ---: | --- |
| CommandMoveAwayFromObject | 21 | 0x66f564, handler 0x66f430 |
| CommandMoveToMultiLocations | 22 | 0x673383, handler 0x673280 |
| CommandSheatheWeapons | 39 | 0x66fbb9, shared handler 0x66fb30 |
| CommandUnsheatheWeapons | 39 | same assignment |
| CommandTurn | 41 | 0x66eabe, handler 0x66e9d0 |

The two weapon commands share one handler. It distinguishes function 158 from
159 at 0x66fb98 and writes a different boolean payload (0x66fbac..0x66fbb4),
then assigns the same type 39. Similarly, move-away is type 21, shared with
move-to-object; constructor identity and payload still distinguish their behavior.
Type names in script.ldf are not a complete enumeration of constructor identities.

OpenThedas now uses these runtime values for Origins. Its executors continue to
dispatch by constructor name, so this change corrects accessor values and command
event payloads without conflating the executors. Payload-layout coverage remains
the separate SCR-056 gap. DA2 retains its previous values pending a DA2 trace.
The regression runs each constructor through the script host and GetCommandType,
including the shared type and the separate DA2 path. No table rows change.
