# Height limit on the failed-path fallback (SIM-012)

Static DAOrigins.exe trace, 2026-11-15. Executable dumps and game fixtures remain outside
the repository. The change covers a failed path search with a large vertical difference;
it does not replace the pathfinder or claim complete failed-path parity.

## Native behavior

The move executor 0x7a9130 calls the path-start helper 0x73ea80. That helper first tries
direct reachability through 0xa626f0. Otherwise it searches through 0xa62640 and handles
its status. A successful direct reachability test constructs a valid two-endpoint path;
this is distinct from inventing a path after the search failed.

After certain failed search statuses, 0x73ee88..0x73eead tests the absolute start/goal
height difference against the float 0.5 at 0xb1b7b4. A difference greater than 0.5 skips
the fallback. At or below the threshold, 0x685920 performs a physics test. Only a passing
test builds the fallback through 0xa62f30, 0xa62990, 0xa629c0 and 0xa62b90; otherwise the
helper returns status 5.

Statuses 5 and 8 from 0x73ea80 take the move executor to 0x7a9880, which stops locomotion
through 0x6a9430 and returns -4 through 0x7a3190. The script constant is
`COMMAND_FAILED_INVALID_PATH`. These statuses do not authorize a straight walk to the
requested goal.

## Simulator correction and limits

When an Origins grid search fails and the endpoint height difference exceeds 0.5 m,
`route_inner` now reports `Routed::InvalidPath`. The harness walk returns false without
moving, advancing time or crossing triggers. Scripted movement and attack/ability
approaches fail with -4 through the existing command-completion path.

Successful paths can still climb or descend by more than 0.5 m overall. The guard only
applies after pathfinding failed. Missing navigation data, the flatter straight fallback
and DA2 retain their prior behavior. The native endpoint-adjustment logic, full status
mapping and physics fallback still need separate work; our grid failure is not proof of
the native pathfinder's answer for every position.

## Trigger investigation that exposed it

In #878's comparison against main d87f9ef2, the arrival branch ends the Lothering bandit
fight at approximately (247.70212, 175.70909, 7.8125). The board goal is
(346.4626, 199.41118, 1.17261). There is no grid route from that checkpoint. The old
fallback walked directly across the approximately 6.64 m descent and bypassed the
introduction and warning triggers. The unconsumed introduction then took priority at
camp. Exchanging the saved hero position reverses which triggers fire; exchanging the
binaries while retaining a fixed save produces identical traces.

That explains the gate discrepancy without justifying a change to the traced arrival
target or compact formation. It also does not establish which route the original would
find from these synthetic post-combat coordinates. This change addresses the confirmed
height guard independently of arrival placement, with no story-specific routing rule.
