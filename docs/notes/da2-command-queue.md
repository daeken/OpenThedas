# DA2 waiting commands and animation readiness

The fresh real-prologue probe on main `d87590e5` finds Carver reaching melee
range and dealing six basic impacts before dying. His animation readiness
notifications execute, validate his current hostile and forward to the next
authored AI round. That round exits at `rules_core` bytecode
`0x240d1..0x2418e` because `GetCommandQueueSize` reports one. The counted entry
is the current swing. His next basic starts at the full animation endpoint.

## Native queue boundary

Read-only tracing of the player's DA2 analysis image establishes the following
separate storage and script interfaces.

- Registration `0x527c5f` binds command 724 to `0x4fd460`. The handler resolves
  the object and reads its waiting-list count at object `+0x2b0`.
- `GetCommandByIndex`, command 536, runs through `0x4e7800 -> 0x5c2400`.
  It iterates the waiting list at object `+0x298`. The index is consumed as an
  unsigned 16-bit value.
- `GetCurrentCommand`, command 539, runs through `0x4e7750 -> 0x5a8d40`.
  It reads the active executor through object `+0x2b8`, separately from the
  waiting-list count and index operation.

The shipped `script.ldf` independently describes the active command as outside
the queue. Its description of index zero identifies the first waiting command.

The modern animation task's ready event reaches `rules_core` event 112.
The script validates event object 1, sets integer 10, changes the event to
COMMAND_COMPLETE and schedules its own handler. For a living follower in
combat, the ensuing AI round admits another command when its waiting queue is
empty. Counting the active animation prevents that authored chain.

## Simulation change

The simulation stores the active executor at the front of its command vector.
Its existing deadline distinguishes a started front from waiting entries.
DA2's queue size and index getters now expose the slice after that started
front. An unstarted front remains waiting. A ready animation remains active
until completion or replacement. Index lookup retains the native 16-bit
conversion. Origins uses its existing command-vector view.

The deadline already survives snapshots, so older in-flight records and reloads
retain this boundary. Regression coverage exercises empty and invalid objects,
an unstarted command, a running command, multiple waiting commands, index
bounds and conversion, readiness, reload, completion and Origins behavior.
The repair concerns the two waiting-queue getters. The broader native command
admission and preparation lifecycle remains provisional.

## Installed-resource cadence

The modern task at `0x6d8860` and its event handler at `0x542a70` distinguish
animation readiness from the animation endpoint. With the installed animation
rows and these actors' attack-speed property of zero, the relevant deadlines
from the cast start are as follows.

| Attack | Impact or release | Ready | Animation end |
| --- | --- | --- | --- |
| Carver 3110 | 209 ms | 672 ms | 1,939 ms |
| Carver 3111 | 181 ms | 672 ms | 1,696 ms |
| Carver 3112 | 209 ms | 672 ms | 1,484 ms |
| Carver 3113 | 245 ms | 672 ms | 1,393 ms |
| Bethany 5510 | 200 ms | 570 ms | 2,100 ms |
| Archer 6410–6413 | 270–300 ms | 2,330 ms | 2,566 ms |
| Emissary 6510–6513 | 300 ms | No marker | 1,600 ms |

The readiness values include the native 100 ms preparation interval. Basic
combo variants and projectile flight account for some larger intervals between
impacts of one ability ID. Cast-start spacing across the combo gives the
corresponding cadence measurement.

## Replay measurements

The supplied `fresh3_02_real_arrival_main_d87590e5` checkpoint is replayed with
the supplied driver, including ordinary attack orders whenever controlled
Hawke idles. Main `249769ac` produces exactly the same 206,496 trace entries as
the requested `d87590e5` control. The candidate clears the first six-hurlock
fight in 22.6 seconds, compared with 29.8 seconds on main.

In the second fight, Carver's median cast-start interval changes from 1,590 to
672 ms. His damage events increase from six to fifteen and his damage total
from 74.68973 to 193.02504. Bethany's median interval changes from 2,100 to
570 ms. Hawke's damage total changes from 188.99231 to 373.39329. The main run
wipes after 48.6 seconds with three initial enemies alive. The candidate
defeats all seven initial enemies and wipes after 38.9 seconds. Both activate
Team 21, at `t=679239` and `t=644667` respectively. Main receives zero
reinforcement damage events before the wipe. The candidate receives 21 for
130.47207 damage, so the durations cover different combat progression. Both
runs have zero script errors, developer heals and forced kills.

The frozen fake-prologue wave-one control clears its initial six enemies in
147.20 seconds on main and 39.51 seconds with the repair. Hawke's mean basic
damage is 5.589890 and 6.215137 over 87 and 82 impacts respectively. Each enemy
loses exactly 100 HP. This historical checkpoint contains weapon power 5 from
before the Champion equipment repair. It measures the same saved equipment on
both builds. The complete probe also advances later waves.

Strict workspace clippy and formatting pass. Workspace tests report 1,350
passed, 22 ignored and zero failures. The connected fifteen-stage Origins
comparison has identical logs, plot sequences and all seventeen produced JSON
checkpoints. Its inherited progression stalls in Korcari Wilds and both board
steps fail on the missing board. Four 600-tick Origins determinism controls
pass repeat and save/reload comparisons.

Five additional independent Origins fixture scenarios have identical logs,
plot sequences and six produced checkpoints. Joining and Lothering's board
complete. Tower's five seeds wipe on level one on both builds. An extra father
fixture attempt was stopped after fifteen minutes in its first seed. The
connected father scenario had already completed identically on both builds.

A separate same-save DA2 comparison starts at `t=634722`, issues the same
ordinary Hawke attack, then advances exactly ten seconds with party AI enabled.
Carver deals 65.11910 damage in five events on main and 112.50782 in ten events
with the repair. His median cast interval is 1,484 versus 672 ms. Both complete
controls wipe. Labelled before/after/delta captures use this ten-second state.

The extra 100-tick DA2 determinism comparison repeats identically on both
builds. Save/reload on each build changes one Bethany effect ID and the next
effect-ID counter by one. All other world fields under the standard
bookkeeping normalization match. This pre-existing difference is retained in
the investigation evidence and reported separately from the passing Origins
determinism gate.
