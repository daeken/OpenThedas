# Report dialog input

The installed popup movie's `TextInput_mc_init` applies `maxChars = 20` and a
`restrict` string independently of `GUIPopupLayer.Filter`. Selecting filter zero
does not make it a free-text box. Report and spot notes use `ask_free_text`, which
clears those two field settings after initialization, on that dialog instance.
Ordinary `ask_text`, script name prompts, save names and character creation retain
their field settings. Enter remains confirmation rather than a newline.

Host ticks do not correspond to movie frames: under uneven scheduling the old
`2..FOCUS_FRAMES` window could expire before `TextInput_mc_init` ran. Its late
writes then restored the name limits or replaced early typing with the default
input. The free-text policy now runs after every tick and before text input for
the lifetime of the claimed popup clip. A marker on that clip identifies the
engine question; a replacement script popup cannot inherit the policy even if
its prompt text is identical.

The initializer assigns the default text, limits and focus before installing
`TextField.onKeyDown` as its final write. Until that handler exists, free-text
dialogs capture characters, editing keys and Enter into an ordered queue rather
than letting the movie accept them. Once initialization finishes, the policy is
applied and the queued keys reach the field through normal input handling.
Focus is established at that transition, independently of the ordinary prompts'
focus-retry window. It is re-established through the installed focus handlers so
an early mouse click cannot leave the field without its key listener.
Closing or cancelling the dialog discards its remaining queue. The host also
avoids focusing this field before initialization: doing that would cause the
initializer's later focus assignment to skip `onSetFocus` and its key-listener
registration. The normal name/script focus paths keep their existing behavior.

Separately, the host text helper used a character's uppercase ASCII value as its
virtual key code. Apostrophe therefore acted as Right, period as Delete, and
`#`, `$`, `%` as End, Home, Left. Character input now carries character data
without an editing-key code. The synthetic-field unit test covers punctuation,
long text, and continued enforcement of name-style limits and restrictions.

The bridge also keeps `GUIPopupLayer.InputText` as a string when the movie writes
it. Numeric coercion previously lost leading zeroes and made a numeric-only note
look like a missing string to the app. Other numeric properties retain their
existing parsing.

`PopupLayer.ShowDialogBox` refuses a new dialog while `ms_mcPopup` is occupied.
The shared bridge values describe a requested question, not necessarily the box
on screen. In a hidden-window replay, an existing dog-name prompt took the report
text and its own callback; checking the new bridge prompt hid that displacement
from the report's retry logic. A later report sequence also left typing with
no text field available. The displayed prompt is now read from the live popup's
`ms_sInitString`, guarded by a live clip check. Pending questions retain their
values until they can be shown, and never change another dialog's field policy.
When a script popup closes, the following binding update removes its popup
properties. Previously that could also erase a report question written during
the script dialog, including its input flag and callbacks, creating a box with
no working text input. Publishing the complete question immediately before
showing it keeps it intact through that binding expiry.
A tick between closing the previous dialog and publishing the next question lets
the previous callback collect its typed answer before `InputText` is replaced.
Timeouts and report completion cancel pending questions so they cannot appear
after their owner has finished.

The ignored app test
`report_popups_wait_cancel_and_keep_typing_across_name_and_choice_boxes` uses
`THEDAS_INSTALL` and an offscreen GPU, without opening a window or embedding game
assets. It cycles name, report and choice boxes three times, checks the original
20-character name limit while a report waits, cancels a blocked report, and types
long punctuated notes. Run it explicitly with `--ignored`; the ordinary workspace
suite also runs the asset-free text-input regression.

The ignored `report_late_input_init_*` app tests deterministically advance thirty
host ticks with zero movie time before letting initialization run. One types a
long punctuated note after that late initialization and checks the following
name prompt still has its 20-character limit. The other queues typing, selection,
replacement, backspace and Enter before initialization, then verifies the exact
submitted note. These use the installed movie and an offscreen GPU, without
embedding assets. Run them with `THEDAS_INSTALL`, an isolated `HOME`, and
`cargo test -j 2 -p thedas-app report_late_input_init -- --ignored --test-threads=1`
through the shared cargo-slot wrapper.

The 2026-11-15 loaded reproduction captured ten empty notes in forty reopens
with the legacy build and native movie. A separate 160-reopen native sweep at a
smaller viewport did not reproduce the character cap. To make that late-init
case reliable, a private test overlay changed only the popup movie's frame-rate
header from 60 to 5 fps; the fifth legacy reopen restored the 20-character limit.
The overlay and all captured game data stay outside the repository. The original
installed movie and its bytecode are unchanged.

With the fix, twenty consecutive reopens in one loaded run saved every character
of each 127-character punctuated note. The run alternated typing before and after
initialization, used the same 5 fps overlay, and used a 640×360 hidden viewport to
keep rendering costs within the replay's resource cap. Separate 1280×720 captures
show both legacy failure modes and successful fixed notes. The runtime proof used
an optimized debug build with assertions and overflow checks enabled; the ignored
late-init tests and workspace checks use the normal debug/test settings.
