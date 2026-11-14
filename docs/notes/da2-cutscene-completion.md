# DA2 cutscene completion

The fresh warrior route can complete the real prologue's Ogre encounter and Flemeth's
conversation, yet remain in Outside Lothering. `pro000cs_voyage.cut` names
`pro000cs_voyageend` in CUT field 5001, inside its LINE wrapper. That shipped script
performs the common cutscene cleanup and requests the transition to
`pro000ar_gallow_court`, waypoint `pro000wp_gallows_start`.

The DA2 executable's loader at 0x59cef0 reads field 5001 at 0x59d15a and stores its
string at cutscene member +0xf0. This establishes the resource field; the native
completion caller, script owner and precise callback ordering are not yet traced.
The implementation runs this authored continuation on the module after releasing
cutscene actors, both for played and headless-skipped scripted cutscenes. That
context and ordering remain provisional. Conversation-line cutscenes retain their
existing completion path.

When a completion script is present it owns cleanup, so the engine does not also
consume the common CUTSCENE_* locals. A cutscene without a completion script keeps
the existing fallback. Origins is unchanged. The script is resolved from the
resource stack at completion, including after a saved playing cutscene is restored.

Synthetic tests cover the LINE wrapper's field, empty callbacks, and a VM script
that runs once on the module for headless completion, player skip, elapsed playback
and playback restored from a world snapshot. No game resource or executable bytes
are included in these tests.
