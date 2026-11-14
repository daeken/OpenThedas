# Origins script timer sources (SCR-032)

Static trace of Origins 1.05, image base `0x400000`; no original-game execution.
DA2's timers have not been traced.

`GetLowResTimer` at `0x64e960` calls `0x41e870` and pushes the returned 32-bit
integer through `0xa2d650`. The helper jumps through import slot `0xada208`;
the PE import descriptor identifies it as `KERNEL32.dll!GetTickCount`. This is
the Windows host's uptime counter in milliseconds, separate from game time.
The handler does not mask, saturate or convert that counter. As a signed script
integer it changes sign after about 24.9 days; the full 32-bit counter wraps
after about 49.7 days.

`GetTime` at `0x653c50` instead reads the game manager's timer at member
`+0xd4`, invokes virtual member `+0x14`, and truncates the floating result
through the CRT conversion at `0x8e34f0`. A missing manager or timer returns
zero. The constructor at `0x5db83d`/`0x5db852` installs the `EC::WorldTimer`
constructed at `0xa35f30`. Its vtable at `0xb58c44` selects `0x4cb250` for
`+0x14`, returning the accumulated double at timer member `+0x18`.

That total starts at zero (`0x4cb130`). The WorldTimer update at `0xa358f0`
does not accumulate while its pause byte is set. Otherwise it derives a
single-precision delta from clock ticks divided by 1,000,000, caps that delta
at 0.1 seconds, applies the scale at `+0x54` (initially 1), and adds it to the
double total. Pause/resume at `0xa359e0` resets the previous clock reading on
resume, excluding the paused interval. `0x5d5540` routes the game manager's
pause changes through this timer. Thus `GetTime` reads accumulated game seconds,
not the raw Windows uptime counter used by `GetLowResTimer`.

The simulation currently derives both functions from saved deterministic
`World::time_ms`. `GetTime` uses integer division by 1,000 followed by an i32
cast; native accumulation and out-of-range conversion are not reproduced.
`GetLowResTimer` casts those simulation milliseconds to i32, so it has the
counter's bit width but a different clock origin and pause/load behavior.
This trace does not change either implementation: using a host clock would
require an explicit policy for deterministic runs, replay and save/reload.
Exact native timer serialization, scaling callers and the underlying clock's
initialization remain outside this trace. SCR-032 stays open.
