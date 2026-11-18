#!/usr/bin/env bash
# Synthetic A–G regressions plus optional read-only installed-save probes.
# CARGO_TARGET_DIR must name this job's build directory on the shared Mac.
# DOORS_SLOT defaults to the coordinator's build limiter when it exists.
# DOORS_SAVE points to an external Orzammar save; no game data is in this repo.
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${CARGO_TARGET_DIR:?set a dedicated CARGO_TARGET_DIR}"
failures=0
slot=${DOORS_SLOT:-/Volumes/External2/projects/OpenThedas-wt/codex-briefs/cargo-slot.sh}
run() {
  if [[ -x "$slot" ]]; then "$slot" "$@"; else "$@"; fi
}
probe() {
  local capped=${DOORS_CAPPED:-/Users/daeken/projects/OpenThedas-testdata/scripts/capped.sh}
  if [[ -x "$capped" ]]; then run "$capped" "$@"; else run "$@"; fi
}
check() {
  local bug=$1 package=$2 filter=$3
  if run cargo test -j 2 -p "$package" --lib "$filter"; then
    echo "PASS $bug: $filter"
  else
    echo "FAIL $bug: $filter"
    failures=$((failures + 1))
  fi
}
check A thedas-sim doors_regression_a_
check B thedas-sim doors_regression_b_
check C thedas-sim doors_regression_c_
check D thedas-ui doors_hold_their_state_pose_and_swing_when_it_changes
check E thedas-sim doors_regression_e_
check F thedas-sim doors_regression_f_
check G thedas-sim doors_regression_g_
echo 'NOTE D covers state-to-presentation pose selection; intermittent visual playback requires rendering review.'
echo 'NOTE E covers repaired-route arrival; the historical hideout jump is not established by the retained journal.'
if [[ -n ${DOORS_SAVE:-} ]]; then
  : "${THEDAS_INSTALL:?set THEDAS_INSTALL for installed-save checks}"
  run cargo build -j 2 -p thedas-sim --example doors_probe
  probe "$CARGO_TARGET_DIR/debug/examples/doors_probe" "$DOORS_SAVE" --checks
fi
if [[ -n ${DOORS_JOURNAL:-} ]]; then
  : "${DOORS_CHECKPOINT:?set DOORS_CHECKPOINT for journal replay}"
  : "${THEDAS_INSTALL:?set THEDAS_INSTALL for journal replay}"
  run cargo build -j 2 -p thedas-sim --example doors_probe
  probe "$CARGO_TARGET_DIR/debug/examples/doors_probe" "$DOORS_CHECKPOINT" "$DOORS_JOURNAL"
fi
exit "$((failures != 0))"
