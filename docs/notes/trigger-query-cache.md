# IsInTrigger reads cached creature membership (SIM-009)

Static DAOrigins.exe trace, 2026-11-15. Image SHA-256:
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Only original prose is recorded here. No DA2 query behavior is inferred.

The script handler **0x66da90** reads its two object-ID arguments through
**0xa2f6d0**. It initializes its result to false and resolves the first ID as a
creature through **0x5d58c0** (**0x66dac5–0x66dad2**). If the creature exists, the
handler searches its membership list at **creature+0xba8** for the second ID
through **0x8663a0** (**0x66dadb–0x66daea**). The resulting byte becomes the script
integer result (**0x66daef–0x66daf9**).

The handler does not resolve the trigger, compare areas, read positions or authored
geometry, or call the polygon membership virtual method. A newly changed position
therefore does not itself change this query's answer before the membership list
is refreshed. The helper **0x6ae2b0** independently searches the same list through
**0x8663a0**.

OpenThedas already stores creature membership in `World::in_triggers` and refreshes
it during `Sim::update_triggers`. The Origins script query can use that existing
list rather than testing geometry at query time. DA2's current geometric query is
outside this trace. The change requires no new saved field.

The synthetic regression should check false before a refresh, true after entering,
the retained cached answer after a direct position edit, false after the next
refresh, rejection of a non-creature first argument, and unchanged DA2 geometry
queries. Querying must not queue membership events as a side effect.

This establishes the handler's source of truth only. Native cache refresh timing,
polygon edge cases, native list order, event phase order and ENTER guards remain
separate contracts.
