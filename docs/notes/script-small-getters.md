# Smaller Origins native queries

Own prose interpretation of the read-only 1.05 executable. Script dispatch below
is Origins-only; DA2's existing implementations/defaults remain separate.

| Native | Trace and behavior |
| --- | --- |
| GetVectorMagnitude 42 | 0x64f120: sqrt of the three squared components, rounded to float before the square root. |
| GetVectorNormalize 43 | 0x64f1b0, helper 0x451a30: zero stays zero; a nonzero vector whose squared magnitude is below 1e-6 yields +X; otherwise multiply by reciprocal magnitude. |
| GetDebugHelpersEnabled 331 | 0x653c30 pushes literal zero. |
| IsPRC 857 | 0x652260 consumes the deprecated string/integer arguments and pushes literal zero. This is independent of installed or loaded DLC. |
| GetTlkTableString 14 | 0x654b50: talk-table reference and validated gender, then the localized text resolver at 0x4e4990. Sim uses its existing English male merged table; female variants remain a localization limitation. Missing strings are empty. |
| GetCreatureAttribute 248 | 0x66d0a0 shares 0x66cf60 with GetCreatureProperty and truncates its float result to integer. Values 1..4 select total/base/current/modifier; invalid creature/value/property requests retain -1. Uses Sim's existing property calculations and bounds. |
| GetAILevel 221 | 0x659de0 resolves CCObject and reads signed +0x2b0. Unresolved or unsupported kinds return -1. CCObject construction initializes -1 at 0x699b0a; SetAILevel already retains explicit writes. |
| IsItemStealable 641 | Shared 0x668a20, item branch 0x668c6c: boolean byte +0x4b1, false for other kinds. Template loader 0x7578a3 names it Stealable. Existing integer import retains it. |
| GetAOEFlags 334 | Shared 0x668a20, AOE branch 0x668cbc: signed +0x4dc, default zero. SetAOEFlags 335 writes the same member at 0x66c109. Constructor initializes zero (0x7e60af); flags are retained separately from effect flags. |
| GetMapPinState 653 | 0x65ee00 resolves virtual +0x30 (map pin), reads +0x444, or returns -1. SetMapPinState 652 normalizes nonzero to true and calls 0x7c4930. Construction initializes zero (0x7c3b01). This state is separate from world-map travel availability. Pin graphics refresh at 0x7c4370 is not modeled. |
| IsFollowerLocked 146 | 0x65ead0 queries the player party's follower registry at 0x6329b0, reading its independent lock byte. SetFollowerLocked 147/0x632950 changes existing records only. Sim's existing party pool represents that registry; it retains the lock separately from follower state. Broader picker lock/recruitment reconstruction remains provisional. |
| GetRoamLocation 826 | 0x6668d0: a creature's saved roam point +0xcd0, its CURRENT area +0x254, and zero orientation. Invalid creature gives the initialized invalid location. Reuses the existing SetRoamLocation point storage. |
| IsMoving 388 | 0x666550 tests the creature's movement mode +0xb20 for nonzero. Sim represents an active movement interval by the front command's `from`, excluding commands that are only queued. The full native animation/movement-mode state machine remains an existing approximation. |

New values use existing object field serialization. No resource parser or format
change is made. No new wall clock, PRC/package heuristic, or hostility conversion
is introduced.
