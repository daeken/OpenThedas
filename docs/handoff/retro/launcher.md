# Launcher stream retro

## What went well

- The launcher was split into a UI-free crate (`thedas-launcher`), a store crate (`thedas-store`) and the egui window. Detection, the mod list editor and the downloaders were tested with synthetic folders and recorded fixtures, so almost nothing needed the game or an account to check.
- Sera's credential rule was written at the top of the store crate before any code, and the design followed it: `Secret` type, log caps in one place, no library cache or token store used. It made review quick.
- Making the launcher scriptable through its accessibility tree paid for itself twice. QA got a way to drive it on Windows, and the same hook became the regression suite. Writing the first-run script found a real wizard bug (picking a folder dropped the installs the search had found) within minutes, which a code read had missed.
- Driving the real window under Xvfb (software rendering) found problems no unit test would: Tab order reversed by a right-to-left layout, focus lost on every page change, unnamed controls, nav tabs in reverse reading order.
- Asking Sera early about the one real policy question (committing GOG's public OAuth pair) with a decision card got an answer in a minute and avoided a rewrite.
- Standing permission to merge green PRs kept the stream from waiting on anyone.

## What could have been better

- I verified on Linux and checked macOS only with `cargo check`. The asks were about macOS and Windows behaviour (VoiceOver, UI Automation). I should have asked for a Mac and VM run slot earlier in the stream, instead of leaving it as the last item that usage limits cut off.
- The store downloads shipped without a single real sign-in. I had no account to use, and the one provisional piece (GOG `secure_link`) rests on public descriptions. That is a known risk, not a surprise, but it should have been flagged in the first PR title, not only in docs.
- A few mistakes cost time: wrong egui feature (`accesskit` is an egui-winit feature), first-frame scripted clicks hitting an empty tree, and a stray heredoc that hung a command and left a file at a wrong path. Smaller steps with a build between them would have caught each sooner.
- I read `launcher_mods.rs` as a diff only when reviewing #732; I did not run it. A running review would have found the stale-report and per-frame filtering problems myself, not as suggestions.
- Replies to the coordinator were sometimes longer than needed while the person was away; short status lines would have saved attention.

## Lessons learned

- If a UI is meant to be accessible, build its test hook on the accessibility tree from the start. Accessibility, scripting and regression testing become one feature.
- Tab order in an immediate-mode UI is draw order. Layout constraints (panels that must come first) fight reading order; the reserved-rect-then-draw-later trick solves it, but put it in on day one.
- Say what is unverified, per platform, in the PR body and in the docs, with the reason. It kept the handoff honest.
- Read the file or the screenshot, not just the diff. Two of my fixes (nav tab order, the footer height) were only visible in screenshots.
- Anything that touches secrets deserves a short list of "where could this end up" (logs, panic messages, screenshots, reports, caches) written before the code.

## How to improve

- Give each stream a standing, scheduled slot on the Mac and Windows VM for its platform checks, so cross-platform verification is not left for the end.
- Keep QA scripts next to the feature from the first PR, not as a last step. Run them in the merge gate for the launcher.
- Get one real Steam and one real GOG account run (by Sera or a tester) before telling players the download works; track the result in `docs/KNOWN_GAPS.md`.
- Put the unverified-on-real-hardware list in each PR's first lines, so reviewers see it before the diff.
- When another stream adds a page or section to a surface you own, agree the naming and focus conventions in the PR before it lands, instead of a follow-up (this worked for #732 and should be the norm).
