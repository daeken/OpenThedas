#!/bin/bash
# Run s13 on an s12-produced save (or the stock seed for comparison) without
# touching the source saves/harness. Invoke via the project's cargo-slot tool;
# this script acquires the game slot itself. OUT must be new and job-local.
set -euo pipefail
if [[ $# != 3 ]]; then
    echo "usage: $0 /abs/thedas-mcp /abs/12_tower_fresh.json /abs/new-output-dir" >&2
    exit 2
fi
bin=$1 save=$2 out=$3
[[ $bin = /* && -x $bin && $save = /* && -f $save && $out = /* ]]
T=${THEDAS_TESTDATA:-/Users/daeken/projects/OpenThedas-testdata}
mkdir "$out"
mkdir "$out/home" "$out/tmp" "$out/logs"
export HOME="$out/home" TMPDIR="$out/tmp" PYTHONDONTWRITEBYTECODE=1
export THEDAS_HIDDEN_WINDOW=1 THEDAS_ASYNC_LOADS=1 THEDAS_NO_AUDIO=1 THEDAS_MODS=none
unset THEDAS_KEYS THEDAS_USER_DIR GAME_SLOTS
export THEDAS_MCP="$bin" DRIVE_TIMEOUT=${DRIVE_TIMEOUT:-600}
export LIM_T=$(( DRIVE_TIMEOUT + 30 ))
cd "$out"
shasum -a 256 "$bin" "$save" > inputs.sha256
cp "$save" 12_tower_fresh.json
cp "$T/drive.py" drive.py
# Use the copied driver, whose HERE points here, not at the shared seed tree.
sed '/^import sys; sys\.path\.insert(0, /d' "$T/s13_beacon.py" > s13_beacon.py
diff -u "$T/s13_beacon.py" s13_beacon.py > harness.diff || test "$?" -eq 1
st=$(date +%s)
rc=0
if [[ -n ${S13_TRACE:-} ]]; then
    run=(python3 -u "$S13_TRACE" s13_beacon.py)
else
    run=(python3 -u s13_beacon.py)
fi
# Close inherited cargo lock descriptors before calling capped.sh. Its
# detached timer sleep can outlive the driver and must not retain a lease.
"$T/marten/game-slot.sh" python3 -c 'import subprocess, sys; sys.exit(subprocess.call(sys.argv[1:], close_fds=True))' \
    bash "$T/scripts/capped.sh" "${run[@]}" > run.log 2>&1 || rc=$?
printf 's13_beacon rc=%s secs=%s\n' "$rc" "$(( $(date +%s) - st ))" | tee result.log
tail -20 run.log
[[ $rc = 0 && -s 13_flemeth_hut.json ]]
grep -Eq '^SAVED (.*/)?13_flemeth_hut\.json area pre211ar_flemeths_hut_int ' run.log
echo 'PASS: reached Flemeth’s hut from the supplied Tower save.'
