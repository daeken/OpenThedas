# DA2 desktop platform and presentation-only script calls

Read-only tracing of the DA2 analysis image identifies these registrations:

| Command | Registration | Handler | Script result |
| --- | --- | --- | --- |
| GetPlatform (694) | 0x52868c | 0x4ebfd0 | Integer 1 |
| StopAdditiveAnimation (932) | 0x5268ba | 0x4f08f0 | Void |
| DoPlayerHitEffect (936) | 0x528887 | 0x4ecfc0 | Void |

`GetPlatform` pushes literal 1; this is the desktop platform expected by the
DA2 scripts, including when the replacement engine runs on macOS.

`StopAdditiveAnimation` resolves the object, obtains its model, checks an
additive channel's state through `0x4eeb20`, then calls the model's animation
stop/fade path. It returns no script value. `DoPlayerHitEffect` ignores
nonpositive effect IDs and forwards positive IDs plus the step parameter to
`0x533640`, which looks up table 3100 and constructs a presentation effect.
It also returns no script value. These native presentation effects are not
combat damage or persistent simulation effects.

The simulation acknowledges both void calls as no-ops, matching its existing
`PlayAdditiveAnimation` boundary. This does not implement additive channels or
screen hit effects. All three handlers are DA2-only, leaving Origins' previous
dispatch unchanged. The script-facing regression checks literal platform 1,
valid/invalid animation targets, several hit-effect IDs and an unchanged
simulation snapshot.

Matched story measurements and Origins regression results follow after the
merged candidate completes its checks.
