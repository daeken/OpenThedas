# Origins deprecated AddToParty / RemoveFromParty

Own prose from the read-only GOG 1.05 executable trace. The shipped declaration
comments out these signatures, but compiled scripts still call them.

Registration `0x676eb0` maps 396 (`AddToParty`) to `0x65e560`; registration
`0x676ec6` maps 397 (`RemoveFromParty`) to `0x65e5c0`. AddToParty reads two
objects, but never uses the second, legacy player argument. Both handlers look
up the global player's party pointer and do nothing when it is missing. They
call `0x63b9a0` with the first object and literal state 1 or 0 respectively.
This is the same state dispatcher used by SetFollowerState (706).

The simulator adds missing inferred signatures and routes these Origins calls
through its existing follower-state implementation. It checks the creature and
player-party representation before doing so. AddToParty always selects state 1;
its ignored player argument cannot select another party. RemoveFromParty
selects state 0. They do not raise party-picker membership events. Existing
follower-state approximations (active party versus the pool, grouping and
placement) remain shared rather than introducing a second membership model.

Three tests compare the complete serialized worlds against explicit
SetFollowerState calls for repeated addition/removal, vary the unused player,
verify missing party/invalid kinds, and retain unimplemented DA2 dispatch.
