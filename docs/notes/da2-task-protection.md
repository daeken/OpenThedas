# DA2 task protection beyond ability animation

The clear helper at `0x5c6210` checks active-command static state separately
from the first executor task's byte `+4`. Neither soft nor hard ordinary
admission overrides a protected task (`0x5c6267..0x5c6277`). The generic task
constructor zeroes that byte, but `0x5ef260` overwrites it from the fourth
enqueue argument. Using the constructor default alone loses the real barrier.

The command preparation registry at `0x6829a0` and task dispatch registry at
`0x6db600` connect these paths. Relevant represented task recipes are:

| Command | Task / enqueue site | Protection |
| --- | --- | --- |
| PlayAnimation 14 | animation 37, `0x61d394` | inverse of native boolean 6 |
| StartConversation 18 | approach, then dialogue 12, `0x656559` | 1 on the dialogue task |
| UseObject 31 | approach, then use 21, `0x656a6c` | 1 on the use task |
| JumpToObject 9 | jump 32, `0x6109a9` | 1 |
| JumpToLocation 10 | jump 33, `0x610a49` | 1 |
| Turn 41 | facing 42, `0x6101e3` | 1 |
| SwitchWeaponSet 40 | switch 22 / continuation 48, `0x610dc6`, `0x610e1f` | 1 |
| Legacy attack 1 | post-approach attack 2, `0x698563` | 1 |

PlayAnimation's script wrapper is registered at `0x527dd2`, enters
`0x4e7e30`, then calls constructor `0x4de010`. It writes integer arguments
0/2/7 and booleans 3/11. It leaves boolean 6 at zero. Thus script-created
animations have protected task 37; the readiness word at task `+8` is a
different field. An animation becoming ready does not make ordinary admission
able to interrupt it. Successful ready handoff has its own override.

Movement task handlers `0x6c8960` and `0x6c9170` clear byte `+4` after setup
at `0x6c8b43` / `0x6c93fa`. Wait task 36 (`0x692980`) also releases it while
initializing its duration at `0x6929ad`. The existing movement/wait phase must
remain interruptible. The simulation uses the active movement origin to
distinguish approach from the protected terminal action. Unknown/unrepresented
task recipes keep their prior behavior. Placeholder ability 1 selects its
concrete ability before constructing tasks, so selection has no task barrier.
Full native task scheduling, native
animation durations for non-ability commands, and preparation callback phases
are separate boundaries; no durations or combat values are changed here.

The admission barrier now covers these represented action phases as well as
the already protected ability animation. Regressions exercise soft and hard
clear, readiness independent of protection, waiting-copy pruning, endpoint
completion, movement/wait release and snapshot persistence. Origins admission
is independent and unchanged.
