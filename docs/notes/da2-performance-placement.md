# DA2 named-stage performance end placement

The Chantry performances activate Karl's templars, but the simulation previously
left every actor at its area-load position. DA2 also prepares a release pose when
it takes a performance. This change implements that side effect for performances
which name an area stage, independently of performance camera playback.

## Native evidence

In the user's read-only DA2 analysis image, preparation at 0x5a36dc and 0x5a4960
calls the actor-take path, then 0x548720 for placement and 0x587db0 to prepare end
poses. The CUT loader reads field 5024 into the runtime stage tag at +0xbc
(0x59d29a..0x59d2fb). Field 5017 is the authoring stage resource filename; it is
not this runtime tag. Field 5028 controls end placement at +0x14a and defaults to
true (0x59d921..0x59d983).

Placement 0x548720 selects the named area stage nearest the player through
0x541070. Distance uses X/Y only; equal distances keep the earlier stage. Its
position and planar orientation become the performance transform. The native
path also recognizes `[Owner Relative]` and `[Speaker Relative]`, and falls back
to its captured owner context if a named stage is absent. Those contexts are
outside this bounded repair; an unsupported or missing stage moves nobody here.

The actor loader reads end position 5221 and rotation 5222 at
0x59154f..0x591678. Field 5235, read at 0x5915d2, enables walkmesh placement.
At 0x587e11..0x587ef8 the engine rotates and translates each end position,
rotates the end orientation and writes the resulting pose into the taken
object's release record. This happens during preparation, before the line
plays. When field 5028 is false, the original pose recorded by 0x53d020 remains.
Release 0x585be0..0x585bf3 applies that prepared record. It has neither Origins'
unmoved-object guard nor its placeable exception.

## Scope

Current performed lines are still matched by their actual TLK id, honoring the
no-performance flag. Named-stage lines now prepare bound world actors' end
poses and apply them when the line ends or the conversation closes. Taking
does not teleport simulation objects at the start of the line. Prepared poses
and the DA2 release marker serialize with the existing bound records; older
records default to the earlier release rules, and the marker is omitted when
false so Origins snapshots do not gain a field.

Activation remains the separately landed behavior. Unresolved bindings create
no objects. Relative stages, native owner-context fallback, processing suspension
and full performance camera playback remain unfinished. Active-party follower
aliases are a separate repair in PR #898. Walkmesh handling uses the existing
simulator floor lookup at the prepared point; native 0x57d200's wider projection
search is not reproduced.
No combat balance, stats, damage or cooldown changes are included.

## Validation

On merged main d0bcc32c, fmt, workspace all-target clippy with warnings denied,
workspace tests (1,214 passed, 17 ignored, zero failures) and forced
simulation/MCP/CLI release rebuilds pass. The DAO corpus passes with the existing
5,219 undeclared-action findings; its parsers are unchanged by the main merges.
The refreshed full s01-s13 A/B sweep has 26 successful runs, identical story logs
and plot sequences, 15 identical complete JSON checkpoints and no refused saves.
Four DAO 600-tick repeat/reload checks are identical. The DA2 repeat is identical;
reload differs only in `next_effect_id` (2 versus 3) on both main and candidate.
Matched main/candidate repeat and reload checkpoints are identical. Later
upstream changes are outside this frozen comparison.

The matched Karl confrontation starts from the same natural Chantry arrival,
takes the same replies and ends at t=1323927 on both binaries. All five templars
now leave their original overlapping positions for the authored stage end poses.
A repeat produces identical complete candidate JSON. The forced rebuild's
performance endpoint is also byte-identical to the earlier candidate endpoint.
On the earlier d87f9ef2 baseline the established natural driver wins Karl's fight
and returns to Anders: main t=1504060, placement t=1555860. On frozen main
d0bcc32c that driver instead wipes main at t=1488427 and placement at t=1483127.
The changed main-baseline outcome's cause is unproven. All runs have zero script
errors and zero developer healing/kill aids. These are conditional-driver
outcomes, not equal-duration comparisons or fresh-run completion.

The pre-Ogre checkpoint still wipes at t=915150 on refreshed main and candidate,
with identical complete JSON, also identical to the earlier baseline's endpoint.
This placement repair does not resolve that wipe.

The same-renderer, same-camera matched capture has Before / After / Delta x4
panels: 4.350% of pixels change at >2/255 in any RGB channel. Media remains staged
for coordinator review, never published by this job.

Integration with the separate follower-binding repair places Bethany and Varric
on their authored marks too. The same established Karl combat driver then wipes
at t=1414027 on both the earlier and refreshed source, with identical complete end
states. A prepared run using available level-ups/rest and a separate manual-input
run also wipe on the earlier integration. Processing suspension, command
admission and actor locomotion remain open. No combat-completion claim is made.

Synthetic regressions cover
runtime-tag parsing, the explicit release switch, duplicate/missing bindings,
stage transforms, player and placeable release, externally moved objects,
prepared-pose serialization and old-record/Origins behavior.
