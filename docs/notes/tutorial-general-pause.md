# Combat Basics: visible Next control owns the pause

## Evidence and scope

PR #1077's original Space-unpause change was incorrect. The original-game QA
capture in `OpenThedas-testdata/reference/original/triage/combat_pause_tutorial/`
shows Combat Basics page one with the Paused banner. Space leaves both unchanged.
The top-right Next arrow advances to the attack instructions and releases the
pause; page two has Close/X and combat continues behind it. These observations
take precedence over the earlier inference from the general pause setter alone.
No original game, VM or desktop was used for this work.

The untouched #1037 save (`triage-reports/1037/save_now.json`) has world time
1,382,304 ms, training mode/step/expected/argument zero, `tutorial_paused=true`,
and Combat Basics text 264857/title 264859 as its sole queued entry. The Dalish
finding's `dalish-combat-stuck.json` has time 35,813 ms, the same inactive training
state, no active/queued cutscene, and seven non-pausing notifications before
Combat Basics. The enabled-tutorial Dalish save has the same symptom. See
`playtest-agent/findings/dalish-disabled-tutorial-pauses-combat.md` in test data.

The shipped `plt_tut_combat_basic` has flag 0 text 264857 and flag 1 text 264858,
with title 264859 and pause field 13023 set. The second text matches QA's second
page. The prior replacement queued only the triggering flag, making page one
show Close rather than Next. Earlier pending notifications also left the actual
pausing lesson at the back of the Dalish queue.

There is a separate **UI visibility failure**: popuplayer.gfx observes
`GUI.TooltipText` and fades `TutorialContainer_mc.Fade_mc` out while a fixed
tooltip occupies the same corner. The enabled Dalish screenshot shows a Lesser
Health Poultice tooltip instead of the prompt. This is not an absent sim queue
or an absent HUD movie: the original queue and bindings exist. A fresh isolated
reload without a tooltip renders the earlier Journal tutorial. Both sim and UI
review are required.

## Changes

- `Unpause` remains journaled but refuses a popup pause. The app toggles effective
  pause, so a refused Space also does not add a manual pause that would remain
  after Next. Guided training still accepts only its instructed event. A deferred
  cinematic pause is retained, and DA2 page construction is unchanged.
- Origins pausing lessons include their subsequent nonempty plot text pages;
  only their first page pauses. Loading a paused older save reconstructs missing
  pages without changing plot flags. Next consumes the shown page and clears its
  pause when no pausing page remains; Close still consumes the full queue.
- A pausing lesson is put ahead of older pending notifications, which are retained.
  This foreground priority is a replacement recovery policy, **not** a traced native
  insertion order (native queue append is at 0x694087). Reconstructing successive
  pages from following journal-text flags is supported by the two-page Combat
  Basics capture; broader tutorial ordering has not been established by original
  runtime capture. The shipped pausing combat plots use this layout.
- While a tutorial holds pause, the app clears fixed tooltips so their movie cannot
  make its release control invisible. Tooltips work again on non-pausing pages.
  A queued pause's controls remain enabled if the user disables new tutorials.

## Tutorials-off trace and option delivery

Static inspection used DAOrigins.exe SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`,
virtual addresses with image base 0x400000:

- `ShowTutorials` registration/default true is at 0x5e8b38–0x5e8bbf (client
  option +0x98, name 0xb01448). Guided training reads it at 0x692ec1–0x692ed8.
- Plot notification reads it at 0x760156–0x760162. The false branch at 0x76016d
  skips the tutorial queue call at 0x7601a2 to 0x693f10, including popup append
  0x694087 and pause call 0x6940ec to 0x5d6740. Disabled tutorials skip both new
  popup and pause. This does not prove that changing the option clears an existing
  popup; the replacement retains existing queues.
- ConsumeTutorial at 0x693630 removes the displayed entry (or all for Close).
  Its empty-queue path clears tutorial bookkeeping at 0x6936d5 and calls general
  unpause at 0x6936fa. The general setter alone did not establish Space's UI route.

`Sim::queue_tutorial` already checked the option. The movie bridge previously
acknowledged SetClientOptionMessage before delivering it to the sim, so the
checkbox could be off while `Sim::show_tutorials` stayed true. The inherited
Dalish fix delivers journaled set/reset messages synchronously or through the
existing deferred path. Both initial UI attach paths now seed persisted options.
Cancel restores tutorial and combat-auto-pause options as well as difficulty.
Tests exercise the real bridge delivery, Cancel/defaults, disabled notification,
popup refusal/replay, page progression/recovery, cinematic deferral, guided
training, and the original movie's fixed-tooltip fade.

## Acceptance and validation

External `sim-work/repro/pausetutorial.sh <thedas-app>` runs nine sequential hidden,
silent, mods-disabled app captures with isolated HOME directories: untouched
#1037, Dalish disabled-finding save, and Dalish enabled-finding save, each with
no action, Space, or Space followed by an actual click on the movie's Next arrow.
It checks the movie display tree's visible parents and alpha, title/text, Next
and Close controls, journaled consume messages, and elapsed tick time. A quickbar
hover deliberately exercises the tooltip conflict. Space must advance zero ms;
Next must advance at least 900 ms and show the attack instructions.

`sim-work/repro/pausetutorial-dalish.sh` additionally re-arms only the notification
boundary (queue/pause and Combat Basics flag 0) while retaining encounter objects,
orders and RNG. Through the movie's original message API it compares enabled,
disabled, Cancel, Restore Defaults, and persisted disabled options. This derived
fixture is not a fresh walk from the origin's beginning.

## Recorded validation (2026-11-16)

Final integration `5e444b2` merges main `3a429faf`. Formatting, workspace/all-targets
clippy with `-D warnings`, and debug workspace tests pass on that integration:
**1,464 passed, 26 ignored, zero failed**. The original-popup-movie
regression was also run explicitly and passed (one test).

Release app/MCP captures, gates and detcheck were run at `7c85225` (merged main
`adf58096`), against a baseline built from main `45e01895`. All nine panel cases
and all seven option checks pass: the three untouched saves show Combat Basics,
Space advances zero ms, and the actual Next arrow shows the attack instructions
and advances 966 ms. Main reproduces the tooltip-hidden panel at alpha zero;
candidate visible ancestors retain alpha at least 90 (the movie's normal fade
clip reports about 95.7). Disabled notifications advance 966–967 ms without a
popup pause; Cancel and Defaults restore pausing for new notifications.

The isolated sequential gate covers s01–s12, s14 and s15 on both binaries:
28 successful sweep runs, zero log differences, identical plot sequences and
ending areas, and no refused saves. Both s13 runs reach Flemeth's hut with all
24 trace checkpoints identical: last checkpoint 600 step calls / 12,633 MCP calls;
final world time 1,979,312 ms. The complete final saves are identical.

Detcheck covers five fixtures × two binaries × 600 ticks, each repeated and
saved/reloaded halfway. All ten checks pass. Across the 20 detcheck saved states
plus the s13 save, all fields match except four Dalish snapshots' tutorial lists:
the pausing combat lesson is foregrounded and attack-instruction page 264858 is
added. Combat, time, RNG, orders and plot flags remain identical.

The later main merges contain the OpenGL-only MSAA fallback, conversation-camera
presentation changes and removal of unused camera helpers, and a DA2-only
cluster-candidate predicate change. `Stage::from_gff4` and `jump_camera` are
unchanged; the Origins predicate and tutorial paths are unchanged. The tested
captures use fixed cameras in combat with no conversation; the gate/detcheck use
Origins headless simulation. The completed runtime evidence is retained with
its original commit recorded above; fmt/clippy/workspace tests were rerun after
the final integration. Runtime checks remain recorded at the earlier commit above.

All heavy commands used `cargo-slot.sh`, Cargo used two jobs, and captures were
hidden/silent/mods-disabled and sequential. Final raw artifacts are under
`OpenThedas-testdata/codex-captures/pausetutorial/panel-validation/`.
Three corrected media comparisons are staged with Delta ×4 and a threshold of
any RGB channel difference >0: panel visibility 7.323611% changed pixels, Dalish
foreground lesson 8.322361%, and Next progression 6.670208%. Superseded
Space-unpause media was removed from staging.
