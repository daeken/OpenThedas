# Origins maximum level (SCR-007)

Static Origins 1.05 trace; no original-game execution. DA2 remains untraced
and retains its previous constant answer.

`GetMaxLevel` at `0x662f00` obtains the party through `0x5d57c0` and reads
its member `+0xc4`. It returns zero when no party exists. This is a CCParty
member, not a module GFF field: the constructor at `0x63a437` installs the
CCParty vtable, and `0x63a4d8` initializes the member to zero.

The party reader computes it at `0x63b765..0x63b801`:

1. Obtain merged M2DA 98 (`properties`) and 164 (`exptable`).
2. Read the float `Max` column (hash `0x61d4debf`) of property row id 19,
   the experience property. If either table or this lookup is unavailable,
   leave the initialized answer zero.
3. Walk the experience table by row position, starting at zero. Read its
   integer `XP` column (hash `0xa0f02e99`). Stop at the first failed read or
   threshold greater than the truncated experience maximum. The conversion
   calls `0x8e34f0`, the same SSE2 CRT helper as `FloatToInt`.
4. Each accepted position replaces the answer. The result is the last
   accepted position, not the row's ID or a search for every affordable row.

The distinction between row IDs and positions follows the accessors:
the properties lookup uses virtual `+0x24`, resolving row ID and column;
the experience lookup uses `+0x48` (`0x4ecd50`), resolving only the column
before forwarding its row position to `+0x38` (`0x4ec420`). M2DA's row count
is the sorted row-vector length (`0x4ecaa8..0x4ecac0`). Its numeric readers
read 32-bit integer/float storage directly (`0x4ebcf0`, `0x4ebd30`), using
zero for an absent cell value after the row/column lookup succeeds.

OpenThedas derives the answer on demand from its simulation's fixed tables,
so no additional saved field is needed. It uses the existing merged tables
and row order without changing precedence, table data or row positions.
Queries leave the saved world and RNG unchanged, including after reload.
The installed Origins tables yield level 25; the previous answer was 20.
