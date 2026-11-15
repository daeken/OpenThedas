# Triage retro

**Went well**
- A fixed gate compared against the last good build turned "is it worse?" into numbers: end areas, plot sets, log diff lines, seed counts and detcheck. Most candidate checks came back as "0 log diff lines", which made late merges cheap to accept or refuse.
- Judging the tower by step and call counts, never by wall time, ended a run of false "hangs" caused by a loaded machine.
- Splitting the cloud half (merge, cross-build, plan) from the Mac half (data, game, traces) let builds and merges go on while the Mac was busy or asleep.
- Delta checks, which re-ran only what the new merges could touch, kept candidate updates to minutes.

**Could have been better**
- Main moved faster than checks finished. The merge rule's "do it again if main moved" became a loop. In practice we re-checked only when the new merges touched the same files. That should be the written rule, not a judgement call each time.
- The laptop was the build and test host. Sleep, Wi-Fi drops and the auto-mode check stopped sessions mid-task, and a stopped session lost its turn's work.
- One shared machine at a load average of 30-110 made frame-rate numbers and timing-sensitive checks noisy. A slow window turned out to be another stream's debug build.
- Approvals relayed by another Claude didn't count on the Mac, so a simple "launch the game for Sera" took three round trips until her own message reached it.
- The Windows build lived outside CI (GitHub Actions disabled). Packages were cross-built by hand, and splitting into 8 MB parts was a workaround for a file-size limit.

**Lessons**
- Write the gate down as a script with a machine-readable verdict, and keep it next to the code.
- Keep one "last good" pointer (play.command, the tester branch) and move it only on a graded commit.
- Route bugs with a repro and a suspect, not just a description. The walk-clip lead from rendering cut the fix to one session.
- Merging is cheap when checks are cheap. Invest in making the checks fast.

**How to improve**
- Get an always-on build and test host with its own GPU, and run the gate on every PR there: CI with the game data mounted, results posted to the PR.
- Make the merge rule "re-check when main changed files you touch", and let a merge queue enforce it.
- Make frame-rate and per-pass timings part of the gate, on a quiet machine, so performance regressions show up like parity ones.
- Build Windows packages and the update manifest in CI from a tag, so a tester update is "tag, wait, smoke test, publish".
