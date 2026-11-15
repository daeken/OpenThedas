# Character creation camera preset blend

Static trace of the GOG 1.05 executable (`DAOrigins.exe`, image base 0x400000,
SHA-256 `2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`).
The original game was not run for this check.

`Paperdoll.SetCameraPreset` dispatches at 0x7b49b0 to the preset builder at
0x7b38a0. In stage mode, a subsequent preset starts an interpolation by setting
the active flag and clearing elapsed time (0x7b4057). The initial preset places
the camera directly.

The frame update at 0x7b4bb0 handles the active blend in this order:

1. Compare the squared distance between current and desired position with
   0.000001 (0x7b4ca1, helper 0x45b080, constant 0xb217f0). The helper sums the
   squared X/Y/Z differences, without a square root. At or below the threshold,
   skip interpolation, retaining the current camera and elapsed time. This is a
   one-millimetre positional tolerance, not a one-micrometre tolerance.
2. Otherwise add frame seconds to elapsed time (0x7b4cc8). The denominator is
   4.0 (double at 0xaf0c88) plus the zero-initialized value at 0xc38aec. The
   disassembly has reads of the latter but no direct write.
3. If elapsed time has reached the denominator, copy the destination and clear
   the interpolation flag/timer (0x7b4cfc).
4. Otherwise use elapsed / denominator as the fraction for this frame's move
   from the **current** position to the destination (0x7b4d73). The vector helper
   0x5b0db0 computes current × (1 − fraction) + destination × fraction.

This repeated interpolation has a rapid middle and a slowing tail. It is not a
linear four-second trip from the initial position. PR #689 already corrected
that distinction on main; the release candidate 8137c5d predates it. The remaining
correction is the squared-distance early exit and retaining that last position
rather than continuing a nearly invisible tail to a snap.

The orientation helper 0x4cbc10 delegates to 0x4cbaa0, which uses the quaternion
dot product, a shortest-arc sign choice, and spherical weights (linear weights
when the dot is close to one). Both chargen presets currently have the same
orientation, so this does not change the body-to-face timing. The application's
general yaw/pitch interpolation remains an approximation for differing angles.

The easing is frame-rate dependent in the executable: elapsed time determines
each frame's fraction. Hidden-window timings therefore describe that run's frame
cadence; they are not a measurement of the original game's wall-clock timing.
