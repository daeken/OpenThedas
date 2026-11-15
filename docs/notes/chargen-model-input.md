# Character creation model input

Static trace of GOG 1.05 `DAOrigins.exe`, image base 0x400000, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`, and the
shipped `chargen.gfx`. No original-game execution or original frames were used.

The movie's `SharedLibrary.CharGenDragArea` covers the left half of the stage.
Press starts tracking mouse movement; release or release outside stops it. Each
movement sends `Paperdoll.RotatePaperdollCamera` with horizontal and vertical
stage-coordinate deltas, both multiplied by -0.8. It has no wheel callback or
zoom message. A stationary click consequently does not rotate anything.

The executable's callback adapter at 0x7b3410 binds this message's type to
0x7b2430. That handler:

- Completes an active preset blend first (0x7b2442).
- Composes a rotation by the negative horizontal argument about Z into the
  creature's orientation (0x7b24c3), then applies it to the drawn creature through
  0x7b22a0. The game camera does not orbit horizontally.
- In camera mode 1 only, adds the vertical argument to camera pitch, clamped to
  -60 degrees (0xafee58) through +10 degrees (0xae7d48). It rotates camera position
  around the cached head position (+0x190) and rotates its orientation by the
  same increment (0x7b2512–0x7b265a).

`SetCameraPreset` stores the preset in the mode property (+0x144) at 0x7b38e0;
the constructor initializes it to zero (0x7bffc0). Presets reset the pitch to
-10 degrees (0x7b38f7). The movie reads `Paperdoll.CameraMode` to select
`panning1_h`/`panning_h` in body mode and `panning1`/`panning` in face mode.

The app already consumed horizontal messages for inventory paperdolls, but never
applied them to the chargen stage, and never bound the mode property. The stage
now applies the negative accumulated turn to its drawn model, without changing
the simulation's creature. Face tilt is presentation state too. Wheel input
continues to reach the movie and does not zoom the stage.
