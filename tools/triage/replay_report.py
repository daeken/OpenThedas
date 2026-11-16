#!/usr/bin/env python3
"""Replay a player report's saves on a binary, for triage.

Usage: tools/triage/replay_report.py <issue-number> [thedas-app] [--steps "run 3000"]

Reads the issue (gh) for its Discord message id, finds the local report folder whose sent.json
holds that id (REPORTS_DIR, default ~/.openthedas/reports), copies it to
THEDAS_TESTDATA/triage-reports/<N>/ (once), then loads the report's "at report" (save_now) and
"before" (save_before) saves headless on the binary, with the report's own camera, and captures
each (c006-style names: <bin>-now.png, <bin>-before.png, with .log). The gait trace is on in those
logs. Writes verdict.md: the note, the marked spots, the captures and the report's frame-rate
lines, with the verdict left for a person to fill in. Nothing is judged and nothing is sent.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import triagelib as t  # noqa: E402

REPORTS = Path(os.environ.get("REPORTS_DIR", t.HOME / ".openthedas" / "reports"))
REPO = os.environ.get("REPO", "daeken/OpenThedas")


def report_folder(issue: int) -> tuple[Path, dict]:
    raw = subprocess.run(["gh", "api", f"repos/{REPO}/issues/{issue}"], check=True, capture_output=True,
                         text=True).stdout
    data = json.loads(raw)
    m = re.search(r"Discord message `(\d+)`", data.get("body") or "")
    if not m:
        raise SystemExit("no Discord message id in the issue body")
    for sent in REPORTS.glob("*/sent.json"):
        try:
            if json.loads(sent.read_text()).get("message") == m.group(1):
                return sent.parent, data
        except (json.JSONDecodeError, OSError):
            continue
    raise SystemExit(f"no local report in {REPORTS} was sent as message {m.group(1)}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("issue", type=int)
    ap.add_argument("binary", nargs="?", default=str(t.HOME / "projects" / "OpenThedas" / "target" / "release" / "thedas-app"))
    ap.add_argument("--steps", default="run 3000")
    a = ap.parse_args()

    src, issue = report_folder(a.issue)
    dest = t.TESTDATA / "triage-reports" / str(a.issue)
    if not dest.exists():
        shutil.copytree(src, dest)
    report = json.loads((dest / "report.json").read_text())
    camera = report.get("game", {}).get("camera")
    tag = Path(a.binary).name.removeprefix("thedas-app-") or "bin"
    shots = []
    for which in ("now", "before"):
        save = dest / f"save_{which}.json"
        if not save.exists():
            continue
        cap = dest / f"{tag}-{which}.png"
        t.headless(a.binary, a.steps, cap, save=save, camera=camera, size=report.get("settings", {}).get("size", "1600x900"),
                   name=f"replay-{a.issue}", extra_env={"RUST_LOG": "info,thedas_ui::locomotion=trace"})
        shots.append(cap)

    note = (dest / "note.txt").read_text().strip() if (dest / "note.txt").exists() else ""
    regions = report.get("regions") or []
    fps = [l.strip() for l in (dest / "log.txt").read_text(errors="replace").splitlines()
           if "frame rate:" in l] if (dest / "log.txt").exists() else []
    lines = [
        f"# #{a.issue}: {issue.get('title', '')}",
        "",
        f"- Report folder: {src.name} (build {report.get('build', {}).get('commit', '?')[:9]})",
        f"- Area: {report.get('game', {}).get('area', '?')}; camera {camera}",
        f"- Note: {note or '(none)'}",
        *[f"- Marked spot {r.get('n')}: {r.get('note')!r} at {r.get('x'):.0f},{r.get('y'):.0f} "
          f"{r.get('w'):.0f}x{r.get('h'):.0f}" for r in regions],
        f"- Player's screenshot: screenshot.png" + (", marked: screenshot_marked.png" if regions else ""),
        f"- Replayed on {a.binary} ({a.steps}):",
        *[f"  - {p.name} (log {p.name}.log)" for p in shots],
        "- Frame rate in the player's log:" if fps else "- No frame-rate lines in the player's log.",
        *[f"  - {l}" for l in fps[-6:]],
        "",
        "## Verdict",
        "",
        "FIXED / STILL PRESENT / NEEDS INFO: (to fill in)",
        "",
        "Cause guess and stream: (to fill in)",
        "",
    ]
    (dest / "verdict.md").write_text("\n".join(lines))
    print(f"wrote {dest}/verdict.md and {len(shots)} captures")
    return 0


if __name__ == "__main__":
    sys.exit(main())
