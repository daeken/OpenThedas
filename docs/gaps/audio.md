# Known gaps: audio

[Back to the index](../KNOWN_GAPS.md)

FMOD event playback, music, voice, ambience, reverb and mixing.

## Part 1: the owning stream's list

Written by the stream that works on this area. Entries marked traced were read from the original executable; trace notes live outside the repository with the test data.

Updated 2026-11-12 by the audio thread. Each item gives what the original does, what we do, the evidence, the rough size and any dependencies. Merged work is noted where an item is partly done.

### Music

1. **Music changes.**
   - Done: a change waits for the playing track's next marker (first sync point 0.4–15 s ahead, else 0.4 s on); the stinger, started paused, and the new track start together there, and the old track fades out (0x5c4b10 states 3–7). An area with the same theme as the last keeps its music playing (SetTheme ignores the theme already playing, 0x5c0ec9).
   - Also done: a change to or from a track other than `main` and `combat` (such as `Death`), and leaving `combat` in a conversation or cutscene, happens at once with no marker wait (0x5c4c69..0x5c4ce2).
   - Not done: the 12 s load timeout ("Music failed to start") and delayed theme changes (state 9, counted in frames); both belong to theme changes (SetTheme 0x5c0e60), not traced from area entry yet.
   - Size: small.
4. **Music during cutscenes and conversations.**
   - Done: the game mode's ducking (0x5bd220): dialog and cutscenes play `OnCutscene` (music −3 dB, ambience and effects lower), death `OnDeath`, loading `OnGameLoading` (world sounds −60 dB; music and voice untouched).
   - Also done: `OnExploreMusic` while `main` plays and `OnCombatMusic` while `combat` does, while the music volume is above 0 (0x5bccd0, 0x5bc6f6); and a theme change during a load waits for the load to end, the old music playing on (0x5bfdf0, 0x5b7070 -> 0x5c4060).
   - Also done: the music director's cues per game mode (0x5bb630): explore `main`, combat `combat` (6 s late when it follows the interface, dialog or a cutscene), dialog and cutscenes `main` (except one after the other), death `Death`, the interface and loading keep the track.
   - Provisional: our combat and interface modes come from the party's fighting and the menus up, not from game modes the simulation keeps.
   - Medium.
5. **Death and party-wipe music.** The `Death` track plays when the game is over; exactly when the original switches isn't traced. Small.
6. **Other main-menu music** (after the credits, DLC menus). Only `music/_menu` is wired. Small.
7. **Credits music** (exe string `credits_df`). Not wired. Small.

### Ambience and placed sounds

8. **Placed-sound fields** (`.are` `SoundList`).
   - Applied: `Volume`, `MaxDistanceMult`, `Active`, `Tag`, and `Pitch` (added to the event's), `FadeIn` and `FadeOut` (replacing the event's; most sounds fade out over 34 ms), as the sound start does (0x7c261d..0x7c2637).
   - Also applied: the cone (`ConeInside`, `ConeOutside`, `ConeVolume`; 710 sounds, mostly half-spaces that are silent behind, such as waterfalls and wall creaks). The sound's facing is the event's orientation (slot 0x68, 0x5bf660); the angles replace the event's and `ConeVolume` (dB, when 0 or below) scales its outside level (0x7c265b..0x7c26c8). Provisional: that the loader turns the `.are` orientation into the facing vector, and a linear fall between the angles.
   - Not applied: `Priority`, `Occludable`.
   - Medium.
9. **Occlusion.**
   - Original: imports `Event::set3DOcclusion`, and sounds have an `Occludable` flag, so they are muffled behind geometry.
   - Ours: nothing.
   - Large; needs collision geometry from the sim or render side.
10. **Audio volume triggers (Type 4, CCTriggerAudioVolume).**
    - Their reverb is applied (#279).
    - Their `SoundList` plays (0x79a000; about 11,900 sounds in 2,829 volumes): each sound stands at the volume, and while the listener is near it follows the listener, at the listener while the party leader is inside and at the volume's nearest point otherwise (0x7c2ee0).
    - Provisional: the nearest point is taken on the outline (0x798de0 not traced past its bounds test); sounds are re-placed every frame instead of every 0.06 s after 0.25 m; `ObjectId` (save identity) is not kept.
    - Small.
12. **Ambient one-shots** (`amb_oneshots`, `glo_amb_creat`). Played only where the data places them; any engine-driven ambient spawning isn't checked. Small to medium.
13. **Placeable sounds** (doors, chests and so on).
    - Done: animation events of kind 27 (`ClosedToOpened`, `OpenedToClosed`, `Destroyed`, `Used`) play the `placeable_types.gda` column they name for the placeable's type (0x74c360).
    - Not done: sounds the exe plays from code rather than animations. The exe's player of `sound_plc_column_names.gda`'s rows (0x74c110, `Locked`, `Hit`) has no caller in the executable: it is only in the placeable vtable (0xb1c15c, slot 0x200) and no call goes through that slot, so it looks unused. Still open: the unlock and lock-pick-failure columns, (the container-opening column, 0x98094859, now plays as a container's inventory opens). They need the sim's lock and container events.
    - Small to medium.

### Voice-over and conversations

14. **VO placement.** Conversation lines are 2D, and 3D for ambient lines. The original's speaker positioning isn't checked. Small to medium.
15. **VO timing and lip-sync.** Check that conversation timing follows the line lengths as the original does. Not audio-only.
16. **The sound-set player** is traced (#290: `sst` counts, shuffle bag, the roll, gender via 0x7f2b70).
    - Applied: the type's chance is scaled by how many lines the set has of it (`ss_probabilitymodif.gda`, table 0x119: 1 line 0.2, 2 0.33, 3 0.5, 4 0.8, 5 and more 1), for types flagged in `ss_types.gda` column 0x6ddd6b8b (0x6b59fe..0x6b5a5f).
    - Not applied: a creature flag (+0x969) that makes a line certain.
    - Small.
17. **Event or sample for a sound-set line.** Inferred from the data: the event if it exists, otherwise the sample on `VOambient_ss`. Small.

### Combat and animation sounds

18. **Blows at the hit frame** are done (#297).
    - Not done: placeable targets (worksheet 93, placeable +0x4f0).
    - The armour material is taken from the chest item's base material; the exe goes through worksheet 89 "Material Types" (column 0xbb8ad13b) and an override (0x7485d0).
    - Small.
19. **Animation event kinds not sounded or not traced.**
    - Kinds 2 (22 events), 8 (7), 11 (27), 13 (10).
    - Kinds 21–38, among them 25 (pause tags), 27 (placeable states, see item 13), and 33, 34, 37 and 38, which carry numbers.
    - Kind 16 is visual effects, not sound.
    - Medium.
20. **Spell and ability sounds.**
    - Kind-20 `vfx_spells` events play.
    - Projectile, impact and area-effect sounds tied to the visual-effect system are not handled.
    - Large; depends on the effects work.
21. **Projectile flight and impact sounds** (bows, crossbows). Not done. Medium; depends on projectiles in sim and render.
22. **Creature vocalisations outside sound sets.** Scripts call `PlaySoundSet`; any engine-driven vocals aren't checked. Small.
23. **The level-of-detail gate** (0x7c4a80, #272).
    - Original: small and medium creatures at their lowest model level of detail make no footsteps or foley, except in dialog or cutscenes.
    - Ours: we gate only on whether the creature is drawn.
    - Small; needs render's level-of-detail state.
24. **Bodyfall and armour foley material** comes from the chest item (see item 18). Small.

### Footsteps and movement

25. **Done:** footsteps, scuffs, vegetation rustles and the silent Metal row (#267, #272).
    - Not checked: placeables and bridges (whether the original uses another material source there), and swimming or water.
    - Small.

### Reverb

26. **The reverb algorithm** is our own I3DL2-style reverb, not FMOD's.
    - The levels follow the documented I3DL2 meanings and are tested (#283).
    - Its colour needs QA's Beraht's Hideout capture.
    - Medium.
27. **Secondary reverb instances.**
    - Original: up to three presets are loaded (0x5c37f0, 0x5c35f0), for events that name a preset (0x5c18c0).
    - Not modelled, and the source of a sound's reverb name (+0x70) isn't found.
    - Small to medium.
28. **Room rolloff.** Not applied; it is 0 in every DA:O preset.

### 3D audio and mixing

29. **Panning.**
    - Ours: a linear balance. fmodex has no sqrt, so there's no constant-power pan.
    - Not applied: speaker spread, cones and the events' Doppler.
    - Medium.
30. **Listener position.** We place it 17% of the way from the leader to the camera (`sound_settings.gda`). Check against the exe. Small.
31. **Distance rolloff.** Envelope rolloff and the min/max curve are approximations; FMOD's 3D rolloff modes aren't traced. Medium.
32. **Voice limits.**
    - Done: max playbacks per event.
    - Not done: FMOD's global virtual-voice priority and channel limits (`setSoftwareChannels`).
    - Medium.
33. **Category effects.**
    - Done: `#duck:` ducking.
    - Not checked: category pitch, pause, and muting in menus.
    - Small.
34. **The limiter at −0.3 dBFS** is ours; the original has none. Accepted as an improvement.
35. **Output format.** The original sets a sample rate and speaker mode (`setSpeakerMode`) and can output 5.1; we output stereo only. Medium.

### UI and settings

36. **Options-screen volumes.**
    - Done: the music, sound-effect and dialogue sliders and their disable switches.
    - Not done: any hardware-acceleration or EAX option, and mute-when-unfocused.
    - Small.
37. **UI sounds.**
    - `PlayGUISound` comes from the interface movies. Rows with no event (for example 55, "open tutorial") are correctly silent.
    - Coverage of the movies' sound calls isn't checked.
    - Small.
38. **Pause.** Done: pausing in explore, combat, dialog or cutscene mode plays `OnGamePause` (0x5d6742), whose ducks apply. Also done: pausing in `GM_GUI` pauses the `VOambient`, `VOconversation` and `VOnarration` categories instead (0x5d684e), and unpausing resumes them. Not done: its `#pause:` categories are only ducked, not paused (AUD-027); `OnMovie` (0x6dc0ba, 0x7ce7cb) ducks nothing. Small.

### Loading and system

39. **Project and bank loading.** The original uses `sound_projects_to_load.gda` for preloading and streaming, plus `setStreamBufferSize` and memory limits. We load on demand. Small to medium.
40. **VO languages.** We follow `en-us`; other languages aren't checked. Small.
41. **Other campaigns.** Music themes for Awakening and the DLC modules don't resolve: 107 unresolved entries in `audio-check`, which only searches the main campaign's folders. Medium.

### Verification

42. **Reference recordings of the original.**
    - Done: the main-menu music (`reference/original/audio/main_menu_20s.wav`, recorded under Wine with `scripts/audio/syscapture`). Ours matches it to within 0.1 dB in level, channel balance, stereo width and brightness over the aligned 22 s (envelope correlation 0.97 at 48.65 s into the track), so music decoding, volume and categories are right.
    - Not done: a reverb recording (Beraht's Hideout), to compare the reverb's character.

## Part 2: code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

### FMOD event playback (thedas-audio engine)

- **AUD-003** **Only volume envelopes are applied.** Original: not traced. Ours: layer envelopes are applied only when they have no effect name and the `VOLUME` flag; pitch, pan, surround pan, 3D speaker spread, reverb level, 3D pan level, reverb balance, spawn intensity and time-offset envelopes are parsed and ignored. Evidence: `crates/thedas-audio/src/engine.rs:884`, `crates/thedas-formats/src/fev.rs:262`. Size: M. Depends: none. Kind: missing.
- **AUD-004** **Layer DSP effects are not run.** Original: envelopes that name a DSP effect (`FMOD Lowpass Simple` and others) drive that effect's parameter. Ours: no DSP chain exists; effect envelopes are skipped. Evidence: `crates/thedas-formats/src/fev.rs:280`, `crates/thedas-audio/src/engine.rs:884`. Size: L. Depends: AUD-003. Kind: missing.
- **AUD-005** **Parameter automation is not modelled.** Original: parameter velocity (self-advancing parameters), seek speed and sustain points / key-off. Ours: `Parameter::velocity`, `seek_speed`, `f2c` and `sustain_points` are parsed and unused; parameters only change when the game sets them. Evidence: `crates/thedas-formats/src/fev.rs:330`. Size: M. Depends: none. Kind: missing.
- **AUD-006** **Built-in angle parameters are never set.** Original: `(listener angle)` and `(event angle)`. Ours: only `(distance)` is written into an instance's parameters; the two angle constants have no user. Evidence: `crates/thedas-formats/src/fev.rs:326`, `crates/thedas-audio/src/engine.rs:847`. Size: S. Depends: AUD-007. Kind: missing.
- **AUD-007** **Sound cones are ignored.** Original: cone inside/outside angle and outside volume. Ours: parsed (`cone_inside_angle`, `cone_outside_angle`, `cone_outside_volume`), unused; placed sounds carry no orientation (`AreaSound` has none). Evidence: `crates/thedas-formats/src/fev.rs:171`, `crates/thedas-audio/src/area.rs:8`. Size: M. Depends: none. Kind: missing.
- **AUD-008** **Doppler is ignored.** Original: doppler. Ours: `Event::doppler` parsed, unused; sources have no velocity. Evidence: `crates/thedas-formats/src/fev.rs:175`. Size: S. Depends: none. Kind: missing.
- **AUD-009** **No priority, voice limit or voice stealing across events.** Original: priority and voice stealing. Ours: `priority` and `steal_priority` unused; the mixer plays any number of voices (no global cap, no virtual voices). Evidence: `crates/thedas-formats/src/fev.rs:161`, `crates/thedas-audio/src/mixer.rs:111`. Size: M. Depends: none. Kind: missing.
- **AUD-010** **Max-playbacks behaviour 3 is approximated.** Original: FMOD Designer's "steal quietest". Ours: per-event max playbacks is honoured; behaviour 3 steals the oldest. Evidence: `crates/thedas-audio/src/engine.rs:322`. Size: S. Depends: none. Kind: provisional.
- **AUD-011** **2D speaker levels and speaker spread are ignored.** Original: per-speaker 2D levels (L, R, C, LFE, back, side) and 3D speaker spread. Ours: parsed, unused. Evidence: `crates/thedas-formats/src/fev.rs:170`, `crates/thedas-formats/src/fev.rs:178`. Size: S. Depends: AUD-044. Kind: missing.
- **AUD-012** **Spawn intensity and position randomization are ignored.** Original: not traced. Ours: `spawn_intensity`, `spawn_intensity_randomization`, event and template `position_randomization` parsed, unused. Evidence: `crates/thedas-formats/src/fev.rs:181`, `crates/thedas-formats/src/fev.rs:371`. Size: S. Depends: none. Kind: missing.
- **AUD-013** **Loop modes and loop count are guessed.** Original: not traced in `fmod_event.dll` playback. Ours: loop mode 0 and 2 loop, 1 plays once; `loop_count` is never read. Evidence: `crates/thedas-audio/src/engine.rs:14`, `crates/thedas-audio/src/engine.rs:594`, `crates/thedas-formats/src/fev.rs:245`. Size: S. Depends: none. Kind: provisional.
- **AUD-014** **No-repeat and shuffle play modes repeat.** Original: random-no-repeat, sequential-no-repeat, shuffle. Ours: random-no-repeat and shuffle are plain weighted random (a repeat is possible), sequential-no-repeat is sequential. Evidence: `crates/thedas-audio/src/engine.rs:551`. Size: S. Depends: none. Kind: missing.
- **AUD-015** **Oscillator waveforms are silent.** Original: oscillator waveforms (shape, frequency). Ours: `WaveformKind::Oscillator` picks nothing. Evidence: `crates/thedas-formats/src/fev.rs:399`, `crates/thedas-audio/src/engine.rs:574`. Size: S. Depends: none. Kind: missing.
- **AUD-016** **Envelope curves are linear only.** Original: not traced. Ours: every envelope interpolates linearly; point curve-shape flags (only 1 = linear is named) are ignored. Evidence: `crates/thedas-formats/src/fev.rs:297`, `crates/thedas-formats/src/fev.rs:304`. Size: S. Depends: none. Kind: provisional.
- **AUD-017** **Spawn timing is guessed.** Original: not traced. Ours: a spawning definition fires first after `spawn_time_min_ms` (at least 1 ms), then at uniform random times in min..max, capped by `max_spawned`. Evidence: `crates/thedas-audio/src/engine.rs:14`, `crates/thedas-audio/src/engine.rs:740`. Size: S. Depends: none. Kind: provisional.
- **AUD-018** **Unnamed sound-instance fields are unused.** Original: not traced. Ours: `unknown0`, `unknown1`, `f1c`..`f30`, `flags_a`, `flags_b` (possibly per-instance fades or offsets) are stored and never read. Evidence: `crates/thedas-formats/src/fev.rs:244`. Size: S. Depends: none. Kind: unverified.
- **AUD-019** **Only an event's first category counts.** Original: not traced. Ours: `event.categories.first()` sets the volume, bus and ducking; other categories are dropped. Evidence: `crates/thedas-audio/src/engine.rs:331`. Size: S. Depends: none. Kind: unverified.
- **AUD-020** **Custom rolloff on simple events never attenuates.** Original: not traced. Ours: with `CUSTOM_ROLLOFF` set, distance gain comes only from layer `(distance)` volume envelopes; a simple event (no layers) with that bit plays at full volume at any distance. Evidence: `crates/thedas-audio/src/engine.rs:849`. Size: S. Depends: none. Kind: unverified.
- **AUD-021** **3D distance model is not checked against FMOD.** Original: FMOD Ex log/linear rolloff. Ours: `LINEAR_ROLLOFF` is linear between min and max; anything else is `min / d` clamped at max; the `LOG_ROLLOFF` bit is not consulted. Evidence: `crates/thedas-audio/src/engine.rs:849`. Size: S. Depends: none. Kind: unverified.
- **AUD-022** **3D panning is left/right only.** Original: not traced. Ours: the pan is the source direction's dot product with the listener's right vector times `pan_level_3d`; no front/back or height cue, the listener's forward and up are never passed in. Evidence: `crates/thedas-audio/src/engine.rs:14`, `crates/thedas-audio/src/engine.rs:496`, `crates/thedas-audio/src/engine.rs:843`. Size: M. Depends: AUD-044. Kind: provisional.
- **AUD-023** **Head-relative and world-relative modes are ignored.** Original: `FMOD_3D_HEADRELATIVE` / `WORLDRELATIVE`. Ours: the bits are named, never tested. Evidence: `crates/thedas-formats/src/fev.rs:142`. Size: S. Depends: none. Kind: missing.
- **AUD-024** **Event-group user properties are unused.** Original: not traced. Ours: `EventGroup::properties` parsed, no reader. Evidence: `crates/thedas-formats/src/fev.rs:133`. Size: S. Depends: none. Kind: unverified.
- **AUD-025** **FEV music-system chunk is kept unparsed.** Original: the module doc says DA:O's chunk is empty (music is ordinary events). Ours: `music_data` raw bytes. Evidence: `crates/thedas-formats/src/fev.rs:10`, `crates/thedas-formats/src/fev.rs:592`. Size: S. Depends: none. Kind: unverified.
- **AUD-026** **No streaming: every sample is decoded whole.** Original: wave-bank load flags (0x80 stream from disk, 0x200 decompress into memory) and max streams. Ours: flags and `max_streams` unused; each sample (streamed music included) is read and decoded fully on a fresh thread, held in a 768 MB cache. Evidence: `crates/thedas-formats/src/fev.rs:96`, `crates/thedas-audio/src/library.rs:25`, `crates/thedas-audio/src/library.rs:176`, `crates/thedas-audio/src/engine.rs:624`. Size: M. Depends: none. Kind: missing.
- **AUD-027** **`#pause:` parameters only duck; duck details guessed.** Original: not traced. Ours: `#duck:` works, and `#pause:` with numbers ducks the same way but does not pause; the third number and the dB ramp (linear in dB) are guesses. Evidence: `crates/thedas-audio/src/engine.rs:236`. Size: S. Depends: none. Kind: provisional.
- **AUD-028** **Snapshots / mixer states.** Original: snapshots/ducking. Ours: no code found beyond per-event `#duck:` parameters. Evidence: `crates/thedas-audio/src/engine.rs:219`. Size: M. Depends: AUD-027. Kind: missing.
- **AUD-029** **Event fade and category routing to buses is by name.** Original: not traced. Ours: buses are chosen by category name (`music`/`stinger`, `vo*`, `gui`, `*ambient*`), and the Ambient and Ui buses take the SFX slider. Evidence: `crates/thedas-audio/src/engine.rs:202`, `crates/thedas-audio/src/engine.rs:822`. Size: S. Depends: none. Kind: unverified.

### Music

- **AUD-034** **Cutscene mode is taken from the cutscene playing.** Original: `GM_CUTSCENE` (5) plays `OnCutscene` (0x5bd220). Ours: a cutscene playing outside dialog counts as `GM_CUTSCENE`, as our simulation does not switch to that mode; an app load state must set `AudioState::loading`. Evidence: `crates/thedas-app/src/audio.rs:1240`. Size: S. Depends: none. Kind: provisional.

### World sound, reverb, occlusion

- **AUD-035** **Secondary reverb instances are not modelled.** Original: loads the next two most-used presets into secondary reverb instances for events that name a preset (per comment). Ours: one world reverb. Evidence: `crates/thedas-audio/src/area.rs:136`. Size: M. Depends: AUD-036. Kind: missing.
- **AUD-036** **The reverb is our own algorithm.** Original: FMOD Ex software I3DL2 reverb. Ours: four taps, two diffusers, 8-line FDN; decay LF ratio, modulation, echo, environment and room rolloff are unused. Evidence: `crates/thedas-audio/src/reverb.rs:27`. Size: M. Depends: none. Kind: provisional.
- **AUD-037** **Reverb trigger entry rule is guessed.** Original: enter handler 0x79b3d0 (caller not traced). Ours: the party leader's entering counts; of several entered in one step, the last listed wins. Evidence: `crates/thedas-audio/src/area.rs:166`. Size: S. Depends: none. Kind: provisional.
- **AUD-064** **Placeable sounds play where the placeable stands.** Original: kind-27 sounds play at the centre of the model's bounds, transformed by its placement (0x74c4e2..0x74c686). Ours: at the placeable's position. Evidence: `crates/thedas-app/src/audio.rs:952`. Size: S. Depends: none. Kind: provisional.
- **AUD-063** **Music volume entry rule is guessed.** Original: the enter handler 0x799380 stores the volume's state and delay on the music director, whose update (0x5bb630) counts the delay down except while loading (`GM_LOADING`, 0x5bb669). Ours: the party leader's entering counts, the last listed of several wins, and the countdown pauses only while the app says it is loading. Evidence: `crates/thedas-audio/src/area.rs:222`, `crates/thedas-app/src/audio.rs:1579`. Size: S. Depends: none. Kind: provisional.
- **AUD-038** **Audio-volume triggers act only as reverb triggers.** Original: `CCTriggerAudioVolume` (type 4, loader 0x79a5d0) subclasses the reverb trigger. Ours: type 4 sets reverb and weighs 0.15 in the starting preset; any other behaviour of the subclass is not traced. Evidence: `crates/thedas-audio/src/area.rs:74`. Size: M. Depends: none. Kind: unverified.
- **AUD-039** **Occlusion and obstruction.** Original: occlusion. Ours: no code found. Evidence: `crates/thedas-audio/src/engine.rs:837`. Size: L. Depends: AUD-004. Kind: missing.
- **AUD-040** **Placed sounds' reach and replay are guessed.** Original: not traced. Ours: a placed sound starts within 1.1 x its event's max distance times `MaxDistanceMult` and stops outside; a one-shot that finished plays again only after the listener leaves and returns. Orientation fields are not read. Evidence: `crates/thedas-app/src/audio.rs:1231`, `crates/thedas-audio/src/area.rs:66`. Size: S. Depends: none. Kind: unverified.
- **AUD-041** **Placeable hit targets are silent; hits sound at the target.** Original: table 93 for placeables (per comment). Ours: not sounded; creature hits play where the target stands. Evidence: `crates/thedas-app/src/audio.rs:917`. Size: S. Depends: none. Kind: provisional.
- **AUD-042** **Bodyfall armour material taken from the chest item.** Original: creature +0x930. Ours: chest item's material. Evidence: `crates/thedas-app/src/audio.rs:960`. Size: S. Depends: none. Kind: provisional.
- **AUD-043** **Animation sound-event edge cases.** Original: every creature starts its rustle check from the origin with 1.0 (0x6b42f2). Ours: events on an animation's first frame are those at time 0 onward; a creature first seen starts its rustle check where it stands. Evidence: `crates/thedas-app/src/audio.rs:782`, `crates/thedas-app/src/audio.rs:795`. Size: S. Depends: none. Kind: provisional.

### Output and mixing

- **AUD-044** **Stereo only; surround is not produced.** Original: surround output (5.1/7.1). Ours: the mixer renders stereo; a device with more channels gets zeros past the first two. Evidence: `crates/thedas-audio/src/device.rs:55`, `crates/thedas-audio/src/mixer.rs:2`. Size: M. Depends: AUD-022. Kind: missing.
- **AUD-045** **Default device only, f32 only, no hot-plug.** Original: output device selection and hot-plug. Ours: cpal default device at its default config; a non-f32 default format runs silent; no device choice, no reopen on device change. Evidence: `crates/thedas-audio/src/device.rs:22`, `crates/thedas-audio/src/device.rs:30`. Size: M. Depends: none. Kind: missing.
- **AUD-046** **Pan law is unchecked.** Original: `fmodex.dll` pan law. Ours: linear balance. Evidence: `crates/thedas-audio/src/mixer.rs:5`, `crates/thedas-audio/src/mixer.rs:298`. Size: S. Depends: none. Kind: provisional.
- **AUD-047** **Output limiter (deliberate difference).** Original: FMOD's mixer clips (per comment; whether the exe adds a limiter is not traced). Ours: a -0.3 dBFS limiter with 0.2 s release. Evidence: `crates/thedas-audio/src/mixer.rs:207`. Size: S. Depends: none. Kind: provisional.
- **AUD-048** **Volume options are not persisted.** Original: volume options persisted. Ours: the options screen's slider values feed `volumes()` but nothing writes the player's settings file. Evidence: `crates/thedas-ui/src/options.rs:11`, `crates/thedas-app/src/audio.rs:1527`. Size: S. Depends: CAM-044. Kind: missing.
- **AUD-049** **Pause only ducks.** Original: the pause toggle (0x5d6742) plays `OnGamePause`, which pauses some categories and ducks others, or pauses the VO categories in `GM_GUI`. Ours: the ducks apply (`AudioState::paused`) and the VO categories pause in `GM_GUI` (`AudioEngine::set_category_paused`); the `#pause:` categories are only ducked. Evidence: `crates/thedas-app/src/audio.rs:398`. Size: S. Depends: AUD-027. Kind: provisional.

### Voice-over

- **AUD-051** **Voice-over is not positioned.** Original: voice-over positioning. Ours: conversation and ambient lines play 2D (`speak` sets no position). Evidence: `crates/thedas-app/src/audio.rs:14`, `crates/thedas-app/src/audio.rs:1513`. Size: M. Depends: none. Kind: missing.
- **AUD-052** **Lip sync.** Original: lip sync data. Ours: no code found (no FaceFX or lip reader). Evidence: `crates/thedas-formats/src/lib.rs:1`. Size: L. Depends: FMT-027. Kind: missing.
- **AUD-053** **VO language is a flag, not the game's locale.** Original: localisation of VO. Ours: `--language` (default `en-us`) picks `audio/vo/<language>/vo`; the options' `Locale` is not consulted; talk tables stay English (FMT-017). Evidence: `crates/thedas-app/src/main.rs:219`, `crates/thedas-audio/src/library.rs:95`. Size: S. Depends: FMT-017. Kind: missing.
- **AUD-054** **Sound-set details guessed.** Original: per-creature probability multiplier (table 0x119) and creature flag +0x969 (per comment). Ours: bag draw order random; the multiplier and flag are not modelled; event-vs-sample choice inferred from data. Evidence: `crates/thedas-app/src/audio.rs:653`. Size: S. Depends: none. Kind: provisional.

### Cutscene and movie sound

- **AUD-055** **Cutscene sound tracks joined late are skipped.** Original: not traced. Ours: an `ACSP` track whose start was missed does not play. Evidence: `crates/thedas-app/src/audio.rs:1304`. Size: S. Depends: none. Kind: provisional.
- **AUD-056** **Movie sound bypasses categories and needs ffmpeg.** Original: not traced. Ours: only boot movies' sound plays, decoded by an external `ffmpeg` at full volume outside the categories (the dialog/music sliders do not apply). Evidence: `crates/thedas-app/src/audio.rs:130`, `crates/thedas-app/src/audio.rs:1039`. Size: M. Depends: CAM-029. Kind: provisional.

### Codecs

- **AUD-057** **IMA ADPCM layout from community docs.** Original: `fmodex.dll` decoder. Ours: frame layout per community description, lengths agree with all banks. Evidence: `crates/thedas-audio/src/decode.rs:153`. Size: S. Depends: none. Kind: provisional.
- **AUD-058** **FSB header field names unverified.** Original: `fmodex.dll`. Ours: names from community (vgmstream, fsbext). Evidence: `crates/thedas-formats/src/fsb.rs:14`. Size: S. Depends: none. Kind: provisional.
- **AUD-059** **Console codecs unsupported.** Original: XMA, VAG, GameCube ADPCM, Vorbis. Ours: `Codec::Other` is refused; PC data ships only IMA ADPCM and MP3. Evidence: `crates/thedas-formats/src/fsb.rs:72`, `crates/thedas-audio/src/decode.rs:71`. Size: L. Depends: FMT-045. Kind: missing.
- **AUD-060** **More than two channels refused.** Original: not traced. Ours: samples with channels outside 1..=2 error. Evidence: `crates/thedas-audio/src/decode.rs:55`. Size: S. Depends: none. Kind: missing.

### Documentation inside the code


### Checked and looks complete

- Event lookup by path, case-insensitive, with groups (`engine.rs` `locate`).
- Per-event max playbacks with steal-oldest / steal-newest / fail.
- Category volume hierarchy from `audio_categories.fev`, own-project fallback.
- `#duck:` ducking of a category and its children with fade in/out.
- Event fade in/out, event volume, template volume, reverb dry/wet per event.
- Layers placed on a control parameter; sounds start/stop as the parameter moves.
- Programmer-selected sounds (voice-over samples by strref, `_f`/`_m`).
- Reverb preset lookup by name (`force_generic`, empty = off) and 0.33 s blended fades, as traced.
- Area starting reverb preset by weighted trigger count, as traced.
- Options sliders to dB (-3 dB per step, -60 dB floor), as traced (0x5b61b0, 0x5b6110).
- Area music themes: main/combat/Death with stingers; changes wait for the playing track's next marker, as traced (0x5c4b10).
- Placed area sounds with `ActivateSoundByTag`.
- UI sounds through `sound_gui.gda`.
- Script `PlaySound` and `PlaySoundSet`.
- Footsteps, scuffs, vegetation rustles, armour foley, swings, weapon (un)sheathe, blows, bodyfalls, as traced.
- Sound-set player (counts, bag, probability, gender), party-picker lines, chargen voice preview.
- Cutscene `ACSP` sound tracks; boot-movie sound.
- FSB4 parse, IMA ADPCM, MP3 (Symphonia), PCM8/16 decode; WAV recording for headless runs.
