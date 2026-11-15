# Retro: issue reports, updates, Discord and packaging

## What went well

- **A useful first slice within hours.** The local report (frame, two saves, log, trace, info)
  landed first and needed no server, so reports were useful before any hosting decision was
  made. Sending, issues and the return trip were layered on as Sera chose the pieces (Discord,
  then her server, then GitHub issues).
- **The secret stayed off testers' machines.** Once the server existed, reports went through
  its forwarder and the downloads carried only a private builds address. No Discord token or
  webhook ever went into a build or the repo.
- **Safe by construction.** After triage pointed out that Sera's real HOME holds the live
  webhook, automated runs were made unable to send. That was better than asking every stream
  to remember a scratch HOME.
- **End-to-end tests in hidden windows** over `--control` caught real bugs before testers did:
  - a preset note lost by a re-polled channel;
  - a "Paused" banner in the screenshot;
  - a child build that made sound and couldn't be driven;
  - movies that would vanish after the first self-update.
- **Small additive PRs into other streams' code** (a trace ring in the sim, pad hooks, report
  wiring), each reviewed by the owner through the coordinator, avoided merge fights.
- **Judging media before posting** kept #media credible: frames whose change didn't read were
  held or swapped for a tighter crop, and labels said which side was which.

## What could have been better

- **I trusted a one-liner's output.**
  - A merge with a conflict was piped through `tail`, and the checks ran on a conflicted tree.
    It was caught before merging, but only by eye.
  - An earlier merge was concluded by an unrelated `git commit`.
  - Scripts that merge and check must stop on a non-zero merge.
- **I claimed a fix that wasn't in the build.** I told Sera the 9173ab3 package had the crowd
  fix when it had a different one. Check the commit's actual contents, or quote the owning
  stream, before describing a build.
- **Time went into dialog details.**
  - The game's text box disables its OK while empty, and a script's box can displace ours. Both
    surfaced in testing and took several rounds.
  - Reading the popup movie's behaviour first, or asking UI earlier, would have been faster.
- **The packaging pipeline grew by hand.** Seven candidates were packaged in a day; a script
  only came after the third. Packaging that will repeat should be scripted on its first run.
- **There was one route for messages.** Everything to other threads went through the
  coordinator, which added latency to small things (QA URLs, review notes).
- **Two loops depend on one Mac staying up.** It slept and moved twice; nothing was lost, but a
  reboot would stop issue sync and media posting silently.

## Lessons

- Put per-machine configuration beside the program or in the data folder, never in the build.
  That one decision made a one-link download possible.
- Make "can this reach the outside world?" a property of how the program is started, not of the
  folder it runs in.
- Test the self-update with a real second build, not just a unit test. The ffmpeg-after-update
  and control-port bugs only showed up then.
- A player-facing caption, update note or README line is a claim about a build. Verify it
  against that build's commit.
- When a box, prompt or button belongs to another stream's code, ask its owner how it behaves
  before building on it.

## How to improve

- Move the two loops (issue sync, media queue) to the test server as a timer, with a narrowly
  scoped GitHub token of Sera's making, so they survive the Mac sleeping or moving.
- Build the Windows packages, the Mac build and `latest.json` in one CI job per tagged commit;
  keep the "publish" switch manual (Sera's go).
- Sign and bundle the Mac build, and add an `.app` with the icon. Use a windowed-subsystem
  Windows exe for testers.
- Give agents a way to message a specific thread directly, and a shared "build ready" channel,
  so QA hears about packages without a relay.
- Add an integration test that runs the report-to-issue-to-fix-to-offer loop against a mock
  server, so changes to `report.rs`, `upload.rs` or `update.rs` can't silently break it.
