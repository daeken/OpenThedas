#!/usr/bin/env python3
"""Find the first bad first-parent commit between a good and a bad one.

Usage: tools/triage/bisect_merges.py <good-sha> <bad-sha> <check> [--min N] [--package thedas-app]

Each tested commit is release-built in the worktree (thedas-app by default) and cached as
THEDAS_TESTDATA/bin/<package>-<sha8>, so a commit is built once across runs. <check> is any
executable run as `<check> <binary> <out-dir>`: when its last output line is a number (a PSNR, a
frame rate), the commit is good if that number is >= --min (default 30); otherwise its exit code
decides (0 good). Only first-parent commits (the merges on main) are tested.
Example check: tools/triage/check_bunk_psnr.py.
"""

import argparse
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import triagelib as t  # noqa: E402


def judge(check: str, binary: Path, out: Path, minimum: float) -> tuple[bool, str]:
    out.mkdir(parents=True, exist_ok=True)
    p = subprocess.run([check, str(binary), str(out)], capture_output=True, text=True)
    last = (p.stdout.strip().splitlines() or [""])[-1].strip()
    try:
        value = float(last)
        return value >= minimum, f"{value:g}"
    except ValueError:
        return p.returncode == 0, f"exit {p.returncode}"


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("good")
    ap.add_argument("bad")
    ap.add_argument("check")
    ap.add_argument("--min", type=float, default=30.0, help="a numeric result at or above this is good")
    ap.add_argument("--package", default="thedas-app")
    a = ap.parse_args()

    commits = t.git("rev-list", "--first-parent", "--reverse", f"{a.good}..{a.bad}").split()
    if not commits:
        raise SystemExit("no first-parent commits between them")
    work = t.TESTDATA / "bisect" / f"{t.short(a.good)}..{t.short(a.bad)}"
    seen = {}

    def test(sha: str) -> bool:
        if sha not in seen:
            binary = t.build(sha, packages=(a.package,), log=work / "build.log")[a.package]
            ok, value = judge(a.check, binary, work / t.short(sha), a.min)
            subject = t.git("log", "-1", "--format=%s", sha).strip()[:70]
            print(f"  {t.short(sha)}  {'good' if ok else 'BAD '}  {value:>8}  {subject}", flush=True)
            seen[sha] = ok
        return seen[sha]

    work.mkdir(parents=True, exist_ok=True)
    print(f"{len(commits)} first-parent commits after {t.short(a.good)}; checking the ends")
    test(a.good)
    if not test(commits[-1]):
        lo, hi = 0, len(commits) - 1  # commits[hi] is bad; find the first bad at or before it
        while lo < hi:
            mid = (lo + hi) // 2
            if test(commits[mid]):
                lo = mid + 1
            else:
                hi = mid
        first = commits[lo]
        print(f"first bad: {t.short(first)}  {t.git('log', '-1', '--format=%s', first).strip()}")
        return 0
    print("the bad end checks good: nothing to bisect")
    return 1


if __name__ == "__main__":
    sys.exit(main())
