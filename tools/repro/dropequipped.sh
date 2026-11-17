#!/bin/bash
# MCP acceptance: equip one helm, discard it, inspect ownership and floor height.
# Usage: dropequipped.sh /absolute/path/to/thedas-mcp [output-directory]
set -euo pipefail
data=${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}
if [ "$#" -lt 1 ] || [ ! -x "$1" ]; then
    echo "FAIL: usage: $0 /path/to/thedas-mcp [output-directory]" >&2
    exit 2
fi
if [ "${DROPEQUIPPED_SLOT_HELD:-0}" != 1 ]; then
    exec /Volumes/External2/projects/OpenThedas-wt/codex-briefs/cargo-slot.sh \
        "$data/scripts/capped.sh" env DROPEQUIPPED_SLOT_HELD=1 "$0" "$@"
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
out = Path(sys.argv[2]).resolve() if len(sys.argv) > 2 else root / 'codex-captures/dropequipped/repro'
out.mkdir(parents=True, exist_ok=True)
(out / 'home').mkdir(exist_ok=True)
os.environ.update(HOME=str(out / 'home'), THEDAS_MODS='none', THEDAS_HIDDEN_WINDOW='1', THEDAS_NO_AUDIO='1')
source = Path(os.environ.get('DROPEQUIPPED_SAVE', root / 'dc_08_proving_crowd.json'))

def snapshot(name):
    path = out / (name + '.json')
    d.call('save_state', path=str(path))
    return json.loads(path.read_text())['world']

def message(name, *args):
    return d.j('ui_invoke', movie='inventory.gfx', path='ExternalCommands.SendMessageArgs',
               args=[name, *args])

def check(ok, description):
    print(('PASS: ' if ok else 'FAIL: ') + description, flush=True)
    if not ok:
        raise AssertionError(description)

d.start(binary)
try:
    d.call('load')
    d.call('load_state', path=str(source))
    hero = d.j('inspect_object', object='player')
    who, area, feet = hero['id'], hero['area'], hero['position']
    # An ordinary, removable helm, rather than Everd's authored locked disguise.
    helm = d.eng('CreateItemOnObject', 'orz200im_guard_helm', who, 1, 'dropequipped_helm')['object']
    d.eng('SetItemIrremovable', helm, 0)
    d.call('ui_open', movies='inventory')
    message('GUI.SelectedChar.EquipItem', helm, 5)
    equipped = d.j('inspect_object', object=str(who))
    check(equipped['equipment'].get('5') == helm, f'helm {helm} equipped through inventory message')
    ui_helm = d.j('ui_invoke', movie='inventory.gfx', path='ExternalCommands.GetValue',
                  args=['GUI.SelectedChar.Equipment.5'])['result']
    check(ui_helm == helm, 'inventory binding shows the equipped helm')
    snapshot('before')
    # Read the navigation floor independently of the object Z, then deliberately
    # lift the creature so copying its position cannot pass the drop check.
    floor = [feet[0], feet[1], equipped.get('floor_z')]
    d.call('move_hero', x=feet[0], y=feet[1], z=feet[2] + 1.25, teleport=True)
    snapshot('elevated-before')
    raised_z = d.j('inspect_object', object=str(who))['position'][2]
    message('GUI.SelectedChar.DropItem', helm)
    after = snapshot('after-drop')
    holders = [o for o in after['objects'].values() if helm in o['inventory']]
    worn = [o['id'] for o in after['objects'].values()
            if helm in o['equipment'].values() or helm in o['other_set'].values()]
    party = [who, *after['objects'][str(who)]['followers']]
    carried = [i for i in party if helm in after['objects'][str(i)]['inventory']]
    check(not worn, f'original helm id absent from both equipment sets: {worn}')
    check(not carried, f'original helm id absent from party inventories: {carried}')
    ui_helm = d.j('ui_invoke', movie='inventory.gfx', path='ExternalCommands.GetValue',
                  args=['GUI.SelectedChar.Equipment.5'])['result']
    check(ui_helm is None, 'inventory binding clears the equipped helm')

    check(len(holders) == 1 and holders[0]['kind'] == 'Placeable', 'one container owns the original helm')
    bag = holders[0]
    check(str(helm) in after['objects'] and bag['inventory'] == [helm], 'same item object survives; no copied helm')
    check(bag['area'] == area and all(abs(a - b) <= 0.0001 for a, b in zip(bag['position'][:2], feet[:2])),
          'bag is at the creature feet in the same area')
    check(floor[2] is not None, f'independent navigation floor sample at feet: {floor}')
    check(abs(raised_z - floor[2]) >= 1.0, f'creature was deliberately above floor at drop: z={raised_z}')
    check(abs(bag['position'][2] - floor[2]) <= 0.01,
          f'bag z={bag["position"][2]:.6f}, floor z={floor[2]:.6f}, delta={bag["position"][2]-floor[2]:.6f}')
    result = {'source': source.name, 'hero': who, 'helm': helm, 'bag': bag['id'],
              'floor': floor, 'bag_position': bag['position'], 'equipment_holders': worn,
              'party_inventory_holders': carried, 'bag_appearance': bag['appearance']}
    (out / 'measurements.json').write_text(json.dumps(result, indent=2) + '\n')
    # Restore creature height for a useful before/after presentation capture.
    d.call('move_hero', x=feet[0], y=feet[1], z=feet[2], teleport=True)
    snapshot('after')
    d.call('load_state', path=str(out / 'after.json'))
    loaded = snapshot('reloaded')
    check(loaded['objects'][str(bag['id'])]['inventory'] == [helm], 'bag retains the original item across save/reload')
    d.call('interact', object=str(bag['id']))
    opened = snapshot('opened')
    check(opened['container'] == bag['id'], 'using the sack opens its normal loot inventory')
    message('GUI.SelectedChar.AcquireItem', helm)
    recovered = d.j('inspect_object', object=str(who))
    check(helm in recovered['inventory'] and helm not in recovered['equipment'].values(), 'same helm can be recovered into the backpack')
    print('PASS: dropequipped ownership, equipment, floor, save/reload and recovery', flush=True)
except Exception as error:
    print(f'FAIL: dropequipped: {error}', file=sys.stderr, flush=True)
    raise SystemExit(1)
finally:
    d.stop()
PY
