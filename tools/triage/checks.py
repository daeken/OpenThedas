"""The fixed spot-checks the triage tools run on a binary. Each returns (passed, detail).

The saves and references they use live in THEDAS_TESTDATA (see triagelib); a check whose input
is missing fails with that said, rather than passing.
"""

import json
import re
import shutil
from pathlib import Path

import triagelib as t

SLUMS_SAVE = t.TESTDATA / "triage-walkanim" / "slums_0442.json"
OSKIAS_REPORT = t.TESTDATA / "triage-reports" / "985"
ELF_CITY_SAVE = t.TESTDATA / "origin_elf_city.json"
COMMONER_SAVE = t.TESTDATA / "origin_dwarf_commoner.json"
# The City Elf opening's bunk as it should look (after #994's root scaling, which matches the
# original's capture reference/original/triage/city_elf_opening_shianni.png).
BUNK_REF = Path(t.os.environ.get("BUNK_REF", t.TESTDATA / "triage-celf" / "ref" / "bunk.png"))
BUNK_CROP = "200:420:680:80"
BUNK_MIN_DB = 30.0
DETCHECK_SAVES = ("04_siege_start.json", "09_wilds_vials.json")

# City Elf through character creation to the first conversation, at 1280x720 (convcap's clicks).
CELF_CHARGEN = (
    "run 3000; pad a; run 2000; click 752,226; run 1000; click 678,322; run 1000; click 826,422; run 1500; "
    "click 874,691; run 2500; click 874,691; run 2500; click 923,157; run 200; click 923,157; run 200; "
    "click 923,157; run 200; click 923,297; run 200; click 923,297; run 500; click 874,691; run 2500; "
    "click 581,178; run 500; click 581,418; run 500; click 581,478; run 800; click 874,691; run 2500; "
    "click 581,213; run 500; click 581,273; run 800; click 581,393; run 500; click 581,453; run 500; "
    "click 581,513; run 500; click 874,691; run 3000; click 872,691; run 4000; click 620,406; run 3000; pad a"
)


def _need(*paths) -> str | None:
    missing = [str(p) for p in paths if not Path(p).exists()]
    return "missing " + ", ".join(missing) if missing else None


def slums_click(binary, out: Path):
    """A ground click in Dust Town moves the hero (it picked the Commons exit before #987)."""
    if (m := _need(SLUMS_SAVE)):
        return False, m
    log = t.headless(binary, "run 1000; click 760,520; run 7000", out / "slums-click.png", save=SLUMS_SAVE,
                     name="slums-click")
    line = next((l for l in log.splitlines() if l.startswith("click 760,520")), "no click line")
    return "move to" in line, line


def oskias_bag(binary, out: Path):
    """Pad A on Oskias's body bag (report #985's save) opens the inventory (#989)."""
    save, report = OSKIAS_REPORT / "save_now.json", OSKIAS_REPORT / "report.json"
    if (m := _need(save, report)):
        return False, m
    camera = json.loads(report.read_text())["game"]["camera"]
    log = t.headless(binary, "run 2000; pad a; run 2500", out / "oskias-bag.png", save=save, camera=camera,
                     name="oskias", extra_env={"RUST_LOG": "info"})
    used = "pad: use gen_ip_bodybag" in log
    up = "Inventory up" in log
    return used and up, f"use bag: {used}, inventory opened: {up}"


def bunk(binary, out: Path):
    """The City Elf opening's hero sits on the bunk (bunk-region PSNR against BUNK_REF)."""
    if (m := _need(BUNK_REF)):
        return False, m
    cap = out / "bunk.png"
    t.headless(binary, CELF_CHARGEN + "; run 8000", cap, new_game=True, name="bunk")
    if not cap.exists():
        return False, "no capture"
    db = t.psnr(cap, BUNK_REF, BUNK_CROP)
    return db >= BUNK_MIN_DB, f"{db:.2f} dB (pass >= {BUNK_MIN_DB})"


def bunk_psnr(binary, out: Path) -> float:
    """The bunk check's number alone (for bisect_merges.py)."""
    cap = out / "bunk.png"
    t.headless(binary, CELF_CHARGEN + "; run 8000", cap, new_game=True, name="bunk")
    return t.psnr(cap, BUNK_REF, BUNK_CROP)


def footlocker(binary, out: Path):
    """The City Elf footlocker opens the inventory screen with its container (#944, #1001)."""
    if (m := _need(ELF_CITY_SAVE)):
        return False, m
    log = t.headless(binary, "pad a; pad a; pad a; pad a; run 3000", out / "footlocker.png", save=ELF_CITY_SAVE,
                     name="footlocker")
    return "screens {Inventory}" in log, next((l for l in log.splitlines() if l.startswith("run 3000")), "")


NOTE = "Pin check: it's 100% (#925) - 0042, can't stop"


def f8_note(binary, out: Path):
    """F8, Just a note, a note with apostrophes past the old 20-character limit: saved in full."""
    if (m := _need(COMMONER_SAVE)):
        return False, m
    game = t.Control(binary, COMMONER_SAVE, t.free_port(), name="f8", log=out / "f8.log")
    try:
        for steps in ("key f8; run 1500", "click 1370,835; run 1000", f"type {NOTE}", "key enter; run 2500"):
            game.input(steps)
    finally:
        game.close()
    notes = sorted((game.home / ".openthedas" / "reports").glob("*/note.txt"))
    got = notes[-1].read_text() if notes else None
    return got == NOTE, f"note {got!r}"


# Per-frame facing log patched into a scratch copy of the source for the facing probe.
_FACING_PATCH = (
    "crates/thedas-ui/src/models.rs",
    "\n    let fighting = o.combat.in_combat;\n",
    '\n    if o.tag == "player" && std::env::var_os("THEDAS_FACING_LOG").is_some() '
    '{ eprintln!("FACINGLOG t={} face={:.2}", now, o.facing); }'
    "\n    let fighting = o.combat.in_combat;\n",
)


def facing(sha: str, out: Path, max_step: float = 30.0):
    """Stop, stick forward 1.5 s, stop, stick right 1 s on the slums save: the hero's facing
    changes by at most `max_step` degrees a frame (no snapping or jitter; #907, #917). Builds a
    logging copy of the binary in the worktree and restores the source afterwards."""
    if (m := _need(SLUMS_SAVE)):
        return False, m
    path, anchor, patched = _FACING_PATCH
    src = t.WORKTREE / path
    t.git("checkout", "-q", "--detach", sha)
    text = src.read_text()
    if anchor not in text:
        return False, "facing log anchor not found in models.rs"
    try:
        src.write_text(text.replace(anchor, patched, 1))
        if t.sh([t.CARGO, "build", "--release", "-p", "thedas-app"], cwd=t.WORKTREE, log=out / "facing-build.log"):
            return False, "logging build failed"
        binary = out / "thedas-app-facing"
        shutil.copy2(t.WORKTREE / "target" / "release" / "thedas-app", binary)
    finally:
        t.git("checkout", "--", path)
    log = t.headless(binary, "run 1000; pad stick-left 0,1 1500; run 600; pad stick-left 1,0 1000; run 600",
                     out / "facing.png", save=SLUMS_SAVE, name="facing", extra_env={"THEDAS_FACING_LOG": 1})
    faces = [float(m.group(1)) for m in re.finditer(r"FACINGLOG t=\d+ face=([-\d.]+)", log)]
    steps = [((b - a + 180) % 360) - 180 for a, b in zip(faces, faces[1:])]
    worst = max((abs(s) for s in steps), default=0.0)
    return bool(faces) and worst <= max_step, f"{len(faces)} frames, largest change {worst:.1f}°"


def detcheck(mcp, out: Path):
    """The simulation is deterministic: two runs, and a save and reload, give the same world."""
    script = t.TESTDATA / "gate" / "detcheck.py"
    if (m := _need(script)):
        return False, m
    lines = []
    for save in DETCHECK_SAVES:
        log = out / f"detcheck-{save}.log"
        env = dict(t.os.environ, THEDAS_MCP=str(mcp), HOME=str(t.scratch_home("detcheck")),
                   THEDAS_INSTALL=t.install())
        t.sh(["python3", "-u", str(script), str(t.TESTDATA / save), "600"], env=env, log=log)
        lines += [l for l in log.read_text().splitlines() if l.startswith("DET")]
    ok = len(lines) == len(DETCHECK_SAVES) and all("twice: identical | save+reload: identical" in l for l in lines)
    return ok, "; ".join(l.removeprefix("DET ") for l in lines) or "no DET lines"
