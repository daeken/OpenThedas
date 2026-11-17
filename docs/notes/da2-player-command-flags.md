# DA2 player-issued combat commands

Read-only tracing of the retained analysis image; no VM or original-game run.

The ordinary attack input at `0x47fa90` creates modern command 25 and calls
`0x5d2330` at `0x47fcad`. Direct ability input reaches the same constructor at
`0x4800d9`. This constructor writes the player-issued field at command `+0x10`
to one, keeps static state zero, and calls ordinary admission with the supplied
front value and default add behavior. Both combat input sites pass front zero.
The basic-attack selector boolean at argument index 2 is a separate field.

Admission `0x5cd3b0` rejects invalid/blocked Drive requests first. For a flagged
command, `0x5cd403..0x5cd464` synchronously dispatches event 94 with object zero
equal to the command owner and integer zero equal to the command type. The
installed interface names this event PLAYER_COMMAND_ADDED. It precedes active
command inspection and table-94 lookup, so a script callback can change what
the subsequent admission sees.

At `0x5cd4da..0x5cd4f8`, hard clear becomes soft clear when the active command
is player-issued, except for incoming type 32. That exception is retained as a
numeric native type; its purpose is not inferred. Payload comparison at
`0x60fe40` includes issuing state. The equal-command suppression at
`0x5cd53d..0x5cd554` permits another equal flagged ability/animation to queue.
Ordinary equal script commands retain the existing executor without insertion.

The simulation saves `Command.da2_player_issued` independently from the basic
selector option. Missing historical fields deserialize as false. Script
constructors remain unflagged; copying an existing flagged command retains its
payload, as the native AddCommand wrapper only overwrites static state.
DA2 attack and ability orders now enter admission through this traced path.
Origins keeps its existing player-order behavior. The separate movement input
and the synthetic talk/use approach helpers are outside this combat-input
repair; they are not assumed to use the same native constructor.

Regression controls cover issuing-state payload inequality, equal script and
player commands, animation/ability aliases, the hard-clear downgrade and type-32
exception, synchronous callback arguments and pre-insertion waiting count,
callback queue mutation, saved flags and older records. Task protection and
preparation callback timing are separate follow-up changes. No HP, damage,
equipment or difficulty values change.
