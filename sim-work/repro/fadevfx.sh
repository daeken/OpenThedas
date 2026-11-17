#!/bin/bash
# Run through cargo-slot.sh and testdata/scripts/capped.sh (one MCP at a time).
# Usage: THEDAS_MCP=/path/to/thedas-mcp ./sim-work/repro/fadevfx.sh [fade-start.json]
set -euo pipefail
export THEDAS_TESTDATA="${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}"
export THEDAS_INSTALL="${THEDAS_INSTALL:-/Users/daeken/projects/OpenThedas-wine/game}"
: "${THEDAS_MCP:?Set THEDAS_MCP to the candidate binary}"
SAVE="${1:-$THEDAS_TESTDATA/codex-captures/playharness/20261116-124242-743654/saves/fade-start.json}"
python3 - "$SAVE" <<'PY'
import os, sys, tempfile
sys.path.insert(0, os.environ['THEDAS_TESTDATA'])
import drive as d

def inspect(object_id):
    return d.j('inspect_object', object=str(object_id))

def check(object_id, label):
    o = inspect(object_id)
    effects = [e for e in o['effects'] if e['ty'] == 4 and e['ints'][:1] == [3006]]
    assert o['locals']['ints'].get('CREATURE_SPAWNED') == 1, (label, 'not spawned')
    assert len(effects) == 1, (label, 'expected one VFX 3006', effects)
    e = effects[0]
    assert e['creator'] == object_id and e['visual_target'] == object_id, (label, e)
    assert e['duration_type'] == 5 and e['expires'] is None, (label, e)
    print(f'PASS {label}: wisp {object_id} has lasting VFX 3006', flush=True)
    return e

d.start()
try:
    d.call('load', install=os.environ['THEDAS_INSTALL'])
    d.call('load_state', path=os.path.abspath(sys.argv[1]))
    wisps = d.j('list_objects', tag='bhm600cr_wispwraith', kind='creature')
    assert wisps, 'fade-start save has no wisps'
    for w in wisps:
        check(w['id'], 'older-save repair')
    # A new template instance receives its queued SPAWN, without editing the save
    # or clearing a spawn guard. This independently exercises the application fix.
    fresh = d.eng('CreateObject', 32, 'bhm600cr_wispwraith', {'near': wisps[0]['id']}, '', 1, 0)['object']
    d.call('step', ticks=1)
    expected = check(fresh, 'fresh SPAWN')
    with tempfile.TemporaryDirectory(prefix='fadevfx-') as tmp:
        path = os.path.join(tmp, 'roundtrip.json')
        d.call('save_state', path=path)
        d.call('load_state', path=path)
        assert check(fresh, 'save/reload') == expected
    print(f'PASS fadevfx: {len(wisps)} repaired wisps + fresh spawn + save/reload', flush=True)
except Exception as error:
    print(f'FAIL fadevfx: {error}', flush=True)
    raise
finally:
    d.stop()
PY
