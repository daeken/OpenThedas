# Issue reports, updates, Discord and tester packaging

The stream that built the player-facing loop around the game: one-press issue reports, the way
they reach the project (Discord, GitHub issues), the self-update that brings a fix back to the
reporter, the private test-build server, packaging of tester and QA builds, and the project's
Discord presence (Barkspawn, #updates, #media).

Code lives in `crates/thedas-app` (`report.rs`, `upload.rs`, `update.rs`, and the report glue in
`main.rs`). Everything else lives outside the repo: scripts in `OpenThedas-testdata/scripts/`,
state and drop folders in `OpenThedas-testdata/report/` and `OpenThedas-testdata/media-queue/`,
and an nginx site on the test server. Paths below are relative to the folder holding
`OpenThedas-testdata` on Sera's Mac (`/Volumes/External2/projects/`).

## The loop, end to end

1. **Report.** F8, both pad sticks clicked in, or the game menu's Report Issue calls
   `App::report`. The game pauses once the frame has been read back, so the screenshot has no
   "Paused" banner. A report folder `reports/<date>-<area>/` in the data folder holds:
   - `screenshot.png`, and `screenshot_marked.png` when the player marked spots;
   - `save_now.json`, plus `save_before.json` (about 50-70 s back) from an in-memory ring of
     snapshots taken every 20 s, 4 kept. Snapshots skip training lessons, chargen and loads, and
     never call PRESAVE: the ring only reads the sim;
   - `report.json`: build commit (from `build.rs`), GPU, window, settings, install add-ins,
     case-sensitivity, mod layers (`player_layers`), status, coverage counters, frame stats,
     snapshot ages and marked `regions`;
   - `log.txt`: the last 400 log lines, from a tee around env_logger;
   - `input.txt`: the last 120 s of input;
   - `trace.json`: `Trace::recent` (the sim's last 4000 command, log, dialog, plot, failed-script
     and missing entries);
   - `journal_checkpoint.json` and `journal.jsonl`: the sim stream's journal, which replays the
     session exactly;
   - `note.txt`.
2. **Mark spots** (mouse and pad). After F8 the player is offered "Mark spots", drags a box
   around each problem and types a note for it. Each spot's note is followed by "Mark another
   spot" / "Done marking"; Done goes straight to the overall note. F8, Enter or Escape (B or
   Start on the pad) also finishes while drawing, dropping an unfinished box. The pad half
   (pointer, A at each corner) is UI's #642.
3. **Note.** Asked in the game's own message box (`UiLayer::ask_free_text`/`ask_choice`).
   Report and spot notes opt out of the movie's 20-character name-field limit and restriction;
   ordinary `ask_text` and the game's name prompts keep their original settings.
   - Either button keeps the report; the second, "Keep here", keeps it unsent.
   - Every box we ask is asked again when a script's box displaces it, and given up after about
     5 s, so the game never waits on it.
4. **Send** (`upload.rs`, only when set up and not in an automated run, see below).
   - The message is the note, the place, build, OS, GPU, mod-layer count and the marked notes.
   - Attached: the screenshot (the marked copy if any) and `<id>.zip` with everything else.
   - Home-folder paths become `~` and the install path is dropped.
   - The total is kept under Discord's 10 MB. Order of giving way: the screenshot first goes to
     half size, then the journal is left out (`journal_left_out` in the uploaded report.json),
     then the screenshot is dropped.
   - `sent.json` in the folder records the Discord message id.
5. **Track.** `report_sync.py` runs every 3 minutes on the Mac.
   - It opens a GitHub issue labelled `player-report` for each new game report in
     #bugs-and-feedback. Images over 5 MB don't show on GitHub, so the issue gets a JPEG of the
     screenshot; the PNG and zip are copied to `https://openthedas.daeken.dev/r/<random>/`.
   - Barkspawn replies "Tracked as <url>" under the report.
   - Notes containing "please ignore" are skipped (test reports).
6. **Fix.** A PR that says `Fixes #N` closes the issue. The next sync run calls
   `report_fixed.py`, which records the closing commit in the fix ledger and has Barkspawn reply
   "we think this is fixed".
7. **Publish.** `publish_build.py` uploads a build and rewrites `latest.json`. Each build lists
   the report message ids whose fix commit is in its history (git ancestry against the ledger).
8. **Return trip** (`update.rs`).
   - At start, the game reads `latest.json` against the reports this machine sent. If the
     newest build fixes one and isn't this build, it offers "Update" or "Later", then "Where I
     am" or "Where I reported".
   - "Where I am" saves through PRESAVE into the save list too; "Where I reported" uses the
     report's `save_before.json`.
   - The build downloads into `builds/<commit>/` in the data folder, with sha256 and file names
     checked; a running exe is never replaced.
   - The new build starts with `--save --check-report <dir>`, keeping this run's `--no-audio`
     and `--control`. It asks "Is the problem fixed?" (a choice), then for an optional note,
     posts the verdict, and writes `checked.json`.
   - `builds/current.json` makes the program the player downloaded hand over to the newest
     fetched build on every later start. `THEDAS_PROGRAM_DIR` is handed down so files placed
     beside the download are still found.

## Per-machine setup files (in the data folder, or beside the program)

Never built in; read by `upload::setup_file` (the data folder first, else beside the exe, then
copied into the data folder):

- `update-url`: the test server's private builds address (`https://openthedas.daeken.dev/t/<token>/`).
  Tester packages carry it beside the exe. With no `discord-webhook`, reports go to `report`
  under this address, which the server forwards to the channel webhook, so a download holds no
  Discord secret.
- `discord-webhook`: a direct webhook address. Only Sera's Mac has one
  (`~/.openthedas/discord-webhook`); testers don't need it.

**Automated runs never send or fetch.** `--control` or `THEDAS_HIDDEN_WINDOW` sets
`upload::set_automated()`; `THEDAS_REPORT_UPLOAD=1` is the explicit opt-in for the sending
tests. Keep it that way: Sera's real home has the live webhook.

## Secrets: where they live (no values here)

| What | Where | Notes |
| --- | --- | --- |
| Barkspawn bot token | `~/projects/Barkspawn/discord.token` (Sera's folder) | read at run time by `discord.py` only; never print, copy or commit it |
| Report webhook address | `~/.openthedas/discord-webhook` (mode 600), and inside the server's nginx conf | the nginx conf is root-only (600) |
| Test builds address (token path) | `~/.openthedas/update-url` (mode 600); in each tester package as `update-url` | private to the 3-person test group; posted only in their Discord |
| QA download addresses | `OpenThedas-testdata/report/for-qa/windows-<commit>[-arm64]-url.txt` (mode 600) | handed to QA by file name, never pasted in messages |
| GitHub | Sera's `gh` login on the Mac | issues, labels, PRs |

## Discord

Server channels, used by Barkspawn:
- #bugs-and-feedback `1570565921259655250`: reports and testers' feedback.
- #updates `1570653965060677712`: one-line "what changed" notes and build announcements.
- #media `1570565948006736064`: screenshots.

`OpenThedas-testdata/scripts/discord.py` (stdlib only, `python3 -I`):
`post {bugs|updates|media|<id>} "text" [--file F] [--reply-to ID]`,
`read CHANNEL [--limit N] [--after ID]`, `attachment URL OUT`, and
`webhook CHANNEL NAME OUT_FILE` (creates a webhook and writes its address only to the file).
Mentions are disabled on every post. The bot has Message Content Intent (Sera enabled it); without
it, reads come back blank.

Only this stream posted, under Sera's standing approval (2026-11-13 04:27Z: post to the three
channels, create channels and webhooks as needed). Other streams read only.

**Media queue.** Streams drop `<name>.png|.gif|.jpg` plus `<name>.txt` (the caption) into
`OpenThedas-testdata/media-queue/<stream>/`. `report/media_loop.sh` runs `media_post.py` every
30 s: it posts a pair once both files have been unchanged for 15 s, downscales to JPEG over
10 MB, and moves the pair to `<stream>/posted/` with the message id. Rules are in
`media-queue/README.txt`: our frames, labelled sides, no paths or secrets, and no standalone
frames of the original. Before the queue existed, frames came by message and were checked first.
Some were held (the DA2 olive-fog frame) or skipped when the change didn't read at chat size.

## Test server (`storage2.daeken.dev`, reached as `openthedas.daeken.dev` through Cloudflare)

- nginx site: `/etc/nginx/conf.d/openthedas.conf` (root 600, holds the webhook address).
- Web root: `/srv/openthedas/www`.
- `/t/<token>/latest.json` and `/t/<token>/builds/<commit>/openthedas-<platform>.zip`: builds;
  platforms are `windows-x86_64`, `windows-aarch64` and `macos-aarch64`.
- `POST /t/<token>/report`: forwards to the webhook, rate-limited to 6 a minute per address.
- `/r/<random>/`: report attachments linked from GitHub issues.
- Everything else answers 404, and the access log is off.
- Removed test paths were moved to `/srv/openthedas/removed-*`, not deleted; they can be deleted.
- `latest.json` now lists 8137c5d (Windows x64, ARM64, Mac), the first published tester build.

## Packaging and publishing

Windows builds come from triage's cloud cross-builder (cargo-zigbuild, llvm-windres for the
icon) as `windows-build/<commit>[-arm64]/thedas-app.zip.part-*` with hashes in `README.txt`.
They are split into 8 MB parts because the project-file fetch refuses larger responses. The Mac
has no Windows toolchain, and installing one needs Sera's OK. The Mac build is made here from the
exact commit in a detached worktree (`OpenThedas-wt/report-mac`, target `report-target-mac`).

- `scripts/package_windows.sh SHORT ZIP_SHA PARTS...` (`PLATFORM=windows-aarch64 SUFFIX=-arm64`
  for ARM) joins and verifies the parts, stages `report/win-<short>/files` (exe, the kit
  README, `update-url`), uploads with `--no-latest`, and writes the QA URL file.
- `scripts/publish_build.py COMMIT PLATFORM FILES_DIR EXE [--no-latest] [--dry-run]` zips,
  uploads, and (without `--no-latest`) rewrites `latest.json`, keeping the 10 newest builds and
  merging platforms that share a commit.
- Kits: `report/kit/README.txt` (Windows, CRLF) and `report/kit-mac/README.txt`.
  - Since #657 both say to start `thedas-app` directly; the launcher finds the install.
  - Since #663 (native Bink) there is no ffmpeg section.
  - The pre-launcher kit, with its `.bat`/`.command`, is in `report/kit-old/` for the
    published 8137c5d build.
- Mac packages are unsigned: first launch is right-click, then Open. They need Apple silicon.
- Publishing to testers needs Sera's word, after QA's grade and a Windows VM smoke test. QA
  packages are private and never touch `latest.json`.
- Drafted, not posted: notes for the first update, at `report/update-notes-draft.txt`.

## Running loops on the Mac (die on reboot; restart with `nohup ./<loop>.sh &` in `report/`)

- `sync_loop.sh`: `report_sync.py` every 3 minutes. State: `report/issues.json` (message id to
  issue) and the fix ledger `report/fixes.json`. Log: `sync.log`.
- `media_loop.sh`: `media_post.py` every 30 s. Log: `media.log`.
- PIDs are in `*.pid`. Kill by pid only.

## How to test this area

Use a scratch HOME and a control port of your own; this stream used 7900-7909.

### Replay an F8 report

`thedas replay <report-folder-or-zip>` reads `journal_checkpoint.json` and every input in
`journal.jsonl`, checks the intermediate world hashes, then compares the complete saved state
with `save_now.json`. A mismatch prints the first differing JSON pointer (including object ID,
field and tag when available). It stops at the first intermediate hash divergence and retains
that actual state for inspection. It never modifies or extracts the report.

```sh
thedas replay captures/report --install "$GAME" --save captures/replayed.json
thedas replay captures/report.zip --install "$GAME"
thedas replay captures/report --install "$GAME" --until 120000 --save captures/early.json
thedas-app --install "$GAME" --save captures/replayed.json
```

- The install defaults to `THEDAS_INSTALL`, then the path recorded in `report.json` (uploaded
  reports redact that path, so normally pass `--install`). Use the same game assets, add-ins,
  keys and mod layers as the reporter. Reports do not contain those resources. Use a scratch
  HOME for automated runs, and `THEDAS_HIDDEN_WINDOW=1` for any engine window.
- The running CLI's commit must equal the journal header's build; refusal prints both.
  There is no bypass. To investigate an older report, check out its commit and transplant only
  the replay CLI files/registration/build script, zip dependency and the additive
  `thedas_sim::replay::load_checkpoint` helper, leaving its existing simulation, script,
  resource and save logic intact. Build there; do not relabel a current binary as old.
- `--until` is **absolute world time in milliseconds**, not elapsed time since the checkpoint.
  It stops before the first input that would exceed the limit, including within a repeat;
  it never shortens a tick. Commands at the same time still run (cutscenes can hold world time).
  A limit earlier than the checkpoint is refused.
- Exit **0** means the complete F8 saved state matched; **1** means a differing state,
  intermediate hash divergence, or an early stop before F8; **2** means replay was impossible
  (build/revision/checkpoint mismatch, missing files/resources, malformed/truncated log, bad zip
  or failed output write). An uploaded report whose journal was left out cannot replay.
- `--save` writes the reached state even on divergence or an early stop, so the app can open
  it with `--save`. Rendering remains the app's job; replay itself needs no GPU or control port.
- Zips can have flat contents or one enclosing report folder. Paths are validated and each
  selected file is limited to 128 MiB after decompression; ambiguous or duplicate files fail.
- Synthetic unit fixtures only: `cargo test -p thedas-cli replay::`. They exercise checkpoint
  serialization, repeated inputs, strict log validation, build refusal, first hash divergence,
  early stopping, field differences outside the world hash, folder/zip parity and unsafe zips.
  For an end-to-end synthetic report, run the app in a scratch HOME with `--journal` plus
  `--control`, play a few ticks, request `report` with a test note, then replay the resulting
  folder and zip using a CLI built at that same commit.

### Report and updater checks

- `report/drive_report.py APP SAVE [PORT] [RUN_MS]`: report files; `POPUP=1` for the typed note;
  `THEDAS_REPORT_UPLOAD=1` plus a copied `discord-webhook` or `update-url` to really send.
- `report/drive_mark.py APP SAVE`: marking two spots. "Mark spots" sits at physical 1054,832 in
  a 1280x720 hidden window.
- `report/drive_update.py APP SAVE` and `report/drive_child.py REPORT_DIR`: the return trip,
  against a throwaway server path whose `latest.json` names a fake commit that "fixes" a fake
  sent report.
  - The child must be a build at or after 01d9537, because the control-port retry lives in the
    child.
  - Remove the path afterwards.
- Unit tests: `cargo test -p thedas-app report:: upload:: update::`. They cover the ring,
  regions, the zip writer and reader, redaction, offers, sent-report scanning and the message
  format.
- Steps: `key f8` drives a report from `--steps` and `--control`; the `report` control command
  takes `"note"`.

## Do not break

- Snapshots and reports only read the sim. Don't call `presave` from the ring, and respect
  `saving_blocked`.
- Automated runs (`--control`, hidden window) must never send or fetch.
- No secret, token path, webhook or user name in the repo, logs, builds or Discord posts.
  Tester packages carry only `update-url`.
- `update-url` and `discord-webhook` are per machine, never built in.
- `latest.json` changes only on Sera's go.
- The updater never replaces a running exe, and checks hashes and names before extracting.
- Keep `Trace::recent` off by default in the sim (the app turns it on), so tools and gates are
  unchanged.

## Known gaps and ideas

- The screenshot readback and `save_json` run on the frame thread (about 20 ms per snapshot on a
  big save). Serializing on a worker would remove the hitch.
- The update offer needs the HUD's popup layer; before a game it waits.
- The Windows exes are console-subsystem, so a log window opens beside the game.
- Mac packages are unsigned and Apple silicon only; there is no app bundle (the icon is set at
  run time).
- `report_sync.py` uses main's head as the fix commit when GitHub names none (for example an
  issue closed by hand). That can over-claim fixes.
- No automated test covers the Discord or GitHub side; it was verified by hand with test
  reports.
