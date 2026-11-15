# Coordinator retro

**Went well:** clear ownership plus announce-before-merge kept parity and determinism intact across ~800+ PRs. The traced-to-the-exe rule plus the gate (s01-s16, detcheck, saves) made "is this a regression?" answerable. Codex as a reviewed worker multiplied throughput cheaply. The media pipeline kept Sera in the loop visually.

**Could've been better:** usage burn. A week's worth went in about a day, mostly from many parallel Opus sessions re-reading context. Relay overhead: every shared-file change cost round trips through me. Remote sessions stop whenever the Mac sleeps. Permission friction: the Mac's auto-mode check blocked actions Sera had approved via me, because approvals had to be written in-thread. Stacked PRs occasionally merged into the wrong base (#772). Codex once posted to Discord directly.

**Lessons:** put durable rules in the repo (CLAUDE.md, gates, gaps list), not in chat memory. Make owners and approval paths explicit up front. Give cheap, well-specified work to cheaper agents, and keep reviewers on the expensive model. Require evidence (trace address, gate diff explained) in every PR.

**Improve:** let streams talk to each other directly, with a shared announce channel instead of hub-and-spoke. Write a one-time standing permission policy per action class. Use a shared PR queue with machine-checkable gate results. Pick an always-on build host rather than the laptop. Write budget-aware scheduling into the plan.
