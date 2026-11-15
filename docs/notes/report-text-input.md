# Report dialog input

The installed popup movie's `TextInput_mc_init` applies `maxChars = 20` and a
`restrict` string independently of `GUIPopupLayer.Filter`. Selecting filter zero
does not make it a free-text box. Report and spot notes use `ask_free_text`, which
clears those two field settings after initialization, on that dialog instance.
Ordinary `ask_text`, script name prompts, save names and character creation retain
their field settings. Enter remains confirmation rather than a newline.

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
