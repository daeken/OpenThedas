# Origins explicitly assigned object names (SCR-029)

Static trace of Origins 1.05, image base `0x400000`; no original-game execution.

`SetName` at `0x66ba00` resolves a scriptable object through `0x5d5be0` and
writes its localized-name property through `0x4d1bb0`. That setter copies the
supplied string and marks the property resolved and explicitly assigned. It
does so for an empty string too; there is no branch restoring the template.
`GetName` at `0x66bba0` reads that same property through `0x4d1eb0`. A resolved
value bypasses the talk-table resolver at `0x4d1d50`, so an explicitly assigned
empty name stays empty.

The simulation represents this with `Object::name = Some("")`. Its existing
display-name and save paths already preserve that distinction from `None`,
which permits a template or tag fallback. Only the Origins setter changes;
DA2's empty-name behavior remains untraced and keeps its earlier fallback.

This fixes empty-name assignment, not every aspect of native names. The
handler's player-name context update is separate from the object's localized
name. Its token consumers and exact scriptable-kind eligibility are outside
this change. SCR-029 retains the separate appearance/base-appearance gap.

## The dog-naming popup does not accept an empty name

The installed `popuplayer.gfx` winner is in `patch003.erf`. Its AVM1
`PopupLayer.TextFieldChange` (function assigned at action offset `0x3026`)
enables the first button only when the input TextField's length is greater
than zero. `TextInput_mc_init` and the first button's initialization both
apply this check; the input's `onChanged` applies it again after edits.
Deleting a previously entered name therefore disables confirmation too.

Enter does not bypass this check. The input's `onKeyDown` calls the first
button's `SimulateButtonPush`. `Behaviour.PushButton` (sprite 382, function
at action offset `0x3e7`) checks `enabled` and `m_bPushButtonEnabled` before
dispatching a press/release. `Behaviour.Disablable.Disable` clears `enabled`
through `DisableEvents`, which also gates mouse hit-area dispatch.
`popups.gda` row 2 has only one button, so there is no second-button callback
that can submit empty text. These are static traces of the shipped movie,
not original-game execution.

`genpt_dog_main` calls `ShowPopup` at NCS offset `0x696`, with popup kind 2,
the dog as owner and input enabled. `sp_module`'s result handler at
`0xec8..0xf99` reads object 0, integers 0/1 and string 0. For kind 2 it calls
`SetName` at `0xf88` with the owner and string, without testing the button or
string length. The native callback at `0x7098c0` builds event type 76, taking
the input text as string 0 (`0x709971`); it supplies no replacement name.

Consequently, ordinary confirmation in the original requires a nonempty
name. Leaving the box empty leaves it open and leaves the dog's previous
name unchanged. If an empty result is injected past the movie, the script
and native setter assign an explicitly empty name; they do not choose a
default. The regression harness's MCP `popup(button=1)` does precisely that
bypass because omitted text defaults to empty. Its 14 changed dog-name save
fields establish script/setter behavior for that injected input, not a
blank name reachable through the original popup's normal controls. This
change leaves popup validation and the harness unchanged.
