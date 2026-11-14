#!/usr/bin/env bash
# Plays a launcher script (docs/launcher-qa/*.steps) in a scratch home, so it never touches
# the player's settings, mods or games. Usage:
#
#   docs/launcher-qa/run.sh first-run [path/to/thedas-app]
#   docs/launcher-qa/run.sh keyboard
#
# Needs a display (on a headless Linux box: `xvfb-run -a docs/launcher-qa/run.sh first-run`).
# Exits non-zero when a step or `expect` fails. Screenshots land in $OUT (default: a
# `launcher-qa-out` folder under the scratch folder, printed at the end).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
script="${1:?which script? first-run or keyboard}"
exe="${2:-${THEDAS_APP:-$here/../../target/debug/thedas-app}}"

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/thedas-launcher-qa.XXXXXX")"
export SCRATCH
export HOME="$SCRATCH/home"
export USERPROFILE="$HOME"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CONFIG_HOME="$HOME/.config"
mkdir -p "$HOME"
export THEDAS_LAUNCHER_CONFIG="$SCRATCH/launcher.json"
export THEDAS_LAUNCHER_DRY_RUN=1
export THEDAS_MODS=none
export OUT="${OUT:-$SCRATCH/out}"
mkdir -p "$OUT"

set +e
"$exe" --launcher-script "$here/$script.steps"
code=$?
# The second start reads what the first left in the scratch settings.
if [ "$script" = first-run ] && [ $code -eq 0 ]; then
    "$exe" --launcher-script "$here/second-run.steps"
    code=$?
fi
set -e
echo "launcher-qa $script: exit $code (scratch folder $SCRATCH, screenshots in $OUT)" >&2
exit $code
