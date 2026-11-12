# Known gaps: audio

[Back to the index](../KNOWN_GAPS.md)

FMOD event playback, music, voice, ambience, reverb and mixing.

## Part 1: the owning stream's list

Written by the stream that works on this area. Entries marked traced were read from the original executable; trace notes live outside the repository with the test data.

Updated 2026-11-12 by the audio thread. Each item gives what the original does, what we do, the evidence, the rough size and any dependencies. Merged work is noted where an item is partly done.

### Music

1. **Music changes: stinger timing and loading.**
   - Done: a change waits for the playing track's next marker (first sync point 0.4–15 s ahead, else 0.4 s on), then the new track starts and the old fades out (0x5c4b10 states 4–6).
   - Not done: the stinger's timing (the exe waits for it in state 7; we play it at the change), the 12 s load timeout ("Music failed to start"), and delayed theme changes (state 9, counted in frames).
   - Size: small.
2. **Script `PlayMusic` and `SetMusicIntensity`.**
   - Original: `PlayMusic` names a project; `SetMusicIntensity` sets the `music_state` parameter.
   - Ours: `SetMusicIntensity` is stored in the World and applied; `PlayMusic` is only logged.
   - Size: small to medium. Depends on item 1.
4. **Music during cutscenes and conversations.**
   - Whether the area music ducks, stops or keeps playing isn't traced.
   - Ours: left alone apart from the `#duck` categories. Medium.
5. **Death and party-wipe music.** The `Death` track plays when the game is over; exactly when the original switches isn't traced. Small.
6. **Other main-menu music** (after the credits, DLC menus). Only `music/_menu` is wired. Small.
7. **Credits music** (exe string `credits_df`). Not wired. Small.

### Ambience and placed sounds

8. **Placed-sound fields** (`.are` `SoundList`).
   - Applied: `Volume`, `MaxDistanceMult`, `Active`, `Tag`.
   - Not applied: `ConeInside`, `ConeOutside`, `ConeVolume` (directional sounds), `FadeIn`, `FadeOut`, `Pitch`, `Priority`, `Occludable`.
   - Medium.
9. **Occlusion.**
   - Original: imports `Event::set3DOcclusion`, and sounds have an `Occludable` flag, so they are muffled behind geometry.
   - Ours: nothing.
   - Large; needs collision geometry from the sim or render side.
10. **Audio volume triggers (Type 4, CCTriggerAudioVolume).**
    - Their reverb is applied (#279).
    - Not handled: their `SoundList` (sounds that play only inside the volume, 0x79a000) and `ObjectId`.
    - Medium.
11. **Music volume triggers (Type 5, CCTriggerAudioMusicVolume).**
    - Original: `EnterState`, `ExitState`, `EnterStateDelay` and `ExitStateDelay` (0x799ad0) change the music state on entering and leaving.
    - Not done. Medium; ties in with item 1.
12. **Ambient one-shots** (`amb_oneshots`, `glo_amb_creat`). Played only where the data places them; any engine-driven ambient spawning isn't checked. Small to medium.
13. **Placeable sounds** (doors, chests and so on): `.evt` kind 27 (`ClosedToOpened`, `OpenedToClosed`, `Destroyed`, `Used`) and `sound_plc_column_names.gda`. Not done. Medium.

### Voice-over and conversations

14. **VO placement.** Conversation lines are 2D, and 3D for ambient lines. The original's speaker positioning isn't checked. Small to medium.
15. **VO timing and lip-sync.** Check that conversation timing follows the line lengths as the original does. Not audio-only.
16. **The sound-set player** is traced (#290: `sst` counts, shuffle bag, the roll, gender via 0x7f2b70).
    - Not applied: a per-creature probability multiplier (table 0x119, column 0x29f4e2fb, when the type's column 0x6ddd6b8b is set) and a creature flag (+0x969) that makes a line certain.
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
38. **Pause.** Whether world audio pauses in menus and the inventory isn't traced. Small.

### Loading and system

39. **Project and bank loading.** The original uses `sound_projects_to_load.gda` for preloading and streaming, plus `setStreamBufferSize` and memory limits. We load on demand. Small to medium.
40. **VO languages.** We follow `en-us`; other languages aren't checked. Small.
41. **Other campaigns.** Music themes for Awakening and the DLC modules don't resolve: 107 unresolved entries in `audio-check`, which only searches the main campaign's folders. Medium.

### Verification

42. **Reference recordings of the original.**
    - Done: the main-menu music (`reference/original/audio/main_menu_20s.wav`, recorded under Wine with `scripts/audio/syscapture`). Ours matches it to within 0.1 dB in level, channel balance, stereo width and brightness over the aligned 22 s (envelope correlation 0.97 at 48.65 s into the track), so music decoding, volume and categories are right.
    - Not done: a reverb recording (Beraht's Hideout), to compare the reverb's character.
