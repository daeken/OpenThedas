#!/bin/bash
# MCP regression from the original Oskias pre-kill report save.
# Usage: deathblow.sh /absolute/path/to/thedas-mcp [output-directory]
set -euo pipefail
data=${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}
if [ "$#" -lt 1 ] || [ ! -x "$1" ]; then
    echo "Usage: $0 /path/to/thedas-mcp [output-directory]" >&2
    exit 2
fi
if [ "${DEATHBLOW_SLOT_HELD:-0}" != 1 ]; then
    exec /Volumes/External2/projects/OpenThedas-wt/codex-briefs/cargo-slot.sh \
        "$data/scripts/capped.sh" env DEATHBLOW_SLOT_HELD=1 "$0" "$@"
fi
python3 - "$@" <<'PY'
import json
import os
from pathlib import Path
import sys

root = Path(os.environ.get('THEDAS_TESTDATA', '/Users/daeken/projects/OpenThedas-testdata'))
sys.path.insert(0, str(root))
import drive as d

binary = str(Path(sys.argv[1]).resolve())
out = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else root / 'codex-captures/deathblow/repro'
out.mkdir(parents=True, exist_ok=True)
save = root / 'triage-reports/oskias-0420/save_before.json'
source = json.loads(save.read_text())
hero = source['world']['hero']
victim = next(o['id'] for o in source['world']['objects'].values() if o['tag'] == 'bdc130cr_oskias')
assert not source['world']['objects'][str(victim)]['dead']
# Isolate the reported executor boundary from later changes to attack rolls and
# the app's auto-attack loop. Keep the report's world, equipment, health and RNG,
# installing the default death-blow command and a stored lethal main-hand result.
attacker = source['world']['objects'][str(hero)]
command = dict(attacker['commands'][0])
command.update(ty=38, name='CommandDeathBlow', target=victim, ints=[0], due=None)
attacker['commands'] = [command]
attacker['combat']['attack'] = {
    'results': [4, 0],
    'effects': [{
        'ty': 1024, 'creator': hero, 'ability': 0, 'ints': [0, 0, 0],
        'floats': [source['world']['objects'][str(victim)]['properties']['7'][1]],
        'objects': [attacker['equipment']['0']], 'strings': [], 'duration_type': 0, 'id': 0,
    }, None],
}
prepared = out / 'prepared-input.json'
prepared.write_text(json.dumps(source) + '\n')
d.start(binary)
try:
    d.call('load')
    d.call('load_state', path=str(prepared))
    initial = d.now()
    blow = reaction = None
    dead_at = None
    captured = False
    for _ in range(2400):
        d.j('step', ticks=1, tick_ms=26)
        attacker = d.j('inspect_object', object=str(hero))
        target = d.j('inspect_object', object=str(victim))
        blow = blow or attacker['combat'].get('death_blow')
        reaction = reaction or target['combat'].get('death_blow_reaction')
        dead_at = dead_at if dead_at is not None else target.get('died_at')
        commands = attacker.get('commands', [])
        if (not captured and d.now() >= initial + 390
                and (blow or (commands and commands[0]['name'] == 'CommandDeathBlow'))):
            d.call('save_state', path=str(out / 'frame.json'))
            captured = True
        if reaction and d.now() >= reaction['ends_ms'] + 1:
            break
        if dead_at is not None and not reaction:
            break
    trace = d.j('get_trace', from_start=True, limit=100000, kinds=['command', 'attack', 'event_delivered'])
    entries = trace.get('entries', []) if isinstance(trace, dict) else trace
    (out / 'trace.json').write_text(json.dumps(entries, indent=2) + '\n')
    start = blow['started_ms'] if blow else initial
    deaths = [e['t'] for e in entries if e.get('event_name') == 'DEATH' and e.get('target') == victim and start is not None and e['t'] >= start]
    hits = [e['t'] - start for e in entries if e.get('kind') == 'attack' and e.get('attacker') == hero and e.get('target') == victim and start is not None and e['t'] >= start]
    result = {
        'source': 'oskias-0420/save_before.json', 'initial_ms': initial,
        'setup': 'report world with queued default death blow and stored lethal main-hand result',
        'blow_started_ms': start,
        'reaction_after_start_ms': reaction['started_ms'] - start if reaction and start is not None else None,
        'IsDead_after_start_ms': dead_at - start if dead_at is not None and start is not None else None,
        'DEATH_after_start_ms': deaths[0] - start if deaths and start is not None else None,
        'stored_hit_after_start_ms': hits, 'attack': blow, 'reaction': reaction,
    }
    (out / 'measurements.json').write_text(json.dumps(result, indent=2) + '\n')
    d.call('save_state', path=str(out / 'after.json'))
    print(json.dumps(result, indent=2))
    passed = (
        blow is not None and reaction is not None
        and result['reaction_after_start_ms'] == 0
        and result['IsDead_after_start_ms'] == 0
        # Independently inspected, unscaled dwarf dual-wield pair in this save.
        and blow['action'] == 'db_h_dw_attk_v1' and blow['ends_ms'] - start == 3133
        and reaction['action'] == 'db_h_dw_die_v1' and reaction['ends_ms'] - start == 3467
        and len(deaths) == 1 and deaths[0] == reaction['ends_ms']
        and hits == [impact['at_ms'] - start for impact in blow['impacts']]
        and d.eng('IsDead', victim) == 1
    )
    print('PASS' if passed else 'FAIL', 'native paired reaction/health start, authored hits, reaction-end DEATH')
    print('Default pair selection and placement blend remain provisional.')
    if not passed:
        raise SystemExit(1)
finally:
    d.stop()
PY
