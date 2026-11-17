#!/bin/bash
# Usage: twohander.sh /path/to/thedas-mcp [output-directory]
set -euo pipefail
data=${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}
if [ "$#" -lt 1 ] || [ ! -x "$1" ]; then
    echo "Usage: $0 /path/to/thedas-mcp [output-directory]" >&2
    exit 2
fi
if [ "${TWOHANDER_SLOT_HELD:-0}" != 1 ]; then
    exec /Volumes/External2/projects/OpenThedas-wt/codex-briefs/cargo-slot.sh \
        "$data/scripts/capped.sh" env TWOHANDER_SLOT_HELD=1 "$0" "$@"
fi
python3 - "$@" <<'PY'
import json
import os
from pathlib import Path
import sys

root = Path(os.environ.get('THEDAS_TESTDATA', '/Users/daeken/projects/OpenThedas-testdata'))
sys.path.insert(0, str(root))
import drive as d

out = Path(sys.argv[2]) if len(sys.argv) > 2 else root / 'codex-captures/twohanderarena/twohander'
out.mkdir(parents=True, exist_ok=True)
source = root / 'triage-reports/1102/save_now.json'
w = json.loads(source.read_text())['world']
hero = w['hero']
greatsword = w['objects'][str(hero)]['equipment']['0']
dagger = w['objects'][str(hero)]['equipment']['1']
d.start(str(Path(sys.argv[1]).resolve()))
try:
    d.call('load')
    d.call('load_state', path=str(source))
    d.eng('UnequipItem', hero, greatsword)
    result = d.eng('EquipItem', hero, greatsword, 0, -1)
    d.call('save_state', path=str(out / 'after.json'))
    after = json.loads((out / 'after.json').read_text())['world']['objects'][str(hero)]
    ok = after['equipment'].get('0') == greatsword and '1' not in after['equipment'] and dagger in after['inventory']
    print(('PASS' if ok else 'FAIL') + ': #1102 script equipping Everd\'s greatsword removes the dagger; ' + str(after['equipment']))
    if not ok:
        raise SystemExit(1)
finally:
    d.stop()
PY
