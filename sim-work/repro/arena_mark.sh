#!/bin/bash
# Usage: arena_mark.sh /path/to/thedas-mcp [output-directory]
set -euo pipefail
data=${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}
if [ "$#" -lt 1 ] || [ ! -x "$1" ]; then
    echo "Usage: $0 /path/to/thedas-mcp [output-directory]" >&2
    exit 2
fi
if [ "${ARENA_MARK_SLOT_HELD:-0}" != 1 ]; then
    exec /Volumes/External2/projects/OpenThedas-wt/codex-briefs/cargo-slot.sh \
        "$data/scripts/capped.sh" env ARENA_MARK_SLOT_HELD=1 "$0" "$@"
fi
python3 - "$@" <<'PY'
import json
import math
import os
from pathlib import Path
import sys

root = Path(os.environ.get('THEDAS_TESTDATA', '/Users/daeken/projects/OpenThedas-testdata'))
sys.path.insert(0, str(root))
import drive as d

out = Path(sys.argv[2]) if len(sys.argv) > 2 else root / 'codex-captures/twohanderarena/arena-mark'
out.mkdir(parents=True, exist_ok=True)
source = root / 'triage-reports/1103/save_before.json'
d.start(str(Path(sys.argv[1]).resolve()))
try:
    d.call('load')
    d.call('load_state', path=str(source))
    # Re-enter the reported script boundary with the same world and actors.
    d.eng('SetPartyPlotFlag', 'player', '8A40B9BC907549CABC73786832385BC1', 1, 1, 1)
    d.step(1)
    for _ in range(12):
        d.call('save_state', path=str(out / 'staged.json'))
        state = json.loads((out / 'staged.json').read_text())
        if (state.get('talk') or {}).get('staged'):
            break
        dialog = d.j('dialog_state')
        if dialog.get('ended'):
            break
        d.call('choose_reply', reply=0)
        d.step(1)
    d.call('save_state', path=str(out / 'staged.json'))
    staged = json.loads((out / 'staged.json').read_text())
    w = staged['world']
    hero = w['objects'][str(w['hero'])]
    opponents = [o for o in w['objects'].values() if o['tag'] == 'bdc140cr_mainar' and o['active'] and o['area'] == hero['area']]
    assert len(opponents) == 1, opponents
    opponent = opponents[0]
    talk = staged.get('talk') or {}
    bindings = talk.get('staged', [])
    bound = next((b for b in bindings if b['object'] == opponent['id']), None)
    # The script's jump is correct. The conversation must take that same opponent,
    # rather than the inactive waiting-room copy, to leave him on the duel mark.
    ok = bound is not None and not bound['restore'] and bound['end'] is not None
    owner = w['objects'].get(str(talk.get('owner')), {})
    ok = ok and owner.get('area') == hero['area']
    print(('PASS' if ok else 'FAIL') + ': #1103 active arena opponent is staged on the duel mark; '
          + f"opponent={opponent['id']}, bound={[b['object'] for b in bindings]}, owner={talk.get('owner')}")
    if not ok:
        raise SystemExit(1)
    # Finish the conversation; release must move the live opponent to its authored pose.
    expected = bound['end'][0]
    for _ in range(30):
        state = d.j('dialog_state')
        if state.get('ended'):
            break
        d.call('choose_reply', reply=0)
        d.call('save_state', path=str(out / 'after.json'))
        actual = json.loads((out / 'after.json').read_text())['world']['objects'][str(opponent['id'])]['position']
        if math.dist(actual[:2], expected[:2]) < 0.1:
            break
    d.call('save_state', path=str(out / 'after.json'))
    after = json.loads((out / 'after.json').read_text())
    actual = after['world']['objects'][str(opponent['id'])]['position']
    ok = math.dist(actual[:2], expected[:2]) < 0.1
    print(('PASS' if ok else 'FAIL') + f': duel release expected={expected}, actual={actual}')
    if not ok:
        raise SystemExit(1)
finally:
    d.stop()
PY
