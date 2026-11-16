#!/bin/bash
# Focused #1033/#1036 regression, using locally held reports (never bundled here).
# Usage: convhang.sh /absolute/path/to/thedas-app [output-directory]
# THEDAS_TESTDATA and THEDAS_INSTALL may override the local defaults.
set -euo pipefail
if [ "$#" -lt 1 ] || [ ! -x "$1" ]; then
  echo "FAIL: usage: $0 /path/to/thedas-app [output-directory]" >&2
  exit 2
fi
data=${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}
slot=/Volumes/External2/projects/OpenThedas-wt/codex-briefs/cargo-slot.sh
if [ "${CONVHANG_SLOT_HELD:-0}" != 1 ]; then
  if "$slot" "$data/scripts/capped.sh" env CONVHANG_SLOT_HELD=1 "$0" "$@"; then
    exit 0
  else
    rc=$?
    echo "FAIL: convhang run or required resource launcher failed (exit $rc)" >&2
    exit "$rc"
  fi
fi
python3 - "$@" <<'PY'
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile

binary = str(Path(sys.argv[1]).resolve())
data = Path(os.environ.get("THEDAS_TESTDATA", "/Users/daeken/projects/OpenThedas-testdata"))
install = os.environ.get("THEDAS_INSTALL", "/Users/daeken/projects/OpenThedas-wine/game")
base = data / "codex-captures/convhang"
base.mkdir(parents=True, exist_ok=True)
out = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else Path(tempfile.mkdtemp(prefix="repro-", dir=base))
out.mkdir(parents=True, exist_ok=True)
passed = True

for number in (1033, 1036):
    case = out / str(number)
    case.mkdir(exist_ok=True)
    report = data / "triage-reports" / str(number)
    state = json.loads((report / "save_now.json").read_text())
    talk = state["talk"]
    nodes = talk["dlg"]["nodes"]
    # Reconstruct just before the last reply from the retained answered node.
    # Both report replies have no action and no children; no plot rollback is needed.
    replies = nodes[talk["line"]]["children"]
    journal = [json.loads(line) for line in (report / "journal.jsonl").read_text().splitlines()]
    choice = [entry["command"]["choice"] for entry in journal
              if entry.get("command", {}).get("command") == "choose_reply"][-1]
    assert talk["ended"] and not talk["replies"] and len(replies) == 1 and choice == 0
    reply = nodes[replies[choice]]
    assert reply["reply"] and not reply["children"]
    assert not reply["action"].get("plot") and not reply["action"].get("script")
    talk["ended"] = False
    talk["replies"] = replies
    world = state["world"]
    world["game_mode"] = 4
    world["mode_before_talk"] = 0
    # Discard historical commands so the output proves a new move was accepted.
    world["command_journal"] = []
    leader = world.get("controlled") or world["hero"]
    initial_position = world["objects"][str(leader)]["position"]
    initial_time = world["time_ms"]
    fixture = case / "before-reply.json"
    fixture.write_text(json.dumps(state))
    home = Path(tempfile.mkdtemp(prefix="home-", dir=case))
    steps = "run 1; reply 0; run 250; pad stick-left 1,0 250; quicksave"
    steps += "; run 250" * 48
    env = dict(os.environ, HOME=str(home), THEDAS_HIDDEN_WINDOW="1",
               THEDAS_MODS="none", THEDAS_NO_AUDIO="1", RUST_LOG="info")
    # Inherited user overrides must not mount resources outside the scratch HOME.
    env.pop("THEDAS_USER_DIR", None)
    env.pop("THEDAS_MOD_LIST", None)
    command = [binary, "--install", install, "--save", str(fixture),
               "--capture", str(case / "settled.png"), "--size", "960x540",
               "--steps", steps]
    with (case / "run.log").open("w") as log:
        result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=480)
    log = (case / "run.log").read_text()
    samples = re.findall(r"run 250: screens (\{[^}]*\})", log)
    for index, screens in enumerate(samples):
        elapsed = 250 if index == 0 else (index + 2) * 250
        print(f"#{number} +{elapsed:05}ms screens={screens}")
    saves = [p for p in (home / ".openthedas/saves").glob("*.json") if not p.name.endswith(".meta.json")]
    moves = []
    for path in saves:
        saved = json.loads(path.read_text())
        current = saved["world"]
        print(f"#{number} +00500ms mode={current.get('game_mode')} "
              f"talk={None if saved['talk'] is None else {k: v for k, v in saved['talk'].items() if k in ('line', 'ended', 'replies')}} "
              f"pending_talks={len(current.get('pending_talks', []))} "
              f"cutscene={current.get('cutscene')} dialog_cutscenes={current.get('dialog_cutscenes', [])}")
        moves.extend(entry for entry in saved["world"].get("command_journal", [])
                     if entry["command"]["command"] == "order_move"
                     and entry["command"]["who"] == leader
                     and entry["t"] > initial_time
                     and entry["outcome"] == {"outcome": "done", "ok": True})
    # A nonzero stick order targets a point ahead, rather than a stop at the feet.
    nonzero = any(all(math.isfinite(x) for x in entry["command"]["to"])
                  and math.dist(entry["command"]["to"], initial_position) > 0.1
                  for entry in moves)
    ok = result.returncode == 0 and bool(samples) and samples[0] == "{Hud}" and nonzero
    print(f"{'PASS' if ok else 'FAIL'}: #{number}, HUD by 250ms, leader move by 500ms: {len(moves)} accepted move orders")
    passed &= ok

print(f"{'PASS' if passed else 'FAIL'}: convhang; evidence: {out}")
sys.exit(0 if passed else 1)
PY
