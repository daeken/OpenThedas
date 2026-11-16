#!/usr/bin/env python3
"""Pin a build for play-testing: check a commit, then put its thedas-app where play.command runs it.

Usage: tools/triage/pin.py <sha> [--dry-run] [--force] [--no-facing]

1. Fetches, checks out <sha> (detached) in the worktree, runs `cargo clippy --workspace
   --all-targets -- -D warnings` and debug `cargo test --workspace`, and release-builds thedas-app
   and thedas-mcp (cached in THEDAS_TESTDATA/bin by sha).
2. Runs the spot-checks (checks.py) on that exact binary and prints one pass/fail table and its
   sha256. Captures and logs go to THEDAS_TESTDATA/pins/<sha8>/.
3. Only if every row passed (or --force), and not with --dry-run: copies the binary over
   PLAY_CHECKOUT/target/release/thedas-app, keeping the one it replaces as
   thedas-app.<its sha8>.bak (other .bak files are kept), and runs play.command --help under a
   scratch HOME. play.command itself and the checkout's other files are not touched.

Env: PLAY_CHECKOUT (default ~/projects/OpenThedas) and those in triagelib.
"""

import argparse
import os
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import checks  # noqa: E402
import triagelib as t  # noqa: E402

PLAY = Path(os.environ.get("PLAY_CHECKOUT", t.HOME / "projects" / "OpenThedas"))


def old_sha(binary: Path) -> str:
    """The cached build the pinned binary is, by content; else a date tag."""
    digest = t.sha256(binary)
    for cached in t.BIN_CACHE.glob("thedas-app-*"):
        if cached.stat().st_size == binary.stat().st_size and t.sha256(cached) == digest:
            return cached.name.removeprefix("thedas-app-")
    return "prev-" + __import__("time").strftime("%Y%m%d%H%M")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("sha")
    ap.add_argument("--dry-run", action="store_true", help="build and check; do not copy")
    ap.add_argument("--force", action="store_true", help="pin even if a check failed")
    ap.add_argument("--no-facing", action="store_true", help="skip the facing probe (it builds a second binary)")
    a = ap.parse_args()

    t.git("fetch", "-q", "origin")
    sha = t.git("rev-parse", a.sha).strip()
    s = sha[:8]
    out = t.TESTDATA / "pins" / s
    out.mkdir(parents=True, exist_ok=True)
    t.git("checkout", "-q", "--detach", sha)
    rows = []
    rc = t.sh([t.CARGO, "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"], cwd=t.WORKTREE,
              log=out / "clippy.log")
    rows.append(("clippy -D warnings", rc == 0, str(out / "clippy.log")))
    rc = t.sh([t.CARGO, "test", "--workspace"], cwd=t.WORKTREE, log=out / "tests.log")
    results = [l for l in (out / "tests.log").read_text(errors="replace").splitlines() if l.startswith("test result")]
    passed = sum(int(l.split()[3]) for l in results)
    failed = sum(int(l.split()[5]) for l in results)
    rows.append(("debug cargo test", rc == 0 and failed == 0, f"{passed} passed, {failed} failed"))
    bins = t.build(sha, log=out / "build.log")
    app, mcp = bins["thedas-app"], bins["thedas-mcp"]
    for name, fn in (("slums click", checks.slums_click), ("Oskias bag", checks.oskias_bag),
                     ("City Elf bunk", checks.bunk), ("footlocker", checks.footlocker),
                     ("F8 note", checks.f8_note)):
        try:
            rows.append((name, *fn(app, out)))
        except Exception as e:  # a check that cannot run fails, with why
            rows.append((name, False, f"error: {e}"))
    if not a.no_facing:
        try:
            rows.append(("facing probe", *checks.facing(sha, out)))
        except Exception as e:
            rows.append(("facing probe", False, f"error: {e}"))
    rows.append(("detcheck", *checks.detcheck(mcp, out)))

    digest = t.sha256(app)
    width = max(len(r[0]) for r in rows)
    print(f"\n{s}  thedas-app sha256 {digest}")
    for name, ok, detail in rows:
        print(f"  {name:<{width}}  {'PASS' if ok else 'FAIL'}  {detail}")
    all_ok = all(ok for _, ok, _ in rows)
    if a.dry_run:
        print("dry run: nothing copied")
        return 0 if all_ok else 1
    if not all_ok and not a.force:
        print("a check failed: nothing copied (--force to pin anyway)")
        return 1

    target = PLAY / "target" / "release" / "thedas-app"
    if target.exists():
        bak = target.with_name(f"thedas-app.{old_sha(target)}.bak")
        if not bak.exists():
            shutil.copy2(target, bak)
        print(f"kept the previous binary as {bak.name}")
    tmp = target.with_name("thedas-app.new")
    shutil.copy2(app, tmp)
    tmp.replace(target)
    home = t.scratch_home("pin-help")
    rc = t.sh([str(PLAY / "play.command"), "--help"], env=dict(os.environ, HOME=str(home)), log=out / "help.log")
    print(f"pinned {s} ({t.sha256(target)}); play.command --help exit {rc}")
    return 0 if rc == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
