#!/usr/bin/env bash
# Cloud-side merge of one PR, following CLAUDE.md's merge rule:
# merge the latest main into the PR head, run fmt, clippy and debug tests for the crates
# the PR touches, check the head has not moved, then merge through the REST API.
#
# Usage: tools/triage/merge-pr.sh <pr-number> [--dry-run]
# Env:   REPO (default daeken/OpenThedas), WT_ROOT (default $HOME), CARGO_TARGET_DIR (optional).
# Needs: gh (authenticated), git, cargo. Never rewrites history; stages nothing by itself.
# The PR head's own branch is fetched by its pull ref, so branches of other streams work.
set -euo pipefail

pr="${1:?usage: merge-pr.sh <pr-number> [--dry-run]}"
dry="${2:-}"
repo="${REPO:-daeken/OpenThedas}"
root="${WT_ROOT:-$HOME}"
wt="$root/wt-merge-$pr"

info=$(gh api "repos/$repo/pulls/$pr" --jq '[.state,.head.sha,.base.ref]|@tsv')
state=$(cut -f1 <<<"$info"); head=$(cut -f2 <<<"$info"); base=$(cut -f3 <<<"$info")
[ "$state" = open ] || { echo "PR $pr is $state"; exit 1; }

git fetch -q origin "$base" "+refs/pull/$pr/head:refs/remotes/pr/$pr"
[ "$(git rev-parse "refs/remotes/pr/$pr")" = "$head" ] || { echo "fetched head differs from the API head"; exit 1; }

rm -rf "$wt"; git worktree prune
git worktree add -q --detach "$wt" "origin/$base"
trap 'git worktree remove --force "$wt" >/dev/null 2>&1 || true' EXIT
cd "$wt"
main_sha=$(git rev-parse --short HEAD)
git merge -q --no-edit "refs/remotes/pr/$pr" || { echo "merge conflict: resolve by hand"; exit 2; }

# The crates the PR touches (workspace member = first path component under crates/).
crates=$(git diff --name-only "origin/$base" HEAD | sed -n 's#^crates/\([^/]*\)/.*#\1#p' | sort -u | sed 's/^/-p /' | tr '\n' ' ')
echo "== PR $pr head ${head:0:8} on $base $main_sha; crates: ${crates:-none}"
git diff --stat "origin/$base" HEAD | tail -3

cargo fmt --all -- --check
if [ -n "$crates" ]; then
  # shellcheck disable=SC2086
  CARGO_INCREMENTAL=0 cargo clippy -q --all-targets $crates -- -D warnings
  # shellcheck disable=SC2086
  CARGO_INCREMENTAL=0 cargo test -q $crates
fi

git fetch -q origin "$base"
if [ "$(git rev-parse --short "origin/$base")" != "$main_sha" ]; then
  echo "$base moved while the checks ran; run again"; exit 3
fi
[ "$dry" = --dry-run ] && { echo "checks passed (dry run, not merged)"; exit 0; }

for _ in 1 2 3 4 5 6 7 8; do
  [ "$(gh api "repos/$repo/pulls/$pr" --jq .mergeable)" = true ] && break; sleep 4
done
gh api -X PUT "repos/$repo/pulls/$pr/merge" -f merge_method=merge -f sha="$head" --jq .sha
