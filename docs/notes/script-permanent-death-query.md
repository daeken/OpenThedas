# Origins permanent-death query (SCR-026)

Static Origins 1.05 trace; no original-game execution. DA2 is untraced and
retains its existing behavior.

`GetCanDiePermanently` at `0x6645a0` initializes its answer to false and
resolves its argument with the creature lookup `0x5d58c0`. Missing objects
and non-creatures therefore return false. For a creature, member `+0xbd8`
(`NoPermDeath`) must equal zero. Any nonzero value prevents permanent death.
The query then obtains the party list when available and uses `0x6316f0`
to test membership; a listed creature returns false. If no party list is
available, that membership exclusion is skipped.

The predicate does not inspect immortality, current health, active state or
the dead flag. In particular, an immortal creature outside the party with
`NoPermDeath == 0` still returns true. OpenThedas already implements the flag
and party exclusions; this change adds the missing Origins creature guard.
It changes no death execution, corpse cleanup or party membership behavior.

The other status functions grouped under SCR-026 remain separate gaps.
This trace does not establish their semantics or DA2's native predicate.
