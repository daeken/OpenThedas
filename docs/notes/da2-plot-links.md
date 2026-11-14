# DA2 linked plot flags

Read-only tracing of the user's DA2 analysis executable; no original game was
run. Addresses identify that image only.

The stored-flag setter at `0x6d9fa0` writes the source bit at
`0x6da289..0x6da2af`, then enumerates its simple flag's field 13306. Accessors
`0x6aea40`, `0x6aea90` and `0x6aeb80` read the count and each target's plot GUID
(13100) and flag (13101). The loop at `0x6da2af..0x6da320` calls the setter
recursively in file order, forwarding the requested value, script-call flag and
owner. It runs for clears and unchanged writes too. The source's script is
invoked later, at `0x6dadb7..0x6dae57`.

The simulation now reads and propagates these links for DA2 stored flags. It
guards active metadata cycles rather than recursing forever on malformed data.
Origins has no new behavior. This does not implement DA2's separate 13320
area-state actions or claim complete native plot/journal presentation parity.

A concrete Act 1 dependency is `dre120pt_main` flag 2 (the introduction to
Bartrand): its link sets `vrc100pt_n_initial` flag 0, which in turn sets that
plot's flag 3. Its composite 256 then admits Varric's Grey Warden conversation.
Without those links, the introduction completes but Varric keeps giving short
remarks and Lirene cannot offer the healer topic. The conversation itself must
still be played; no quest-specific flags or map pins are synthesized.

Existing saves do not retroactively replay writes or plot scripts. A checkpoint
before the missed source write is needed to exercise the complete authored
chain.
