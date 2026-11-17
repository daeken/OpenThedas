#!/bin/bash
# MCP: apply stun, kill the creature, inspect effects/flags/Dexterity, save and reload.
# Usage: stundeath.sh /path/to/thedas-mcp [output-directory]
set -euo pipefail
data=${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}
if [ "$#" -lt 1 ] || [ ! -x "$1" ]; then
    echo "Usage: $0 /path/to/thedas-mcp [output-directory]" >&2
    exit 2
fi
if [ "${STUNDEATH_SLOT_HELD:-0}" != 1 ]; then
    exec /Volumes/External2/projects/OpenThedas-wt/codex-briefs/cargo-slot.sh \
        "$data/scripts/capped.sh" env STUNDEATH_SLOT_HELD=1 "$0" "$@"
fi
python3 - "$@" <<'PY'
import json
import os
from pathlib import Path
import re
import struct
import sys

root = Path(os.environ.get('THEDAS_TESTDATA', '/Users/daeken/projects/OpenThedas-testdata'))
sys.path.insert(0, str(root))
import drive as d

out = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else root / 'sim-work/stundeath/repro'
out.mkdir(parents=True, exist_ok=True)
source = Path(os.environ.get('STUNDEATH_SAVE', str(root / 'triage-reports/1103/save_now.json')))
world = json.loads(source.read_text())['world']
hero = world['hero']
victim = next(o['id'] for o in world['objects'].values()
              if o['tag'] == 'bdc140cr_mainar' and o['active'] and not o['dead'])
# MCP's scalar API cannot take effect handles. Mount two independently authored
# fixture scripts in a private mod list, then run them through MCP and the real
# APPLY_EFFECT/REMOVE_EFFECT handlers. No resources in the install are changed.
fixtures = out / 'fixtures'
fixtures.mkdir(exist_ok=True)
ldf = (root / 'script.ldf').read_text(encoding='latin-1')
def action(name, argc):
    fid = int(re.search(r'\b' + name + r'\s*\([^;]*?\)\s*=\s*(\d+)\s*;', ldf).group(1))
    return bytes([5, 0]) + struct.pack('>H', fid) + bytes([argc])
def const(ty, value):
    return bytes([4, ty]) + struct.pack('>f' if ty == 4 else '>I', value)
def script(name, effect, duration, seconds, stun=False):
    # An environmental caster bypasses the between-duels neutrality check without
    # starting the whole Proving's AI fight during run_script's settling tick.
    code = const(3, 0) + const(6, world['area']) + const(4, seconds) + const(6, victim)
    if stun:
        code += const(3, 1009) + const(3, 2)
    code += const(3, effect) + action('Effect', 1)
    if stun:
        code += action('SetEffectEngineInteger', 3)
    code += const(3, duration) + action('Engine_ApplyEffectOnObject', 6) + bytes([0x20, 0])
    (fixtures / (name + '.ncs')).write_bytes(b'NCS V1.0B' + struct.pack('>I', 13 + len(code)) + code)
script('stundeath_stun', 1027, 2, 120.0, stun=True)
script('stundeath_kill', 7, 1, 0.0)
mod_list = out / 'mods.txt'
mod_list.write_text(str(fixtures) + '\n')
os.environ['THEDAS_MODS'] = str(mod_list)
d.INSTALL = os.environ.get('THEDAS_INSTALL', '/Users/daeken/projects/OpenThedas-wine/game')
d.start(str(Path(sys.argv[1]).resolve()))
try:
    d.call('load')
    d.call('load_state', path=str(source))
    # Complete queued AREALOADSAVE handlers before applying the test effect.
    d.j('step', ticks=1, tick_ms=1)
    d.j('watch_object', object=str(victim))
    def snapshot(label):
        obj = d.j('inspect_object', object=str(victim))
        result = {
            'dead': obj['dead'], 'effects': obj['effects'],
            'flags': d.eng('GetEffectsFlags', victim),
            'dexterity_modifier': d.eng('GetCreatureProperty', victim, 2, 4),
        }
        (out / (label + '.json')).write_text(json.dumps(result, indent=2) + '\n')
        d.call('save_state', path=str(out / (label + '-save.json')))
        return result
    before = snapshot('before')
    result = d.j('run_script', script='stundeath_stun', caller=str(hero))
    (out / 'apply-trace.json').write_text(json.dumps(result, indent=2) + '\n')
    stunned = snapshot('stunned')
    assert not stunned['dead'], 'target died before the killing step'
    assert any(e['ty'] == 1027 for e in stunned['effects']), 'stun was not applied'
    assert stunned['flags'] & 543 == 543, 'stun flags were not set'
    assert any(e['ty'] == 1027 and [2, 1009] in e['engine_ints'] for e in stunned['effects'])
    assert stunned['dexterity_modifier'] == before['dexterity_modifier'] - 1000.0
    result = d.j('run_script', script='stundeath_kill', caller=str(hero))
    (out / 'kill-trace.json').write_text(json.dumps(result, indent=2) + '\n')
    dead = snapshot('dead')
    d.call('load_state', path=str(out / 'dead-save.json'))
    restored = snapshot('reloaded')
    for state in (dead, restored):
        assert state['dead'], 'target did not die'
        assert not any(e['ty'] == 1027 for e in state['effects']), 'corpse still has stun'
        # A retained native death effect can legitimately disable control too.
        # An empty list, however, must leave no cached aggregate flags behind.
        if not state['effects']:
            assert state['flags'] == 0, 'effect-free corpse still has effect flags'
        assert state['dexterity_modifier'] == before['dexterity_modifier'], 'stun modifier was not undone'
    print('PASS: stun applied; death removed stun, VFX payload, flags and Dexterity penalty; reload stays clear')
    print(json.dumps({'victim': victim, 'stunned_effects': [e['ty'] for e in stunned['effects']],
                      'dead_effects': [e['ty'] for e in dead['effects']], 'dead_flags': dead['flags']}))
except Exception as error:
    print('FAIL:', error)
    raise
finally:
    d.stop()
PY
